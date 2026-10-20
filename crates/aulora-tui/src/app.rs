use std::collections::HashMap;

use aulora_core::model::*;
use ratatui::layout::Rect;

use crate::conv::{self, Conversation};
use crate::keymap::Scope;
use crate::term::Term;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RowRef {
    Project(String),
    Session(String),
    Pane(String),
}

impl RowRef {
    pub fn is_project(&self) -> bool {
        matches!(self, RowRef::Project(_))
    }
}

/// Which region keys go to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Sidebar,
    Main,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Terminal,
    Conversation,
    Overview,
    Changes,
}

impl Tab {
    pub fn label(self) -> &'static str {
        match self {
            Tab::Terminal => "Terminal",
            Tab::Conversation => "Conversation",
            Tab::Overview => "Overview",
            Tab::Changes => "Changes",
        }
    }
}

/// The five states shown everywhere (Herdr).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Status {
    /// Needs you.
    Blocked,
    /// Finished a turn you haven't looked at.
    Done,
    Working,
    /// Finished and seen, or waiting for a first prompt.
    Idle,
    /// Not running.
    Ended,
}

impl Status {
    pub fn word(self) -> &'static str {
        match self {
            Status::Blocked => "blocked",
            Status::Done => "done",
            Status::Working => "working",
            Status::Idle => "idle",
            Status::Ended => "ended",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Conn {
    Connecting,
    Live,
    Offline(String),
    Demo,
}

/// What a screen cell belongs to, recorded during drawing for the mouse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hit {
    Row(RowRef),
    /// The × on a row.
    Close(RowRef),
    /// The + on a project row.
    Add(String),
    MenuItem(usize),
    /// A row of the Changes tree, by index.
    ChangeRow(usize),
    ChangeTree,
    Diff,
    /// A button that runs an action.
    Action(crate::keymap::Action),
    /// The prompt box under a session's conversation.
    Compose(String),
    /// The resume button above a session.
    Resume(String),
    /// Behind an open menu.
    Dismiss,
    Tab(Tab),
    Terminal(String),
    Conversation(String),
    NewClaude,
    NewShell,
    Sidebar,
    Main,
}

pub struct Toast {
    pub msg: String,
    pub at: Ms,
    pub error: bool,
    /// How long it stays; errors stay until the next key.
    pub ttl: Ms,
}

/// What a menu entry does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuItem {
    Type(String),
    Resume(String),
    Restart(String),
    Close(RowRef),
    OpenEditor(String),
    NewClaude(String),
    NewShell(String),
}

impl MenuItem {
    pub fn label(&self) -> &'static str {
        match self {
            MenuItem::Type(_) => "Type here",
            MenuItem::Resume(_) => "Resume in a terminal",
            MenuItem::Restart(_) => "Restart",
            MenuItem::Close(RowRef::Session(_)) => "Dismiss",
            MenuItem::Close(_) => "Close",
            MenuItem::OpenEditor(_) => "Open in VS Code",
            MenuItem::NewClaude(_) => "New Claude here",
            MenuItem::NewShell(_) => "New shell here",
        }
    }

    pub fn danger(&self) -> bool {
        matches!(self, MenuItem::Close(_))
    }
}

pub struct Menu {
    pub x: u16,
    pub y: u16,
    pub items: Vec<MenuItem>,
    pub selected: usize,
}

pub struct HelpState {
    pub query: String,
    pub selected: usize,
}

pub struct Confirm {
    pub title: String,
    pub body: String,
    pub action: crate::ConfirmAction,
    /// Cancel is focused unless the user moves to the action (k9s).
    pub on_action: bool,
}

pub struct App {
    pub snap: Option<Snapshot>,
    pub conn: Conn,
    pub now: Ms,
    pub selected: Option<RowRef>,
    pub focus: Focus,
    /// Keys go straight to the selected pane (Terminal tab, focused).
    pub typing: bool,
    /// The row under the mouse.
    pub hover: Option<RowRef>,
    /// An open right-click menu.
    pub menu: Option<Menu>,
    pub show_ended: bool,
    pub show_history: bool,
    pub filter: String,
    pub filtering: bool,
    pub help: Option<HelpState>,
    pub toast: Option<Toast>,
    pub hooks_installed: bool,
    pub editor: String,
    pub sidebar_scroll: usize,
    pub zoom: bool,
    pub mouse: bool,
    /// Local screens of attached panes.
    pub terms: HashMap<String, Term>,
    /// Pane (and generation) this client is attached to.
    pub attached: Option<(String, u32)>,
    /// Select (and maybe focus) this pane as soon as it appears.
    pub pending_select: Option<(String, bool)>,
    /// Size of the pane view in the last frame.
    pub view_size: Option<(String, u16, u16)>,
    /// Height of the conversation viewport in the last frame.
    pub conv_height: usize,
    /// The tab last chosen; it stays as you move between rows, when the
    /// new row has it (VS Code keeps Source Control open as you switch files).
    pub tab_pref: Tab,
    pub worker: Option<crate::changes::Worker>,
    /// A prompt being written for a session in an editor.
    pub compose: Option<crate::compose::Compose>,
    /// A commit message being written, for the project in `session`.
    pub commit_edit: Option<crate::compose::Compose>,
    pub commit_drafts: HashMap<String, String>,
    /// Unsent prompts, by session.
    pub drafts: HashMap<String, String>,
    pub sent_tx: std::sync::mpsc::Sender<crate::compose::Sent>,
    pub sent_rx: std::sync::mpsc::Receiver<crate::compose::Sent>,
    pub changes: HashMap<String, crate::changes::Project>,
    pub changes_view: HashMap<String, crate::changes::View>,
    /// Styled diffs by root and file, with the change they were made from.
    pub diffs: HashMap<String, (u64, crate::changes::DiffState)>,
    pub conversations: HashMap<String, (Conversation, Ms)>,
    pub theme_picker: Option<usize>,
    pub theme_before: usize,
    pub searching: bool,
    pub query: String,
    /// When each session was last looked at, for the "done" state.
    pub seen: HashMap<String, Ms>,
    pub seen_dirty: bool,
    /// Mouse targets from the last frame, last drawn on top.
    pub hits: Vec<(Rect, Hit)>,
    /// A destructive action waiting for confirmation.
    pub confirm: Option<Confirm>,
}

fn seen_path() -> std::path::PathBuf {
    aulora_core::paths::home().join("seen.json")
}

impl App {
    pub fn new(conn: Conn, hooks_installed: bool, editor: String) -> Self {
        let (sent_tx, sent_rx) = std::sync::mpsc::channel();
        let seen = if conn == Conn::Demo {
            HashMap::new()
        } else {
            std::fs::read_to_string(seen_path())
                .ok()
                .and_then(|t| serde_json::from_str::<HashMap<String, Ms>>(&t).ok())
                .unwrap_or_default()
        };
        App {
            snap: None,
            conn,
            now: now_ms(),
            selected: None,
            focus: Focus::Sidebar,
            typing: false,
            hover: None,
            menu: None,
            show_ended: true,
            show_history: false,
            filter: String::new(),
            filtering: false,
            help: None,
            toast: None,
            hooks_installed,
            editor,
            sidebar_scroll: 0,
            zoom: false,
            mouse: true,
            terms: HashMap::new(),
            attached: None,
            pending_select: None,
            view_size: None,
            conv_height: 20,
            tab_pref: Tab::Terminal,
            compose: None,
            drafts: HashMap::new(),
            commit_edit: None,
            commit_drafts: HashMap::new(),
            sent_tx,
            sent_rx,
            worker: None,
            changes: HashMap::new(),
            changes_view: HashMap::new(),
            diffs: HashMap::new(),
            conversations: HashMap::new(),
            theme_picker: None,
            theme_before: 0,
            searching: false,
            query: String::new(),
            seen,
            seen_dirty: false,
            hits: Vec::new(),
            confirm: None,
        }
    }

    pub fn save_seen(&mut self) {
        if !self.seen_dirty || self.conn == Conn::Demo {
            return;
        }
        self.seen_dirty = false;
        let cutoff = self.now - 14 * 24 * 3600 * 1000;
        self.seen.retain(|_, at| *at > cutoff);
        let _ = aulora_core::paths::ensure_home();
        let _ = std::fs::write(seen_path(), serde_json::to_string(&self.seen).unwrap_or_default());
    }

    pub fn set_snapshot(&mut self, snap: Snapshot) {
        let prev_root = self.selected_root();
        self.snap = Some(snap);
        if let Some((id, focus)) = self.pending_select.clone()
            && self.pane(&id).is_some()
        {
            self.selected = Some(RowRef::Pane(id));
            self.tab_pref = Tab::Terminal;
            if focus {
                self.focus = Focus::Main;
                self.typing = true;
            }
            self.pending_select = None;
        }
        let rows = self.rows();
        if !self.selected.as_ref().is_some_and(|s| rows.contains(s)) {
            // When a row goes away (closed, dismissed), stay in its project;
            // on start, begin on whatever needs attention most.
            let project = prev_root.map(RowRef::Project).filter(|p| rows.contains(p));
            self.selected = project.or_else(|| self.agents().first().cloned()).or(rows.first().cloned());
        }
        if self.typing && self.selected_pane().is_none_or(|p| !p.is_running()) {
            self.typing = false;
        }
        let live: std::collections::HashSet<String> =
            self.snap.as_ref().map(|s| s.panes.iter().map(|p| p.id.clone()).collect()).unwrap_or_default();
        self.terms.retain(|id, _| live.contains(id));
    }

    // --- lookups ----------------------------------------------------------

    pub fn session(&self, id: &str) -> Option<&Session> {
        self.snap.as_ref()?.sessions.iter().find(|s| s.id == id)
    }

    pub fn project(&self, root: &str) -> Option<&Project> {
        self.snap.as_ref()?.projects.iter().find(|p| p.root == root)
    }

    pub fn pane(&self, id: &str) -> Option<&Pane> {
        self.snap.as_ref()?.panes.iter().find(|p| p.id == id)
    }

    pub fn selected_pane(&self) -> Option<&Pane> {
        match &self.selected {
            Some(RowRef::Pane(id)) => self.pane(id),
            _ => None,
        }
    }

    /// The Claude session running in a pane: live ones first, then the latest.
    pub fn pane_session(&self, pane_id: &str) -> Option<&Session> {
        self.snap
            .as_ref()?
            .sessions
            .iter()
            .filter(|s| s.terminal.pane_id.as_deref() == Some(pane_id))
            .max_by_key(|s| (s.state.is_live(), s.last_event_at))
    }

    pub fn selected_session_id(&self) -> Option<String> {
        match &self.selected {
            Some(RowRef::Session(id)) => Some(id.clone()),
            Some(RowRef::Pane(id)) => self.pane_session(id).map(|s| s.id.clone()),
            _ => None,
        }
    }

    pub fn root_of(&self, r: &RowRef) -> Option<String> {
        match r {
            RowRef::Project(root) => Some(root.clone()),
            RowRef::Session(id) => self.session(id).map(|s| s.project_root.clone()),
            RowRef::Pane(id) => self.pane(id).map(|p| p.project_root.clone()),
        }
    }

    pub fn selected_root(&self) -> Option<String> {
        self.selected.as_ref().and_then(|r| self.root_of(r))
    }

    // --- states -----------------------------------------------------------

    pub fn session_status(&self, s: &Session) -> Status {
        match &s.state {
            SessionState::NeedsYou { .. } | SessionState::Errored { .. } => Status::Blocked,
            SessionState::Working | SessionState::Compacting => Status::Working,
            SessionState::YourTurn => {
                let seen = self.seen.get(&s.id).copied().unwrap_or(0);
                if seen >= s.state_since { Status::Idle } else { Status::Done }
            }
            SessionState::Idle => Status::Idle,
            SessionState::Ended { .. } => Status::Ended,
        }
    }

    pub fn status(&self, r: &RowRef) -> Status {
        match r {
            RowRef::Session(id) => self.session(id).map(|s| self.session_status(s)).unwrap_or(Status::Ended),
            RowRef::Pane(id) => {
                let Some(p) = self.pane(id) else { return Status::Ended };
                if !p.is_running() {
                    return Status::Ended;
                }
                match self.pane_session(id).filter(|s| s.state.is_live()) {
                    Some(s) => self.session_status(s),
                    None if self.now - p.last_output_at < 2_000 => Status::Working,
                    None => Status::Idle,
                }
            }
            RowRef::Project(root) => self.agents_in(root).iter().map(|a| self.status(a)).min().unwrap_or(Status::Ended),
        }
    }

    /// When an agent last changed state, for sorting and "4m" labels.
    pub fn since(&self, r: &RowRef) -> Ms {
        match r {
            RowRef::Session(id) => self.session(id).map(|s| s.state_since).unwrap_or(0),
            RowRef::Pane(id) => match self.pane_session(id).filter(|s| s.state.is_live()) {
                Some(s) => s.state_since,
                None => self.pane(id).map(|p| p.exit.as_ref().map(|e| e.at).unwrap_or(p.created_at)).unwrap_or(0),
            },
            RowRef::Project(_) => 0,
        }
    }

    pub fn title(&self, r: &RowRef) -> String {
        match r {
            RowRef::Project(root) => self.project(root).map(|p| p.name.clone()).unwrap_or_default(),
            RowRef::Session(id) => self.session(id).map(|s| s.display_title()).unwrap_or_default(),
            RowRef::Pane(id) => {
                let Some(p) = self.pane(id) else { return String::new() };
                match self.pane_session(id).filter(|s| s.state.is_live()) {
                    Some(s) => format!("{} · {}", p.name, s.display_title()),
                    None => p.name.clone(),
                }
            }
        }
    }

    /// Mark what's on screen as seen, so "done" settles into "idle".
    pub fn mark_seen(&mut self) {
        if let Some(id) = self.selected_session_id()
            && let Some(s) = self.session(&id)
            && self.session_status(s) == Status::Done
        {
            self.seen.insert(id, self.now);
            self.seen_dirty = true;
        }
    }

    // --- sidebar contents ---------------------------------------------------

    fn matches_filter(&self, r: &RowRef) -> bool {
        if self.filter.is_empty() {
            return true;
        }
        let root = self.root_of(r).unwrap_or_default();
        let hay = format!("{} {}", aulora_core::util::basename(&root), self.title(r)).to_lowercase();
        hay.contains(&self.filter.to_lowercase())
    }

    /// Whether a session already shows as one of our panes: it runs in
    /// one, or one was started to resume it.
    fn in_existing_pane(&self, s: &Session) -> bool {
        s.terminal.pane_id.as_deref().is_some_and(|id| self.pane(id).is_some()) || self.resumed_in(&s.id).is_some()
    }

    /// The pane started with `claude --resume <id>`, if any.
    pub fn resumed_in(&self, session_id: &str) -> Option<&Pane> {
        self.snap
            .as_ref()?
            .panes
            .iter()
            .find(|p| p.kind == PaneKind::Claude && p.args.windows(2).any(|w| w[0] == "--resume" && w[1] == session_id))
    }

    /// Everything running in one project.
    pub fn agents_in(&self, root: &str) -> Vec<RowRef> {
        let Some(snap) = &self.snap else { return vec![] };
        let panes = snap.panes.iter().filter(|p| p.project_root == root).map(|p| RowRef::Pane(p.id.clone()));
        let sessions = snap
            .sessions
            .iter()
            .filter(|s| s.project_root == root && !self.in_existing_pane(s))
            .filter(|s| self.show_ended || s.state.is_live())
            .filter(|s| self.show_history || s.origin != Origin::History)
            .map(|s| RowRef::Session(s.id.clone()));
        panes.chain(sessions).filter(|r| self.matches_filter(r)).collect()
    }

    /// Projects with something to show, most urgent first.
    pub fn projects(&self) -> Vec<RowRef> {
        let Some(snap) = &self.snap else { return vec![] };
        let mut v: Vec<(Status, Ms, String)> = snap
            .projects
            .iter()
            .filter_map(|p| {
                let agents = self.agents_in(&p.root);
                if agents.is_empty() {
                    return None;
                }
                let status = agents.iter().map(|a| self.status(a)).min().unwrap_or(Status::Ended);
                let last = agents.iter().map(|a| self.since(a)).max().unwrap_or(0);
                Some((status, last, p.root.clone()))
            })
            .collect();
        v.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)).then(a.2.cmp(&b.2)));
        v.into_iter().map(|(_, _, root)| RowRef::Project(root)).collect()
    }

    /// A project's agents, most urgent first.
    pub fn agents_sorted(&self, root: &str) -> Vec<RowRef> {
        let mut a = self.agents_in(root);
        a.sort_by(|x, y| self.status(x).cmp(&self.status(y)).then(self.since(y).cmp(&self.since(x))));
        a
    }

    /// Every agent, in sidebar order.
    pub fn agents(&self) -> Vec<RowRef> {
        self.rows().into_iter().filter(|r| !r.is_project()).collect()
    }

    /// Every selectable row, in sidebar order: each project, then its agents.
    pub fn rows(&self) -> Vec<RowRef> {
        let mut rows = Vec::new();
        for p in self.projects() {
            let RowRef::Project(root) = &p else { continue };
            let agents = self.agents_sorted(root);
            rows.push(p);
            rows.extend(agents);
        }
        rows
    }

    /// Whether a row has a × (panes, and sessions that have ended).
    pub fn closable(&self, r: &RowRef) -> bool {
        match r {
            RowRef::Pane(id) => self.pane(id).is_some(),
            RowRef::Session(id) => self.session(id).is_some_and(|s| !s.state.is_live()),
            RowRef::Project(_) => false,
        }
    }

    /// The menu for a row (right-click).
    pub fn menu_for(&self, r: &RowRef) -> Vec<MenuItem> {
        let mut items = Vec::new();
        match r {
            RowRef::Pane(id) => {
                let running = self.pane(id).is_some_and(|p| p.is_running());
                if running {
                    items.push(MenuItem::Type(id.clone()));
                } else {
                    items.push(MenuItem::Restart(id.clone()));
                }
                items.push(MenuItem::Close(r.clone()));
            }
            RowRef::Session(id) => {
                items.push(MenuItem::Resume(id.clone()));
                if self.session(id).is_some_and(|s| !s.state.is_live()) {
                    items.push(MenuItem::Close(r.clone()));
                }
            }
            RowRef::Project(_) => {}
        }
        if let Some(root) = self.root_of(r) {
            items.push(MenuItem::NewClaude(root.clone()));
            items.push(MenuItem::NewShell(root.clone()));
            items.push(MenuItem::OpenEditor(root));
        }
        items
    }

    pub fn move_selection(&mut self, delta: i32) {
        let rows = self.rows();
        if rows.is_empty() {
            self.selected = None;
            return;
        }
        let cur = self.selected.as_ref().and_then(|s| rows.iter().position(|r| r == s)).unwrap_or(0) as i32;
        let next = (cur + delta).clamp(0, rows.len() as i32 - 1) as usize;
        self.select(rows[next].clone());
    }

    pub fn select(&mut self, r: RowRef) {
        if self.selected.as_ref() != Some(&r) {
            self.typing = false;
            self.close_compose();
        }
        self.selected = Some(r);
    }

    pub fn select_edge(&mut self, last: bool) {
        let rows = self.rows();
        if let Some(r) = if last { rows.last() } else { rows.first() } {
            self.select(r.clone());
        }
    }

    /// Cycle through what needs you, then what finished unseen.
    pub fn next_attention(&mut self) -> bool {
        let mut targets: Vec<RowRef> =
            self.agents().into_iter().filter(|r| matches!(self.status(r), Status::Blocked | Status::Done)).collect();
        targets.sort_by_key(|r| (self.status(r), self.since(r)));
        if targets.is_empty() {
            return false;
        }
        let cur = self.selected.as_ref().and_then(|s| targets.iter().position(|r| r == s));
        let next = cur.map(|i| (i + 1) % targets.len()).unwrap_or(0);
        self.select(targets[next].clone());
        true
    }

    /// Sessions that are blocked, oldest first, for the alert line.
    pub fn alerts(&self) -> Vec<&Session> {
        let Some(snap) = &self.snap else { return vec![] };
        let mut v: Vec<&Session> =
            snap.sessions.iter().filter(|s| matches!(s.state, SessionState::NeedsYou { .. })).collect();
        v.sort_by_key(|s| s.state_since);
        v
    }

    pub fn counts(&self) -> HashMap<Status, usize> {
        let mut c = HashMap::new();
        for r in self.agents() {
            *c.entry(self.status(&r)).or_insert(0) += 1;
        }
        c
    }

    pub fn history_count(&self) -> usize {
        self.snap.as_ref().map(|s| s.sessions.iter().filter(|s| s.origin == Origin::History).count()).unwrap_or(0)
    }

    // --- tabs and focus ---------------------------------------------------

    pub fn tabs(&self) -> Vec<Tab> {
        let Some(sel) = &self.selected else { return vec![] };
        let mut tabs = match sel {
            RowRef::Pane(_) => vec![Tab::Terminal],
            RowRef::Session(_) => vec![Tab::Conversation],
            RowRef::Project(_) => vec![Tab::Overview],
        };
        if self.root_of(sel).and_then(|r| self.project(&r)).is_some_and(|p| p.git.is_some()) {
            tabs.push(Tab::Changes);
        }
        tabs
    }

    pub fn current_tab(&self) -> Tab {
        let tabs = self.tabs();
        if tabs.contains(&self.tab_pref) {
            return self.tab_pref;
        }
        tabs.first().copied().unwrap_or(Tab::Overview)
    }

    pub fn set_tab(&mut self, tab: Tab) {
        if self.tabs().contains(&tab) {
            self.tab_pref = tab;
            if tab != Tab::Terminal {
                self.typing = false;
            }
        }
    }

    pub fn cycle_tab(&mut self, delta: i32) {
        let tabs = self.tabs();
        if tabs.len() < 2 {
            return;
        }
        let cur = tabs.iter().position(|t| *t == self.current_tab()).unwrap_or(0) as i32;
        self.set_tab(tabs[(cur + delta).rem_euclid(tabs.len() as i32) as usize]);
    }

    /// The keymap scope for where focus is now.
    pub fn scope(&self) -> Scope {
        match self.focus {
            Focus::Sidebar => Scope::Sidebar,
            Focus::Main => match self.current_tab() {
                Tab::Terminal => Scope::Pane,
                Tab::Conversation => Scope::Conversation,
                Tab::Overview => Scope::Overview,
                Tab::Changes => Scope::Changes,
            },
        }
    }

    // --- conversations ------------------------------------------------------

    /// A session's conversation, read from its transcript and kept current,
    /// or pieced together from the snapshot when there's no transcript.
    pub fn conversation(&mut self, session_id: &str) -> Option<&mut Conversation> {
        let s = self.session(session_id)?.clone();
        let now = self.now;
        let notes: Vec<FeedEvent> = self
            .snap
            .as_ref()
            .map(|snap| conv::feed_for(&snap.feed, session_id).into_iter().cloned().collect())
            .unwrap_or_default();
        let path = s.transcript_path.clone().filter(|p| std::path::Path::new(p).is_file());
        match path {
            Some(p) => {
                let entry =
                    self.conversations.entry(session_id.to_string()).or_insert_with(|| (Conversation::new(Some(p)), 0));
                if now - entry.1 > 250 {
                    entry.0.refresh();
                    entry.1 = now;
                }
            }
            None => {
                let mut fresh = Conversation::from_session(&s);
                if let Some((old, _)) = self.conversations.get(session_id) {
                    fresh.adopt_view(old);
                }
                self.conversations.insert(session_id.to_string(), (fresh, now));
            }
        }
        let c = &mut self.conversations.get_mut(session_id)?.0;
        let refs: Vec<&FeedEvent> = notes.iter().collect();
        c.add_notes(&refs);
        c.working = matches!(s.state, SessionState::Working | SessionState::Compacting);
        Some(c)
    }

    pub fn toast(&mut self, msg: impl Into<String>) {
        self.toast = Some(Toast { msg: msg.into(), at: self.now, error: false, ttl: 2_500 });
    }

    pub fn error(&mut self, msg: impl Into<String>) {
        self.toast = Some(Toast { msg: msg.into(), at: self.now, error: true, ttl: Ms::MAX });
    }

    /// An agent started needing you (Herdr: 8 s, red).
    pub fn alert(&mut self, msg: impl Into<String>) {
        self.toast = Some(Toast { msg: msg.into(), at: self.now, error: true, ttl: 8_000 });
    }

    /// The topmost mouse target at a cell.
    pub fn hit(&self, col: u16, row: u16) -> Option<(Rect, Hit)> {
        self.hits
            .iter()
            .rev()
            .find(|(r, _)| col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height)
            .cloned()
    }
}
