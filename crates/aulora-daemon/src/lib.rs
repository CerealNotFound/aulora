//! The aulora daemon. Receives hook events over a Unix socket, keeps the
//! state of every Claude Code session, and streams snapshots to clients.

pub mod git;
pub mod hub;
pub mod procs;
pub mod pty;
pub mod store;

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::os::unix::io::AsRawFd;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use aulora_core::model::now_ms;
use aulora_core::paths;
use aulora_core::proto::{ClientMsg, HookEnvelope, PROTOCOL_VERSION, ServerMsg};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Notify, watch};

use crate::hub::Hub;

struct Shared {
    hub: Mutex<Hub>,
    /// Bumped on every state change.
    version: watch::Sender<u64>,
    roots: Mutex<HashMap<String, String>>,
    git_kick: Notify,
    shutdown: Notify,
}

impl Shared {
    fn changed(&self) {
        self.version.send_modify(|v| *v += 1);
    }

    /// Resolve a directory to its project root (git toplevel), cached.
    async fn root_for(&self, dir: &str) -> String {
        if let Some(r) = self.roots.lock().unwrap().get(dir) {
            return r.clone();
        }
        let d = dir.to_string();
        let root = tokio::task::spawn_blocking(move || git::project_root(&d)).await.unwrap_or_else(|_| dir.to_string());
        self.roots.lock().unwrap().insert(dir.to_string(), root.clone());
        root
    }

    async fn handle_hook(&self, env: HookEnvelope) {
        let dir = env
            .env
            .project_dir
            .clone()
            .filter(|d| !d.is_empty())
            .or_else(|| env.payload.get("cwd").and_then(|v| v.as_str()).map(str::to_string));
        let Some(dir) = dir else { return };
        let root = self.root_for(&dir).await;
        let new_project = self.hub.lock().unwrap().ingest(&env, &root);
        if new_project {
            self.git_kick.notify_one();
        }
        self.changed();
    }
}

/// Hold an exclusive lock so only one daemon runs per AULORA_HOME.
fn acquire_lock() -> Result<std::fs::File> {
    let f = OpenOptions::new().create(true).truncate(false).write(true).open(paths::lock())?;
    let rc = unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc != 0 {
        bail!("another aulora daemon is already running");
    }
    Ok(f)
}

/// Start the daemon in the background, detached from the caller's session.
pub fn spawn_detached(exe: &Path) -> std::io::Result<()> {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    let mut cmd = Command::new(exe);
    cmd.arg("daemon").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    cmd.spawn().map(|_| ())
}

pub async fn run() -> Result<()> {
    paths::ensure_home()?;
    let _lock = acquire_lock()?;
    let sock = paths::socket();
    let _ = std::fs::remove_file(&sock);
    let listener = UnixListener::bind(&sock).with_context(|| format!("binding {}", sock.display()))?;
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o600));
    }

    let keep_since = now_ms() - 7 * 24 * 3600 * 1000;
    let (loaded, store_tx) = store::start(&paths::db(), keep_since, hub::FEED_LIMIT)?;
    let hub = Hub::new(store_tx, loaded.sessions, loaded.feed);
    let (version, _) = watch::channel(0u64);
    let shared = Arc::new(Shared {
        hub: Mutex::new(hub),
        version,
        roots: Mutex::new(HashMap::new()),
        git_kick: Notify::new(),
        shutdown: Notify::new(),
    });
    tracing::info!("aulora daemon {} listening on {}", aulora_core::VERSION, sock.display());

    ingest_spool(&shared).await;
    tokio::spawn(git_loop(shared.clone()));
    tokio::spawn(liveness_loop(shared.clone()));
    tokio::spawn(transcript_loop(shared.clone()));

    loop {
        tokio::select! {
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, _)) => {
                        let sh = shared.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_conn(sh, stream).await {
                                tracing::debug!("connection ended: {e}");
                            }
                        });
                    }
                    Err(e) => tracing::warn!("accept failed: {e}"),
                }
            }
            _ = shared.shutdown.notified() => break,
            _ = tokio::signal::ctrl_c() => break,
        }
    }
    let _ = std::fs::remove_file(&sock);
    tracing::info!("aulora daemon stopped");
    Ok(())
}

/// Replay hook events that arrived while the daemon was down.
async fn ingest_spool(shared: &Arc<Shared>) {
    let spool = paths::spool();
    let work = spool.with_extension(format!("{}.ingest", std::process::id()));
    if std::fs::rename(&spool, &work).is_err() {
        return;
    }
    let data = std::fs::read_to_string(&work).unwrap_or_default();
    let mut n = 0;
    for line in data.lines() {
        if let Ok(env) = serde_json::from_str::<HookEnvelope>(line) {
            shared.handle_hook(env).await;
            n += 1;
        }
    }
    let _ = std::fs::remove_file(&work);
    tracing::info!("replayed {n} spooled hook events");
}

async fn send(w: &mut tokio::net::unix::OwnedWriteHalf, msg: &ServerMsg) -> Result<()> {
    let mut line = serde_json::to_vec(msg)?;
    line.push(b'\n');
    w.write_all(&line).await?;
    Ok(())
}

async fn handle_conn(shared: Arc<Shared>, stream: UnixStream) -> Result<()> {
    let (r, mut w) = stream.into_split();
    let mut lines = BufReader::new(r).lines();
    let mut subscribed = false;
    let mut rx = shared.version.subscribe();

    loop {
        tokio::select! {
            line = lines.next_line() => {
                let Some(line) = line? else { return Ok(()) };
                if line.trim().is_empty() { continue; }
                let msg: ClientMsg = match serde_json::from_str(&line) {
                    Ok(m) => m,
                    Err(e) => {
                        send(&mut w, &ServerMsg::Error { message: format!("bad message: {e}") }).await?;
                        continue;
                    }
                };
                match msg {
                    ClientMsg::Hook(env) => shared.handle_hook(env).await,
                    ClientMsg::Ping => {
                        send(&mut w, &ServerMsg::Pong { version: aulora_core::VERSION.into(), protocol: PROTOCOL_VERSION }).await?;
                    }
                    ClientMsg::Snapshot => {
                        let snap = shared.hub.lock().unwrap().snapshot();
                        send(&mut w, &ServerMsg::Snapshot(Box::new(snap))).await?;
                    }
                    ClientMsg::Subscribe => {
                        subscribed = true;
                        rx.mark_unchanged();
                        let snap = shared.hub.lock().unwrap().snapshot();
                        send(&mut w, &ServerMsg::Snapshot(Box::new(snap))).await?;
                    }
                    ClientMsg::Dismiss { session_id } => {
                        if shared.hub.lock().unwrap().dismiss(&session_id) { shared.changed(); }
                        if !subscribed { send(&mut w, &ServerMsg::Ok).await?; }
                    }
                    ClientMsg::ClearEnded => {
                        if shared.hub.lock().unwrap().clear_ended() { shared.changed(); }
                        if !subscribed { send(&mut w, &ServerMsg::Ok).await?; }
                    }
                    ClientMsg::Shutdown => {
                        send(&mut w, &ServerMsg::Ok).await?;
                        shared.shutdown.notify_one();
                        return Ok(());
                    }
                }
            }
            changed = rx.changed(), if subscribed => {
                changed?;
                // Coalesce bursts of events into one snapshot.
                tokio::time::sleep(Duration::from_millis(40)).await;
                rx.mark_unchanged();
                let snap = shared.hub.lock().unwrap().snapshot();
                send(&mut w, &ServerMsg::Snapshot(Box::new(snap))).await?;
            }
        }
    }
}

async fn git_loop(shared: Arc<Shared>) {
    let mut tick = 0u64;
    loop {
        let now = now_ms();
        // Active projects every 3s, everything else every minute.
        let roots = {
            let hub = shared.hub.lock().unwrap();
            if tick.is_multiple_of(20) { hub.projects.keys().cloned().collect() } else { hub.active_roots(now) }
        };
        for root in roots {
            let r = root.clone();
            let info = tokio::task::spawn_blocking(move || git::info(&r)).await.ok().flatten();
            if shared.hub.lock().unwrap().set_git(&root, info) {
                shared.changed();
            }
        }
        tick += 1;
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(3)) => {}
            _ = shared.git_kick.notified() => {}
        }
    }
}

async fn liveness_loop(shared: Arc<Shared>) {
    let mut live = procs::Liveness::default();
    loop {
        tokio::time::sleep(Duration::from_secs(3)).await;
        let pids = shared.hub.lock().unwrap().live_pids();
        let dead = live.dead(&pids);
        let mut hub = shared.hub.lock().unwrap();
        let mut changed = !dead.is_empty() && hub.mark_dead(&dead);
        changed |= hub.prune(now_ms());
        drop(hub);
        if changed {
            shared.changed();
        }
    }
}

/// Keep token counts and titles fresh while sessions are busy.
async fn transcript_loop(shared: Arc<Shared>) {
    loop {
        tokio::time::sleep(Duration::from_secs(2)).await;
        let mut hub = shared.hub.lock().unwrap();
        let mut changed = false;
        for id in hub.working_ids() {
            changed |= hub.refresh_transcript(&id);
        }
        drop(hub);
        if changed {
            shared.changed();
        }
    }
}
