//! `aulora hook`: run by Claude Code for every hook event.
//!
//! Rules: never print to stdout (some events feed stdout into Claude's
//! context), never block for long, always exit 0.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use aulora_core::model::now_ms;
use aulora_core::paths;
use aulora_core::proto::{ClientMsg, HookEnv, HookEnvelope, PROTOCOL_VERSION};
use aulora_core::reduce::trim_payload;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

const MAX_STDIN: u64 = 32 * 1024 * 1024;
const SHELLS: &[&str] = &["sh", "bash", "zsh", "dash", "fish", "env"];

fn var(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.is_empty())
}

/// The Claude Code process that invoked us, skipping a wrapping shell.
fn claude_pid() -> Option<u32> {
    let mut sys = System::new();
    let mut pid = std::os::unix::process::parent_id();
    for _ in 0..3 {
        let p = Pid::from_u32(pid);
        sys.refresh_processes_specifics(ProcessesToUpdate::Some(&[p]), false, ProcessRefreshKind::nothing());
        let proc_ = sys.process(p)?;
        let name = proc_.name().to_string_lossy().to_string();
        if SHELLS.contains(&name.as_str()) {
            pid = proc_.parent()?.as_u32();
            continue;
        }
        return Some(pid);
    }
    Some(pid)
}

pub fn run() {
    let at = now_ms();
    let mut input = String::new();
    if std::io::stdin().take(MAX_STDIN).read_to_string(&mut input).is_err() {
        return;
    }
    let Ok(payload) = serde_json::from_str::<serde_json::Value>(&input) else { return };
    if !payload.is_object() {
        return;
    }
    let env = HookEnv {
        term_program: var("TERM_PROGRAM"),
        bundle_id: var("__CFBundleIdentifier"),
        pane_id: var("AULORA_PANE_ID"),
        project_dir: var("CLAUDE_PROJECT_DIR"),
        tmux: var("TMUX").is_some(),
        claude_pid: claude_pid(),
    };
    let msg = ClientMsg::Hook(HookEnvelope { v: PROTOCOL_VERSION, at, payload: trim_payload(payload), env });
    let Ok(mut line) = serde_json::to_string(&msg) else { return };
    line.push('\n');

    if send(&line).is_ok() {
        return;
    }
    // Daemon is down: keep the event and start the daemon.
    if let Ok(env) = serde_json::to_string(match &msg {
        ClientMsg::Hook(e) => e,
        _ => return,
    }) {
        spool(&env);
    }
    if should_start_daemon(&msg)
        && let Ok(exe) = std::env::current_exe()
    {
        let _ = aulora_daemon::spawn_detached(&exe);
    }
}

/// Start the daemon for a hook event unless it was just stopped on purpose,
/// or the event is a session ending (nothing to show for it).
fn should_start_daemon(msg: &ClientMsg) -> bool {
    let ClientMsg::Hook(env) = msg else { return false };
    if aulora_core::reduce::event_name(&env.payload) == "SessionEnd" {
        return false;
    }
    let recently_stopped = std::fs::metadata(paths::stopped_marker())
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age < Duration::from_secs(15));
    !recently_stopped
}

fn send(line: &str) -> std::io::Result<()> {
    let mut s = UnixStream::connect(paths::socket())?;
    s.set_write_timeout(Some(Duration::from_millis(300)))?;
    s.write_all(line.as_bytes())?;
    s.flush()
}

fn spool(env_json: &str) {
    if paths::ensure_home().is_err() {
        return;
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(paths::spool()) {
        let _ = writeln!(f, "{env_json}");
    }
}
