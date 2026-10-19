//! Terminals owned by the daemon.
//!
//! Each pane runs its program on a pseudo-terminal and feeds everything
//! the program prints into a vt100 parser, so the daemon always knows what
//! the screen looks like. A client that attaches gets that screen redrawn
//! from scratch, then the raw output as it arrives.

use std::io::{Read, Write};
use std::num::NonZeroU16;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};

use anyhow::{Context, Result};
use aulora_core::model::{Ms, PaneExit, PaneKind, now_ms};
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use tokio::sync::broadcast;

const SCROLLBACK: usize = 5_000;
const CHANNEL: usize = 1_024;

/// Environment variables a pane must not inherit from whoever started the
/// daemon (often a Claude Code hook running inside some other terminal).
const STRIP_ENV: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_SSE_PORT",
    "CLAUDE_PROJECT_DIR",
    "CLAUDE_ENV_FILE",
    "CLAUDE_PLUGIN_ROOT",
    "CLAUDE_PLUGIN_DATA",
    "CLAUDE_EFFORT",
    "CLAUDE_CODE_BRIDGE_SESSION_ID",
    "TERM_PROGRAM",
    "TERM_PROGRAM_VERSION",
    "TERM_SESSION_ID",
    "__CFBundleIdentifier",
    "TMUX",
    "TMUX_PANE",
    "ITERM_SESSION_ID",
    "GHOSTTY_RESOURCES_DIR",
    "WEZTERM_PANE",
    "KITTY_WINDOW_ID",
    "AULORA_PANE_ID",
];

#[derive(Debug, Clone)]
pub enum PaneEvent {
    Output(Arc<[u8]>),
    Exited(PaneExit),
}

#[derive(Default)]
struct Signals {
    title: Option<String>,
    bell: bool,
}

impl vt100::Callbacks for Signals {
    fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
        self.title = Some(String::from_utf8_lossy(title).trim().to_string()).filter(|t| !t.is_empty());
    }
    fn audible_bell(&mut self, _: &mut vt100::Screen) {
        self.bell = true;
    }
}

/// What to run in a pane.
#[derive(Debug, Clone)]
pub struct Spec {
    pub id: String,
    pub cwd: String,
    pub kind: PaneKind,
    pub args: Vec<String>,
    pub cols: u16,
    pub rows: u16,
}

pub struct Proc {
    pub pid: Option<u32>,
    term: Mutex<vt100::Parser<Signals>>,
    tx: broadcast::Sender<PaneEvent>,
    input: mpsc::Sender<Vec<u8>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    pub last_output: AtomicI64,
    pub bytes_out: AtomicU64,
}

fn nz(n: u16) -> NonZeroU16 {
    NonZeroU16::new(n.max(1)).expect("non-zero")
}

fn shell() -> String {
    std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/zsh".into())
}

fn quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./=:,@%+".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// The argv for a pane. Everything runs under the user's login shell so
/// PATH and other profile settings match a normal terminal.
pub fn argv(kind: PaneKind, args: &[String]) -> Vec<String> {
    let sh = shell();
    match kind {
        PaneKind::Shell => vec![sh, "-l".into()],
        PaneKind::Claude => {
            let extra: Vec<String> = args.iter().map(|a| quote(a)).collect();
            let claude = if extra.is_empty() { "claude".to_string() } else { format!("claude {}", extra.join(" ")) };
            // Fall back to a shell when Claude exits, like a normal terminal.
            vec![sh.clone(), "-l".into(), "-i".into(), "-c".into(), format!("{claude}; exec {} -l", quote(&sh))]
        }
        PaneKind::Command => {
            let cmd: Vec<String> = args.iter().map(|a| quote(a)).collect();
            vec![sh, "-l".into(), "-c".into(), cmd.join(" ")]
        }
    }
}

impl Proc {
    /// Start the pane's program. `on_exit` runs once when it ends;
    /// `on_wake` runs when output arrives after a quiet spell.
    pub fn spawn(
        spec: &Spec,
        on_exit: impl FnOnce(PaneExit) + Send + 'static,
        on_wake: impl Fn() + Send + Sync + 'static,
    ) -> Result<Arc<Proc>> {
        let size = PtySize { rows: spec.rows.max(1), cols: spec.cols.max(1), pixel_width: 0, pixel_height: 0 };
        let pair = native_pty_system().openpty(size).context("opening a pseudo-terminal")?;

        let argv = argv(spec.kind, &spec.args);
        let mut cmd = CommandBuilder::new(&argv[0]);
        cmd.args(&argv[1..]);
        cmd.cwd(&spec.cwd);
        for k in STRIP_ENV {
            cmd.env_remove(k);
        }
        for (k, _) in std::env::vars_os() {
            if k.to_string_lossy().starts_with("VSCODE_") {
                cmd.env_remove(k);
            }
        }
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("TERM_PROGRAM", "aulora");
        cmd.env("TERM_PROGRAM_VERSION", aulora_core::VERSION);
        cmd.env("AULORA_PANE_ID", &spec.id);

        let child = pair.slave.spawn_command(cmd).with_context(|| format!("starting {}", argv.join(" ")))?;
        drop(pair.slave);
        let pid = child.process_id();
        let killer = child.clone_killer();
        let mut reader = pair.master.try_clone_reader()?;
        let mut writer = pair.master.take_writer()?;

        let (tx, _) = broadcast::channel(CHANNEL);
        let (input, input_rx) = mpsc::channel::<Vec<u8>>();
        let proc_ = Arc::new(Proc {
            pid,
            term: Mutex::new(vt100::Parser::new_with_callbacks(
                nz(spec.rows),
                nz(spec.cols),
                SCROLLBACK,
                Signals::default(),
            )),
            tx,
            input,
            master: Mutex::new(pair.master),
            killer: Mutex::new(killer),
            last_output: AtomicI64::new(now_ms()),
            bytes_out: AtomicU64::new(0),
        });

        // Writes can block when the program isn't reading, so they get a thread.
        std::thread::Builder::new().name(format!("pane-{}-in", spec.id)).spawn(move || {
            while let Ok(data) = input_rx.recv() {
                if writer.write_all(&data).and_then(|_| writer.flush()).is_err() {
                    break;
                }
            }
        })?;

        let out = proc_.clone();
        std::thread::Builder::new().name(format!("pane-{}-out", spec.id)).spawn(move || {
            let mut buf = vec![0u8; 32 * 1024];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let now = now_ms();
                        let quiet = now - out.last_output.swap(now, Ordering::Relaxed) > 1_500;
                        out.bytes_out.fetch_add(n as u64, Ordering::Relaxed);
                        let chunk: Arc<[u8]> = Arc::from(&buf[..n]);
                        let bell = {
                            // Parse and publish under one lock so attach() never sees
                            // a screen that disagrees with the stream that follows it.
                            let mut term = out.term.lock().unwrap();
                            term.process(&chunk);
                            let _ = out.tx.send(PaneEvent::Output(chunk));
                            std::mem::take(&mut term.callbacks_mut().bell)
                        };
                        if quiet || bell {
                            on_wake();
                        }
                    }
                }
            }
        })?;

        let done = proc_.clone();
        let mut child = child;
        std::thread::Builder::new().name(format!("pane-{}-wait", spec.id)).spawn(move || {
            let status = child.wait();
            let exit = match status {
                Ok(s) => PaneExit { code: s.exit_code(), signal: s.signal().map(str::to_string), at: now_ms() },
                Err(_) => PaneExit { code: 1, signal: None, at: now_ms() },
            };
            // Hold the terminal lock so the exit is ordered after all output.
            let term = done.term.lock().unwrap();
            let _ = done.tx.send(PaneEvent::Exited(exit.clone()));
            drop(term);
            on_exit(exit);
        })?;

        Ok(proc_)
    }

    /// The current screen as escape sequences, its size, and a receiver for
    /// everything after it, taken atomically.
    pub fn attach(&self) -> (Vec<u8>, u16, u16, broadcast::Receiver<PaneEvent>) {
        let term = self.term.lock().unwrap();
        let screen = term.screen();
        let (rows, cols) = screen.size();
        let mut data = b"\x1b[H\x1b[2J".to_vec();
        data.extend_from_slice(screen.state_formatted().as_bytes());
        let rx = self.tx.subscribe();
        (data, cols.get(), rows.get(), rx)
    }

    pub fn write(&self, data: Vec<u8>) {
        let _ = self.input.send(data);
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        let (cols, rows) = (cols.max(2), rows.max(1));
        let mut term = self.term.lock().unwrap();
        let (r, c) = term.screen().size();
        if (r.get(), c.get()) == (rows, cols) {
            return Ok(());
        }
        term.set_size(nz(rows), nz(cols));
        self.master.lock().unwrap().resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })?;
        Ok(())
    }

    pub fn size(&self) -> (u16, u16) {
        let (rows, cols) = self.term.lock().unwrap().screen().size();
        (cols.get(), rows.get())
    }

    pub fn title(&self) -> Option<String> {
        self.term.lock().unwrap().callbacks().title.clone()
    }

    /// Plain-text screen contents, for tests and previews.
    pub fn text(&self) -> String {
        self.term.lock().unwrap().screen().contents()
    }

    pub fn kill(&self) {
        let _ = self.killer.lock().unwrap().kill();
    }

    pub fn last_output_at(&self) -> Ms {
        self.last_output.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn wait_for(p: &Proc, needle: &str) -> bool {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(10) {
            if p.text().contains(needle) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    fn spec(kind: PaneKind, args: &[&str]) -> Spec {
        Spec {
            id: "test".into(),
            cwd: std::env::temp_dir().to_string_lossy().to_string(),
            kind,
            args: args.iter().map(|s| s.to_string()).collect(),
            cols: 80,
            rows: 24,
        }
    }

    #[test]
    fn runs_a_command_and_reports_exit() {
        let (tx, rx) = mpsc::channel();
        let p = Proc::spawn(
            &spec(PaneKind::Command, &["sh", "-c", "printf 'hello from %s' \"$AULORA_PANE_ID\"; exit 3"]),
            move |e| tx.send(e).unwrap(),
            || {},
        )
        .unwrap();
        assert!(wait_for(&p, "hello from test"), "screen was: {:?}", p.text());
        let exit = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(exit.code, 3);
    }

    #[test]
    fn input_reaches_the_program_and_attach_replays_the_screen() {
        let p = Proc::spawn(&spec(PaneKind::Command, &["cat"]), |_| {}, || {}).unwrap();
        p.write(b"ping\r".to_vec());
        assert!(wait_for(&p, "ping"));

        let (screen, cols, rows, _rx) = p.attach();
        assert_eq!((cols, rows), (80, 24));
        let mut replay = vt100::Parser::new(nz(24), nz(80), 0);
        replay.process(&screen);
        assert_eq!(replay.screen().contents(), p.text());
        p.kill();
    }

    #[test]
    fn resize_updates_screen_and_pty() {
        let p = Proc::spawn(&spec(PaneKind::Command, &["sh", "-c", "sleep 0.3; stty size; sleep 5"]), |_| {}, || {})
            .unwrap();
        p.resize(100, 30).unwrap();
        assert_eq!(p.size(), (100, 30));
        assert!(wait_for(&p, "30 100"), "screen was: {:?}", p.text());
        p.kill();
    }

    #[test]
    fn claude_panes_fall_back_to_a_shell() {
        let a = argv(PaneKind::Claude, &["--resume".into(), "abc def".into()]);
        assert_eq!(a[1..4], ["-l", "-i", "-c"]);
        assert!(a[4].starts_with("claude --resume 'abc def'; exec "));
    }
}
