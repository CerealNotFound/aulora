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
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use aulora_core::model::{Pane, PaneKind, Snapshot, now_ms};
use aulora_core::paths;
use aulora_core::proto::{self, ClientMsg, HookEnvelope, NewPane, PROTOCOL_VERSION, ServerMsg};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::broadcast::error::{RecvError, TryRecvError};
use tokio::sync::{Notify, mpsc, watch};

use crate::hub::Hub;
use crate::pty::{PaneEvent, Proc, Spec};

struct Shared {
    hub: Mutex<Hub>,
    /// Running (or exited but still viewable) pane processes.
    procs: Mutex<HashMap<String, Arc<Proc>>>,
    /// Bumped on every state change.
    version: watch::Sender<u64>,
    roots: Mutex<HashMap<String, String>>,
    git_kick: Notify,
    shutdown: Notify,
}

fn expand_home(dir: &str) -> String {
    match (dir.strip_prefix('~'), std::env::var("HOME")) {
        (Some(rest), Ok(home)) if rest.is_empty() || rest.starts_with('/') => format!("{home}{rest}"),
        _ => dir.to_string(),
    }
}

fn new_pane_id(taken: &HashMap<String, Arc<Proc>>) -> String {
    let mut seed = now_ms() as u64 ^ ((std::process::id() as u64) << 32);
    loop {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let id = format!("{:06x}", (seed >> 40) & 0xff_ffff);
        if !taken.contains_key(&id) {
            return id;
        }
    }
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

    /// The hub's snapshot plus live numbers from the pane processes.
    fn snapshot(&self) -> Snapshot {
        let mut snap = self.hub.lock().unwrap().snapshot();
        let procs = self.procs.lock().unwrap();
        for pane in &mut snap.panes {
            if let Some(p) = procs.get(&pane.id) {
                pane.last_output_at = p.last_output_at();
                pane.bytes_out = p.bytes_out.load(Ordering::Relaxed);
                pane.title = p.title();
                (pane.cols, pane.rows) = p.size();
            }
        }
        snap
    }

    fn proc_(&self, id: &str) -> Option<Arc<Proc>> {
        self.procs.lock().unwrap().get(id).cloned()
    }

    fn start(self: &Arc<Self>, spec: &Spec, generation: u32) -> Result<Arc<Proc>> {
        let (on_exit, on_wake) = (Arc::downgrade(self), Arc::downgrade(self));
        let id = spec.id.clone();
        let proc_ = Proc::spawn(
            spec,
            move |exit| {
                if let Some(s) = on_exit.upgrade()
                    && s.hub.lock().unwrap().pane_exited(&id, generation, exit)
                {
                    s.changed();
                }
            },
            move || {
                if let Some(s) = on_wake.upgrade() {
                    s.changed();
                }
            },
        )?;
        self.procs.lock().unwrap().insert(spec.id.clone(), proc_.clone());
        Ok(proc_)
    }

    async fn new_pane(self: &Arc<Self>, req: NewPane) -> Result<String> {
        let cwd = expand_home(req.cwd.trim_end_matches('/'));
        let cwd = if cwd.is_empty() { "/".to_string() } else { cwd };
        if !Path::new(&cwd).is_dir() {
            bail!("{cwd} is not a directory");
        }
        if req.kind == PaneKind::Command && req.args.is_empty() {
            bail!("a command pane needs a command to run");
        }
        let root = self.root_for(&cwd).await;
        let id = new_pane_id(&self.procs.lock().unwrap());
        let base = match (&req.name, req.kind) {
            (Some(n), _) if !n.trim().is_empty() => n.trim().to_string(),
            (_, PaneKind::Shell) => "shell".into(),
            (_, PaneKind::Claude) => "claude".into(),
            (_, PaneKind::Command) => aulora_core::util::basename(&req.args[0]),
        };
        let name = self.hub.lock().unwrap().pane_name(&root, &base);
        let spec = Spec {
            id: id.clone(),
            cwd: cwd.clone(),
            kind: req.kind,
            args: req.args.clone(),
            cols: req.cols,
            rows: req.rows,
        };
        let proc_ = self.start(&spec, 0)?;
        let now = now_ms();
        let pane = Pane {
            id: id.clone(),
            project_root: root,
            cwd,
            name,
            kind: req.kind,
            args: req.args,
            pid: proc_.pid,
            cols: req.cols,
            rows: req.rows,
            created_at: now,
            last_output_at: now,
            bytes_out: 0,
            title: None,
            bell_at: None,
            exit: None,
            generation: 0,
        };
        if self.hub.lock().unwrap().add_pane(pane) {
            self.git_kick.notify_one();
        }
        tracing::info!("started pane {id}");
        self.changed();
        Ok(id)
    }

    fn restart_pane(self: &Arc<Self>, id: &str) -> Result<()> {
        let pane = self.hub.lock().unwrap().panes.get(id).cloned().with_context(|| format!("no pane {id}"))?;
        if pane.is_running() {
            bail!("pane {id} is still running");
        }
        let (cols, rows) = self.proc_(id).map(|p| p.size()).unwrap_or((pane.cols, pane.rows));
        let generation = pane.generation + 1;
        let spec = Spec { id: id.to_string(), cwd: pane.cwd, kind: pane.kind, args: pane.args, cols, rows };
        let proc_ = self.start(&spec, generation)?;
        if let Some(p) = self.hub.lock().unwrap().panes.get_mut(id) {
            p.generation = generation;
            p.exit = None;
            p.pid = proc_.pid;
        }
        self.changed();
        Ok(())
    }

    fn close_pane(&self, id: &str) -> bool {
        if let Some(p) = self.procs.lock().unwrap().remove(id) {
            p.kill();
        }
        let removed = self.hub.lock().unwrap().remove_pane(id).is_some();
        if removed {
            self.changed();
        }
        removed
    }

    fn kill_all(&self) {
        for p in self.procs.lock().unwrap().values() {
            p.kill();
        }
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
        procs: Mutex::new(HashMap::new()),
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
    shared.kill_all();
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

/// Stream one pane to a client until it exits or the client detaches.
async fn forward(p: Arc<Proc>, pane_id: String, out: mpsc::Sender<ServerMsg>) {
    loop {
        let (screen, cols, rows, mut rx) = p.attach();
        let msg = ServerMsg::PaneScreen { pane_id: pane_id.clone(), cols, rows, data: proto::encode(&screen) };
        if out.send(msg).await.is_err() {
            return;
        }
        'stream: loop {
            let mut next = match rx.recv().await {
                Ok(ev) => Some(ev),
                // Too slow to keep up: start over from a fresh screen.
                Err(RecvError::Lagged(_)) => break,
                Err(RecvError::Closed) => return,
            };
            while let Some(ev) = next.take() {
                match ev {
                    PaneEvent::Output(chunk) => {
                        // Merge whatever else is already queued into one message.
                        let mut buf = chunk.to_vec();
                        let mut lagged = false;
                        while buf.len() < 256 * 1024 {
                            match rx.try_recv() {
                                Ok(PaneEvent::Output(more)) => buf.extend_from_slice(&more),
                                Ok(other) => {
                                    next = Some(other);
                                    break;
                                }
                                Err(TryRecvError::Lagged(_)) => {
                                    lagged = true;
                                    break;
                                }
                                Err(_) => break,
                            }
                        }
                        let msg = ServerMsg::PaneOutput { pane_id: pane_id.clone(), data: proto::encode(&buf) };
                        if out.send(msg).await.is_err() {
                            return;
                        }
                        if lagged {
                            break 'stream;
                        }
                    }
                    PaneEvent::Exited(exit) => {
                        let _ = out.send(ServerMsg::PaneExited { pane_id: pane_id.clone(), code: exit.code }).await;
                        return;
                    }
                }
            }
        }
    }
}

async fn handle_conn(shared: Arc<Shared>, stream: UnixStream) -> Result<()> {
    let (r, mut w) = stream.into_split();
    // One writer per connection, so snapshots and every attached pane can
    // share the socket without interleaving partial lines.
    let (out, mut out_rx) = mpsc::channel::<ServerMsg>(512);
    let writer = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            if send(&mut w, &msg).await.is_err() {
                break;
            }
        }
    });
    let mut lines = BufReader::new(r).lines();
    let mut subscribed = false;
    let mut rx = shared.version.subscribe();
    let mut attached: HashMap<String, tokio::task::JoinHandle<()>> = HashMap::new();

    let result: Result<()> = loop {
        tokio::select! {
            line = lines.next_line() => {
                let line = match line {
                    Ok(Some(l)) => l,
                    Ok(None) => break Ok(()),
                    Err(e) => break Err(e.into()),
                };
                if line.trim().is_empty() { continue; }
                let msg: ClientMsg = match serde_json::from_str(&line) {
                    Ok(m) => m,
                    Err(e) => {
                        let _ = out.send(ServerMsg::Error { message: format!("bad message: {e}") }).await;
                        continue;
                    }
                };
                let err = |e: anyhow::Error| Some(ServerMsg::Error { message: format!("{e:#}") });
                let ack = if subscribed { None } else { Some(ServerMsg::Ok) };
                let reply = match msg {
                    ClientMsg::Hook(env) => {
                        shared.handle_hook(env).await;
                        None
                    }
                    ClientMsg::Ping => Some(ServerMsg::Pong { version: aulora_core::VERSION.into(), protocol: PROTOCOL_VERSION }),
                    ClientMsg::Snapshot => Some(ServerMsg::Snapshot(Box::new(shared.snapshot()))),
                    ClientMsg::Subscribe => {
                        subscribed = true;
                        rx.mark_unchanged();
                        Some(ServerMsg::Snapshot(Box::new(shared.snapshot())))
                    }
                    ClientMsg::Dismiss { session_id } => {
                        if shared.hub.lock().unwrap().dismiss(&session_id) { shared.changed(); }
                        ack
                    }
                    ClientMsg::ClearEnded => {
                        if shared.hub.lock().unwrap().clear_ended() { shared.changed(); }
                        ack
                    }
                    ClientMsg::Shutdown => {
                        let _ = out.send(ServerMsg::Ok).await;
                        shared.shutdown.notify_one();
                        break Ok(());
                    }
                    ClientMsg::NewPane(req) => match shared.new_pane(req).await {
                        Ok(pane_id) => Some(ServerMsg::PaneCreated { pane_id }),
                        Err(e) => err(e),
                    },
                    ClientMsg::RestartPane { pane_id } => match shared.restart_pane(&pane_id) {
                        Ok(()) => ack,
                        Err(e) => err(e),
                    },
                    ClientMsg::KillPane { pane_id } => match shared.proc_(&pane_id) {
                        Some(p) => { p.kill(); ack }
                        None => err(anyhow::anyhow!("no pane {pane_id}")),
                    },
                    ClientMsg::ClosePane { pane_id } => {
                        if shared.close_pane(&pane_id) { ack } else { err(anyhow::anyhow!("no pane {pane_id}")) }
                    }
                    ClientMsg::Attach { pane_id } => match shared.proc_(&pane_id) {
                        Some(p) => {
                            if let Some(old) = attached.remove(&pane_id) { old.abort(); }
                            attached.insert(pane_id.clone(), tokio::spawn(forward(p, pane_id, out.clone())));
                            None
                        }
                        None => err(anyhow::anyhow!("no pane {pane_id}")),
                    },
                    ClientMsg::Detach { pane_id } => {
                        if let Some(h) = attached.remove(&pane_id) { h.abort(); }
                        None
                    }
                    ClientMsg::Input { pane_id, data } => {
                        if let Some(p) = shared.proc_(&pane_id) { p.write(proto::decode(&data)); }
                        None
                    }
                    ClientMsg::Resize { pane_id, cols, rows } => {
                        if let Some(p) = shared.proc_(&pane_id)
                            && let Err(e) = p.resize(cols, rows) { tracing::debug!("resize {pane_id}: {e}"); }
                        None
                    }
                };
                if let Some(reply) = reply
                    && out.send(reply).await.is_err() { break Ok(()); }
            }
            changed = rx.changed(), if subscribed => {
                if changed.is_err() { break Ok(()); }
                // Coalesce bursts of events into one snapshot.
                tokio::time::sleep(Duration::from_millis(40)).await;
                rx.mark_unchanged();
                if out.send(ServerMsg::Snapshot(Box::new(shared.snapshot()))).await.is_err() { break Ok(()); }
            }
        }
    };
    for (_, h) in attached {
        h.abort();
    }
    drop(out);
    let _ = writer.await;
    result
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
