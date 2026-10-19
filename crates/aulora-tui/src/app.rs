use std::collections::HashMap;

use aulora_core::model::*;

use crate::term::Term;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowRef {
    Project(String),
    Session(String),
    Pane(String),
}

/// Where key presses go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Keys drive the dashboard.
    Navigate,
    /// Keys go to the selected pane; Ctrl-Q returns to Navigate.
    Terminal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Conn {
    Connecting,
    Live,
    Offline(String),
    Demo,
}

pub struct App {
    pub snap: Option<Snapshot>,
    pub conn: Conn,
    pub now: Ms,
    pub selected: Option<RowRef>,
    pub show_ended: bool,
    /// Show past sessions found in transcripts.
    pub show_history: bool,
    pub filter: String,
    pub filtering: bool,
    pub help: bool,
    pub toast: Option<(String, Ms)>,
    pub hooks_installed: bool,
    pub editor: String,
    /// First row drawn in the session list.
    pub scroll: usize,
    pub mode: Mode,
    /// Hide the sidebar and give the pane the whole screen.
    pub zoom: bool,
    /// Local screens of attached panes.
    pub terms: HashMap<String, Term>,
    /// Pane (and generation) this client is attached to.
    pub attached: Option<(String, u32)>,
    /// Select (and focus) this pane as soon as it appears.
    pub pending_select: Option<(String, bool)>,
    /// Size of the pane view in the last frame.
    pub view_size: Option<(String, u16, u16)>,
    /// A destructive action waiting for a second key press.
    pub confirm: Option<(char, String, Ms)>,
}

impl App {
    pub fn new(conn: Conn, hooks_installed: bool, editor: String) -> Self {
        App {
            snap: None,
            conn,
            now: now_ms(),
            selected: None,
            show_ended: true,
            show_history: false,
            filter: String::new(),
            filtering: false,
            help: false,
            toast: None,
            hooks_installed,
            editor,
            scroll: 0,
            mode: Mode::Navigate,
            zoom: false,
            terms: HashMap::new(),
            attached: None,
            pending_select: None,
            view_size: None,
            confirm: None,
        }
    }

    pub fn set_snapshot(&mut self, snap: Snapshot) {
        self.snap = Some(snap);
        if let Some((id, focus)) = self.pending_select.clone()
            && self.pane(&id).is_some()
        {
            self.selected = Some(RowRef::Pane(id));
            if focus {
                self.mode = Mode::Terminal;
            }
            self.pending_select = None;
        }
        let rows = self.rows();
        let valid = self.selected.as_ref().is_some_and(|s| rows.contains(s));
        if !valid {
            // Prefer whatever needs attention most.
            self.selected = rows.iter().find(|r| !matches!(r, RowRef::Project(_))).cloned().or(rows.first().cloned());
        }
        if self.mode == Mode::Terminal && self.selected_pane().is_none() {
            self.mode = Mode::Navigate;
        }
        // Forget screens of panes that are gone.
        let live: std::collections::HashSet<String> =
            self.snap.as_ref().map(|s| s.panes.iter().map(|p| p.id.clone()).collect()).unwrap_or_default();
        self.terms.retain(|id, _| live.contains(id));
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

    /// The Claude session running in a pane, if any: live ones first, then the latest.
    pub fn pane_session(&self, pane_id: &str) -> Option<&Session> {
        self.snap
            .as_ref()?
            .sessions
            .iter()
            .filter(|s| s.terminal.pane_id.as_deref() == Some(pane_id))
            .max_by_key(|s| (s.state.is_live(), s.last_event_at))
    }

    fn in_existing_pane(&self, s: &Session) -> bool {
        s.terminal.pane_id.as_deref().is_some_and(|id| self.pane(id).is_some())
    }

    fn pane_matches_filter(&self, p: &Pane) -> bool {
        if self.filter.is_empty() {
            return true;
        }
        let f = self.filter.to_lowercase();
        let title = self.pane_session(&p.id).map(|s| s.display_title()).unwrap_or_default();
        format!(
            "{} {} {} {}",
            aulora_core::util::basename(&p.project_root),
            p.name,
            title,
            p.title.as_deref().unwrap_or("")
        )
        .to_lowercase()
        .contains(&f)
    }

    /// Panes in a project, oldest first.
    pub fn visible_panes(&self, root: &str) -> Vec<&Pane> {
        let Some(snap) = &self.snap else { return vec![] };
        snap.panes.iter().filter(|p| p.project_root == root).filter(|p| self.pane_matches_filter(p)).collect()
    }

    /// Urgency of a pane for sorting projects.
    pub fn pane_rank(&self, p: &Pane) -> u8 {
        match self.pane_session(&p.id) {
            Some(s) if s.state.is_live() => s.state.rank(),
            _ if p.is_running() => 5,
            _ => 8,
        }
    }

    pub fn session(&self, id: &str) -> Option<&Session> {
        self.snap.as_ref()?.sessions.iter().find(|s| s.id == id)
    }

    pub fn project(&self, root: &str) -> Option<&Project> {
        self.snap.as_ref()?.projects.iter().find(|p| p.root == root)
    }

    fn matches_filter(&self, s: &Session) -> bool {
        if self.filter.is_empty() {
            return true;
        }
        let f = self.filter.to_lowercase();
        let hay = format!(
            "{} {} {} {}",
            aulora_core::util::basename(&s.project_root),
            s.display_title(),
            s.last_prompt.as_deref().unwrap_or(""),
            s.git_branch.as_deref().unwrap_or("")
        )
        .to_lowercase();
        hay.contains(&f)
    }

    /// Visible sessions, sorted by urgency then recency.
    pub fn visible_sessions(&self, root: &str) -> Vec<&Session> {
        let Some(snap) = &self.snap else { return vec![] };
        let mut v: Vec<&Session> = snap
            .sessions
            .iter()
            .filter(|s| s.project_root == root)
            .filter(|s| !self.in_existing_pane(s))
            .filter(|s| self.show_ended || s.state.is_live())
            .filter(|s| self.show_history || s.origin != Origin::History)
            .filter(|s| self.matches_filter(s))
            .collect();
        v.sort_by_key(|s| (s.state.rank(), -s.last_event_at));
        v
    }

    /// Projects that have visible sessions, most urgent first.
    pub fn visible_projects(&self) -> Vec<&Project> {
        let Some(snap) = &self.snap else { return vec![] };
        let mut v: Vec<(&Project, u8, Ms)> = snap
            .projects
            .iter()
            .filter_map(|p| {
                let ss = self.visible_sessions(&p.root);
                let ps = self.visible_panes(&p.root);
                let rank = ss.iter().map(|s| s.state.rank()).chain(ps.iter().map(|x| self.pane_rank(x))).min()?;
                let last = ss
                    .iter()
                    .map(|s| s.last_event_at)
                    .chain(ps.iter().map(|x| x.created_at))
                    .max()
                    .unwrap_or(p.last_activity);
                Some((p, rank, last))
            })
            .collect();
        v.sort_by_key(|(p, rank, last)| (*rank, -*last, p.name.clone()));
        v.into_iter().map(|(p, _, _)| p).collect()
    }

    pub fn rows(&self) -> Vec<RowRef> {
        let mut rows = Vec::new();
        for p in self.visible_projects() {
            rows.push(RowRef::Project(p.root.clone()));
            for x in self.visible_panes(&p.root) {
                rows.push(RowRef::Pane(x.id.clone()));
            }
            for s in self.visible_sessions(&p.root) {
                rows.push(RowRef::Session(s.id.clone()));
            }
        }
        rows
    }

    pub fn move_selection(&mut self, delta: i32) {
        let rows = self.rows();
        if rows.is_empty() {
            self.selected = None;
            return;
        }
        let cur = self.selected.as_ref().and_then(|s| rows.iter().position(|r| r == s)).unwrap_or(0) as i32;
        let next = (cur + delta).clamp(0, rows.len() as i32 - 1) as usize;
        self.selected = Some(rows[next].clone());
    }

    pub fn select_edge(&mut self, last: bool) {
        let rows = self.rows();
        self.selected = if last { rows.last().cloned() } else { rows.first().cloned() };
    }

    /// Jump between projects.
    pub fn move_project(&mut self, delta: i32) {
        let projects: Vec<String> = self.visible_projects().iter().map(|p| p.root.clone()).collect();
        if projects.is_empty() {
            return;
        }
        let cur_root = self.selected_root();
        let cur = cur_root.and_then(|r| projects.iter().position(|p| *p == r)).unwrap_or(0) as i32;
        let next = (cur + delta).rem_euclid(projects.len() as i32) as usize;
        self.selected = Some(RowRef::Project(projects[next].clone()));
    }

    /// Sessions that need you, most urgent first.
    pub fn alerts(&self) -> Vec<&Session> {
        let Some(snap) = &self.snap else { return vec![] };
        let mut v: Vec<&Session> =
            snap.sessions.iter().filter(|s| matches!(s.state, SessionState::NeedsYou { .. })).collect();
        v.sort_by_key(|s| s.state_since);
        v
    }

    /// Cycle through sessions that need you, then ones whose turn is done.
    pub fn next_attention(&mut self) {
        let Some(snap) = &self.snap else { return };
        let mut targets: Vec<&Session> = snap
            .sessions
            .iter()
            .filter(|s| {
                matches!(s.state, SessionState::NeedsYou { .. } | SessionState::Errored { .. } | SessionState::YourTurn)
            })
            .collect();
        targets.sort_by_key(|s| (s.state.rank(), s.state_since));
        if targets.is_empty() {
            self.toast("Nothing needs you right now");
            return;
        }
        let row_of = |s: &Session| match s.terminal.pane_id.as_deref().filter(|id| self.pane(id).is_some()) {
            Some(pane) => RowRef::Pane(pane.to_string()),
            None => RowRef::Session(s.id.clone()),
        };
        let rows: Vec<RowRef> = targets.iter().map(|s| row_of(s)).collect();
        let cur = self.selected.as_ref().and_then(|sel| rows.iter().position(|r| r == sel));
        let next = cur.map(|i| (i + 1) % rows.len()).unwrap_or(0);
        self.selected = Some(rows[next].clone());
    }

    /// Ask for the second key press of a destructive action.
    /// Returns true when this press confirms an earlier one.
    pub fn confirmed(&mut self, key: char, target: &str, prompt: &str) -> bool {
        if let Some((k, t, at)) = &self.confirm
            && *k == key
            && t == target
            && self.now - at < 2_500
        {
            self.confirm = None;
            return true;
        }
        self.confirm = Some((key, target.to_string(), self.now));
        self.toast(prompt.to_string());
        false
    }

    pub fn selected_root(&self) -> Option<String> {
        match &self.selected {
            Some(RowRef::Project(r)) => Some(r.clone()),
            Some(RowRef::Session(id)) => self.session(id).map(|s| s.project_root.clone()),
            Some(RowRef::Pane(id)) => self.pane(id).map(|p| p.project_root.clone()),
            None => None,
        }
    }

    pub fn toast(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), self.now));
    }

    pub fn counts(&self) -> Counts {
        let mut c = Counts::default();
        if let Some(snap) = &self.snap {
            for s in snap.sessions.iter().filter(|s| s.origin != Origin::History) {
                match s.state {
                    SessionState::NeedsYou { .. } => c.needs_you += 1,
                    SessionState::Errored { .. } => c.errored += 1,
                    SessionState::YourTurn => c.your_turn += 1,
                    SessionState::Working | SessionState::Compacting => c.working += 1,
                    SessionState::Idle => c.idle += 1,
                    SessionState::Ended { .. } => c.ended += 1,
                }
            }
            c.history = snap.sessions.iter().filter(|s| s.origin == Origin::History).count();
            c.projects = self.visible_projects().len();
        }
        c
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Counts {
    pub needs_you: usize,
    pub errored: usize,
    pub your_turn: usize,
    pub working: usize,
    pub idle: usize,
    pub ended: usize,
    pub history: usize,
    pub projects: usize,
}

impl Counts {
    pub fn live(&self) -> usize {
        self.needs_you + self.errored + self.your_turn + self.working + self.idle
    }
}
