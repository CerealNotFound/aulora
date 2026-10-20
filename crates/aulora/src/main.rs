mod hook;
mod install;
mod vscode;

use std::io::{IsTerminal, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use aulora_core::model::{Pane, PaneKind, SessionState, Snapshot, now_ms};
use aulora_core::paths;
use aulora_core::proto::{self, ClientMsg, NewPane, ServerMsg};
use aulora_core::util::{short_duration, truncate};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "aulora",
    version,
    about = "See every Claude Code session across all your workspaces, and what each one needs from you."
)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Open the dashboard (the default when no command is given).
    Tui {
        /// Show made-up sessions instead of live data.
        #[arg(long)]
        demo: bool,
        /// Print one frame as plain text at WIDTHxHEIGHT and exit.
        #[arg(long, value_name = "WxH")]
        snapshot: Option<String>,
        /// Select this pane on start (id, prefix, name or workspace/name).
        #[arg(long)]
        select: Option<String>,
        /// Theme to use (press t in the dashboard to browse them).
        #[arg(long)]
        theme: Option<String>,
        /// Tab to start on: terminal, conversation, overview, changes or files.
        #[arg(long)]
        tab: Option<String>,
        /// With --snapshot: print cells with colors as JSON lines.
        #[arg(long, hide = true)]
        json: bool,
    },
    /// Open the dashboard with made-up sessions.
    Demo {
        #[arg(long)]
        theme: Option<String>,
    },
    /// Add or remove aulora's Claude Code hooks.
    Hooks {
        #[command(subcommand)]
        action: HooksCmd,
    },
    /// Run or control the background daemon.
    Daemon {
        #[command(subcommand)]
        action: Option<DaemonCmd>,
    },
    /// Print every session and its state.
    Status {
        #[arg(long)]
        json: bool,
        /// Include past sessions found in transcripts.
        #[arg(long)]
        all: bool,
    },
    /// Check that hooks, daemon and editor are set up.
    Doctor,
    /// Start a new terminal pane in the daemon.
    New {
        /// Run Claude Code (falls back to a shell when it exits).
        #[arg(long)]
        claude: bool,
        /// Pane name (default: "shell", "claude" or the command name).
        #[arg(long, short)]
        name: Option<String>,
        /// Working directory (default: the current directory).
        #[arg(long)]
        cwd: Option<PathBuf>,
        /// Attach to the pane right away.
        #[arg(long, short)]
        attach: bool,
        /// Command to run instead of a shell. Extra arguments for Claude with --claude.
        #[arg(last = true, value_name = "COMMAND")]
        command: Vec<String>,
    },
    /// List the daemon's panes.
    #[command(visible_alias = "ls")]
    Panes {
        #[arg(long)]
        json: bool,
    },
    /// Put a pane in this terminal. Ctrl-Q detaches.
    Attach {
        /// Pane id, id prefix, name, or workspace/name.
        pane: String,
    },
    /// Stop a pane's process, keeping its last screen.
    Kill { pane: String },
    /// Remove a pane, stopping it first if needed.
    Close { pane: String },
    /// Start an exited pane's program again.
    Restart { pane: String },
    /// Type a prompt into a session, or a command into a pane, as if typed
    /// there, then press Enter. Reads stdin when no text is given.
    Send {
        /// A pane (id, name, workspace/name), or a session (id prefix or title).
        target: String,
        text: Vec<String>,
    },
    /// The VS Code (or Cursor) extension that lets aulora type into
    /// sessions running in the editor's terminals.
    Vscode {
        #[command(subcommand)]
        action: VscodeCmd,
    },
    /// Open a folder as a workspace, like `code .`: start a shell (or Claude)
    /// there and show it in the dashboard.
    Open {
        /// The folder; the current one if left out.
        dir: Option<PathBuf>,
        /// Start Claude Code instead of a shell.
        #[arg(long)]
        claude: bool,
        /// Only start the pane; don't open the dashboard.
        #[arg(long)]
        no_ui: bool,
    },
    /// Bring the terminal window running the dashboard to the front
    /// (Terminal.app, iTerm2). Bind it to a global key with macOS Shortcuts
    /// or Raycast; starts the dashboard in a new Terminal window if none runs.
    Focus,
    /// Show the bytes each key press sends and what they mean, to check
    /// what your terminal does with Option, Shift-Enter and the like.
    Keys,
    /// Entry point for Claude Code hooks (reads the event on stdin).
    #[command(hide = true)]
    Hook,
}

#[derive(Subcommand)]
enum VscodeCmd {
    /// Install or update the extension.
    Install {
        /// The editor's command: code, code-insiders, cursor, windsurf.
        #[arg(long, default_value = "code")]
        editor: String,
    },
    /// Remove the extension.
    Uninstall {
        #[arg(long, default_value = "code")]
        editor: String,
    },
    /// Show whether the extension is installed.
    Status {
        #[arg(long, default_value = "code")]
        editor: String,
    },
    /// Write the extension as a .vsix file.
    Package {
        #[arg(default_value = "aulora.vsix")]
        out: PathBuf,
    },
}

#[derive(Subcommand)]
enum HooksCmd {
    /// Add aulora's hooks to your Claude Code settings.
    Install {
        /// Settings file to edit (default: ~/.claude/settings.json).
        #[arg(long)]
        settings: Option<PathBuf>,
        /// Show the result without writing anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// Remove aulora's hooks, leaving everything else untouched.
    Uninstall {
        #[arg(long)]
        settings: Option<PathBuf>,
        #[arg(long)]
        dry_run: bool,
    },
    /// Show whether the hooks are installed.
    Status {
        #[arg(long)]
        settings: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum DaemonCmd {
    /// Run in the foreground (what `aulora daemon` does with no subcommand).
    Run {
        /// Take over from a daemon that is upgrading (used by the daemon).
        #[arg(long, hide = true)]
        handover: bool,
    },
    /// Restart in place with this aulora, keeping every pane running.
    Restart,
    /// Start in the background if it isn't running.
    Start,
    /// Stop the running daemon. Refuses while panes are running.
    Stop {
        /// Stop anyway, ending every running pane.
        #[arg(long)]
        force: bool,
    },
    /// Show whether the daemon is running.
    Status,
}

fn main() {
    // The hook path must be fast and silent: handle it before anything else.
    if std::env::args().nth(1).as_deref() == Some("hook") {
        hook::run();
        return;
    }
    let cli = Cli::parse();
    if let Err(e) = dispatch(cli) {
        eprintln!("aulora: {e:#}");
        std::process::exit(1);
    }
}

fn dispatch(cli: Cli) -> Result<()> {
    match cli.cmd {
        None => {
            if !std::io::stdout().is_terminal() {
                // Never start a full-screen UI without a terminal.
                use clap::CommandFactory;
                Cli::command().print_help()?;
                return Ok(());
            }
            tui(false, None, None, None, None, false)
        }
        Some(Cmd::Tui { demo, snapshot, select, theme, tab, json }) => tui(demo, snapshot, select, theme, tab, json),
        Some(Cmd::Demo { theme }) => tui(true, None, None, theme, None, false),
        Some(Cmd::Hook) => {
            hook::run();
            Ok(())
        }
        Some(Cmd::Hooks { action }) => hooks(action),
        Some(Cmd::Daemon { action }) => daemon(action.unwrap_or(DaemonCmd::Run { handover: false })),
        Some(Cmd::Status { json, all }) => status(json, all),
        Some(Cmd::Doctor) => doctor(),
        Some(Cmd::New { claude, name, cwd, attach, command }) => new_pane(claude, name, cwd, attach, command),
        Some(Cmd::Panes { json }) => list_panes(json),
        Some(Cmd::Attach { pane }) => {
            ensure_daemon()?;
            let p = find_pane(&pane)?;
            aulora_tui::attach(&paths::socket(), &p.id, &pane_label(&p))
        }
        Some(Cmd::Kill { pane }) => pane_command(&pane, "stopped", |id| ClientMsg::KillPane { pane_id: id }),
        Some(Cmd::Close { pane }) => pane_command(&pane, "closed", |id| ClientMsg::ClosePane { pane_id: id }),
        Some(Cmd::Restart { pane }) => pane_command(&pane, "restarted", |id| ClientMsg::RestartPane { pane_id: id }),
        Some(Cmd::Send { target, text }) => send(&target, text),
        Some(Cmd::Keys) => Ok(aulora_tui::keys_probe()?),
        Some(Cmd::Focus) => focus(),
        Some(Cmd::Open { dir, claude, no_ui }) => open(dir, claude, no_ui),
        Some(Cmd::Vscode { action }) => match action {
            VscodeCmd::Install { editor } => vscode::install(&editor),
            VscodeCmd::Uninstall { editor } => vscode::uninstall(&editor),
            VscodeCmd::Status { editor } => vscode::status(&editor),
            VscodeCmd::Package { out } => {
                std::fs::write(&out, vscode::vsix())?;
                println!("wrote {}", out.display());
                Ok(())
            }
        },
    }
}

fn exe() -> Result<PathBuf> {
    let p = std::env::current_exe().context("finding the aulora executable")?;
    Ok(p.canonicalize().unwrap_or(p))
}

fn hooks_installed() -> bool {
    install::read(&install::default_settings_path()).map(|v| !install::installed_events(&v).is_empty()).unwrap_or(false)
}

fn tui(
    demo: bool,
    snapshot: Option<String>,
    select: Option<String>,
    theme: Option<String>,
    tab: Option<String>,
    json: bool,
) -> Result<()> {
    let exe = exe()?;
    // Panes are looked up by name; anything else (a session id, or a demo
    // pane) is passed through as given.
    let select = match (&select, demo) {
        (Some(q), false) => Some(find_pane(q).map(|p| p.id).unwrap_or_else(|_| q.clone())),
        (q, true) => q.clone(),
        (None, _) => None,
    };
    if let Some(t) = &theme {
        anyhow::ensure!(
            aulora_tui::theme_exists(t),
            "unknown theme {t:?}; known: {}",
            aulora_tui::theme_names().join(", ")
        );
    }
    let opts = aulora_tui::Options {
        theme,
        tab,
        select: select.clone(),
        demo,
        socket: paths::socket(),
        hooks_installed: demo || hooks_installed(),
        editor: std::env::var("AULORA_EDITOR").unwrap_or_else(|_| "code".into()),
        spawn_daemon: if demo {
            None
        } else {
            Some(Box::new(move || {
                let _ = aulora_daemon::spawn_detached(&exe);
            }))
        },
    };
    if let Some(size) = snapshot {
        let (w, h) = size
            .split_once(['x', 'X'])
            .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
            .context("--snapshot takes WIDTHxHEIGHT, e.g. 180x50")?;
        let live = if demo { None } else { request_snapshot().ok() };
        let screens = select.iter().filter_map(|id| fetch_screen(id).ok()).collect();
        let frame = if json {
            aulora_tui::render_json(&opts, live, screens, w, h)
        } else {
            aulora_tui::render_text(&opts, live, screens, w, h)
        };
        print!("{frame}");
        return Ok(());
    }
    aulora_tui::run(opts)
}

// --- daemon -----------------------------------------------------------------

fn request(msg: &ClientMsg) -> Result<ServerMsg> {
    request_after(&[], msg)
}

/// Send `before` (messages with no reply), then `msg`, and read its reply.
fn request_after(before: &[ClientMsg], msg: &ClientMsg) -> Result<ServerMsg> {
    let mut s = UnixStream::connect(paths::socket()).context("the aulora daemon is not running")?;
    // Long enough for an editor to confirm it typed a prompt.
    s.set_read_timeout(Some(Duration::from_secs(8)))?;
    let mut line = String::new();
    for m in before.iter().chain([msg]) {
        line.push_str(&serde_json::to_string(m)?);
        line.push('\n');
    }
    s.write_all(line.as_bytes())?;
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    // Read exactly one line.
    loop {
        match s.read(&mut byte)? {
            0 => break,
            _ if byte[0] == b'\n' => break,
            _ => buf.push(byte[0]),
        }
    }
    Ok(serde_json::from_slice(&buf)?)
}

fn request_snapshot() -> Result<Snapshot> {
    let mut s = UnixStream::connect(paths::socket()).context("the aulora daemon is not running")?;
    s.set_read_timeout(Some(Duration::from_secs(3)))?;
    s.write_all(b"{\"type\":\"snapshot\"}\n")?;
    let mut reader = std::io::BufReader::new(s);
    let mut line = String::new();
    std::io::BufRead::read_line(&mut reader, &mut line)?;
    match serde_json::from_str::<ServerMsg>(&line)? {
        ServerMsg::Snapshot(s) => Ok(*s),
        other => bail!("unexpected reply: {other:?}"),
    }
}

/// One pane's current screen, for `tui --snapshot`.
fn fetch_screen(pane_id: &str) -> Result<(String, u16, u16, Vec<u8>)> {
    let mut s = UnixStream::connect(paths::socket()).context("the aulora daemon is not running")?;
    s.set_read_timeout(Some(Duration::from_secs(3)))?;
    let mut line = serde_json::to_string(&ClientMsg::Attach { pane_id: pane_id.to_string() })?;
    line.push('\n');
    s.write_all(line.as_bytes())?;
    let mut reader = std::io::BufReader::new(s);
    let mut reply = String::new();
    std::io::BufRead::read_line(&mut reader, &mut reply)?;
    match serde_json::from_str::<ServerMsg>(&reply)? {
        ServerMsg::PaneScreen { pane_id, cols, rows, data } => {
            Ok((pane_id, cols, rows, aulora_core::proto::decode(&data)))
        }
        other => bail!("unexpected reply: {other:?}"),
    }
}

fn daemon_running() -> Option<String> {
    match request(&ClientMsg::Ping) {
        Ok(ServerMsg::Pong { version, .. }) => Some(version),
        _ => None,
    }
}

/// Start the daemon if needed. Returns its version.
fn ensure_daemon() -> Result<String> {
    if let Some(v) = daemon_running() {
        return Ok(v);
    }
    aulora_daemon::spawn_detached(&exe()?)?;
    for _ in 0..60 {
        std::thread::sleep(Duration::from_millis(50));
        if let Some(v) = daemon_running() {
            return Ok(v);
        }
    }
    bail!("daemon did not start; see {}", paths::log().display())
}

fn daemon(action: DaemonCmd) -> Result<()> {
    match action {
        DaemonCmd::Run { handover } => {
            init_logging()?;
            let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?;
            let res = rt.block_on(aulora_daemon::run(handover));
            if let Err(e) = &res {
                tracing::error!("daemon failed: {e:#}");
            }
            // Don't let a stuck background task keep the process (and its lock) alive.
            rt.shutdown_timeout(Duration::from_secs(1));
            res
        }
        DaemonCmd::Restart => {
            let Ok(snap) = request_snapshot() else {
                let v = ensure_daemon()?;
                println!("daemon wasn't running; started it (v{v})");
                return Ok(());
            };
            let running = snap.panes.iter().filter(|p| p.is_running()).count();
            let exe = exe()?;
            match request(&ClientMsg::Upgrade { exe: Some(exe.to_string_lossy().to_string()) })? {
                ServerMsg::Error { message } if message.contains("bad message") => bail!(
                    "this daemon is from before in-place restarts, so it can't keep its panes.\n\
                     Restart it the old way, ending {running} pane(s): aulora daemon stop --force && aulora daemon start\n\
                     From then on, restarts and updates keep panes."
                ),
                ServerMsg::Error { message } => bail!(message),
                _ => {}
            }
            // The socket goes away and comes back with the new daemon.
            std::thread::sleep(Duration::from_millis(300));
            for _ in 0..100 {
                if let Ok(s) = request_snapshot() {
                    let kept = s.panes.iter().filter(|p| p.is_running()).count();
                    println!("daemon restarted; {kept} of {running} running pane(s) kept");
                    return Ok(());
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            bail!("the daemon didn't come back within 10 seconds; see {}", paths::log().display())
        }
        DaemonCmd::Start => {
            if let Some(v) = daemon_running() {
                println!("daemon already running (v{v})");
                return Ok(());
            }
            let v = ensure_daemon()?;
            println!("daemon started (v{v})");
            Ok(())
        }
        DaemonCmd::Stop { force } => {
            let Ok(snap) = request_snapshot() else {
                println!("daemon is not running");
                return Ok(());
            };
            let running: Vec<String> = snap.panes.iter().filter(|p| p.is_running()).map(pane_label).collect();
            if !running.is_empty() && !force {
                bail!(
                    "{} running pane(s) would be ended: {}\nTo keep them, use `aulora daemon restart` instead. Use --force to stop anyway.",
                    running.len(),
                    running.join(", ")
                );
            }
            request(&ClientMsg::Shutdown)?;
            // Report success only once it's really gone.
            for _ in 0..60 {
                if daemon_running().is_none() && std::os::unix::net::UnixStream::connect(paths::socket()).is_err() {
                    println!("daemon stopped");
                    return Ok(());
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            bail!("the daemon didn't exit within 3 seconds; see {}", paths::log().display())
        }
        DaemonCmd::Status => {
            match daemon_running() {
                Some(v) => println!("running · v{v} · {}", paths::socket().display()),
                None => println!("not running"),
            }
            Ok(())
        }
    }
}

fn init_logging() -> Result<()> {
    paths::ensure_home()?;
    let file = std::fs::OpenOptions::new().create(true).append(true).open(paths::log())?;
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_env("AULORA_LOG").unwrap_or_else(|_| "info".into()))
        .with_ansi(false)
        .with_writer(std::sync::Mutex::new(file))
        .init();
    Ok(())
}

// --- hooks ------------------------------------------------------------------

fn hooks(action: HooksCmd) -> Result<()> {
    match action {
        HooksCmd::Install { settings, dry_run } => {
            let path = settings.unwrap_or_else(install::default_settings_path);
            let exe = exe()?;
            let out = install::install(&path, &exe, dry_run)?;
            if dry_run {
                println!("Would write these hooks to {}:\n{}", path.display(), out.preview);
            } else if !out.changed {
                println!("Hooks already installed in {}", path.display());
            } else {
                println!("Installed {} hooks in {}", install::EVENTS.len(), path.display());
                println!("  command: {}", install::hook_command(&exe));
                if let Some(b) = out.backup {
                    println!("  backup:  {}", b.display());
                }
                println!("New and resumed Claude Code sessions will now show up in `aulora`.");
            }
            Ok(())
        }
        HooksCmd::Uninstall { settings, dry_run } => {
            let path = settings.unwrap_or_else(install::default_settings_path);
            let out = install::uninstall(&path, dry_run)?;
            if dry_run {
                println!("Hooks after removal:\n{}", out.preview);
            } else if out.changed {
                println!("Removed aulora's hooks from {}", path.display());
                if let Some(b) = out.backup {
                    println!("  backup: {}", b.display());
                }
            } else {
                println!("No aulora hooks found in {}", path.display());
            }
            Ok(())
        }
        HooksCmd::Status { settings } => {
            let path = settings.unwrap_or_else(install::default_settings_path);
            let v = install::read(&path)?;
            let events = install::installed_events(&v);
            if events.is_empty() {
                println!("not installed in {}", path.display());
            } else {
                println!("installed for {}/{} events in {}", events.len(), install::EVENTS.len(), path.display());
                if let Some(cmd) = install::installed_command(&v) {
                    println!("  command: {cmd}");
                }
            }
            Ok(())
        }
    }
}

// --- status / doctor -------------------------------------------------------

fn status(json: bool, all: bool) -> Result<()> {
    let snap = request_snapshot()?;
    if json {
        println!("{}", serde_json::to_string_pretty(&snap)?);
        return Ok(());
    }
    let color = std::io::stdout().is_terminal();
    let paint = |code: &str, s: &str| if color { format!("\x1b[{code}m{s}\x1b[0m") } else { s.to_string() };
    let now = now_ms();
    let mut sessions: Vec<_> =
        snap.sessions.iter().filter(|s| all || s.origin != aulora_core::model::Origin::History).collect();
    sessions.sort_by_key(|s| (s.state.rank(), -s.last_event_at));
    if sessions.is_empty() {
        println!("No sessions yet. Start `claude` in a folder (hooks: `aulora hooks status`).");
        return Ok(());
    }
    for s in sessions {
        let project = aulora_core::util::basename(&s.project_root);
        let (code, label) = match &s.state {
            SessionState::NeedsYou { .. } => ("1;33", "NEEDS YOU"),
            SessionState::Errored { .. } => ("1;31", "ERROR"),
            SessionState::YourTurn => ("1;32", "YOUR TURN"),
            SessionState::Working => ("1;36", "WORKING"),
            SessionState::Compacting => ("1;35", "COMPACTING"),
            SessionState::Idle => ("2", "IDLE"),
            SessionState::Ended { .. } => ("2", "ENDED"),
        };
        let detail = match &s.state {
            SessionState::NeedsYou { detail, .. } | SessionState::Errored { detail } => detail.clone(),
            SessionState::Working => {
                s.current_tool.as_ref().map(|t| format!("{} {}", t.name, t.target)).unwrap_or_default()
            }
            _ => String::new(),
        };
        println!(
            "{}  {:<16} {:<44} {:>6}  {}",
            paint(code, &format!("{label:<10}")),
            truncate(&project, 16),
            truncate(&s.display_title(), 44),
            short_duration(now - s.state_since),
            paint("2", &truncate(&detail, 60)),
        );
    }
    Ok(())
}

fn doctor() -> Result<()> {
    let ok = |b: bool| if b { "✓" } else { "✗" };
    let exe = exe()?;
    println!("aulora {}", aulora_core::VERSION);
    println!("  binary    {}", exe.display());
    println!("  home      {}", paths::home().display());

    let settings = install::default_settings_path();
    let v = install::read(&settings).unwrap_or_default();
    let events = install::installed_events(&v);
    let cmd = install::installed_command(&v);
    let same_binary = cmd.as_deref() == Some(install::hook_command(&exe).as_str());
    println!(
        "{} hooks     {}/{} events in {}",
        ok(events.len() == install::EVENTS.len()),
        events.len(),
        install::EVENTS.len(),
        settings.display()
    );
    if !events.is_empty() && !same_binary {
        println!("    note: hooks point at a different binary: {}", cmd.unwrap_or_default());
        println!("    run `aulora hooks install` to update them");
    }
    if events.is_empty() {
        println!("    run `aulora hooks install`");
    }

    let daemon = daemon_running();
    println!(
        "{} daemon    {}",
        ok(daemon.is_some()),
        daemon.map(|v| format!("running · v{v}")).unwrap_or_else(|| "not running (starts automatically)".into())
    );

    let which = |bin: &str| {
        std::process::Command::new("sh")
            .args(["-c", &format!("command -v {bin}")])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    let editor = std::env::var("AULORA_EDITOR").unwrap_or_else(|_| "code".into());
    let ed = which(&editor);
    println!("{} editor    {} {}", ok(ed.is_some()), editor, ed.unwrap_or_else(|| "not found on PATH".into()));
    let claude = which("claude");
    println!("{} claude    {}", ok(claude.is_some()), claude.unwrap_or_else(|| "not found on PATH".into()));
    let _ = now_ms();
    Ok(())
}

// --- panes ------------------------------------------------------------------

fn pane_label(p: &Pane) -> String {
    format!("{}/{}", aulora_core::util::basename(&p.project_root), p.name)
}

/// Find a pane by id, unique id prefix, name, or project/name.
fn find_pane(query: &str) -> Result<Pane> {
    let panes = request_snapshot()?.panes;
    let exact = |p: &&Pane| p.id == query || pane_label(p) == query;
    if let Some(p) = panes.iter().find(exact) {
        return Ok(p.clone());
    }
    let matches: Vec<&Pane> = panes.iter().filter(|p| p.id.starts_with(query) || p.name == query).collect();
    match matches.as_slice() {
        [p] => Ok((*p).clone()),
        [] => bail!("no pane matches \"{query}\" (see `aulora ls`)"),
        many => bail!(
            "\"{query}\" matches {} panes: {}",
            many.len(),
            many.iter().map(|p| format!("{} ({})", p.id, pane_label(p))).collect::<Vec<_>>().join(", ")
        ),
    }
}

fn send(target: &str, text: Vec<String>) -> Result<()> {
    let mut text = text.join(" ");
    if text.is_empty() {
        std::io::stdin().read_to_string(&mut text)?;
        text = text.trim_end_matches('\n').to_string();
    }
    if text.trim().is_empty() {
        bail!("nothing to send");
    }
    let snap = request_snapshot()?;
    // A pane first: the text goes straight to its terminal.
    if let Ok(p) = find_pane(target) {
        let mut bytes =
            if text.contains('\n') { format!("\x1b[200~{text}\x1b[201~") } else { text.clone() }.into_bytes();
        bytes.push(b'\r');
        // Input has no reply; the ping's answer says it was handled.
        request_after(&[ClientMsg::Input { pane_id: p.id.clone(), data: proto::encode(&bytes) }], &ClientMsg::Ping)?;
        println!("sent to {} ({})", pane_label(&p), p.id);
        return Ok(());
    }
    let live: Vec<_> = snap.sessions.iter().filter(|s| s.state.is_live()).collect();
    let matches: Vec<_> = live
        .iter()
        .filter(|s| s.id.starts_with(target) || s.display_title().to_lowercase().contains(&target.to_lowercase()))
        .collect();
    let s = match matches.as_slice() {
        [s] => s,
        [] => bail!("no pane or live session matches \"{target}\" (see `aulora ls` and `aulora status`)"),
        many => bail!(
            "\"{target}\" matches {} sessions: {}",
            many.len(),
            many.iter().map(|s| format!("{} ({})", &s.id[..8], s.display_title())).collect::<Vec<_>>().join(", ")
        ),
    };
    // A session in one of our panes: type into the pane.
    if let Some(pane) = s.terminal.pane_id.as_deref().filter(|id| snap.panes.iter().any(|p| &p.id == id)) {
        let mut bytes =
            if text.contains('\n') { format!("\x1b[200~{text}\x1b[201~") } else { text.clone() }.into_bytes();
        bytes.push(b'\r');
        request_after(
            &[ClientMsg::Input { pane_id: pane.to_string(), data: proto::encode(&bytes) }],
            &ClientMsg::Ping,
        )?;
        println!("sent to {} (pane {pane})", s.display_title());
        return Ok(());
    }
    match request(&ClientMsg::SendPrompt { session_id: s.id.clone(), text })? {
        ServerMsg::Error { message } => bail!(message),
        _ => {
            let place = s.terminal.editor.clone().unwrap_or_else(|| "its editor".into());
            println!("sent to {} in {place}", s.display_title());
            Ok(())
        }
    }
}

fn pane_command(query: &str, done: &str, msg: impl Fn(String) -> ClientMsg) -> Result<()> {
    let p = find_pane(query)?;
    match request(&msg(p.id.clone()))? {
        ServerMsg::Error { message } => bail!(message),
        _ => {
            println!("{done} {} ({})", pane_label(&p), p.id);
            Ok(())
        }
    }
}

fn new_pane(
    claude: bool,
    name: Option<String>,
    cwd: Option<PathBuf>,
    attach: bool,
    command: Vec<String>,
) -> Result<()> {
    ensure_daemon()?;
    let cwd = match cwd {
        Some(c) => c,
        None => std::env::current_dir()?,
    };
    let cwd = cwd.canonicalize().with_context(|| format!("{} does not exist", cwd.display()))?;
    let kind = if claude {
        PaneKind::Claude
    } else if command.is_empty() {
        PaneKind::Shell
    } else {
        PaneKind::Command
    };
    let (cols, rows) = ratatui_size();
    let req = NewPane { cwd: cwd.to_string_lossy().to_string(), kind, args: command, name, cols, rows };
    let id = match request(&ClientMsg::NewPane(req))? {
        ServerMsg::PaneCreated { pane_id } => pane_id,
        ServerMsg::Error { message } => bail!(message),
        other => bail!("unexpected reply: {other:?}"),
    };
    let p = find_pane(&id)?;
    if attach {
        return aulora_tui::attach(&paths::socket(), &id, &pane_label(&p));
    }
    println!("{id}  {}", pane_label(&p));
    Ok(())
}

/// The terminals (ttys) of running dashboards.
fn dashboard_ttys() -> Vec<String> {
    let Ok(out) = std::process::Command::new("ps").args(["-axo", "tty=,args="]).output() else { return vec![] };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let tty = it.next()?;
            let exe = it.next()?;
            let rest: Vec<&str> = it.collect();
            let is_aulora = exe == "aulora" || exe.ends_with("/aulora");
            let ui = rest.is_empty() || rest[0] == "tui" || rest[0] == "open";
            (is_aulora && ui && tty != "??" && !rest.contains(&"--snapshot")).then(|| format!("/dev/{tty}"))
        })
        .collect()
}

fn focus() -> Result<()> {
    let ttys = dashboard_ttys();
    for tty in &ttys {
        let terminal = format!(
            "tell application \"Terminal\"
               repeat with w in windows
                 repeat with t in tabs of w
                   if tty of t is \"{tty}\" then
                     set selected of t to true
                     set index of w to 1
                     activate
                     return \"ok\"
                   end if
                 end repeat
               end repeat
             end tell"
        );
        let iterm = format!(
            "tell application \"System Events\" to if not (exists process \"iTerm2\") then return \"\"
             tell application \"iTerm2\"
               repeat with w in windows
                 repeat with t in tabs of w
                   repeat with s in sessions of t
                     if tty of s is \"{tty}\" then
                       select w
                       select t
                       activate
                       return \"ok\"
                     end if
                   end repeat
                 end repeat
               end repeat
             end tell"
        );
        for script in [terminal, iterm] {
            let out = std::process::Command::new("osascript").args(["-e", &script]).output();
            if out.is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "ok") {
                return Ok(());
            }
        }
    }
    // No dashboard found in a terminal we can script: open one.
    let exe = exe()?;
    let script = format!("tell application \"Terminal\"\n do script \"{}\"\n activate\n end tell", exe.display());
    std::process::Command::new("osascript").args(["-e", &script]).status()?;
    Ok(())
}

fn open(dir: Option<PathBuf>, claude: bool, no_ui: bool) -> Result<()> {
    ensure_daemon()?;
    let dir = match dir {
        Some(d) => d,
        None => std::env::current_dir()?,
    };
    let dir = dir.canonicalize().with_context(|| format!("{} does not exist", dir.display()))?;
    if !dir.is_dir() {
        bail!("{} is not a folder", dir.display());
    }
    let kind = if claude { PaneKind::Claude } else { PaneKind::Shell };
    let (cols, rows) = ratatui_size();
    let req = NewPane { cwd: dir.to_string_lossy().to_string(), kind, args: vec![], name: None, cols, rows };
    let id = match request(&ClientMsg::NewPane(req))? {
        ServerMsg::PaneCreated { pane_id } => pane_id,
        ServerMsg::Error { message } => bail!(message),
        other => bail!("unexpected reply: {other:?}"),
    };
    if no_ui || !std::io::stdout().is_terminal() {
        println!("opened {} ({id})", dir.display());
        return Ok(());
    }
    tui(false, None, Some(id), None, None, false)
}

fn ratatui_size() -> (u16, u16) {
    if std::io::stdout().is_terminal()
        && let Some((c, r)) = aulora_tui::terminal_size()
    {
        return (c, r);
    }
    (120, 36)
}

fn list_panes(json: bool) -> Result<()> {
    let snap = request_snapshot()?;
    if json {
        println!("{}", serde_json::to_string_pretty(&snap.panes)?);
        return Ok(());
    }
    if snap.panes.is_empty() {
        println!("No panes. Start one with `aulora new` or `aulora new --claude`, or press c in the dashboard.");
        return Ok(());
    }
    let now = now_ms();
    for p in &snap.panes {
        let session = snap
            .sessions
            .iter()
            .filter(|s| s.terminal.pane_id.as_deref() == Some(p.id.as_str()))
            .max_by_key(|s| (s.state.is_live(), s.last_event_at));
        let state = match (&p.exit, session) {
            (Some(e), _) => format!("exited ({})", e.code),
            (None, Some(s)) if s.state.is_live() => format!("claude: {}", s.state.label().to_lowercase()),
            (None, _) => format!("running · output {} ago", short_duration(now - p.last_output_at)),
        };
        println!(
            "{}  {:<28} {:>7}  {:>3}×{:<3} {}",
            p.id,
            truncate(&pane_label(p), 28),
            format!("{:?}", p.kind).to_lowercase(),
            p.cols,
            p.rows,
            state
        );
    }
    Ok(())
}
