//! Tiled layouts, as in Herdr, zellij and tmux: each project has tabs, and
//! each tab is a tree of splits whose leaves are terminals or aulora's own
//! views (Changes, Files, a conversation). The daemon owns the terminals;
//! the dashboard owns how they're arranged, saved in ~/.aulora/layouts.json.

use std::collections::HashMap;

use ratatui::layout::Rect;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    Overview,
}

impl View {
    pub fn is_terminal(&self) -> bool {
        matches!(self, View::Pane { .. })
    }

    /// Changes and Files open in a panel down the right edge of a tab.
    pub fn is_panel(&self) -> bool {
        matches!(self, View::Changes { .. } | View::Files)
    }
}

/// Which way a split lays its two sides out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Dir {
    /// Side by side.
    Right,
    /// One above the other.
    Down,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "node", rename_all = "snake_case")]
pub enum Node {
    Leaf { id: u64, view: View },
    Split { id: u64, dir: Dir, ratio: f32, a: Box<Node>, b: Box<Node> },
}

impl Node {
    pub fn leaves(&self) -> Vec<(u64, &View)> {
        let mut out = Vec::new();
        self.walk(&mut |n| {
            if let Node::Leaf { id, view } = n {
                out.push((*id, view));
            }
        });
        out
    }

    fn walk<'a>(&'a self, f: &mut impl FnMut(&'a Node)) {
        f(self);
        if let Node::Split { a, b, .. } = self {
            a.walk(f);
            b.walk(f);
        }
    }

    fn leaf_mut(&mut self, leaf: u64) -> Option<&mut Node> {
        match self {
            Node::Leaf { id, .. } if *id == leaf => Some(self),
            Node::Leaf { .. } => None,
            Node::Split { a, b, .. } => a.leaf_mut(leaf).or_else(|| b.leaf_mut(leaf)),
        }
    }

    fn split_mut(&mut self, split: u64) -> Option<&mut Node> {
        match self {
            Node::Split { id, .. } if *id == split => Some(self),
            Node::Split { a, b, .. } => a.split_mut(split).or_else(|| b.split_mut(split)),
            Node::Leaf { .. } => None,
        }
    }

    /// Remove a leaf; its sibling takes the parent's place. None when the
    /// leaf was the whole tree.
    fn without(self, leaf: u64) -> Option<Node> {
        match self {
            Node::Leaf { id, .. } if id == leaf => None,
            Node::Split { id, dir, ratio, a, b } => match (a.without(leaf), b.without(leaf)) {
                (Some(a), Some(b)) => Some(Node::Split { id, dir, ratio, a: Box::new(a), b: Box::new(b) }),
                (Some(only), None) | (None, Some(only)) => Some(only),
                (None, None) => None,
            },
            leaf => Some(leaf),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tab {
    pub root: Node,
    pub focus: u64,
    #[serde(default)]
    pub zoom: bool,
}

impl Tab {
    pub fn leaves(&self) -> Vec<(u64, &View)> {
        self.root.leaves()
    }

    pub fn view(&self, leaf: u64) -> Option<&View> {
        self.leaves().into_iter().find(|(id, _)| *id == leaf).map(|(_, v)| v)
    }

    pub fn focused(&self) -> Option<&View> {
        self.view(self.focus)
    }

    /// Where a new pane goes: beside the focused leaf, unless that's the
    /// Files or Changes panel, whose narrow strip it would halve; then
    /// beside the main view next to it.
    pub fn split_target(&self) -> u64 {
        let leaves = self.leaves();
        match leaves.iter().find(|(id, _)| *id == self.focus) {
            Some((_, v)) if v.is_panel() => {
                leaves.iter().rev().find(|(_, v)| !v.is_panel()).map_or(self.focus, |(id, _)| *id)
            }
            _ => self.focus,
        }
    }

    /// The leaf a new pane should take over instead of splitting: the
    /// split target when it's only a view, like a conversation or the
    /// overview. Views are cheap to open again; terminals and the panel
    /// aren't covered.
    pub fn view_to_replace(&self) -> Option<u64> {
        let target = self.split_target();
        self.view(target).is_some_and(|v| !v.is_terminal() && !v.is_panel()).then_some(target)
    }

    pub fn find(&self, pred: impl Fn(&View) -> bool) -> Option<u64> {
        self.leaves().into_iter().find(|(_, v)| pred(v)).map(|(id, _)| id)
    }

    /// Put `view` beside `target`, in `dir`, and focus it.
    pub fn split(&mut self, target: u64, dir: Dir, view: View, leaf_id: u64, split_id: u64) {
        let Some(node) = self.root.leaf_mut(target) else { return };
        let old = std::mem::replace(node, Node::Leaf { id: 0, view: View::Overview });
        *node = Node::Split {
            id: split_id,
            dir,
            ratio: 0.5,
            a: Box::new(old),
            b: Box::new(Node::Leaf { id: leaf_id, view }),
        };
        self.focus = leaf_id;
    }

    /// Add a full-height panel down the right edge, and focus it.
    pub fn add_panel(&mut self, view: View, leaf_id: u64, split_id: u64) {
        let old = std::mem::replace(&mut self.root, Node::Leaf { id: 0, view: View::Overview });
        self.root = Node::Split {
            id: split_id,
            dir: Dir::Right,
            ratio: 0.58,
            a: Box::new(old),
            b: Box::new(Node::Leaf { id: leaf_id, view }),
        };
        self.focus = leaf_id;
        self.zoom = false;
    }

    /// Show something else in a leaf.
    pub fn replace(&mut self, leaf: u64, view: View) {
        if let Some(Node::Leaf { view: v, .. }) = self.root.leaf_mut(leaf) {
            *v = view;
        }
    }

    /// Remove a leaf. Returns false when it was the last one.
    pub fn close(&mut self, leaf: u64) -> bool {
        let order: Vec<u64> = self.leaves().iter().map(|(id, _)| *id).collect();
        let root = std::mem::replace(&mut self.root, Node::Leaf { id: 0, view: View::Overview });
        match root.without(leaf) {
            Some(rest) => {
                self.root = rest;
                if self.focus == leaf {
                    // Focus the leaf that was next to it in reading order.
                    let left: Vec<u64> = self.leaves().iter().map(|(id, _)| *id).collect();
                    let i = order.iter().position(|id| *id == leaf).unwrap_or(0);
                    self.focus = left.get(i.saturating_sub(1)).or(left.first()).copied().unwrap_or(0);
                }
                self.zoom = self.zoom && self.leaves().len() > 1;
                true
            }
            None => false,
        }
    }

    pub fn set_ratio(&mut self, split: u64, ratio: f32) {
        if let Some(Node::Split { ratio: r, .. }) = self.root.split_mut(split) {
            *r = ratio.clamp(0.1, 0.9);
        }
    }

    /// Grow or shrink the focused leaf along its nearest split in `dir`.
    pub fn nudge(&mut self, dir: Dir, delta: f32) -> bool {
        fn find(n: &mut Node, leaf: u64, dir: Dir, delta: f32) -> Option<bool> {
            let Node::Split { dir: d, ratio, a, b, .. } = n else {
                return matches!(n, Node::Leaf { id, .. } if *id == leaf).then_some(false);
            };
            let in_a = find(a, leaf, dir, delta);
            let in_b = if in_a.is_none() { find(b, leaf, dir, delta) } else { None };
            match (in_a, in_b) {
                (Some(true), _) | (_, Some(true)) => Some(true),
                (Some(false), _) if *d == dir => {
                    *ratio = (*ratio + delta).clamp(0.1, 0.9);
                    Some(true)
                }
                (_, Some(false)) if *d == dir => {
                    *ratio = (*ratio - delta).clamp(0.1, 0.9);
                    Some(true)
                }
                (None, None) => None,
                _ => Some(false),
            }
        }
        find(&mut self.root, self.focus, dir, delta) == Some(true)
    }

    /// Make every split even.
    pub fn equalize(&mut self) {
        fn go(n: &mut Node) {
            if let Node::Split { ratio, a, b, .. } = n {
                *ratio = 0.5;
                go(a);
                go(b);
            }
        }
        go(&mut self.root);
    }
}

/// Where everything goes on screen.
pub struct Placed {
    pub leaves: Vec<(u64, Rect)>,
    /// Each split's id, direction, the area it divides, and the line
    /// between its sides (for dragging).
    pub splits: Vec<(u64, Dir, Rect, Rect)>,
}

const MIN_W: u16 = 12;
const MIN_H: u16 = 4;

pub fn place(tab: &Tab, area: Rect) -> Placed {
    let mut p = Placed { leaves: Vec::new(), splits: Vec::new() };
    if tab.zoom {
        p.leaves.push((tab.focus, area));
        return p;
    }
    fn go(n: &Node, r: Rect, p: &mut Placed) {
        match n {
            Node::Leaf { id, .. } => p.leaves.push((*id, r)),
            Node::Split { id, dir, ratio, a, b } => {
                let (ra, rb, line) = match dir {
                    Dir::Right => {
                        let aw = ((r.width as f32 * ratio).round() as u16)
                            .clamp(MIN_W.min(r.width / 2), r.width.saturating_sub(MIN_W.min(r.width / 2)));
                        let ra = Rect { width: aw, ..r };
                        let rb = Rect { x: r.x + aw, width: r.width - aw, ..r };
                        (ra, rb, Rect { x: (r.x + aw).saturating_sub(1), width: 2, ..r })
                    }
                    Dir::Down => {
                        let ah = ((r.height as f32 * ratio).round() as u16)
                            .clamp(MIN_H.min(r.height / 2), r.height.saturating_sub(MIN_H.min(r.height / 2)));
                        let ra = Rect { height: ah, ..r };
                        let rb = Rect { y: r.y + ah, height: r.height - ah, ..r };
                        (ra, rb, Rect { y: (r.y + ah).saturating_sub(1), height: 2, ..r })
                    }
                };
                p.splits.push((*id, *dir, r, line));
                go(a, ra, p);
                go(b, rb, p);
            }
        }
    }
    go(&tab.root, area, &mut p);
    p
}

/// The leaf next to `from` in a direction: the one that touches its edge
/// and overlaps it most (zellij's focus movement).
pub fn neighbor(placed: &Placed, from: u64, dx: i32, dy: i32) -> Option<u64> {
    let (_, r) = placed.leaves.iter().find(|(id, _)| *id == from)?;
    let overlap = |a0: u16, a1: u16, b0: u16, b1: u16| a1.min(b1) as i32 - a0.max(b0) as i32;
    placed
        .leaves
        .iter()
        .filter(|(id, _)| *id != from)
        .filter_map(|(id, o)| {
            let touches = match (dx, dy) {
                (1, _) => o.x >= r.right().saturating_sub(1) && o.x <= r.right() + 1,
                (-1, _) => o.right() <= r.x + 1 && o.right() + 1 >= r.x,
                (_, 1) => o.y >= r.bottom().saturating_sub(1) && o.y <= r.bottom() + 1,
                _ => o.bottom() <= r.y + 1 && o.bottom() + 1 >= r.y,
            };
            let ov = if dx != 0 {
                overlap(r.y, r.bottom(), o.y, o.bottom())
            } else {
                overlap(r.x, r.right(), o.x, o.right())
            };
            // On a tie, the one nearer the top (or the left).
            let pos = if dx != 0 { o.y } else { o.x };
            (touches && ov > 0).then_some((ov, std::cmp::Reverse(pos), *id))
        })
        .max_by_key(|(ov, pos, _)| (*ov, *pos))
        .map(|(_, _, id)| id)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Space {
    pub tabs: Vec<Tab>,
    pub active: usize,
}

impl Space {
    pub fn tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active)
    }

    pub fn tab_mut(&mut self) -> Option<&mut Tab> {
        self.tabs.get_mut(self.active)
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Layouts {
    pub spaces: HashMap<String, Space>,
    #[serde(default)]
    next: u64,
}

impl Layouts {
    pub fn id(&mut self) -> u64 {
        self.next += 1;
        self.next
    }

    pub fn space(&mut self, root: &str) -> &mut Space {
        self.spaces.entry(root.to_string()).or_default()
    }

    /// A new tab showing `view`, made active.
    pub fn new_tab(&mut self, root: &str, view: View) -> u64 {
        let id = self.id();
        let space = self.space(root);
        space.tabs.push(Tab { root: Node::Leaf { id, view }, focus: id, zoom: false });
        space.active = space.tabs.len() - 1;
        id
    }

    /// Where a pane is shown: (project, tab, leaf).
    pub fn find_pane(&self, pane: &str) -> Option<(String, usize, u64)> {
        for (root, space) in &self.spaces {
            for (i, tab) in space.tabs.iter().enumerate() {
                if let Some(leaf) = tab.find(|v| matches!(v, View::Pane { id } if id == pane)) {
                    return Some((root.clone(), i, leaf));
                }
            }
        }
        None
    }

    /// Drop leaves for panes that are gone, and tabs left empty.
    pub fn prune(&mut self, alive: impl Fn(&str) -> bool) -> bool {
        let mut changed = false;
        for space in self.spaces.values_mut() {
            let mut i = 0;
            while i < space.tabs.len() {
                let dead: Vec<u64> = space.tabs[i]
                    .leaves()
                    .into_iter()
                    .filter(|(_, v)| matches!(v, View::Pane { id } if !alive(id)))
                    .map(|(id, _)| id)
                    .collect();
                let mut empty = false;
                for leaf in dead {
                    changed = true;
                    if !space.tabs[i].close(leaf) {
                        empty = true;
                        break;
                    }
                }
                if empty {
                    space.tabs.remove(i);
                } else {
                    i += 1;
                }
            }
            space.active = space.active.min(space.tabs.len().saturating_sub(1));
        }
        self.spaces.retain(|_, s| !s.tabs.is_empty());
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
    fn a_new_pane_never_halves_the_panel() {
        let mut l = Layouts::default();
        let a = l.new_tab("/p", View::Conversation { session: "s".into() });
        let (p, s1) = (l.id(), l.id());
        let tab = l.space("/p").tab_mut().unwrap();
        tab.add_panel(View::Files, p, s1);
        tab.focus = p;
        assert_eq!(tab.split_target(), a, "beside the main view instead");
        tab.focus = a;
        assert_eq!(tab.split_target(), a);
    }

    #[test]
    fn a_new_pane_takes_a_views_place_but_not_a_terminals() {
        let mut l = Layouts::default();
        let a = l.new_tab("/p", View::Conversation { session: "s".into() });
        let (p, s1) = (l.id(), l.id());
        let tab = l.space("/p").tab_mut().unwrap();
        tab.add_panel(View::Files, p, s1);
        assert_eq!(tab.view_to_replace(), Some(a), "from the panel, the view beside it");
        tab.replace(a, pane("a"));
        assert_eq!(tab.view_to_replace(), None);
    }

    #[test]
    fn split_close_and_place() {
        let mut l = Layouts::default();
        let a = l.new_tab("/p", pane("a"));
        let (b, s1) = (l.id(), l.id());
        let (c, s2) = (l.id(), l.id());
        let tab = l.space("/p").tab_mut().unwrap();
        tab.split(a, Dir::Right, pane("b"), b, s1);
        tab.split(b, Dir::Down, View::Files, c, s2);
        assert_eq!(tab.focus, c);
        let placed = place(tab, Rect::new(0, 0, 100, 40));
        let r = |id| placed.leaves.iter().find(|(l, _)| *l == id).unwrap().1;
        assert_eq!(r(a), Rect::new(0, 0, 50, 40));
        assert_eq!(r(b), Rect::new(50, 0, 50, 20));
        assert_eq!(r(c), Rect::new(50, 20, 50, 20));
        assert_eq!(neighbor(&placed, a, 1, 0), Some(b), "the bigger overlap wins a tie at the top");
        assert_eq!(neighbor(&placed, c, 0, -1), Some(b));
        assert_eq!(neighbor(&placed, c, -1, 0), Some(a));
        assert_eq!(neighbor(&placed, a, -1, 0), None);

        assert!(tab.nudge(Dir::Right, 0.1));
        // The focused leaf is on the right, so it grows and the left shrinks.
        assert_eq!(place(tab, Rect::new(0, 0, 100, 40)).leaves[0].1.width, 40);
        tab.equalize();

        assert!(tab.close(b));
        assert_eq!(tab.leaves().iter().map(|(id, _)| *id).collect::<Vec<_>>(), [a, c]);
        assert!(tab.close(c));
        assert!(!tab.close(a), "the last leaf");
    }

    #[test]
    fn prunes_gone_panes_and_finds_live_ones() {
        let mut l = Layouts::default();
        let a = l.new_tab("/p", pane("a"));
        let (b, s) = (l.id(), l.id());
        l.space("/p").tab_mut().unwrap().split(a, Dir::Right, pane("b"), b, s);
        l.new_tab("/p", pane("c"));
        assert_eq!(l.find_pane("b"), Some(("/p".into(), 0, b)));
        assert!(l.prune(|id| id == "b"));
        let space = l.space("/p");
        assert_eq!(space.tabs.len(), 1, "the tab with only c went away");
        assert_eq!(space.tabs[0].leaves().len(), 1);
        let json = serde_json::to_string(&l).unwrap();
        let back: Layouts = serde_json::from_str(&json).unwrap();
        assert_eq!(back.find_pane("b").map(|x| x.2), Some(b));
    }
}
