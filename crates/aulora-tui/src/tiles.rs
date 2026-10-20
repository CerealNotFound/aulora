//! How the app uses tiled layouts: which tab and leaf the sidebar's
//! selection shows, where new panes and views go, focus movement, and
//! saving layouts between runs.

use ratatui::layout::Rect;

use crate::app::{App, Focus, RowRef, Tab};
use crate::layout::{self, Dir, Layouts, View};

fn layouts_path() -> std::path::PathBuf {
    aulora_core::paths::home().join("layouts.json")
}

pub fn load() -> Layouts {
    std::fs::read_to_string(layouts_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

impl App {
    /// The project whose layout the main area shows.
    pub fn space_root(&self) -> Option<String> {
        self.selected_root()
    }

    pub fn tab(&self) -> Option<&layout::Tab> {
        self.layouts.spaces.get(&self.space_root()?)?.tab()
    }

    pub fn tab_mut(&mut self) -> Option<&mut layout::Tab> {
        let root = self.space_root()?;
        self.layouts_dirty = true;
        self.layouts.spaces.get_mut(&root)?.tab_mut()
    }

    pub fn focused_view(&self) -> Option<View> {
        self.tab()?.focused().cloned()
    }

    /// The pane in the focused leaf.
    pub fn focused_pane(&self) -> Option<String> {
        match self.focused_view()? {
            View::Pane { id } => Some(id),
            _ => None,
        }
    }

    /// What kind of view has focus, for keys and hints.
    pub fn current_tab(&self) -> Tab {
        match self.focused_view() {
            Some(View::Pane { .. }) => Tab::Terminal,
            Some(View::Changes { .. }) => Tab::Changes,
            Some(View::Files) => Tab::Files,
            Some(View::Conversation { .. }) => Tab::Conversation,
            _ => Tab::Overview,
        }
    }

    /// The session filter of the focused Changes view.
    pub fn focused_filter(&self) -> Option<String> {
        match self.focused_view()? {
            View::Changes { filter } => filter,
            _ => None,
        }
    }

    /// Give every live pane a place: a pane nobody placed (started from
    /// the command line, or before this layout existed) gets its own tab.
    /// Leaves for panes that are gone go away.
    pub fn reconcile_layouts(&mut self) {
        let Some(snap) = &self.snap else { return };
        let panes: Vec<(String, String)> = snap.panes.iter().map(|p| (p.id.clone(), p.project_root.clone())).collect();
        let now = self.now;
        let fresh = &self.fresh_leaves;
        let alive =
            |id: &str| panes.iter().any(|(p, _)| p == id) || fresh.iter().any(|(p, at)| p == id && now - at < 5000);
        if self.layouts.prune(alive) {
            self.layouts_dirty = true;
        }
        self.fresh_leaves.retain(|(_, at)| now - at < 5000);
        for (id, root) in panes {
            if self.layouts.find_pane(&id).is_none() {
                let active = self.layouts.spaces.get(&root).map(|s| s.active);
                self.layouts.new_tab(&root, View::Pane { id });
                // Don't pull the view away from what's being looked at.
                if let (Some(a), Some(space)) = (active, self.layouts.spaces.get_mut(&root)) {
                    space.active = a;
                }
                self.layouts_dirty = true;
            }
        }
    }

    /// Make sure the selected project has something to show.
    pub fn ensure_space(&mut self) {
        let Some(root) = self.space_root() else { return };
        if self.layouts.spaces.get(&root).is_none_or(|s| s.tabs.is_empty()) {
            let view = if self.project(&root).is_some_and(|p| p.git.is_some()) {
                View::Changes { filter: None }
            } else {
                View::Overview
            };
            self.layouts.new_tab(&root, view);
            self.layouts_dirty = true;
        }
    }

    /// When the sidebar's selection changes, show it: a pane's leaf, or a
    /// session's conversation.
    pub fn follow_selection(&mut self) {
        if self.selected == self.shown_row {
            return;
        }
        self.shown_row = self.selected.clone();
        self.ensure_space();
        match self.selected.clone() {
            Some(RowRef::Pane(id)) => {
                if let Some((root, tab, leaf)) = self.layouts.find_pane(&id) {
                    let space = self.layouts.space(&root);
                    space.active = tab;
                    space.tabs[tab].focus = leaf;
                    self.layouts_dirty = true;
                }
            }
            Some(RowRef::Session(id)) => {
                if let Some(root) = self.space_root() {
                    self.open_view(&root, View::Conversation { session: id }, false);
                }
            }
            _ => {}
        }
    }

    /// Point the sidebar at what the focused leaf shows.
    pub fn sync_selection(&mut self) {
        let row = match self.focused_view() {
            Some(View::Pane { id }) if self.pane(&id).is_some() => Some(RowRef::Pane(id)),
            Some(View::Conversation { session }) if self.session(&session).is_some() => {
                // A session shown from its pane stays that pane's row.
                Some(RowRef::Session(session))
            }
            _ => self.space_root().map(RowRef::Project),
        };
        if row.is_some() && row != self.selected {
            let typing = self.typing;
            self.select(row.clone().expect("checked"));
            self.typing = typing && self.focused_pane().is_some();
        }
        self.shown_row = self.selected.clone();
    }

    /// Show a view in a project: focus it if this tab has it, replace the
    /// focused leaf if that's a view, or open it beside a terminal (views
    /// never cover terminals).
    pub fn open_view(&mut self, root: &str, view: View, focus_main: bool) {
        self.layouts_dirty = true;
        let same_kind = |a: &View, b: &View| std::mem::discriminant(a) == std::mem::discriminant(b);
        let (leaf_id, split_id) = (self.layouts.id(), self.layouts.id());
        let space = self.layouts.space(root);
        match space.tab_mut() {
            None => {
                self.layouts.new_tab(root, view);
            }
            Some(tab) => {
                if let Some(leaf) = tab.find(|v| v == &view) {
                    tab.focus = leaf;
                } else if let Some(leaf) = tab.find(|v| same_kind(v, &view)) {
                    tab.replace(leaf, view);
                    tab.focus = leaf;
                } else if tab.focused().is_some_and(|v| !v.is_terminal()) {
                    let f = tab.focus;
                    tab.replace(f, view);
                } else {
                    let f = tab.focus;
                    tab.split(f, Dir::Right, view, leaf_id, split_id);
                }
            }
        }
        if focus_main {
            self.focus = Focus::Main;
            self.typing = false;
        }
    }

    /// Open a kind of view in the selected project (`d`, `f`, `--tab`,
    /// Ctrl-P): Changes filtered to the selected agent, Files, the overview,
    /// the selected session's conversation, or its terminal.
    pub fn set_tab(&mut self, kind: Tab) {
        let Some(root) = self.space_root() else { return };
        let session = self.selected_session_id().filter(|_| !matches!(self.selected, Some(RowRef::Project(_))));
        let view = match kind {
            Tab::Changes => View::Changes { filter: session },
            Tab::Files => View::Files,
            Tab::Overview => View::Overview,
            Tab::Conversation => match session {
                Some(s) => View::Conversation { session: s },
                None => return,
            },
            Tab::Terminal => {
                let pane = match &self.selected {
                    Some(RowRef::Pane(id)) => id.clone(),
                    _ => return,
                };
                if let Some((root, tab, leaf)) = self.layouts.find_pane(&pane) {
                    let space = self.layouts.space(&root);
                    space.active = tab;
                    space.tabs[tab].focus = leaf;
                }
                self.focus = Focus::Main;
                return;
            }
        };
        self.open_view(&root, view, true);
    }

    /// Put a new pane beside the focused leaf (or in a new tab).
    pub fn place_pane(&mut self, root: &str, pane: &str, dir: Option<Dir>) {
        self.layouts_dirty = true;
        self.fresh_leaves.push((pane.to_string(), self.now));
        let view = View::Pane { id: pane.to_string() };
        let (leaf_id, split_id) = (self.layouts.id(), self.layouts.id());
        let space = self.layouts.space(root);
        match (dir, space.tab_mut()) {
            (Some(dir), Some(tab)) => {
                let f = tab.focus;
                tab.split(f, dir, view, leaf_id, split_id);
            }
            _ => {
                self.layouts.new_tab(root, view);
            }
        }
    }

    /// Focus a leaf of the current tab.
    pub fn focus_leaf(&mut self, leaf: u64) {
        if let Some(tab) = self.tab_mut()
            && tab.focus != leaf
        {
            tab.focus = leaf;
        }
        self.focus = Focus::Main;
        self.sync_selection();
    }

    /// Tab and Shift-Tab: sidebar → each leaf in reading order → sidebar.
    pub fn cycle_focus(&mut self, delta: i32) {
        let order: Vec<u64> = self.leaf_rects.iter().map(|(id, _)| *id).collect();
        let cur = match self.focus {
            Focus::Sidebar => None,
            Focus::Main => self.tab().map(|t| t.focus),
        };
        let i = cur.and_then(|c| order.iter().position(|id| *id == c));
        let n = order.len() as i32;
        let next = match (i, delta > 0) {
            (None, true) => Some(0),
            (None, false) => Some(n - 1),
            (Some(i), _) => {
                let j = i as i32 + delta;
                (0..n).contains(&j).then_some(j)
            }
        };
        self.typing = false;
        match next.filter(|_| n > 0) {
            Some(j) => self.focus_leaf(order[j as usize]),
            None => self.focus = Focus::Sidebar,
        }
    }

    /// Alt+arrows: the leaf in that direction.
    pub fn move_focus(&mut self, dx: i32, dy: i32) -> bool {
        let Some(tab) = self.tab() else { return false };
        let placed = layout::Placed { leaves: self.leaf_rects.clone(), splits: vec![] };
        if self.focus == Focus::Sidebar {
            if dx > 0 {
                let first = self.leaf_rects.iter().min_by_key(|(_, r)| (r.x, r.y)).map(|(id, _)| *id);
                if let Some(id) = first {
                    self.focus_leaf(id);
                    return true;
                }
            }
            return false;
        }
        match layout::neighbor(&placed, tab.focus, dx, dy) {
            Some(id) => {
                let typing = self.typing;
                self.focus_leaf(id);
                // Keep typing if the new leaf is a terminal too (zellij).
                self.typing = typing && self.focused_pane().is_some();
                true
            }
            None if dx < 0 => {
                self.typing = false;
                self.focus = Focus::Sidebar;
                true
            }
            None => false,
        }
    }

    pub fn switch_tab(&mut self, i: usize) -> bool {
        let Some(root) = self.space_root() else { return false };
        let space = self.layouts.space(&root);
        if i >= space.tabs.len() {
            return false;
        }
        space.active = i;
        self.layouts_dirty = true;
        self.typing = false;
        self.sync_selection();
        true
    }

    pub fn cycle_tab(&mut self, delta: i32) {
        let Some(root) = self.space_root() else { return };
        let space = self.layouts.space(&root);
        let n = space.tabs.len() as i32;
        if n > 1 {
            let next = (space.active as i32 + delta).rem_euclid(n) as usize;
            self.switch_tab(next);
        }
    }

    /// Where a pane in the focused leaf's place would get its size, for
    /// starting a new pane at roughly the right size.
    pub fn new_pane_size(&self, dir: Option<Dir>) -> Option<(u16, u16)> {
        let tab = self.tab()?;
        let (_, r): &(u64, Rect) = self.leaf_rects.iter().find(|(id, _)| *id == tab.focus)?;
        let (w, h) = (r.width.saturating_sub(2), r.height.saturating_sub(2));
        Some(match dir {
            Some(Dir::Right) => (w / 2, h),
            Some(Dir::Down) => (w, h / 2),
            None => (w, h),
        })
    }

    pub fn save_layouts(&mut self) {
        if !self.layouts_dirty || self.conn == crate::app::Conn::Demo {
            return;
        }
        self.layouts_dirty = false;
        if let Ok(json) = serde_json::to_string(&self.layouts) {
            let _ = aulora_core::paths::ensure_home();
            let path = layouts_path();
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(tmp, path);
            }
        }
    }
}
