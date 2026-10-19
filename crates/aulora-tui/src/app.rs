use aulora_core::model::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowRef {
    Project(String),
    Session(String),
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
    pub filter: String,
    pub filtering: bool,
    pub help: bool,
    pub toast: Option<(String, Ms)>,
    pub hooks_installed: bool,
    pub editor: String,
    /// First row drawn in the session list.
    pub scroll: usize,
}

impl App {
    pub fn new(conn: Conn, hooks_installed: bool, editor: String) -> Self {
        App {
            snap: None,
            conn,
            now: now_ms(),
            selected: None,
            show_ended: true,
            filter: String::new(),
            filtering: false,
            help: false,
            toast: None,
            hooks_installed,
            editor,
            scroll: 0,
        }
    }

    pub fn set_snapshot(&mut self, snap: Snapshot) {
        self.snap = Some(snap);
        let rows = self.rows();
        let valid = self.selected.as_ref().is_some_and(|s| rows.contains(s));
        if !valid {
            // Prefer whatever needs attention most.
            self.selected = rows.iter().find(|r| matches!(r, RowRef::Session(_))).cloned().or(rows.first().cloned());
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
            .filter(|s| self.show_ended || s.state.is_live())
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
                let rank = ss.iter().map(|s| s.state.rank()).min()?;
                let last = ss.iter().map(|s| s.last_event_at).max().unwrap_or(p.last_activity);
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
        let cur = match &self.selected {
            Some(RowRef::Session(id)) => targets.iter().position(|s| &s.id == id),
            _ => None,
        };
        let next = cur.map(|i| (i + 1) % targets.len()).unwrap_or(0);
        self.selected = Some(RowRef::Session(targets[next].id.clone()));
    }

    pub fn selected_root(&self) -> Option<String> {
        match &self.selected {
            Some(RowRef::Project(r)) => Some(r.clone()),
            Some(RowRef::Session(id)) => self.session(id).map(|s| s.project_root.clone()),
            None => None,
        }
    }

    pub fn toast(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), self.now));
    }

    pub fn counts(&self) -> Counts {
        let mut c = Counts::default();
        if let Some(snap) = &self.snap {
            for s in &snap.sessions {
                match s.state {
                    SessionState::NeedsYou { .. } => c.needs_you += 1,
                    SessionState::Errored { .. } => c.errored += 1,
                    SessionState::YourTurn => c.your_turn += 1,
                    SessionState::Working | SessionState::Compacting => c.working += 1,
                    SessionState::Idle => c.idle += 1,
                    SessionState::Ended { .. } => c.ended += 1,
                }
            }
            c.projects = snap.projects.len();
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
    pub projects: usize,
}

impl Counts {
    pub fn live(&self) -> usize {
        self.needs_you + self.errored + self.your_turn + self.working + self.idle
    }
}
