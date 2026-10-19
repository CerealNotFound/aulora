mod hook;
mod install;

use std::io::{IsTerminal, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use aulora_core::model::{SessionState, Snapshot, now_ms};
use aulora_core::paths;
use aulora_core::proto::{ClientMsg, ServerMsg};
use aulora_core::util::{short_duration, truncate};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "aulora",
    version,
    about = "See every Claude Code session across all your projects, and what each one needs from you."
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
    },
    /// Open the dashboard with made-up sessions.
    Demo,
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
    },
    /// Check that hooks, daemon and editor are set up.
    Doctor,
    /// Entry point for Claude Code hooks (reads the event on stdin).
    #[command(hide = true)]
    Hook,
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
    Run,
    /// Start in the background if it isn't running.
    Start,
    /// Stop the running daemon.
    Stop,
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
            tui(false, None)
        }
        Some(Cmd::Tui { demo, snapshot }) => tui(demo, snapshot),
        Some(Cmd::Demo) => tui(true, None),
        Some(Cmd::Hook) => {
            hook::run();
            Ok(())
        }
        Some(Cmd::Hooks { action }) => hooks(action),
        Some(Cmd::Daemon { action }) => daemon(action.unwrap_or(DaemonCmd::Run)),
        Some(Cmd::Status { json }) => status(json),
        Some(Cmd::Doctor) => doctor(),
    }
}

fn exe() -> Result<PathBuf> {
    let p = std::env::current_exe().context("finding the aulora executable")?;
    Ok(p.canonicalize().unwrap_or(p))
}

fn hooks_installed() -> bool {
    install::read(&install::default_settings_path()).map(|v| !install::installed_events(&v).is_empty()).unwrap_or(false)
}

fn tui(demo: bool, snapshot: Option<String>) -> Result<()> {
    let exe = exe()?;
    let opts = aulora_tui::Options {
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
        print!("{}", aulora_tui::render_text(&opts, live, w, h));
        return Ok(());
    }
    aulora_tui::run(opts)
}

// --- daemon -----------------------------------------------------------------

fn request(msg: &ClientMsg) -> Result<ServerMsg> {
    let mut s = UnixStream::connect(paths::socket()).context("the aulora daemon is not running")?;
    s.set_read_timeout(Some(Duration::from_secs(3)))?;
    let mut line = serde_json::to_string(msg)?;
    line.push('\n');
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

fn daemon_running() -> Option<String> {
    match request(&ClientMsg::Ping) {
        Ok(ServerMsg::Pong { version, .. }) => Some(version),
        _ => None,
    }
}

fn daemon(action: DaemonCmd) -> Result<()> {
    match action {
        DaemonCmd::Run => {
            init_logging()?;
            let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?;
            let res = rt.block_on(aulora_daemon::run());
            if let Err(e) = &res {
                tracing::error!("daemon failed: {e:#}");
            }
            res
        }
        DaemonCmd::Start => {
            if let Some(v) = daemon_running() {
                println!("daemon already running (v{v})");
                return Ok(());
            }
            aulora_daemon::spawn_detached(&exe()?)?;
            for _ in 0..40 {
                std::thread::sleep(Duration::from_millis(50));
                if let Some(v) = daemon_running() {
                    println!("daemon started (v{v})");
                    return Ok(());
                }
            }
            bail!("daemon did not start; see {}", paths::log().display())
        }
        DaemonCmd::Stop => match request(&ClientMsg::Shutdown) {
            Ok(_) => {
                println!("daemon stopped");
                Ok(())
            }
            Err(_) => {
                println!("daemon is not running");
                Ok(())
            }
        },
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

fn status(json: bool) -> Result<()> {
    let snap = request_snapshot()?;
    if json {
        println!("{}", serde_json::to_string_pretty(&snap)?);
        return Ok(());
    }
    let color = std::io::stdout().is_terminal();
    let paint = |code: &str, s: &str| if color { format!("\x1b[{code}m{s}\x1b[0m") } else { s.to_string() };
    let now = now_ms();
    let mut sessions: Vec<_> = snap.sessions.iter().collect();
    sessions.sort_by_key(|s| (s.state.rank(), -s.last_event_at));
    if sessions.is_empty() {
        println!("No sessions yet. Start `claude` in a project (hooks: `aulora hooks status`).");
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
