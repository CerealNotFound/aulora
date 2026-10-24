//! What the main area shows for each workspace: one view, the one the
//! sidebar selected (a terminal, a session's conversation, the overview),
//! and maybe a panel down the right edge with Changes or Files. The
//! daemon owns the terminals; the dashboard remembers each workspace's
//! view and panel in ~/.aulora/layouts.json.

use std::collections::HashMap;

use ratatui::layout::Rect;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum View {
    Pane {
        id: String,
    },
    /// Changes, optionally only one session's.
    Changes {
        filter: Option<String>,
    },
    Files,
    Conversation {
        session: String,
    },
    #[default]
    Overview,
}

impl View {
    pub fn is_terminal(&self) -> bool {
        matches!(self, View::Pane { .. })
    }

    /// Changes and Files open in the panel down the right edge.
    pub fn is_panel(&self) -> bool {
        matches!(self, View::Changes { .. } | View::Files)
    }
}

/// The two places in the main area.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Part {
    #[default]
    Main,
    Panel,
}

/// One workspace's main area.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Space {
    pub main: View,
    /// Changes or Files, down the right edge.
    pub panel: Option<View>,
    /// The main view's share of the width while the panel shows.
    pub ratio: f32,
    pub focus: Part,
}

impl Default for Space {
    fn default() -> Self {
        Space { main: View::Overview, panel: None, ratio: 0.58, focus: Part::Main }
    }
}

impl Space {
    pub fn view(&self, part: Part) -> Option<&View> {
        match part {
            Part::Main => Some(&self.main),
            Part::Panel => self.panel.as_ref(),
        }
    }

    pub fn focused(&self) -> Option<&View> {
        self.view(self.focus)
    }

    /// Show `view`: Changes and Files in the panel, anything else in the
    /// main place. It gets the focus.
    pub fn show(&mut self, view: View) {
        if view.is_panel() {
            self.panel = Some(view);
            self.focus = Part::Panel;
        } else {
            self.main = view;
            self.focus = Part::Main;
        }
    }

    pub fn hide_panel(&mut self) {
        self.panel = None;
        self.focus = Part::Main;
    }

    pub fn set_ratio(&mut self, ratio: f32) {
        self.ratio = ratio.clamp(0.1, 0.9);
    }
}

/// Where everything goes on screen.
pub struct Placed {
    pub parts: Vec<(Part, Rect)>,
    /// The line between the main view and the panel, for dragging.
    pub edge: Option<Rect>,
}

const MIN_W: u16 = 12;

pub fn place(space: &Space, area: Rect) -> Placed {
    if space.panel.is_none() {
        return Placed { parts: vec![(Part::Main, area)], edge: None };
    }
    let min = MIN_W.min(area.width / 2);
    let aw = ((area.width as f32 * space.ratio).round() as u16).clamp(min, area.width.saturating_sub(min));
    let main = Rect { width: aw, ..area };
    let panel = Rect { x: area.x + aw, width: area.width - aw, ..area };
    let edge = Rect { x: (area.x + aw).saturating_sub(1), width: 2, ..area };
    Placed { parts: vec![(Part::Main, main), (Part::Panel, panel)], edge: Some(edge) }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Layouts {
    #[serde(default)]
    pub spaces: HashMap<String, Space>,
}

impl Layouts {
    pub fn space(&mut self, root: &str) -> &mut Space {
        self.spaces.entry(root.to_string()).or_default()
    }

    /// The workspace showing a pane.
    pub fn find_pane(&self, pane: &str) -> Option<String> {
        self.spaces.iter().find(|(_, s)| matches!(&s.main, View::Pane { id } if id == pane)).map(|(r, _)| r.clone())
    }

    /// A workspace showing a pane that's gone shows its overview instead.
    pub fn prune(&mut self, alive: impl Fn(&str) -> bool) -> bool {
        let mut changed = false;
        for space in self.spaces.values_mut() {
            if matches!(&space.main, View::Pane { id } if !alive(id)) {
                space.main = View::Overview;
                space.focus = Part::Main;
                changed = true;
            }
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(id: &str) -> View {
        View::Pane { id: id.into() }
    }

    #[test]
    fn panels_go_on_the_right_and_everything_else_in_the_middle() {
        let mut s = Space::default();
        s.show(pane("a"));
        s.show(View::Files);
        assert_eq!((&s.main, s.panel.as_ref(), s.focus), (&pane("a"), Some(&View::Files), Part::Panel));
        s.show(View::Conversation { session: "x".into() });
        assert_eq!(s.panel, Some(View::Files), "the panel stays");
        assert_eq!(s.focus, Part::Main);
        s.hide_panel();
        assert_eq!(s.panel, None);
    }

    #[test]
    fn place_splits_only_for_the_panel() {
        let mut s = Space::default();
        let area = Rect::new(0, 0, 100, 40);
        let p = place(&s, area);
        assert_eq!(p.parts, [(Part::Main, area)]);
        assert!(p.edge.is_none());
        s.panel = Some(View::Files);
        s.ratio = 0.6;
        let p = place(&s, area);
        assert_eq!(p.parts, [(Part::Main, Rect::new(0, 0, 60, 40)), (Part::Panel, Rect::new(60, 0, 40, 40))]);
        assert_eq!(p.edge, Some(Rect::new(59, 0, 2, 40)));
    }

    #[test]
    fn prunes_gone_panes_and_finds_live_ones() {
        let mut l = Layouts::default();
        l.space("/p").show(pane("a"));
        l.space("/q").show(pane("b"));
        assert_eq!(l.find_pane("b"), Some("/q".into()));
        assert!(l.prune(|id| id == "b"));
        assert_eq!(l.space("/p").main, View::Overview);
        assert!(!l.prune(|id| id == "b"), "nothing left to drop");
        let json = serde_json::to_string(&l).unwrap();
        let back: Layouts = serde_json::from_str(&json).unwrap();
        assert_eq!(back.find_pane("b"), Some("/q".into()));
    }

    #[test]
    fn layouts_saved_with_tabs_still_load() {
        let old = r#"{"spaces":{"/p":{"tabs":[{"root":{"node":"leaf","id":1,"view":{"kind":"files"}},"focus":1}],"active":0}},"next":3}"#;
        let l: Layouts = serde_json::from_str(old).unwrap();
        assert_eq!(l.spaces.get("/p"), Some(&Space::default()));
    }
}
