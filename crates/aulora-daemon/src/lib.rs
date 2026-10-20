//! The aulora daemon. Receives hook events over a Unix socket, keeps the
//! state of every Claude Code session, and streams snapshots to clients.

pub mod discover;
pub mod editors;
pub mod git;
pub mod hub;
pub mod notify;
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
use aulora_core::model::{Pane, PaneKind, SessionState, Snapshot, now_ms};
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
    editors: editors::Editors,
    /// Re-match sessions to editor terminals now.
    reach_kick: Notify,
    /// The single-instance lock, kept open across an upgrade.
    lock: Mutex<Option<std::fs::File>>,
    notifier: notify::Notifier,
}

/// What a daemon hands the one replacing it: every pane, with its
/// terminal's master fd left open across exec.
#[derive(serde::Serialize, serde::Deserialize)]
struct Handover {
    lock_fd: Option<i32>,
    panes: Vec<HandedPane>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct HandedPane {
    pane: Pane,
    fd: Option<i32>,
    screen: String,
    title: Option<String>,
}

fn handover_path() -> std::path::PathBuf {
    paths::home().join("handover.json")
}

/// The running panes, kept on disk so they can come back after a crash or
/// a reboot. A deliberate stop clears it.
fn saved_panes_path() -> std::path::PathBuf {
    paths::home().join("panes.json")
}

/// A restored pane's arguments: a Claude pane resumes its last session
/// (replacing any `--resume` it was started with); others run as before.
fn restore_args(kind: PaneKind, args: &[String], session: Option<&str>) -> Vec<String> {
    if kind != PaneKind::Claude {
        return args.to_vec();
    }
    let mut rest = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--resume" || a == "-r" {
            it.next();
        } else if a != "--fork-session" {
            rest.push(a.clone());
        }
    }
    match session {
        Some(s) => [vec!["--resume".to_string(), s.to_string()], rest].concat(),
        None => rest,
    }
}

#[derive(serde::Serialize, serde::Deserialize, PartialEq)]
struct SavedPane {
    id: String,
    name: String,
    cwd: String,
    kind: PaneKind,
    args: Vec<String>,
    /// The Claude session last seen in it, to resume.
    session: Option<String>,
}

static NEXT_CONN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

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
        let id = env.payload.get("session_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let was_blocked = matches!(self.hub.lock().unwrap().session_state(&id), Some(SessionState::NeedsYou { .. }));
        let new_project = self.hub.lock().unwrap().ingest(&env, &root);
        // Tell the desktop when it starts needing you.
        if !was_blocked {
            let now = self.hub.lock().unwrap().session(&id);
            if let Some(s) = now
                && let SessionState::NeedsYou { reason, detail } = &s.state
            {
                let project = aulora_core::util::basename(&s.project_root);
                let title = format!("{project} › {}", s.display_title());
                self.notifier.needs_you(&id, &title, &format!("{}: {detail}", reason.label()));
            }
        }
        if new_project {
            self.git_kick.notify_one();
        }
        if self.editors.any() && env.payload.get("hook_event_name").and_then(|v| v.as_str()) == Some("SessionStart") {
            self.reach_kick.notify_one();
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
        let apps = self.editors.apps();
        for s in &mut snap.sessions {
            s.terminal.editor = apps.get(&s.id).cloned();
        }
        snap
    }

    /// Type a prompt into the editor terminal a session runs in.
    async fn send_prompt(self: &Arc<Self>, session_id: &str, text: String) -> Result<()> {
        use aulora_core::model::SessionState;
        let state = self.hub.lock().unwrap().session_state(session_id);
        match state {
            None => bail!("no session {session_id}"),
            Some(SessionState::Ended { .. }) => bail!("the session has ended; resume it instead"),
            // Keys typed now would answer the dialog, not reach Claude.
            Some(SessionState::NeedsYou { .. }) => bail!("Claude is waiting for an answer there; answer it first"),
            Some(_) => {}
        }
        if self.editors.target(session_id).is_none() {
            self.recompute_reach().await;
        }
        let Some(target) = self.editors.target(session_id) else {
            bail!("no editor with the aulora extension has this session's terminal (aulora vscode install)")
        };
        self.editors.type_into(&target, text).await.map_err(|e| anyhow::anyhow!(e))
    }

    async fn recompute_reach(self: &Arc<Self>) {
        let sessions = self.hub.lock().unwrap().live_sessions();
        let me = self.clone();
        let changed = tokio::task::spawn_blocking(move || me.editors.recompute(&sessions)).await.unwrap_or(false);
        if changed {
            self.changed();
        }
    }

    /// Replace this daemon with `exe`, keeping every pane: write down the
    /// panes and their screens, keep their terminals' fds and the lock open,
    /// and exec. The pid stays the same, so the programs in the panes stay
    /// our children and never notice. Only returns if exec failed.
    fn upgrade(self: &Arc<Self>, exe: &Path) -> Result<()> {
        use std::os::unix::io::AsRawFd;
        use std::os::unix::process::CommandExt;
        let panes: Vec<Pane> = self.hub.lock().unwrap().panes.values().cloned().collect();
        let procs = self.procs.lock().unwrap();
        let handed: Vec<HandedPane> = panes
            .into_iter()
            .map(|pane| {
                let exported = procs.get(&pane.id).map(|p| p.export());
                let (fd, screen, title) = match exported {
                    Some(e) => (e.fd.filter(|_| pane.is_running()), e.screen, e.title),
                    None => (None, Vec::new(), None),
                };
                HandedPane { pane, fd, screen: proto::encode(&screen), title }
            })
            .collect();
        drop(procs);
        let lock_fd = self.lock.lock().unwrap().as_ref().map(|f| {
            let fd = f.as_raw_fd();
            // SAFETY: fcntl on our own open fd.
            unsafe {
                let flags = libc::fcntl(fd, libc::F_GETFD);
                libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC);
            }
            fd
        });
        let n = handed.len();
        std::fs::write(handover_path(), serde_json::to_vec(&Handover { lock_fd, panes: handed })?)?;
        self.hub.lock().unwrap().flush();
        let _ = std::fs::remove_file(paths::socket());
        tracing::info!("upgrading to {}, keeping {n} panes", exe.display());
        let err = std::process::Command::new(exe).args(["daemon", "run", "--handover"]).exec();
        Err(anyhow::anyhow!("couldn't start {}: {err}", exe.display()))
    }

    /// Take over the panes the previous daemon handed over.
    fn adopt(self: &Arc<Self>, handed: Vec<HandedPane>) {
        for h in handed {
            let id = h.pane.id.clone();
            let generation = h.pane.generation;
            let (on_exit, on_wake) = (Arc::downgrade(self), Arc::downgrade(self));
            let exit_id = id.clone();
            let adopted = Proc::adopt(
                &id,
                h.fd,
                h.pane.pid,
                h.pane.cols,
                h.pane.rows,
                &proto::decode(&h.screen),
                h.title,
                move |exit| {
                    if let Some(s) = on_exit.upgrade()
                        && s.hub.lock().unwrap().pane_exited(&exit_id, generation, exit)
                    {
                        s.changed();
                    }
                },
                move || {
                    if let Some(s) = on_wake.upgrade() {
                        s.changed();
                    }
                },
            );
            match adopted {
                Ok(p) => {
                    self.procs.lock().unwrap().insert(id.clone(), p);
                    self.hub.lock().unwrap().add_pane(h.pane);
                }
                Err(e) => tracing::warn!("couldn't adopt pane {id}: {e:#}"),
            }
        }
    }

    /// What to bring back if the daemon dies: every running pane.
    fn saved_panes(&self) -> Vec<SavedPane> {
        let snap = self.hub.lock().unwrap().snapshot();
        let mut panes: Vec<SavedPane> = snap
            .panes
            .iter()
            .filter(|p| p.is_running())
            .map(|p| SavedPane {
                id: p.id.clone(),
                name: p.name.clone(),
                cwd: p.cwd.clone(),
                kind: p.kind,
                args: p.args.clone(),
                session: snap
                    .sessions
                    .iter()
                    .filter(|s| s.terminal.pane_id.as_deref() == Some(p.id.as_str()) && s.transcript_path.is_some())
                    .max_by_key(|s| s.last_event_at)
                    .map(|s| s.id.clone()),
            })
            .collect();
        panes.sort_by(|a, b| a.id.cmp(&b.id));
        panes
    }

    /// Start the panes that were running when the daemon last died, with
    /// their ids, so layouts still fit. Claude panes resume their session.
    async fn restore(self: &Arc<Self>, saved: Vec<SavedPane>) {
        for p in saved {
            if !Path::new(&p.cwd).is_dir() {
                continue;
            }
            let args = restore_args(p.kind, &p.args, p.session.as_deref());
            let root = self.root_for(&p.cwd).await;
            let spec =
                Spec { id: p.id.clone(), cwd: p.cwd.clone(), kind: p.kind, args: args.clone(), cols: 120, rows: 36 };
            match self.start(&spec, 0) {
                Ok(proc_) => {
                    let now = now_ms();
                    self.hub.lock().unwrap().add_pane(Pane {
                        id: p.id.clone(),
                        project_root: root,
                        cwd: p.cwd,
                        name: p.name,
                        kind: p.kind,
                        args,
                        pid: proc_.pid,
                        cols: 120,
                        rows: 36,
                        created_at: now,
                        last_output_at: now,
                        bytes_out: 0,
                        title: None,
                        bell_at: None,
                        exit: None,
                        generation: 0,
                    });
                    tracing::info!("restored pane {}", p.id);
                }
                Err(e) => tracing::warn!("couldn't restore pane {}: {e:#}", p.id),
            }
        }
        self.changed();
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

pub async fn run(handover: bool) -> Result<()> {
    use std::os::unix::io::FromRawFd;
    paths::ensure_home()?;
    // Taking over from a daemon that just upgraded: its lock and panes.
    let handed: Option<Handover> = if handover {
        let h = std::fs::read(handover_path()).ok().and_then(|b| serde_json::from_slice(&b).ok());
        let _ = std::fs::remove_file(handover_path());
        h
    } else {
        None
    };
    let lock = match handed.as_ref().and_then(|h| h.lock_fd) {
        // SAFETY: the previous daemon left its lock fd open for us.
        Some(fd) => unsafe { std::fs::File::from_raw_fd(fd) },
        None => acquire_lock()?,
    };
    let _ = std::fs::remove_file(paths::stopped_marker());
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
        editors: editors::Editors::default(),
        reach_kick: Notify::new(),
        lock: Mutex::new(Some(lock)),
        notifier: notify::Notifier::default(),
    });
    let fresh_start = handed.is_none();
    if let Some(h) = handed {
        let n = h.panes.len();
        shared.adopt(h.panes);
        tracing::info!("took over {n} panes");
    }
    tracing::info!("aulora daemon {} listening on {}", aulora_core::VERSION, sock.display());

    ingest_spool(&shared).await;
    tokio::spawn(git_loop(shared.clone()));
    tokio::spawn(liveness_loop(shared.clone()));
    tokio::spawn(cwd_loop(shared.clone()));
    tokio::spawn(transcript_loop(shared.clone()));
    tokio::spawn(discover_loop(shared.clone()));
    tokio::spawn(reach_loop(shared.clone()));
    tokio::spawn(upgrade_loop(shared.clone()));
    // Started fresh while panes were saved: the last daemon died (a crash,
    // a reboot). Bring them back.
    if fresh_start
        && let Some(saved) =
            std::fs::read(saved_panes_path()).ok().and_then(|b| serde_json::from_slice::<Vec<SavedPane>>(&b).ok())
        && !saved.is_empty()
    {
        tracing::info!("restoring {} panes", saved.len());
        shared.restore(saved).await;
    }
    tokio::spawn(save_panes_loop(shared.clone()));

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
    let conn = NEXT_CONN.fetch_add(1, Ordering::Relaxed);

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
                        // Stopped on purpose: don't bring the panes back next time.
                        let _ = std::fs::remove_file(saved_panes_path());
                        let _ = std::fs::write(paths::stopped_marker(), now_ms().to_string());
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
                    ClientMsg::Editor { app, terminals } => {
                        tracing::debug!("{app}: {} terminals", terminals.len());
                        shared.editors.register(conn, app, terminals, out.clone());
                        shared.reach_kick.notify_one();
                        None
                    }
                    ClientMsg::Typed { request_id, error } => {
                        shared.editors.typed(request_id, error);
                        None
                    }
                    ClientMsg::Upgrade { exe } => {
                        let exe = exe.map(std::path::PathBuf::from).or_else(|| std::env::current_exe().ok());
                        match exe {
                            Some(exe) if exe.is_file() => {
                                let _ = out.send(ServerMsg::Ok).await;
                                tokio::time::sleep(Duration::from_millis(100)).await;
                                let sh = shared.clone();
                                if let Ok(Err(e)) = tokio::task::spawn_blocking(move || sh.upgrade(&exe)).await {
                                    tracing::error!("upgrade failed: {e:#}");
                                }
                                None
                            }
                            _ => err(anyhow::anyhow!("no program to upgrade to")),
                        }
                    }
                    ClientMsg::Answer { session_id, keys } => {
                        let waiting = matches!(
                            shared.hub.lock().unwrap().session_state(&session_id),
                            Some(SessionState::NeedsYou { .. })
                        );
                        if shared.editors.target(&session_id).is_none() {
                            shared.recompute_reach().await;
                        }
                        match (waiting, shared.editors.target(&session_id)) {
                            (false, _) => err(anyhow::anyhow!("it isn't waiting on an answer anymore")),
                            (true, None) => err(anyhow::anyhow!("no editor with the aulora extension has this session's terminal")),
                            (true, Some(t)) => match shared.editors.send_keys(&t, keys, false).await {
                                Ok(()) => Some(ServerMsg::Ok),
                                Err(e) => err(anyhow::anyhow!(e)),
                            },
                        }
                    }
                    ClientMsg::ShowSession { session_id } => {
                        if shared.editors.target(&session_id).is_none() {
                            shared.recompute_reach().await;
                        }
                        match shared.editors.target(&session_id) {
                            Some(t) => match shared.editors.show(&t).await {
                                Ok(()) => Some(ServerMsg::Ok),
                                Err(e) => err(anyhow::anyhow!(e)),
                            },
                            None => err(anyhow::anyhow!("no editor with the aulora extension has this session's terminal")),
                        }
                    }
                    ClientMsg::EndSession { session_id } => {
                        if shared.editors.target(&session_id).is_none() {
                            shared.recompute_reach().await;
                        }
                        let pid = shared.hub.lock().unwrap().session_pid(&session_id);
                        match (shared.editors.target(&session_id), pid) {
                            (Some(t), _) => match shared.editors.close(&t).await {
                                Ok(()) => Some(ServerMsg::Ok),
                                Err(e) => err(anyhow::anyhow!(e)),
                            },
                            (None, Some(pid)) => match procs::end_claude(pid) {
                                Ok(()) => Some(ServerMsg::Ok),
                                Err(e) => err(anyhow::anyhow!(e)),
                            },
                            (None, None) => err(anyhow::anyhow!("aulora doesn't know this session's process; end it where it runs")),
                        }
                    }
                    ClientMsg::SendPrompt { session_id, text } => match shared.send_prompt(&session_id, text).await {
                        Ok(()) => Some(ServerMsg::Ok),
                        Err(e) => err(e),
                    },
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
    if shared.editors.unregister(conn) {
        shared.changed();
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

/// Pick up sessions hooks haven't reported: running ones from Claude's
/// session list every few seconds, past ones from transcripts every minute.
async fn discover_loop(shared: Arc<Shared>) {
    let mut live = procs::Liveness::default();
    let mut scanned: HashMap<std::path::PathBuf, i64> = HashMap::new();
    let mut tick = 0u64;
    loop {
        let mut changed = false;
        let mut new_project = false;

        let entries = tokio::task::spawn_blocking(discover::running).await.unwrap_or_default();
        let pids: Vec<u32> = entries.iter().map(|e| e.pid).collect();
        let dead = live.dead(&pids);
        for e in entries.iter().filter(|e| !dead.contains(&e.pid)) {
            let root = shared.root_for(&e.cwd).await;
            let known_path = shared.hub.lock().unwrap().transcript_path(&e.session_id);
            let transcript = match known_path {
                Some(p) => Some(p),
                None => {
                    let id = e.session_id.clone();
                    tokio::task::spawn_blocking(move || discover::find_transcript(&id))
                        .await
                        .ok()
                        .flatten()
                        .map(|p| p.to_string_lossy().to_string())
                }
            };
            let mut hub = shared.hub.lock().unwrap();
            let is_new = !hub.knows(&e.session_id);
            if hub.upsert_running(e, &root, transcript) {
                changed = true;
                new_project |= is_new;
            }
        }

        if tick.is_multiple_of(30) {
            let since = now_ms() - hub::HISTORY_MS;
            let files = tokio::task::spawn_blocking(move || discover::transcripts(since)).await.unwrap_or_default();
            for (id, path, mtime) in files {
                if scanned.get(&path) == Some(&mtime) || shared.hub.lock().unwrap().knows(&id) {
                    continue;
                }
                scanned.insert(path.clone(), mtime);
                let p = path.to_string_lossy().to_string();
                let parsed = tokio::task::spawn_blocking(move || {
                    let (mut cur, mut sum) = (aulora_core::transcript::Cursor::default(), Default::default());
                    aulora_core::transcript::read_new(&p, &mut cur, &mut sum).ok().map(|_| (p, cur, sum))
                })
                .await
                .ok()
                .flatten();
                let Some((p, cur, sum)) = parsed else { continue };
                // Transcripts without a single prompt are noise (e.g. aborted starts).
                if sum.prompt_count == 0 {
                    continue;
                }
                let Some(cwd) = sum.cwd.clone() else { continue };
                let root = shared.root_for(&cwd).await;
                if shared.hub.lock().unwrap().add_history(&id, &p, &root, cur, sum) {
                    changed = true;
                    new_project = true;
                }
            }
        }

        if new_project {
            shared.git_kick.notify_one();
        }
        if changed {
            shared.changed();
        }
        tick += 1;
        tokio::time::sleep(Duration::from_secs(2)).await;
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

/// Follow each shell's `cd`: a pane whose directory moves into another
/// workspace moves there too. Reading a process's directory is a cheap
/// system call; git only runs when a directory changed.
async fn cwd_loop(shared: Arc<Shared>) {
    let mut cwds = procs::Cwds::default();
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let panes = shared.hub.lock().unwrap().pane_dirs();
        let pids: Vec<u32> = panes.iter().map(|(_, pid, _)| *pid).collect();
        let now = cwds.of(&pids);
        for (id, pid, was) in panes {
            let Some(cwd) = now.get(&pid).filter(|c| **c != was).cloned() else { continue };
            let dir = cwd.clone();
            let repo = tokio::task::spawn_blocking(move || git::repo_root(&dir)).await.ok().flatten();
            let roots = shared.hub.lock().unwrap().project_roots();
            let root = hub::workspace_for(&cwd, repo.as_deref(), &roots);
            let (changed, new_project) = shared.hub.lock().unwrap().pane_moved(&id, &cwd, root.as_deref());
            if new_project {
                shared.git_kick.notify_one();
            }
            if changed {
                shared.changed();
            }
        }
    }
}

/// Keep the list of running panes on disk, for restoring after a crash.
async fn save_panes_loop(shared: Arc<Shared>) {
    let mut last: Option<Vec<u8>> = None;
    loop {
        tokio::time::sleep(Duration::from_secs(3)).await;
        let Ok(json) = serde_json::to_vec(&shared.saved_panes()) else { continue };
        if last.as_ref() != Some(&json) {
            let tmp = saved_panes_path().with_extension("json.tmp");
            if std::fs::write(&tmp, &json).is_ok() && std::fs::rename(&tmp, saved_panes_path()).is_ok() {
                last = Some(json);
            }
        }
    }
}

/// Upgrade in place when the aulora program is replaced (a `cargo install`
/// or a release), so updates never cost the panes.
async fn upgrade_loop(shared: Arc<Shared>) {
    use std::os::unix::fs::MetadataExt;
    let Ok(exe) = std::env::current_exe() else { return };
    let stamp = |p: &Path| std::fs::metadata(p).ok().map(|m| (m.ino(), m.mtime(), m.len()));
    let mut current = stamp(&exe);
    let mut seen_change: Option<(u64, i64, u64)> = None;
    loop {
        tokio::time::sleep(Duration::from_secs(5)).await;
        let now = stamp(&exe);
        if now.is_none() || now == current {
            seen_change = None;
            continue;
        }
        // Wait one more look, so a file still being written settles.
        if seen_change != now {
            seen_change = now;
            continue;
        }
        let sh = shared.clone();
        let target = exe.clone();
        if let Ok(Err(e)) = tokio::task::spawn_blocking(move || sh.upgrade(&target)).await {
            tracing::error!("upgrade failed: {e:#}");
        }
        current = now;
    }
}

/// Keep track of which sessions run in editor terminals we can type into.
async fn reach_loop(shared: Arc<Shared>) {
    loop {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(3)) => {}
            _ = shared.reach_kick.notified() => {}
        }
        if shared.editors.any() || !shared.editors.apps().is_empty() {
            shared.recompute_reach().await;
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

#[cfg(test)]
mod restore_tests {
    use super::*;

    #[test]
    fn claude_panes_resume_their_last_session() {
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            restore_args(PaneKind::Claude, &a(&["--resume", "old", "--model", "opus"]), Some("new")),
            a(&["--resume", "new", "--model", "opus"])
        );
        assert_eq!(restore_args(PaneKind::Claude, &a(&["--resume", "old", "--fork-session"]), None), a(&[]));
        assert_eq!(restore_args(PaneKind::Command, &a(&["pnpm", "dev"]), Some("x")), a(&["pnpm", "dev"]));
    }
}
