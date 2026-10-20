//! In-memory state: every session, project and feed entry.

use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::Sender;

use aulora_core::model::*;
use aulora_core::proto::HookEnvelope;
use aulora_core::reduce;
use aulora_core::transcript;
use aulora_core::util::basename;

use crate::store::StoreCmd;

pub const FEED_LIMIT: usize = 400;
/// Ended sessions stay visible this long.
pub const ENDED_TTL_MS: Ms = 12 * 3600 * 1000;
/// Live-looking sessions with no PID and no events for this long are stale.
pub const STALE_MS: Ms = 6 * 3600 * 1000;
/// How far back past sessions are shown.
pub const HISTORY_MS: Ms = 7 * 24 * 3600 * 1000;

/// Map a status from Claude's running-session list to a session state.
fn state_for(status: Option<&str>, turns: u32) -> SessionState {
    match status.unwrap_or("") {
        "busy" | "working" | "running" => SessionState::Working,
        s if s.contains("wait") || s.contains("permission") || s.contains("input") => SessionState::NeedsYou {
            reason: Attention::Input,
            detail: "waiting for you (from Claude's session list)".into(),
        },
        _ if turns > 0 => SessionState::YourTurn,
        _ => SessionState::Idle,
    }
}

pub struct Hub {
    pub started_at: Ms,
    pub hooks_seen: u64,
    pub sessions: HashMap<String, Session>,
    pub projects: HashMap<String, Project>,
    pub feed: VecDeque<FeedEvent>,
    pub panes: HashMap<String, Pane>,
    /// History sessions the user dismissed; not rediscovered.
    dismissed: std::collections::HashSet<String>,
    transcripts: HashMap<String, (transcript::Cursor, transcript::Summary)>,
    store: Sender<StoreCmd>,
}

impl Hub {
    pub fn new(store: Sender<StoreCmd>, sessions: Vec<Session>, feed: Vec<FeedEvent>) -> Self {
        let mut hub = Hub {
            started_at: now_ms(),
            hooks_seen: 0,
            sessions: HashMap::new(),
            projects: HashMap::new(),
            feed: feed.into_iter().collect(),
            panes: HashMap::new(),
            dismissed: Default::default(),
            transcripts: HashMap::new(),
            store,
        };
        for s in sessions {
            hub.ensure_project(&s.project_root, s.last_event_at);
            hub.sessions.insert(s.id.clone(), s);
        }
        hub
    }

    fn ensure_project(&mut self, root: &str, at: Ms) -> bool {
        match self.projects.get_mut(root) {
            Some(p) => {
                p.last_activity = p.last_activity.max(at);
                false
            }
            None => {
                self.projects.insert(
                    root.to_string(),
                    Project { root: root.to_string(), name: basename(root), git: None, last_activity: at },
                );
                true
            }
        }
    }

    fn push_feed(&mut self, events: Vec<FeedEvent>) {
        for e in events {
            let _ = self.store.send(StoreCmd::Feed(e.clone()));
            self.feed.push_back(e);
        }
        while self.feed.len() > FEED_LIMIT {
            self.feed.pop_front();
        }
    }

    fn persist(&self, id: &str) {
        if let Some(s) = self.sessions.get(id) {
            let _ = self.store.send(StoreCmd::Upsert(Box::new(s.clone())));
        }
    }

    /// Apply a hook event. `root` is the resolved project root.
    /// Returns true when this created a new project.
    pub fn ingest(&mut self, env: &HookEnvelope, root: &str) -> bool {
        let Some(id) = env.payload.get("session_id").and_then(|v| v.as_str()).map(str::to_string) else {
            return false;
        };
        self.hooks_seen += 1;
        let cwd = env.payload.get("cwd").and_then(|v| v.as_str()).unwrap_or(root).to_string();
        let sess = self.sessions.entry(id.clone()).or_insert_with(|| Session::new(&id, root, &cwd, env.at));
        let feed = reduce::apply_hook(sess, env);
        let event = reduce::event_name(&env.payload).to_string();
        let project_root = sess.project_root.clone();

        self.refresh_transcript(&id);
        if event == "Stop"
            && let Some(s) = self.sessions.get_mut(&id)
            && let Some(t) = s.context_tokens
        {
            reduce::push_context(s, t);
        }
        let new_project = self.ensure_project(&project_root, env.at);
        self.push_feed(feed);
        self.persist(&id);
        new_project
    }

    /// Read new transcript lines for a session. Returns true if anything changed.
    pub fn refresh_transcript(&mut self, id: &str) -> bool {
        let Some(sess) = self.sessions.get_mut(id) else { return false };
        let Some(path) = sess.transcript_path.clone() else { return false };
        let entry = self.transcripts.entry(id.to_string()).or_default();
        match transcript::read_new(&path, &mut entry.0, &mut entry.1) {
            Ok(true) => {
                let before = sess.clone();
                // Without hooks, the transcript is the only source of counts.
                if sess.origin == Origin::Hooks {
                    entry.1.apply_to(sess);
                } else {
                    entry.1.apply_history(sess);
                }
                *sess != before
            }
            _ => false,
        }
    }

    pub fn knows(&self, id: &str) -> bool {
        self.sessions.contains_key(id) || self.dismissed.contains(id)
    }

    pub fn transcript_path(&self, id: &str) -> Option<String> {
        self.sessions.get(id).and_then(|s| s.transcript_path.clone())
    }

    /// A session found in Claude's running-session list. Hooks win: this only
    /// fills in sessions that no hook has described yet. Returns true when
    /// something changed.
    pub fn upsert_running(&mut self, r: &crate::discover::Running, root: &str, transcript: Option<String>) -> bool {
        let id = r.session_id.clone();
        if let Some(s) = self.sessions.get_mut(&id) {
            if s.origin == Origin::Hooks {
                if s.pid.is_none() && s.state.is_live() {
                    s.pid = Some(r.pid);
                    return true;
                }
                return false;
            }
            let before = s.clone();
            s.origin = Origin::Running;
            s.pid = Some(r.pid);
            let next = state_for(r.status.as_deref(), s.turns);
            if !s.state.same_kind(&next) {
                s.state_since = r.status_updated_at.unwrap_or_else(now_ms);
            }
            s.state = next;
            if s.transcript_path.is_none() {
                s.transcript_path = transcript;
            }
            let changed = *s != before;
            self.refresh_transcript(&id);
            if changed {
                self.persist(&id);
            }
            return changed;
        }
        let started = r.started_at.unwrap_or_else(now_ms);
        let mut s = Session::new(&id, root, &r.cwd, started);
        s.origin = Origin::Running;
        s.pid = Some(r.pid);
        s.transcript_path = transcript;
        s.state_since = r.status_updated_at.unwrap_or(started);
        self.sessions.insert(id.clone(), s);
        self.refresh_transcript(&id);
        if let Some(s) = self.sessions.get_mut(&id) {
            s.state = state_for(r.status.as_deref(), s.turns);
            let at = s.last_event_at;
            self.ensure_project(root, at);
        }
        self.persist(&id);
        true
    }

    /// A past session read from its transcript. Returns true if it was added.
    pub fn add_history(
        &mut self,
        id: &str,
        path: &str,
        root: &str,
        cursor: transcript::Cursor,
        summary: transcript::Summary,
    ) -> bool {
        if self.knows(id) {
            return false;
        }
        let cwd = summary.cwd.clone().unwrap_or_else(|| root.to_string());
        let mut s = Session::new(id, root, &cwd, summary.first_at.unwrap_or(0));
        s.origin = Origin::History;
        s.transcript_path = Some(path.to_string());
        summary.apply_history(&mut s);
        let at = s.last_event_at;
        self.sessions.insert(id.to_string(), s);
        self.transcripts.insert(id.to_string(), (cursor, summary));
        self.ensure_project(root, at);
        self.persist(id);
        true
    }

    /// Live sessions with a known Claude process: (id, pid).
    pub fn live_sessions(&self) -> Vec<(String, u32)> {
        self.sessions.values().filter(|s| s.state.is_live()).filter_map(|s| Some((s.id.clone(), s.pid?))).collect()
    }

    pub fn session_state(&self, id: &str) -> Option<aulora_core::model::SessionState> {
        self.sessions.get(id).map(|s| s.state.clone())
    }

    pub fn live_pids(&self) -> Vec<u32> {
        let mut pids: Vec<u32> = self.sessions.values().filter(|s| s.state.is_live()).filter_map(|s| s.pid).collect();
        pids.sort_unstable();
        pids.dedup();
        pids
    }

    pub fn mark_dead(&mut self, pids: &[u32]) -> bool {
        let now = now_ms();
        let ids: Vec<String> = self
            .sessions
            .values()
            .filter(|s| s.state.is_live() && s.pid.is_some_and(|p| pids.contains(&p)))
            .map(|s| s.id.clone())
            .collect();
        for id in &ids {
            let ev = self.sessions.get_mut(id).and_then(|s| reduce::mark_gone(s, now));
            if let Some(ev) = ev {
                self.push_feed(vec![ev]);
            }
            self.persist(id);
        }
        !ids.is_empty()
    }

    pub fn working_ids(&self) -> Vec<String> {
        self.sessions
            .values()
            .filter(|s| {
                matches!(s.state, SessionState::Working | SessionState::Compacting | SessionState::NeedsYou { .. })
            })
            .map(|s| s.id.clone())
            .collect()
    }

    /// Roots worth polling git for: anything with a live session or recent activity.
    pub fn active_roots(&self, now: Ms) -> Vec<String> {
        self.projects
            .values()
            .filter(|p| {
                now - p.last_activity < 15 * 60_000
                    || self.sessions.values().any(|s| s.project_root == p.root && s.state.is_live())
            })
            .map(|p| p.root.clone())
            .collect()
    }

    pub fn set_git(&mut self, root: &str, git: Option<GitInfo>) -> bool {
        match self.projects.get_mut(root) {
            Some(p) if p.git != git => {
                p.git = git;
                true
            }
            _ => false,
        }
    }

    /// The user removed a session: keep it from being rediscovered.
    pub fn dismiss(&mut self, id: &str) -> bool {
        self.dismissed.insert(id.to_string());
        self.remove(id)
    }

    /// Forget a session. Expired ones can come back as history.
    fn remove(&mut self, id: &str) -> bool {
        let removed = self.sessions.remove(id).is_some();
        if removed {
            self.transcripts.remove(id);
            let _ = self.store.send(StoreCmd::Delete(id.to_string()));
            self.drop_empty_projects();
        }
        removed
    }

    pub fn clear_ended(&mut self) -> bool {
        let ids: Vec<String> = self.sessions.values().filter(|s| !s.state.is_live()).map(|s| s.id.clone()).collect();
        for id in &ids {
            self.dismiss(id);
        }
        !ids.is_empty()
    }

    /// Expire old ended sessions and stale ones. Returns true if anything changed.
    pub fn prune(&mut self, now: Ms) -> bool {
        let mut changed = false;
        let stale: Vec<String> = self
            .sessions
            .values()
            .filter(|s| s.state.is_live() && s.pid.is_none() && now - s.last_event_at > STALE_MS)
            .map(|s| s.id.clone())
            .collect();
        for id in stale {
            if let Some(s) = self.sessions.get_mut(&id) {
                s.state = SessionState::Ended { reason: "stale".into() };
                changed = true;
            }
            self.persist(&id);
        }
        let expired: Vec<String> = self
            .sessions
            .values()
            .filter(|s| {
                let ttl = if s.origin == Origin::History { HISTORY_MS } else { ENDED_TTL_MS };
                !s.state.is_live() && now - s.last_event_at > ttl
            })
            .map(|s| s.id.clone())
            .collect();
        for id in expired {
            changed |= self.remove(&id);
        }
        changed
    }

    fn drop_empty_projects(&mut self) {
        let roots: std::collections::HashSet<&String> = self
            .sessions
            .values()
            .map(|s| &s.project_root)
            .chain(self.panes.values().map(|p| &p.project_root))
            .collect();
        self.projects.retain(|root, _| roots.contains(root));
    }

    /// Register a new pane. Returns true when this created a new project.
    pub fn add_pane(&mut self, pane: Pane) -> bool {
        let new_project = self.ensure_project(&pane.project_root, pane.created_at);
        self.panes.insert(pane.id.clone(), pane);
        new_project
    }

    /// Record a pane's exit, unless it has been restarted since.
    pub fn pane_exited(&mut self, id: &str, generation: u32, exit: PaneExit) -> bool {
        match self.panes.get_mut(id) {
            Some(p) if p.generation == generation => {
                p.exit = Some(exit);
                p.pid = None;
                true
            }
            _ => false,
        }
    }

    pub fn remove_pane(&mut self, id: &str) -> Option<Pane> {
        let p = self.panes.remove(id);
        if p.is_some() {
            self.drop_empty_projects();
        }
        p
    }

    /// A unique name for a new pane in a project: "claude", "claude 2"…
    pub fn pane_name(&self, root: &str, base: &str) -> String {
        let taken: std::collections::HashSet<&str> =
            self.panes.values().filter(|p| p.project_root == root).map(|p| p.name.as_str()).collect();
        if !taken.contains(base) {
            return base.to_string();
        }
        (2..).map(|n| format!("{base} {n}")).find(|n| !taken.contains(n.as_str())).expect("a free name")
    }

    pub fn snapshot(&self) -> Snapshot {
        let mut projects: Vec<Project> = self.projects.values().cloned().collect();
        projects.sort_by(|a, b| a.root.cmp(&b.root));
        let mut sessions: Vec<Session> = self.sessions.values().cloned().collect();
        sessions.sort_by(|a, b| a.id.cmp(&b.id));
        let mut panes: Vec<Pane> = self.panes.values().cloned().collect();
        panes.sort_by_key(|p| (p.created_at, p.id.clone()));
        Snapshot {
            daemon: DaemonInfo {
                version: aulora_core::VERSION.to_string(),
                pid: std::process::id(),
                started_at: self.started_at,
                hooks_seen: self.hooks_seen,
            },
            projects,
            sessions,
            feed: self.feed.iter().cloned().collect(),
            panes,
        }
    }
}
