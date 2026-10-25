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
use aulora_core::model::{Ms, PaneExit, PaneKind, ShellCommand, now_ms};
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use tokio::sync::broadcast;

use crate::zsh::{self, Mark, ShellState};

const SCROLLBACK: usize = 5_000;
const CHANNEL: usize = 1_024;

/// Environment variables a pane must not inherit from whoever started the
/// daemon (often a Claude Code hook running inside some other terminal).
/// Claude Code gives its hooks and commands markers of its own session;
/// a Claude that inherits CLAUDE_CODE_CHILD_SESSION thinks it's a child
/// session and doesn't save its transcript.
const STRIP_ENV: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_PID",
    "AI_AGENT",
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
    "AULORA_RUN",
    "AULORA_ZDOTDIR",
    "AULORA_USER_ZDOTDIR",
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
    marks: Vec<Mark>,
}

impl vt100::Callbacks for Signals {
    fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
        self.title = Some(String::from_utf8_lossy(title).trim().to_string()).filter(|t| !t.is_empty());
    }
    fn audible_bell(&mut self, _: &mut vt100::Screen) {
        self.bell = true;
    }
    fn unhandled_osc(&mut self, _: &mut vt100::Screen, params: &[&[u8]]) {
        self.marks.extend(Mark::parse(params));
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
    /// The shell to run under; None for yours ($SHELL).
    pub shell: Option<String>,
    /// More environment for the pane, set last.
    pub env: Vec<(String, String)>,
}

/// The terminal's master side: opened here, or adopted from the daemon
/// this one replaced.
enum Master {
    Pty(Box<dyn MasterPty + Send>),
    Fd(std::fs::File),
    Gone,
}

impl Master {
    fn resize(&self, rows: u16, cols: u16) -> Result<()> {
        match self {
            Master::Pty(m) => m.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })?,
            Master::Fd(f) => {
                use std::os::unix::io::AsRawFd;
                let ws = libc::winsize { ws_row: rows, ws_col: cols, ws_xpixel: 0, ws_ypixel: 0 };
                // SAFETY: a valid fd and a valid winsize.
                if unsafe { libc::ioctl(f.as_raw_fd(), libc::TIOCSWINSZ, &ws) } != 0 {
                    return Err(std::io::Error::last_os_error().into());
                }
            }
            Master::Gone => {}
        }
        Ok(())
    }

    fn raw_fd(&self) -> Option<std::os::unix::io::RawFd> {
        use std::os::unix::io::AsRawFd;
        match self {
            Master::Pty(m) => m.as_raw_fd(),
            Master::Fd(f) => Some(f.as_raw_fd()),
            Master::Gone => None,
        }
    }
}

enum Killer {
    Pty(Box<dyn ChildKiller + Send + Sync>),
    Pid(u32),
    Gone,
}

/// A pane's state, for handing it to the daemon that replaces this one.
pub struct Exported {
    /// The master fd, left open across exec.
    pub fd: Option<i32>,
    pub screen: Vec<u8>,
    pub title: Option<String>,
}

pub struct Proc {
    pub pid: Option<u32>,
    term: Mutex<vt100::Parser<Signals>>,
    tx: broadcast::Sender<PaneEvent>,
    input: mpsc::Sender<Vec<u8>>,
    master: Mutex<Master>,
    killer: Mutex<Killer>,
    pub last_output: AtomicI64,
    pub bytes_out: AtomicU64,
    shell: Mutex<ShellState>,
}

/// A signal's number from its description, as strsignal gives it.
fn signal_number(name: &str) -> Option<u32> {
    (1..32)
        .find(|&n| {
            // SAFETY: strsignal returns a C string (or null) for any number.
            let s = unsafe { libc::strsignal(n) };
            !s.is_null() && unsafe { std::ffi::CStr::from_ptr(s) }.to_string_lossy() == name
        })
        .map(|n| n as u32)
}

fn nz(n: u16) -> NonZeroU16 {
    NonZeroU16::new(n.max(1)).expect("non-zero")
}

/// A terminal's size (cols, rows), asked of its master fd.
fn winsize(fd: i32) -> Option<(u16, u16)> {
    let mut ws = libc::winsize { ws_row: 0, ws_col: 0, ws_xpixel: 0, ws_ypixel: 0 };
    // SAFETY: TIOCGWINSZ only writes into `ws`.
    let ok = unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut ws) } == 0;
    (ok && ws.ws_col > 0 && ws.ws_row > 0).then_some((ws.ws_col, ws.ws_row))
}

fn user_shell() -> String {
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
/// PATH and other profile settings match a normal terminal. `zdotdir` is
/// aulora's zsh integration, when the shell is zsh: a command is then
/// typed at the shell's first prompt (AULORA_RUN, see `command_line`).
pub fn argv(sh: &str, kind: PaneKind, args: &[String], zdotdir: Option<&std::path::Path>) -> Vec<String> {
    let sh = sh.to_string();
    // The shell to fall back to when a program exits, like a normal terminal.
    let fallback = match zdotdir {
        Some(dir) => format!("ZDOTDIR={} exec {} -l", quote(&dir.to_string_lossy()), quote(&sh)),
        None => format!("exec {} -l", quote(&sh)),
    };
    match kind {
        PaneKind::Shell => vec![sh, "-l".into()],
        PaneKind::Claude => {
            let extra: Vec<String> = args.iter().map(|a| quote(a)).collect();
            let claude = if extra.is_empty() { "claude".to_string() } else { format!("claude {}", extra.join(" ")) };
            vec![sh, "-l".into(), "-i".into(), "-c".into(), format!("{claude}; {fallback}")]
        }
        PaneKind::Command if zdotdir.is_some() => vec![sh, "-l".into()],
        PaneKind::Command => {
            vec![sh, "-l".into(), "-i".into(), "-c".into(), format!("{}; {fallback}", command_line(args))]
        }
    }
}

/// A command pane's command as it's typed at the prompt.
pub fn command_line(args: &[String]) -> String {
    args.iter().map(|a| quote(a)).collect::<Vec<_>>().join(" ")
}

/// Start a pane's input and output threads: writes go to the program
/// from a queue (they can block), output feeds the screen and subscribers.
fn wire(
    proc_: &Arc<Proc>,
    id: &str,
    mut reader: Box<dyn Read + Send>,
    mut writer: Box<dyn Write + Send>,
    input_rx: mpsc::Receiver<Vec<u8>>,
    on_wake: impl Fn() + Send + Sync + 'static,
) -> Result<()> {
    std::thread::Builder::new().name(format!("pane-{id}-in")).spawn(move || {
        while let Ok(data) = input_rx.recv() {
            if writer.write_all(&data).and_then(|_| writer.flush()).is_err() {
                break;
            }
        }
    })?;
    let out = proc_.clone();
    std::thread::Builder::new().name(format!("pane-{id}-out")).spawn(move || {
        let mut buf = vec![0u8; 32 * 1024];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let now = now_ms();
                    let quiet = now - out.last_output.swap(now, Ordering::Relaxed) > 1_500;
                    out.bytes_out.fetch_add(n as u64, Ordering::Relaxed);
                    let chunk: Arc<[u8]> = Arc::from(&buf[..n]);
                    let (bell, marks) = {
                        // Parse and publish under one lock so attach() never sees
                        // a screen that disagrees with the stream that follows it.
                        let mut term = out.term.lock().unwrap();
                        term.process(&chunk);
                        let _ = out.tx.send(PaneEvent::Output(chunk));
                        let signals = term.callbacks_mut();
                        (std::mem::take(&mut signals.bell), std::mem::take(&mut signals.marks))
                    };
                    let command_changed = out.note(&marks, now);
                    if quiet || bell || command_changed {
                        on_wake();
                    }
                }
            }
        }
    })?;
    Ok(())
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

        let sh = spec.shell.clone().unwrap_or_else(user_shell);
        let zdotdir = if zsh::is_zsh(&sh) { zsh::dir() } else { None };
        let argv = argv(&sh, spec.kind, &spec.args, zdotdir);
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
        if let Some(dir) = zdotdir {
            // Your own ZDOTDIR, if you have one, is where the integration
            // finds your startup files.
            match std::env::var_os("ZDOTDIR").filter(|z| !z.is_empty() && std::path::Path::new(z) != dir) {
                Some(z) => cmd.env("AULORA_USER_ZDOTDIR", z),
                None => cmd.env_remove("AULORA_USER_ZDOTDIR"),
            }
            cmd.env("ZDOTDIR", dir);
            cmd.env("AULORA_ZDOTDIR", dir);
            if spec.kind == PaneKind::Command {
                cmd.env("AULORA_RUN", command_line(&spec.args));
            }
        }
        for (k, v) in &spec.env {
            cmd.env(k, v);
        }

        let child = pair.slave.spawn_command(cmd).with_context(|| format!("starting {}", argv.join(" ")))?;
        drop(pair.slave);
        let pid = child.process_id();
        let killer = child.clone_killer();
        let reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;

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
            master: Mutex::new(Master::Pty(pair.master)),
            killer: Mutex::new(Killer::Pty(killer)),
            last_output: AtomicI64::new(now_ms()),
            bytes_out: AtomicU64::new(0),
            shell: Mutex::new(ShellState::default()),
        });

        wire(&proc_, &spec.id, reader, writer, input_rx, on_wake)?;

        let done = proc_.clone();
        let mut child = child;
        std::thread::Builder::new().name(format!("pane-{}-wait", spec.id)).spawn(move || {
            let status = child.wait();
            let exit = match status {
                // Killed by a signal: no exit code. portable-pty says 1 and
                // names the signal; give it 128 + its number, as shells do.
                Ok(s) => match s.signal() {
                    Some(name) => PaneExit {
                        code: signal_number(name).map(|n| 128 + n).unwrap_or(1),
                        signal: Some(name.to_string()),
                        at: now_ms(),
                    },
                    None => PaneExit { code: s.exit_code(), signal: None, at: now_ms() },
                },
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

    /// Take over a pane from the daemon this one replaced: its terminal's
    /// master fd (inherited across exec), its process (still our child,
    /// since exec keeps the pid) and its last screen. `fd` is None for a
    /// pane whose program had already exited.
    #[allow(clippy::too_many_arguments)]
    pub fn adopt(
        id: &str,
        fd: Option<i32>,
        pid: Option<u32>,
        cols: u16,
        rows: u16,
        screen: &[u8],
        title: Option<String>,
        on_exit: impl FnOnce(PaneExit) + Send + 'static,
        on_wake: impl Fn() + Send + Sync + 'static,
    ) -> Result<Arc<Proc>> {
        use std::os::unix::io::FromRawFd;
        // The size handed over can be the one the pane started at, not the
        // one it has now; the screen came from a parser at the terminal's
        // real size. Rebuilt any narrower, wide lines would wrap, and the
        // program's next redraw would leave broken copies of them behind.
        let (cols, rows) = fd.and_then(winsize).unwrap_or((cols, rows));
        let mut parser = vt100::Parser::new_with_callbacks(nz(rows), nz(cols), SCROLLBACK, Signals::default());
        parser.process(screen);
        parser.callbacks_mut().title = title;
        let (tx, _) = broadcast::channel(CHANNEL);
        let (input, input_rx) = mpsc::channel::<Vec<u8>>();
        let running = fd.zip(pid);
        // SAFETY: the fd was handed over open by the daemon we replaced, and
        // nothing else in this process owns it.
        let file = running.map(|(fd, _)| unsafe { std::fs::File::from_raw_fd(fd) });
        let (reader, writer) = match &file {
            Some(f) => (Some(f.try_clone()?), Some(f.try_clone()?)),
            None => (None, None),
        };
        let proc_ = Arc::new(Proc {
            pid,
            term: Mutex::new(parser),
            tx,
            input,
            master: Mutex::new(file.map(Master::Fd).unwrap_or(Master::Gone)),
            killer: Mutex::new(running.map(|(_, p)| Killer::Pid(p)).unwrap_or(Killer::Gone)),
            last_output: AtomicI64::new(now_ms()),
            bytes_out: AtomicU64::new(0),
            shell: Mutex::new(ShellState::default()),
        });
        let (Some(reader), Some(writer), Some((_, pid))) = (reader, writer, running) else {
            return Ok(proc_);
        };
        wire(&proc_, id, Box::new(reader), Box::new(writer), input_rx, on_wake)?;
        let done = proc_.clone();
        std::thread::Builder::new().name(format!("pane-{id}-wait")).spawn(move || {
            let mut status = 0;
            loop {
                // SAFETY: waiting on our own child.
                let r = unsafe { libc::waitpid(pid as i32, &mut status, 0) };
                if r == -1 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                break;
            }
            let exit = if libc::WIFEXITED(status) {
                PaneExit { code: libc::WEXITSTATUS(status) as u32, signal: None, at: now_ms() }
            } else if libc::WIFSIGNALED(status) {
                let sig = libc::WTERMSIG(status);
                PaneExit { code: 128 + sig as u32, signal: Some(format!("signal {sig}")), at: now_ms() }
            } else {
                PaneExit { code: 1, signal: None, at: now_ms() }
            };
            let term = done.term.lock().unwrap();
            let _ = done.tx.send(PaneEvent::Exited(exit.clone()));
            drop(term);
            on_exit(exit);
        })?;
        Ok(proc_)
    }

    /// Get ready to hand this pane to a new daemon over exec: its screen,
    /// and its master fd with close-on-exec cleared.
    pub fn export(&self) -> Exported {
        let fd = self.master.lock().unwrap().raw_fd();
        if let Some(fd) = fd {
            // SAFETY: fcntl on our own open fd.
            unsafe {
                let flags = libc::fcntl(fd, libc::F_GETFD);
                if flags >= 0 {
                    libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC);
                }
            }
        }
        let term = self.term.lock().unwrap();
        let mut screen = b"\x1b[H\x1b[2J".to_vec();
        screen.extend_from_slice(term.screen().state_formatted().as_bytes());
        Exported { fd, screen, title: term.callbacks().title.clone() }
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
        self.master.lock().unwrap().resize(rows, cols)?;
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
        match &mut *self.killer.lock().unwrap() {
            Killer::Pty(k) => {
                let _ = k.kill();
            }
            // SAFETY: signalling our own child; SIGHUP as a closed terminal would.
            Killer::Pid(pid) => unsafe {
                libc::kill(*pid as i32, libc::SIGHUP);
            },
            Killer::Gone => {}
        }
    }

    pub fn last_output_at(&self) -> Ms {
        self.last_output.load(Ordering::Relaxed)
    }

    /// What the pane's shell is doing with its last command, when it says.
    pub fn command(&self) -> Option<ShellCommand> {
        self.shell.lock().unwrap().command()
    }

    /// Carry over what a replaced daemon knew.
    pub fn set_command(&self, command: Option<ShellCommand>) {
        *self.shell.lock().unwrap() = ShellState::resume(command);
    }

    /// Take in the shell's reports; true when the command state changed.
    fn note(&self, marks: &[Mark], now: Ms) -> bool {
        if marks.is_empty() {
            return false;
        }
        let mut shell = self.shell.lock().unwrap();
        let before = shell.command();
        for m in marks {
            shell.note(*m, now);
        }
        shell.command() != before
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
            // A plain shell, so tests don't depend on your zsh setup.
            shell: Some("/bin/sh".into()),
            env: vec![],
        }
    }

    fn wait_until(what: impl Fn() -> bool) -> bool {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(10) {
            if what() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn runs_a_shell_and_reports_exit() {
        let (tx, rx) = mpsc::channel();
        let p = Proc::spawn(&spec(PaneKind::Shell, &[]), move |e| tx.send(e).unwrap(), || {}).unwrap();
        p.write(b"printf 'hello from %s' \"$AULORA_PANE_ID\"; exit 3\r".to_vec());
        assert!(wait_for(&p, "hello from test"), "screen was: {:?}", p.text());
        let exit = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(exit.code, 3);
    }

    #[test]
    fn a_pane_killed_by_a_signal_says_which() {
        let (tx, rx) = mpsc::channel();
        let p = Proc::spawn(&spec(PaneKind::Shell, &[]), move |e| tx.send(e).unwrap(), || {}).unwrap();
        p.write(b"kill -9 $$\r".to_vec());
        let exit = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(exit.code, 128 + 9);
        assert!(exit.signal.is_some());
    }

    #[test]
    fn a_command_pane_drops_back_to_its_shell() {
        let (tx, rx) = mpsc::channel();
        let p = Proc::spawn(
            &spec(PaneKind::Command, &["sh", "-c", "printf served; exit 4"]),
            move |e| tx.send(e).unwrap(),
            || {},
        )
        .unwrap();
        assert!(wait_for(&p, "served"), "screen was: {:?}", p.text());
        p.write(b"echo still here\r".to_vec());
        assert!(wait_for(&p, "still here"), "screen was: {:?}", p.text());
        assert!(rx.try_recv().is_err(), "the pane is still open");
        p.kill();
    }

    #[test]
    fn zsh_says_how_each_command_ended_and_keeps_it_in_history() {
        if !std::path::Path::new("/bin/zsh").exists() {
            return;
        }
        // An empty ZDOTDIR of your own: none of your startup files.
        let home = std::env::temp_dir().join(format!("aulora-zsh-test-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        let mut spec = spec(PaneKind::Command, &["sh", "-c", "printf served; exit 3"]);
        spec.shell = Some("/bin/zsh".into());
        spec.env = vec![("AULORA_USER_ZDOTDIR".into(), home.to_string_lossy().to_string())];
        let (tx, rx) = mpsc::channel();
        let p = Proc::spawn(&spec, move |e| tx.send(e).unwrap(), || {}).unwrap();
        let code = || match p.command() {
            Some(ShellCommand::Ended { code, .. }) => Some(code),
            _ => None,
        };
        assert!(wait_until(|| code() == Some(3)), "screen was: {:?}", p.text());
        assert!(p.text().contains("served"));

        // Up, Enter: the same command again, from the shell's history.
        p.write(b"\x1b[A\r".to_vec());
        assert!(wait_until(|| p.text().matches("served").count() >= 2), "screen was: {:?}", p.text());
        p.write(b"true\r".to_vec());
        assert!(wait_until(|| code() == Some(0)), "screen was: {:?}", p.text());
        p.write(b"sleep 30\r".to_vec());
        assert!(wait_until(|| matches!(p.command(), Some(ShellCommand::Running { .. }))), "and says one runs");
        assert!(rx.try_recv().is_err(), "the pane is still open");
        p.kill();
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_dev_server_that_stops_without_an_error_is_reported_as_ended() {
        if !std::path::Path::new("/bin/zsh").exists() {
            return;
        }
        let home = std::env::temp_dir().join(format!("aulora-zsh-clean-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        let mut spec = spec(PaneKind::Command, &["sh", "-c", "echo listening on :3000; sleep 0.3; exit 0"]);
        spec.shell = Some("/bin/zsh".into());
        spec.env = vec![("AULORA_USER_ZDOTDIR".into(), home.to_string_lossy().to_string())];
        let (tx, rx) = mpsc::channel();
        let p = Proc::spawn(&spec, move |e| tx.send(e).unwrap(), || {}).unwrap();
        assert!(wait_until(|| matches!(p.command(), Some(ShellCommand::Running { .. }))), "screen: {:?}", p.text());
        assert!(
            wait_until(|| matches!(p.command(), Some(ShellCommand::Ended { code: 0, .. }))),
            "screen: {:?}",
            p.text()
        );
        assert!(p.text().contains("listening on :3000"));
        assert!(rx.try_recv().is_err(), "the shell is still open");
        p.kill();
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn zsh_says_when_jobs_run_behind_the_prompt_are_paused_and_end() {
        if !std::path::Path::new("/bin/zsh").exists() {
            return;
        }
        let home = std::env::temp_dir().join(format!("aulora-zsh-jobs-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        let mut spec = spec(PaneKind::Shell, &[]);
        spec.shell = Some("/bin/zsh".into());
        spec.env = vec![("AULORA_USER_ZDOTDIR".into(), home.to_string_lossy().to_string())];
        let p = Proc::spawn(&spec, |_| {}, || {}).unwrap();
        let is = |want: &dyn Fn(&ShellCommand) -> bool| wait_until(|| p.command().as_ref().is_some_and(want));

        p.write(b"sh -c 'sleep 1; exit 3' &\r".to_vec());
        assert!(is(&|c| matches!(c, ShellCommand::Running { background: true, .. })), "screen: {:?}", p.text());
        assert!(is(&|c| matches!(c, ShellCommand::Ended { code: 3, .. })), "screen: {:?}", p.text());

        p.write(b"sleep 30\r".to_vec());
        assert!(is(&|c| matches!(c, ShellCommand::Running { background: false, .. })));
        p.write(b"\x1a".to_vec());
        assert!(is(&|c| matches!(c, ShellCommand::Paused { .. })), "screen: {:?}", p.text());
        p.write(b"kill -9 %1\r".to_vec());
        assert!(is(&|c| matches!(c, ShellCommand::Ended { code: 137, .. })), "screen: {:?}", p.text());
        p.kill();
        let _ = std::fs::remove_dir_all(&home);
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
    fn an_adopted_pane_takes_its_terminals_real_size() {
        // A pane started at 120x36 and widened since: an older daemon
        // hands over the start size, with a screen drawn 160 wide.
        let pair =
            native_pty_system().openpty(PtySize { rows: 20, cols: 160, pixel_width: 0, pixel_height: 0 }).unwrap();
        let fd = pair.master.as_raw_fd().unwrap();
        let rule = "─".repeat(160);
        let screen = format!("\x1b[H\x1b[2J{rule}\r\n❯ hi");
        let p = Proc::adopt("t", Some(fd), None, 120, 36, screen.as_bytes(), None, |_| {}, || {}).unwrap();
        assert_eq!(p.size(), (160, 20));
        assert_eq!(p.text().lines().collect::<Vec<_>>(), [rule.as_str(), "❯ hi"], "the rule wrapped");
    }

    #[test]
    fn claude_session_markers_are_not_inherited() {
        // What Claude Code 2.1 sets for a hook, which may start the daemon.
        for k in [
            "AI_AGENT",
            "CLAUDECODE",
            "CLAUDE_CODE_CHILD_SESSION",
            "CLAUDE_CODE_ENTRYPOINT",
            "CLAUDE_CODE_MESSAGING_SOCKET",
            "CLAUDE_CODE_MESSAGING_TOKEN",
            "CLAUDE_CODE_SESSION_ATTENDED",
            "CLAUDE_CODE_SESSION_ID",
            "CLAUDE_PID",
            "CLAUDE_PROJECT_DIR",
        ] {
            assert!(STRIP_ENV.contains(&k), "{k} reaches panes");
        }
    }

    #[test]
    fn claude_panes_fall_back_to_a_shell() {
        let a = argv("/bin/zsh", PaneKind::Claude, &["--resume".into(), "abc def".into()], None);
        assert_eq!(a[1..4], ["-l", "-i", "-c"]);
        assert_eq!(a[4], "claude --resume 'abc def'; exec /bin/zsh -l");
        let dir = std::path::Path::new("/a/zsh dir");
        let a = argv("/bin/zsh", PaneKind::Claude, &[], Some(dir));
        assert_eq!(a[4], "claude; ZDOTDIR='/a/zsh dir' exec /bin/zsh -l", "the shell after it is integrated too");
        assert_eq!(argv("/bin/zsh", PaneKind::Command, &["pnpm".into(), "dev".into()], Some(dir)), ["/bin/zsh", "-l"]);
    }
}
