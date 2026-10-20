//! Links on screen: the URL under the pointer is underlined, and a click
//! on it opens it in the browser. Cmd-click works too: terminals don't tell
//! programs about Cmd, so the click arrives as a click. Links are found in
//! the frame as drawn, so terminal output, conversations and files all
//! work the same.

use ratatui::buffer::Buffer;
use ratatui::style::Modifier;

/// A link on screen: the URL and the cells it covers, one span per row
/// (row, first column, last column + 1). A URL that wrapped covers several.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub url: String,
    pub spans: Vec<(u16, u16, u16)>,
}

impl Link {
    pub fn covers(&self, col: u16, row: u16) -> bool {
        self.spans.iter().any(|&(r, x0, x1)| r == row && col >= x0 && col < x1)
    }
}

/// The URL covering the cell at index `at` of a line of text, given as
/// (column, row, char) in reading order: (first index, last index + 1, url).
pub fn url_at(cells: &[(u16, u16, char)], at: usize) -> Option<(usize, usize, String)> {
    let chars: Vec<char> = cells.iter().map(|c| c.2).collect();
    let mut i = 0;
    while i < chars.len() {
        let rest: String = chars[i..].iter().take(8).collect();
        if !(rest.starts_with("https://") || rest.starts_with("http://")) {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < chars.len() && !chars[j].is_whitespace() && !"│╭╮╰╯─▕▐\"'<>`".contains(chars[j]) {
            j += 1;
        }
        // Cut short to fit (…): opening it would open the wrong page.
        if chars[i..j].contains(&'…') {
            i = j;
            continue;
        }
        // Trailing punctuation belongs to the sentence, and a closing
        // bracket only to the URL if it opened one.
        while j > i {
            let c = chars[j - 1];
            let opens = |o: char| chars[i..j].iter().filter(|&&x| x == o).count();
            let closes = |cl: char| chars[i..j].iter().filter(|&&x| x == cl).count();
            let unbalanced = match c {
                ')' => closes(')') > opens('('),
                ']' => closes(']') > opens('['),
                '}' => closes('}') > opens('{'),
                _ => false,
            };
            if ".,;:!?".contains(c) || unbalanced {
                j -= 1;
            } else {
                break;
            }
        }
        if at >= i && at < j && j - i > 8 {
            return Some((i, j, chars[i..j].iter().collect()));
        }
        i = j.max(i + 1);
    }
    None
}

/// The cells of some rows of the drawn frame, between two columns, as
/// (column, row, char), skipping the second half of wide characters.
fn cells_of(buf: &Buffer, run: &Run) -> Vec<(u16, u16, char)> {
    let mut out = Vec::new();
    for (i, (y, cols)) in run.rows.iter().enumerate() {
        let mut lead = run.indented && i > 0;
        for x in cols.clone() {
            if let Some(c) = buf.cell((x, *y)).and_then(|c| c.symbol().chars().next()) {
                if lead && c == ' ' {
                    continue;
                }
                lead = false;
                out.push((x, *y, c));
            }
        }
    }
    out
}

/// Where the text under the pointer runs when it's more than its row:
/// each row with its columns, in reading order. In an `indented` run,
/// rows after the first start at their first character, as a
/// conversation indents the rest of a word it cut.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub rows: Vec<(u16, std::ops::Range<u16>)>,
    pub indented: bool,
}

/// Underline the URL under the pointer, if any, and return it. `run` is
/// where the text under the pointer goes on past its row: a terminal
/// line that wrapped, or a word a conversation cut to fit.
pub fn hover(buf: &mut Buffer, pos: Option<(u16, u16)>, run: Option<Run>) -> Option<Link> {
    let (col, row) = pos?;
    if row >= buf.area.bottom() {
        return None;
    }
    let run = run.unwrap_or(Run { rows: vec![(row, buf.area.x..buf.area.right())], indented: false });
    let cells = cells_of(buf, &run);
    let at = cells.iter().position(|&(x, y, _)| x == col && y == row)?;
    let (i, j, url) = url_at(&cells, at)?;
    let mut spans: Vec<(u16, u16, u16)> = Vec::new();
    for &(x, y, _) in &cells[i..j] {
        let cell = &mut buf[(x, y)];
        cell.set_style(cell.style().add_modifier(Modifier::UNDERLINED));
        match spans.last_mut() {
            Some((r, _, x1)) if *r == y => *x1 = x + 1,
            _ => spans.push((y, x, x + 1)),
        }
    }
    Some(Link { url, spans })
}

/// Open a URL with the system's handler.
pub fn open(url: &str) -> std::io::Result<()> {
    let program = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    std::process::Command::new(program)
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(s: &str) -> Vec<(u16, u16, char)> {
        s.chars().enumerate().map(|(i, c)| (i as u16, 0, c)).collect()
    }

    #[test]
    fn finds_the_url_under_the_pointer() {
        let r = row("see https://example.com/a_(b)/c. or (http://x.io/p) │ next");
        let (x0, x1, url) = url_at(&r, 10).unwrap();
        assert_eq!(url, "https://example.com/a_(b)/c");
        assert_eq!((x0, x1), (4, 31));
        assert_eq!(url_at(&r, 40).unwrap().2, "http://x.io/p", "the closing bracket isn't the URL's");
        assert!(url_at(&r, 1).is_none());
        assert!(url_at(&row("   - Local:        http://localhost:3000"), 25).is_some());
        assert!(url_at(&row("URL = \"https://example.com/some/…"), 10).is_none(), "cut short");
    }

    #[test]
    fn a_url_that_wrapped_is_one_link() {
        use ratatui::layout::Rect;
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 3));
        buf.set_string(0, 0, "│PR: https://github.com/abc│", ratatui::style::Style::new());
        buf.set_string(0, 1, "│/pull/12 and more         │", ratatui::style::Style::new());
        // Its row alone: only the first half, so not as a terminal line.
        let link = hover(&mut buf.clone(), Some((10, 0)), None).unwrap();
        assert_eq!(link.url, "https://github.com/abc");
        let run = Run { rows: vec![(0, 1..27), (1, 1..27)], indented: false };
        let link = hover(&mut buf, Some((3, 1)), Some(run)).unwrap();
        assert_eq!(link.url, "https://github.com/abc/pull/12");
        assert_eq!(link.spans, vec![(0, 5, 27), (1, 1, 9)]);
        assert!(link.covers(26, 0) && link.covers(8, 1) && !link.covers(9, 1));
    }

    #[test]
    fn a_url_a_conversation_cut_is_one_link() {
        use ratatui::layout::Rect;
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 3));
        buf.set_string(0, 0, " > see https://example.com/ab  18:08", ratatui::style::Style::new());
        buf.set_string(0, 1, "   cdef/ghi and more", ratatui::style::Style::new());
        // The first row was cut after column 28, before the time.
        let run = Run { rows: vec![(0, 0..29), (1, 0..30)], indented: true };
        let link = hover(&mut buf, Some((5, 1)), Some(run)).unwrap();
        assert_eq!(link.url, "https://example.com/abcdef/ghi");
        assert_eq!(link.spans, vec![(0, 7, 29), (1, 3, 11)]);
    }
}
