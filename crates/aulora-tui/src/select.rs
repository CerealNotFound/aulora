//! Selecting text with the mouse, as in Claude Code: drag over any text
//! aulora draws, it highlights, and letting go copies it. aulora takes the
//! mouse, so the terminal's own selection can't work; this replaces it.
//! Double-click selects a word in a pane, a file or a diff.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;

/// A selection in progress or just made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    /// What it stays inside: a pane's screen, a view's inside, the sidebar.
    pub region: Rect,
    /// Where the drag started and where it is now (column, row).
    pub anchor: (u16, u16),
    pub head: (u16, u16),
    /// The pane under it, whose own screen knows which lines wrapped.
    pub pane: Option<String>,
}

impl Selection {
    pub fn new(region: Rect, at: (u16, u16), pane: Option<String>) -> Self {
        let at = clamp(region, at);
        Selection { region, anchor: at, head: at, pane }
    }

    /// Move the moving end, kept inside the region.
    pub fn extend(&mut self, to: (u16, u16)) {
        self.head = clamp(self.region, to);
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    /// The two ends in reading order.
    fn ends(&self) -> ((u16, u16), (u16, u16)) {
        let key = |p: &(u16, u16)| (p.1, p.0);
        if key(&self.anchor) <= key(&self.head) { (self.anchor, self.head) } else { (self.head, self.anchor) }
    }

    /// Whether a cell is selected: from the start to the end, line by line,
    /// as terminals select, within the region's columns.
    pub fn contains(&self, x: u16, y: u16) -> bool {
        let (a, b) = self.ends();
        let r = self.region;
        if y < a.1 || y > b.1 || x < r.x || x >= r.right() {
            return false;
        }
        let from = if y == a.1 { a.0 } else { r.x };
        let to = if y == b.1 { b.0 } else { r.right().saturating_sub(1) };
        x >= from && x <= to
    }

    /// Paint the selection over a drawn frame.
    pub fn paint(&self, buf: &mut Buffer) {
        let (a, b) = self.ends();
        for y in a.1..=b.1 {
            for x in self.region.x..self.region.right() {
                if self.contains(x, y)
                    && let Some(cell) = buf.cell_mut((x, y))
                {
                    cell.modifier.insert(Modifier::REVERSED);
                }
            }
        }
    }

    /// The selected text, read from the frame: each row trimmed at the
    /// end, rows joined by line breaks.
    pub fn text_from(&self, buf: &Buffer) -> String {
        let (a, b) = self.ends();
        let mut lines = Vec::new();
        for y in a.1..=b.1 {
            let mut line = String::new();
            let mut skip = 0;
            for x in self.region.x..self.region.right() {
                let Some(cell) = buf.cell((x, y)) else { continue };
                if skip > 0 {
                    skip -= 1;
                    continue;
                }
                let sym = cell.symbol();
                skip = unicode_width::UnicodeWidthStr::width(sym).saturating_sub(1);
                if self.contains(x, y) {
                    line.push_str(sym);
                }
            }
            lines.push(line.trim_end().to_string());
        }
        lines.join("\n")
    }

    /// The selected text, read from a pane's own screen, so a line that
    /// wrapped comes out as one line.
    pub fn text_from_screen(&self, screen: &vt100::Screen) -> String {
        let (a, b) = self.ends();
        let r = self.region;
        let text = screen.contents_between(a.1 - r.y, a.0 - r.x, b.1 - r.y, b.0 - r.x + 1);
        text.lines().map(str::trim_end).collect::<Vec<_>>().join("\n")
    }
}

fn clamp(r: Rect, (x, y): (u16, u16)) -> (u16, u16) {
    (x.clamp(r.x, r.right().saturating_sub(1)), y.clamp(r.y, r.bottom().saturating_sub(1)))
}

/// The word around a cell in a drawn frame, as a selection.
pub fn word_at(buf: &Buffer, region: Rect, at: (u16, u16), pane: Option<String>) -> Option<Selection> {
    let is_word = |x: u16| {
        buf.cell((x, at.1)).is_some_and(|c| {
            let s = c.symbol();
            !s.trim().is_empty() && !s.chars().all(|ch| "│─╭╮╰╯()[]{}<>\"'`,;".contains(ch))
        })
    };
    if !is_word(at.0) {
        return None;
    }
    let mut from = at.0;
    while from > region.x && is_word(from - 1) {
        from -= 1;
    }
    let mut to = at.0;
    while to + 1 < region.right() && is_word(to + 1) {
        to += 1;
    }
    let mut s = Selection::new(region, (from, at.1), pane);
    s.extend((to, at.1));
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::text::Line;
    use ratatui::widgets::{Paragraph, Widget};

    fn frame(lines: &[&str]) -> Buffer {
        let area = Rect::new(0, 0, 20, lines.len() as u16);
        let mut buf = Buffer::empty(area);
        Paragraph::new(lines.iter().map(|l| Line::raw(*l)).collect::<Vec<_>>()).render(area, &mut buf);
        buf
    }

    #[test]
    fn selects_like_a_terminal() {
        let buf = frame(&["│first line      │", "│second line     │", "│third           │"]);
        let region = Rect::new(1, 0, 16, 3);
        // From "line" on the first row to "third" on the last, backwards.
        let mut s = Selection::new(region, (5, 2), None);
        s.extend((7, 0));
        assert_eq!(s.text_from(&buf), "line\nsecond line\nthird");
        // Dragging past the region keeps to it: no borders in the copy.
        let mut s = Selection::new(region, (7, 0), None);
        s.extend((40, 2));
        assert_eq!(s.text_from(&buf), "line\nsecond line\nthird");
    }

    #[test]
    fn double_click_takes_the_word() {
        let buf = frame(&["see src/ui.rs:120 now"]);
        let s = word_at(&buf, Rect::new(0, 0, 20, 1), (6, 0), None).unwrap();
        assert_eq!(s.text_from(&buf), "src/ui.rs:120");
        assert!(word_at(&buf, Rect::new(0, 0, 20, 1), (3, 0), None).is_none(), "a space is no word");
    }

    #[test]
    fn a_pane_gives_wrapped_lines_back_whole() {
        let n = |v: u16| std::num::NonZeroU16::new(v).unwrap();
        let mut p = vt100::Parser::new(n(3), n(10), 0);
        p.process(b"abcdefghijklmn\r\nxy");
        let region = Rect::new(5, 5, 10, 3);
        let mut s = Selection::new(region, (5, 5), Some("p".into()));
        s.extend((6, 6 + 1));
        assert_eq!(s.text_from_screen(p.screen()), "abcdefghijklmn\nxy");
    }
}
