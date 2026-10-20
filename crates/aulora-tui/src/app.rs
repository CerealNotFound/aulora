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
    Files,
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
    FileTree,
    FileRow(usize),
    FileResult(usize),
    FileView,
    FinderItem(usize),
    /// A row of the Changes tree, by index.
    ChangeRow(usize),
    ChangeTree,
    Diff,
    /// A button that runs an action.
    Action(crate::keymap::Action),
    /// The prompt box under a session's conversation.
    Compose(String),
    /// The sidebar's `new` and `menu`.
    NewMenu,
    AppMenu,
    /// The resume button above a session.
    Resume(String),
    /// Behind an open menu.
    Dismiss,
    /// A project tab in the strip.
    SpaceTab(usize),
    NewTab,
    /// Anywhere in a leaf.
    Leaf(u64),
    /// A changed file listed at the top of the Files panel.
    ChangedFile(String),
    /// A row of Claude's question picker.
    AnswerRow(usize),
    /// The border between a split's sides.
    Split(u64),
    Terminal(String),
    Conversation(String),
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
    /// End a live session running outside aulora.
    End(String),
    OpenEditor(String),
    NewClaude(String),
    NewShell(String),
    NewTab(String),
    /// Bring a session's VS Code terminal to the front.
    Jump(String),
    /// Show a file or folder in Finder.
    Reveal(String),
    /// Run an action, with its label.
    Run(crate::keymap::Action, &'static str),
}

impl MenuItem {
    pub fn label(&self) -> &'static str {
        match self {
            MenuItem::Type(_) => "Type here",
            MenuItem::Resume(_) => "Resume in a terminal",
            MenuItem::Restart(_) => "Restart",
            MenuItem::Close(RowRef::Session(_)) => "Dismiss",
            MenuItem::End(_) => "End session",
            MenuItem::Close(_) => "Close",
            MenuItem::OpenEditor(_) => "Open in VS Code",
            MenuItem::NewClaude(_) => "New Claude here",
            MenuItem::NewShell(_) => "New shell here",
            MenuItem::NewTab(_) => "New tab",
            MenuItem::Jump(_) => "Go to its terminal in VS Code",
            MenuItem::Reveal(_) => "Open in Finder",
            MenuItem::Run(_, label) => label,
        }
    }

    pub fn danger(&self) -> bool {
        matches!(self, MenuItem::Close(_) | MenuItem::End(_))
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
    /// The selection the sidebar last scrolled to. It scrolls to the
    /// selection only when that changes, so the wheel can move it away.
    pub sidebar_followed: Option<RowRef>,
    pub mouse: bool,
    /// Local screens of attached panes.
    pub terms: HashMap<String, Term>,
    /// Pane (and generation) this client is attached to.
    /// Panes streaming to us, with the generation attached.
    pub attached: HashMap<String, u32>,
    /// Select (and maybe focus) this pane as soon as it appears.
    pub pending_select: Option<(String, bool)>,
    /// Size of the pane view in the last frame.
    pub view_sizes: Vec<(String, u16, u16)>,
    /// Height of the conversation viewport in the last frame.
    pub conv_height: usize,
    /// The tab last chosen; it stays as you move between rows, when the
    /// new row has it (VS Code keeps Source Control open as you switch files).
    pub layouts: crate::layout::Layouts,
    pub layouts_dirty: bool,
    /// Where each leaf of the current tab was drawn last frame.
    pub leaf_rects: Vec<(u64, ratatui::layout::Rect)>,
    /// Splits drawn last frame: id, direction, area, dividing line.
    pub split_rects: Vec<(u64, crate::layout::Dir, ratatui::layout::Rect, ratatui::layout::Rect)>,
    /// Where the next new pane goes: Some(dir) beside the focused leaf,
    /// None in a new tab.
    pub place_next: Option<Option<crate::layout::Dir>>,
    /// A split border being dragged.
    pub dragging: Option<u64>,
    /// Panes just placed, kept even before the daemon lists them.
    pub fresh_leaves: Vec<(String, Ms)>,
    /// The sidebar row the main area last followed.
    pub shown_row: Option<RowRef>,
    /// The sidebar as glyphs (Some(true)), full (Some(false)), or decided
    /// by width (None).
    pub sidebar_compact: Option<bool>,
    /// The Files panel down the right of each workspace (VS Code's
    /// explorer): shown unless you hid it with `f` or its ×.
    pub panel_shown: bool,
    /// The main area's width in the last frame, to size the panel by.
    pub main_width: u16,
    /// Where the mouse is, and the link under it in the last frame.
    pub mouse_pos: Option<(u16, u16)>,
    pub hover_url: Option<crate::links::Link>,
    /// Rows drawn this frame whose text was cut mid-word and goes on in
    /// the next row: (row, columns of the view, column where it was cut).
    pub cut_rows: Vec<(u16, std::ops::Range<u16>, u16)>,
    /// While drawing: whether the leaf being drawn has focus.
    pub leaf_focused: bool,
    pub worker: Option<crate::changes::Worker>,
    /// A prompt being written for a session in an editor.
    pub compose: Option<crate::compose::Compose>,
    /// A commit message being written, for the project in `session`.
    pub commit_edit: Option<crate::compose::Compose>,
    pub commit_drafts: HashMap<String, String>,
    /// Slash commands by project, read when first needed.
    pub slash: HashMap<String, Vec<(String, String)>>,
    /// Unsent prompts, by session.
    pub drafts: HashMap<String, String>,
    /// Images pasted into prompts; `[Image #n]` in a prompt is entry n-1.
    pub images: Vec<std::path::PathBuf>,
    pub sent_tx: std::sync::mpsc::Sender<crate::compose::Sent>,
    pub sent_rx: std::sync::mpsc::Receiver<crate::compose::Sent>,
    pub changes: HashMap<String, crate::changes::Project>,
    pub changes_view: HashMap<String, crate::changes::View>,
    pub files: HashMap<String, crate::files::Tree>,
    pub files_view: HashMap<String, crate::files::View>,
    /// `--select` and `--tab`, applied when the first snapshot arrives.
    pub preselect: Option<(Option<String>, Option<String>)>,
    /// Git repositories found on disk, for opening projects.
    pub folders: Option<Vec<String>>,
    pub folders_requested: bool,
    /// The project the next new pane belongs to, when it isn't the
    /// selected one (opening a project).
    pub place_root: Option<String>,
    /// When each project was last looked at, and the card for coming back.
    pub visits: HashMap<String, Ms>,
    pub away: Option<crate::away::Card>,
    /// The sidebar's project order, kept across restarts.
    pub project_order: Vec<String>,
    /// The first-run checklist.
    pub welcome: Option<crate::welcome::Welcome>,
    /// Ctrl-P.
    pub finder: Option<crate::finder::Finder>,
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
    /// Text selected with the mouse.
    pub selection: Option<crate::select::Selection>,
    /// Where you are in the question picker of a session waiting on you.
    pub picker: Option<crate::picker::Picker>,
    /// The last frame as drawn, before the selection was painted, to copy from.
    pub frame: Option<ratatui::buffer::Buffer>,
}

fn order_path() -> std::path::PathBuf {
    aulora_core::paths::home().join("order.json")
}

/// The sidebar's saved order of workspaces.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct Order {
    #[serde(default)]
    pub workspaces: Vec<String>,
}

pub fn load_order() -> Order {
    let Ok(bytes) = std::fs::read(order_path()) else { return Order::default() };
    // Older versions saved just the list of workspaces.
    serde_json::from_slice::<Order>(&bytes)
        .or_else(|_| serde_json::from_slice::<Vec<String>>(&bytes).map(|workspaces| Order { workspaces }))
        .unwrap_or_default()
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
            sidebar_followed: None,
            mouse: true,
            terms: HashMap::new(),
            attached: HashMap::new(),
            pending_select: None,
            view_sizes: Vec::new(),
            conv_height: 20,
            layouts: crate::layout::Layouts::default(),
            layouts_dirty: false,
            leaf_rects: Vec::new(),
            split_rects: Vec::new(),
            dragging: None,
            place_next: None,
            fresh_leaves: Vec::new(),
            shown_row: None,
            leaf_focused: false,
            mouse_pos: None,
            sidebar_compact: None,
            panel_shown: true,
            main_width: 0,
            hover_url: None,
            cut_rows: Vec::new(),
            compose: None,
            drafts: HashMap::new(),
            images: Vec::new(),
            slash: HashMap::new(),
            commit_edit: None,
            commit_drafts: HashMap::new(),
            sent_tx,
            sent_rx,
            worker: None,
            changes: HashMap::new(),
            changes_view: HashMap::new(),
            files: HashMap::new(),
            files_view: HashMap::new(),
            finder: None,
            welcome: None,
            project_order: Vec::new(),
            visits: crate::away::load(),
            away: None,
            folders: None,
            folders_requested: false,
            place_root: None,
            preselect: None,
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
            selection: None,
            picker: None,
            frame: None,
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
        // A picker is for one asking: once answered, the same question
        // asked again starts on its first row, unanswered.
        if self.picker.as_ref().is_some_and(|p| self.session(&p.session).is_none_or(|s| s.ask.is_none())) {
            self.picker = None;
        }
        self.reconcile_layouts();
        self.remember_order();
        if let Some((id, focus)) = self.pending_select.clone()
            && self.pane(&id).is_some()
        {
            self.selected = Some(RowRef::Pane(id));
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
            let urgent = self.agents().into_iter().min_by_key(|a| self.status(a));
            self.selected = project.or(urgent).or(rows.first().cloned());
        }
        if self.typing && self.focused_pane().and_then(|id| self.pane(&id).cloned()).is_none_or(|p| !p.is_running()) {
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

    /// The session whose question picker is on screen and can be answered
    /// from here: the focused conversation, or the selected row while
    /// that conversation shows.
    pub fn answer_target(&self) -> Option<String> {
        let shown = match self.focused_view() {
            Some(crate::layout::View::Conversation { session }) => session,
            _ => return None,
        };
        let waiting = self.session(&shown).is_some_and(|s| {
            matches!(s.state, SessionState::NeedsYou { .. }) && s.ask.is_some() && s.terminal.editor.is_some()
        });
        let here = match self.focus {
            Focus::Main => true,
            Focus::Sidebar => self.selected == Some(RowRef::Session(shown.clone())),
        };
        (waiting && here).then_some(shown)
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
                match self.pane_session(id).filter(|s| s.state.is_live() && s.title.is_some()) {
                    // A Claude pane is its session; "claude ·" says nothing.
                    Some(s) if p.kind == PaneKind::Claude => s.display_title(),
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
    /// Projects with something to show, in a steady order: the order they
    /// first appeared in (or were moved to with J and K). States change all
    /// the time; the list doesn't move with them; the dots and `n` say what
    /// needs you.
    pub fn projects(&self) -> Vec<RowRef> {
        let Some(snap) = &self.snap else { return vec![] };
        let mut visible: Vec<(usize, Ms, String)> = snap
            .projects
            .iter()
            .filter_map(|p| {
                let agents = self.agents_in(&p.root);
                if agents.is_empty() {
                    return None;
                }
                let rank = self.project_order.iter().position(|r| *r == p.root).unwrap_or(usize::MAX);
                let first = agents.iter().map(|a| self.started(a)).min().unwrap_or(0);
                Some((rank, first, p.root.clone()))
            })
            .collect();
        visible.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
        visible.into_iter().map(|(_, _, root)| RowRef::Project(root)).collect()
    }

    /// When an agent started, for keeping them in a steady order.
    pub fn started(&self, r: &RowRef) -> Ms {
        match r {
            RowRef::Pane(id) => self.pane(id).map(|p| p.created_at).unwrap_or(0),
            RowRef::Session(id) => self.session(id).map(|s| s.started_at).unwrap_or(0),
            RowRef::Project(_) => 0,
        }
    }

    /// When an agent last became active: started, got a prompt, or
    /// changed state (finished, needs you). Not every tool call, so two
    /// busy agents don't keep swapping places.
    pub fn active_at(&self, r: &RowRef) -> Ms {
        let of = |s: &Session| s.started_at.max(s.last_prompt_at.unwrap_or(0)).max(s.state_since);
        match r {
            RowRef::Session(id) => self.session(id).map(of).unwrap_or(0),
            RowRef::Pane(id) => {
                let created = self.pane(id).map(|p| p.created_at).unwrap_or(0);
                self.pane_session(id).map(of).unwrap_or(0).max(created)
            }
            RowRef::Project(_) => 0,
        }
    }

    /// A project's agents, the most recently active first. The projects
    /// themselves only move when you move them (J and K).
    pub fn agents_sorted(&self, root: &str) -> Vec<RowRef> {
        let mut a = self.agents_in(root);
        a.sort_by_key(|x| (std::cmp::Reverse(self.active_at(x)), self.started(x), self.title(x)));
        a
    }

    /// Remember the order of projects as they appear, so it holds across
    /// restarts. Returns true when a new one was added.
    pub fn remember_order(&mut self) -> bool {
        let shown: Vec<String> = self
            .projects()
            .into_iter()
            .filter_map(|r| match r {
                RowRef::Project(root) => Some(root),
                _ => None,
            })
            .collect();
        let mut added = false;
        for root in shown {
            if !self.project_order.contains(&root) {
                self.project_order.push(root);
                added = true;
            }
        }
        if added {
            self.save_order();
        }
        added
    }

    /// Move a project up or down the list (J and K).
    pub fn move_project(&mut self, root: &str, delta: isize) {
        self.remember_order();
        let shown: Vec<String> = self
            .projects()
            .into_iter()
            .filter_map(|r| match r {
                RowRef::Project(r) => Some(r),
                _ => None,
            })
            .collect();
        let Some(i) = shown.iter().position(|r| r == root) else { return };
        let j = (i as isize + delta).clamp(0, shown.len() as isize - 1) as usize;
        if i == j {
            return;
        }
        // Reorder the visible ones, keeping hidden ones where they were.
        let mut order = shown.clone();
        let item = order.remove(i);
        order.insert(j, item);
        let mut rest: Vec<String> = self.project_order.iter().filter(|r| !order.contains(r)).cloned().collect();
        order.append(&mut rest);
        self.project_order = order;
        self.save_order();
    }

    fn save_order(&self) {
        if self.conn == Conn::Demo {
            return;
        }
        let _ = aulora_core::paths::ensure_home();
        let order = Order { workspaces: self.project_order.clone() };
        let _ = std::fs::write(order_path(), serde_json::to_vec(&order).unwrap_or_default());
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
            // Live ones too, when aulora can reach them: through the
            // editor's extension, or their Claude process.
            RowRef::Session(id) => {
                self.session(id).is_some_and(|s| !s.state.is_live() || s.terminal.editor.is_some() || s.pid.is_some())
            }
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
                items.push(MenuItem::Run(crate::keymap::Action::SplitRight, "Split right  |"));
                items.push(MenuItem::Run(crate::keymap::Action::SplitDown, "Split down  -"));
                items.push(MenuItem::Run(crate::keymap::Action::NewClaude, "Claude beside it  C"));
                items.push(MenuItem::Run(crate::keymap::Action::Zoom, "Zoom  z"));
                items.push(MenuItem::Close(r.clone()));
            }
            RowRef::Session(id) => {
                if self.session(id).is_some_and(|s| s.state.is_live() && s.terminal.editor.is_some()) {
                    items.push(MenuItem::Jump(id.clone()));
                }
                items.push(MenuItem::Resume(id.clone()));
                if self.session(id).is_some_and(|s| s.state.is_live()) {
                    items.push(MenuItem::End(id.clone()));
                } else {
                    items.push(MenuItem::Close(r.clone()));
                }
            }
            RowRef::Project(_) => {}
        }
        if r.is_project() {
            items.push(MenuItem::Run(crate::keymap::Action::MoveUp, "Move up  K"));
            items.push(MenuItem::Run(crate::keymap::Action::MoveDown, "Move down  J"));
        }
        if let Some(root) = self.root_of(r) {
            items.push(MenuItem::NewClaude(root.clone()));
            items.push(MenuItem::NewShell(root.clone()));
            items.push(MenuItem::OpenEditor(root.clone()));
            let dir = match r {
                RowRef::Pane(id) => self.pane(id).map(|p| p.cwd.clone()),
                RowRef::Session(id) => self.session(id).map(|s| s.cwd.clone()),
                RowRef::Project(_) => None,
            };
            items.push(MenuItem::Reveal(dir.filter(|d| !d.is_empty()).unwrap_or(root)));
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

    /// Whether the view being drawn has the keyboard.
    pub fn here(&self) -> bool {
        self.focus == Focus::Main && self.leaf_focused
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
                Tab::Files => Scope::Files,
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
