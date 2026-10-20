//! Links on screen: the URL under the pointer is underlined, and a click
//! on it opens it in the browser. Cmd-click works too: terminals don't tell
//! programs about Cmd, so the click arrives as a click. Links are found in
//! the frame as drawn, so terminal output, conversations and files all
//! work the same.

use ratatui::buffer::Buffer;
use ratatui::style::Modifier;

/// The URL covering `col` on a row of text: (first column, last column + 1, url).
pub fn url_at(row: &[(u16, char)], col: u16) -> Option<(u16, u16, String)> {
    let text: String = row.iter().map(|(_, c)| *c).collect();
    let chars: Vec<char> = text.chars().collect();
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
        let (x0, x1) = (row[i].0, row[j - 1].0 + 1);
        if col >= x0 && col < x1 && j - i > 8 {
            return Some((x0, x1, chars[i..j].iter().collect()));
        }
        i = j.max(i + 1);
    }
    None
}

/// A row of the drawn frame as (column, char), skipping the second half
/// of wide characters.
fn row_of(buf: &Buffer, y: u16) -> Vec<(u16, char)> {
    let area = buf.area;
    let mut out = Vec::with_capacity(area.width as usize);
    for x in area.x..area.right() {
        let sym = buf[(x, y)].symbol();
        if let Some(c) = sym.chars().next() {
            out.push((x, c));
        }
    }
    out
}

/// Underline the URL under the pointer, if any, and return it.
pub fn hover(buf: &mut Buffer, pos: Option<(u16, u16)>) -> Option<(u16, u16, u16, String)> {
    let (col, row) = pos?;
    if row >= buf.area.bottom() {
        return None;
    }
    let (x0, x1, url) = url_at(&row_of(buf, row), col)?;
    for x in x0..x1 {
        let cell = &mut buf[(x, row)];
        cell.set_style(cell.style().add_modifier(Modifier::UNDERLINED));
    }
    Some((row, x0, x1, url))
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

    fn row(s: &str) -> Vec<(u16, char)> {
        s.chars().enumerate().map(|(i, c)| (i as u16, c)).collect()
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
    }
}
