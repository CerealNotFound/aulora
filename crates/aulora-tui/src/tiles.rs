//! How the app uses each workspace's layout: the sidebar's selection
//! picks the main view, the Files and Changes panel comes and goes,
//! focus moves between them, and layouts are saved between runs.

use ratatui::layout::Rect;

use crate::app::{App, Focus, RowRef, Tab};
use crate::layout::{Layouts, Part, Space, View};

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

    pub fn space(&self) -> Option<&Space> {
        self.layouts.spaces.get(&self.space_root()?)
    }

    pub fn space_mut(&mut self) -> Option<&mut Space> {
        let root = self.space_root()?;
        self.layouts_dirty = true;
        Some(self.layouts.space(&root))
    }

    pub fn focused_view(&self) -> Option<View> {
        self.space()?.focused().cloned()
    }

    /// The pane in the focused view.
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

    /// A workspace showing a pane that's gone shows its overview; a pane
    /// whose shell moved to another workspace goes with it.
    pub fn reconcile_layouts(&mut self) {
        let Some(snap) = &self.snap else { return };
        let panes: Vec<(String, String)> = snap.panes.iter().map(|p| (p.id.clone(), p.project_root.clone())).collect();
        let now = self.now;
        // A pane just started may not be in the snapshot yet, so it's kept
        // for a while; once the daemon has listed it, it isn't fresh any
        // more, and it goes as soon as the pane does.
        self.fresh_panes.retain(|(id, at)| now - at < 5000 && !panes.iter().any(|(p, _)| p == id));
        let fresh = &self.fresh_panes;
        let alive = |id: &str| panes.iter().any(|(p, _)| p == id) || fresh.iter().any(|(p, _)| p == id);
        if self.layouts.prune(alive) {
            self.layouts_dirty = true;
        }
        for (id, root) in panes {
            let Some(at) = self.layouts.find_pane(&id).filter(|at| *at != root) else { continue };
            self.layouts.space(&at).main = View::Overview;
            self.layouts_dirty = true;
            // The selected one: the view goes with it, to its new workspace.
            if self.selected == Some(RowRef::Pane(id.clone())) {
                self.layouts.space(&root).show(View::Pane { id: id.clone() });
                let name = self.pane(&id).map(|p| p.name.clone()).unwrap_or_default();
                self.toast(format!("{name} moved to {}, where its shell is now", aulora_core::util::basename(&root)));
            }
        }
    }

    /// When the sidebar's selection changes, show it: a pane's terminal,
    /// or a session's conversation. A workspace shows what it last did.
    pub fn follow_selection(&mut self) {
        if self.selected == self.shown_row {
            return;
        }
        let before = self.shown_row.as_ref().and_then(|r| self.root_of(r));
        self.shown_row = self.selected.clone();
        // Coming to another project: maybe a "since you were here" card.
        if let Some(root) = self.space_root()
            && before.as_deref() != Some(root.as_str())
        {
            if let Some(prev) = before {
                self.visits.insert(prev, self.now);
            }
            self.visit(&root);
        }
        let view = match self.selected.clone() {
            Some(RowRef::Pane(id)) => View::Pane { id },
            Some(RowRef::Session(session)) => View::Conversation { session },
            _ => return,
        };
        if let Some(space) = self.space_mut() {
            space.show(view);
        }
    }

    /// The tab of what the main view shows: a conversation shown from its
    /// pane is that pane's tab.
    pub fn shown_tab(&self) -> Option<RowRef> {
        match self.space()?.main.clone() {
            View::Pane { id } => Some(RowRef::Pane(id)),
            View::Conversation { session } => {
                let pane = self
                    .session(&session)
                    .and_then(|s| s.terminal.pane_id.clone())
                    .filter(|p| self.pane(p).is_some())
                    .or_else(|| self.resumed_in(&session).map(|p| p.id.clone()));
                Some(pane.map_or(RowRef::Session(session), RowRef::Pane))
            }
            _ => None,
        }
    }

    /// Scroll the tabs without changing which one is shown (the wheel,
    /// `‹n` and `n›`). Drawing keeps it in range.
    pub fn scroll_tabs(&mut self, delta: isize) {
        let Some(root) = self.space_root() else { return };
        if self.tab_scroll.0 != root {
            self.tab_scroll = (root, 0);
        }
        self.tab_scroll.1 = (self.tab_scroll.1 as isize + delta).max(0) as usize;
    }

    /// `{` and `}`: the previous or next tab in this workspace.
    pub fn cycle_tab(&mut self, delta: i32) {
        let Some(root) = self.space_root() else { return };
        let tabs = self.tabs(&root);
        if tabs.is_empty() {
            return;
        }
        let n = tabs.len() as i32;
        let next = match self.shown_tab().and_then(|t| tabs.iter().position(|r| *r == t)) {
            Some(i) => (i as i32 + delta).rem_euclid(n),
            None if delta > 0 => 0,
            None => n - 1,
        };
        self.select(tabs[next as usize].clone());
    }

    /// Point the sidebar at what the main view shows.
    pub fn sync_selection(&mut self) {
        let row = match self.space().map(|s| s.main.clone()) {
            Some(View::Pane { id }) if self.pane(&id).is_some() => Some(RowRef::Pane(id)),
            Some(View::Conversation { session }) if self.session(&session).is_some() => Some(RowRef::Session(session)),
            _ => self.space_root().map(RowRef::Project),
        };
        if row.is_some() && row != self.selected {
            let typing = self.typing;
            self.select(row.clone().expect("checked"));
            self.typing = typing && self.focused_pane().is_some();
        }
        self.shown_row = self.selected.clone();
    }

    /// Show a view in a project: Changes and Files in the panel down the
    /// right edge, anything else in the main place.
    pub fn open_view(&mut self, root: &str, view: View, focus_main: bool) {
        self.layouts_dirty = true;
        self.layouts.space(root).show(view);
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
        // f shows or hides the panel everywhere; d switches it between
        // Changes and Files.
        let shown = self.layouts.space(&root).panel.clone();
        if matches!(kind, Tab::Files | Tab::Changes) {
            match (kind, &shown) {
                (Tab::Files, Some(View::Files)) => {
                    self.hide_panel();
                    return;
                }
                (Tab::Changes, Some(View::Changes { .. })) => {
                    self.open_view(&root, View::Files, true);
                    return;
                }
                _ => self.set_panel_shown(true),
            }
        }
        let new_panel = matches!(kind, Tab::Files | Tab::Changes) && shown.is_none();
        let view = match kind {
            // Every change; `a` narrows it to one agent's.
            Tab::Changes => View::Changes { filter: None },
            Tab::Files => View::Files,
            Tab::Overview => View::Overview,
            Tab::Conversation => match session {
                Some(s) => View::Conversation { session: s },
                None => return,
            },
            Tab::Terminal => match &self.selected {
                Some(RowRef::Pane(id)) => View::Pane { id: id.clone() },
                _ => return,
            },
        };
        self.open_view(&root, view, true);
        // The same size it comes up at by itself.
        let width = self.main_width;
        if new_panel && width > 0 {
            self.layouts.space(&root).ratio = panel_ratio(width);
        }
    }

    /// Hide the panel in every workspace, and remember that.
    pub fn hide_panel(&mut self) {
        for space in self.layouts.spaces.values_mut() {
            space.hide_panel();
        }
        self.layouts_dirty = true;
        self.set_panel_shown(false);
    }

    pub fn set_panel_shown(&mut self, shown: bool) {
        if self.panel_shown != shown {
            self.panel_shown = shown;
            if self.conn != crate::app::Conn::Demo {
                crate::save_config_bool("files_panel", shown);
            }
        }
    }

    /// Give the shown workspace its Files panel, unless hidden, too
    /// narrow, or the main view is only the overview. Focus stays where
    /// it was.
    pub fn ensure_panel(&mut self, width: u16) {
        self.main_width = width;
        if !self.panel_shown || width < 110 {
            return;
        }
        let Some(root) = self.space_root() else { return };
        let space = self.layouts.space(&root);
        if space.panel.is_some() || !(space.main.is_terminal() || matches!(space.main, View::Conversation { .. })) {
            return;
        }
        space.panel = Some(View::Files);
        space.ratio = panel_ratio(width);
        self.layouts_dirty = true;
    }

    /// Show a new pane in its workspace.
    pub fn place_pane(&mut self, root: &str, pane: &str) {
        self.layouts_dirty = true;
        self.fresh_panes.push((pane.to_string(), self.now));
        self.layouts.space(root).show(View::Pane { id: pane.to_string() });
    }

    /// Focus the main view or the panel.
    pub fn focus_part(&mut self, part: Part) {
        if let Some(space) = self.space_mut()
            && space.view(part).is_some()
        {
            space.focus = part;
        }
        self.focus = Focus::Main;
        self.sync_selection();
    }

    /// Tab and Shift-Tab: sidebar → main view → panel → sidebar.
    pub fn cycle_focus(&mut self, delta: i32) {
        let order: Vec<Part> = self.part_rects.iter().map(|(p, _)| *p).collect();
        let cur = match self.focus {
            Focus::Sidebar => None,
            Focus::Main => self.space().map(|s| s.focus),
        };
        let i = cur.and_then(|c| order.iter().position(|p| *p == c));
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
            Some(j) => self.focus_part(order[j as usize]),
            None => self.focus = Focus::Sidebar,
        }
    }

    /// Option-Left and Option-Right: sidebar ↔ main view ↔ panel.
    pub fn move_focus(&mut self, dx: i32) -> bool {
        self.typing = false;
        let here = match self.focus {
            Focus::Sidebar => None,
            Focus::Main => self.space().map(|s| s.focus),
        };
        match (here, dx > 0) {
            (None, true) => self.focus_part(Part::Main),
            (Some(Part::Main), true) if self.space().is_some_and(|s| s.panel.is_some()) => self.focus_part(Part::Panel),
            (Some(Part::Main), false) => self.focus = Focus::Sidebar,
            (Some(Part::Panel), false) => self.focus_part(Part::Main),
            _ => return false,
        }
        true
    }

    /// The size a new pane in the main view would get, to start it at
    /// about the right size.
    pub fn new_pane_size(&self) -> Option<(u16, u16)> {
        let (_, r): &(Part, Rect) = self.part_rects.iter().find(|(p, _)| *p == Part::Main)?;
        Some((r.width.saturating_sub(2), r.height.saturating_sub(2)))
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

/// Where the panel's edge goes: about 42 columns for the panel, more on
/// wide screens.
fn panel_ratio(width: u16) -> f32 {
    (1.0 - (42.0_f32.max(width as f32 * 0.26) / width as f32)).clamp(0.5, 0.8)
}
