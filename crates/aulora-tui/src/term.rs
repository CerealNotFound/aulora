//! The dashboard's copy of a pane's screen, and how it is drawn.

use std::num::NonZeroU16;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

use crate::theme;

const SCROLLBACK: usize = 5_000;

fn nz(n: u16) -> NonZeroU16 {
    NonZeroU16::new(n.max(1)).expect("non-zero")
}

pub struct Term {
    pub parser: vt100::Parser,
    /// Pane generation this screen belongs to.
    pub generation: u32,
    pub exited: bool,
}

impl Term {
    pub fn new(cols: u16, rows: u16, generation: u32) -> Self {
        Term { parser: vt100::Parser::new(nz(rows), nz(cols), SCROLLBACK), generation, exited: false }
    }

    /// Replace the screen with a fresh redraw from the daemon.
    pub fn reset(&mut self, cols: u16, rows: u16, data: &[u8]) {
        self.parser = vt100::Parser::new(nz(rows), nz(cols), SCROLLBACK);
        self.parser.process(data);
        self.exited = false;
    }

    pub fn feed(&mut self, data: &[u8]) {
        self.parser.process(data);
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        let (r, c) = self.parser.screen().size();
        if (c.get(), r.get()) != (cols, rows) {
            self.parser.set_size(nz(rows), nz(cols));
        }
    }

    pub fn scroll_by(&mut self, delta: i32) {
        let screen = self.parser.screen_mut();
        let next = (screen.scrollback() as i64 + delta as i64).max(0) as usize;
        screen.set_scrollback(next);
    }

    /// Lines of scrollback kept above the screen.
    pub fn history(&self) -> usize {
        self.parser.screen().scrollback_len()
    }

    pub fn scrolled(&self) -> usize {
        self.parser.screen().scrollback()
    }

    pub fn reset_scroll(&mut self) {
        self.parser.screen_mut().set_scrollback(0);
    }
}

fn color(c: vt100::Color, default: Color) -> Color {
    match c {
        vt100::Color::Default => default,
        vt100::Color::Idx(i) => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

/// Draw the screen into `area`. Returns the cursor position when it should
/// be shown.
pub fn render(term: &Term, area: Rect, buf: &mut Buffer) -> Option<(u16, u16)> {
    let screen = term.parser.screen();
    let (rows, cols) = screen.size();
    let (rows, cols) = (rows.get().min(area.height), cols.get().min(area.width));
    for row in 0..rows {
        let mut col = 0;
        while col < cols {
            let Some(cell) = screen.cell(row, col) else {
                col += 1;
                continue;
            };
            if cell.is_wide_continuation() {
                col += 1;
                continue;
            }
            let mut fg = color(cell.fgcolor(), theme::get().text);
            let mut bg = color(cell.bgcolor(), theme::get().bg);
            if cell.inverse() {
                std::mem::swap(&mut fg, &mut bg);
            }
            let mut style = Style::new().fg(fg).bg(bg);
            if cell.bold() {
                style = style.add_modifier(Modifier::BOLD);
            }
            if cell.dim() {
                style = style.add_modifier(Modifier::DIM);
            }
            if cell.italic() {
                style = style.add_modifier(Modifier::ITALIC);
            }
            if cell.underline() {
                style = style.add_modifier(Modifier::UNDERLINED);
            }
            let contents = cell.contents();
            let symbol = if contents.is_empty() { " " } else { contents };
            let x = area.x + col;
            let y = area.y + row;
            if let Some(c) = buf.cell_mut((x, y)) {
                c.set_symbol(symbol).set_style(style);
            }
            // A wide character covers the next column too.
            if cell.is_wide() && col + 1 < cols {
                if let Some(c) = buf.cell_mut((x + 1, y)) {
                    c.set_symbol("").set_style(style);
                }
                col += 2;
            } else {
                col += 1;
            }
        }
    }
    if screen.hide_cursor() || screen.scrollback() > 0 {
        return None;
    }
    let (cy, cx) = screen.cursor_position();
    (cy < rows && cx < cols).then_some((area.x + cx, area.y + cy))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_text_colors_and_wide_chars() {
        let mut t = Term::new(10, 2, 0);
        t.feed("\x1b[31mhi\x1b[0m 日本".as_bytes());
        let area = Rect::new(0, 0, 10, 2);
        let mut buf = Buffer::empty(area);
        let cursor = render(&t, area, &mut buf);
        assert_eq!(buf[(0, 0)].symbol(), "h");
        assert_eq!(buf[(0, 0)].fg, Color::Indexed(1));
        assert_eq!(buf[(3, 0)].symbol(), "日");
        assert_eq!(buf[(5, 0)].symbol(), "本");
        assert_eq!(cursor, Some((7, 0)));
    }

    #[test]
    fn scrollback() {
        let mut t = Term::new(10, 2, 0);
        t.feed(b"1\r\n2\r\n3\r\n4");
        assert_eq!(t.parser.screen().contents(), "3\n4");
        t.scroll_by(2);
        assert_eq!(t.scrolled(), 2);
        assert_eq!(t.parser.screen().contents(), "1\n2");
        t.reset_scroll();
        assert_eq!(t.scrolled(), 0);
    }
}
