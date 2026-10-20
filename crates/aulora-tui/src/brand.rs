//! aulora's wordmark and mark. The art is kept as plain text; colour is
//! applied when drawing, from the theme, so it follows every theme.
//!
//! The wordmark is "ANSI Shadow" capitals: solid strokes (`█`) with a
//! box-drawn shadow edge (`╗║═╝`), shaded top to bottom. Solid strokes
//! are drawn as coloured cell backgrounds, never as block glyphs, so the
//! gap Terminal.app leaves between rows never shows as a seam.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::theme::Theme;

/// The wordmark, 6 rows by 50 columns.
pub const WORDMARK: [&str; 6] = [
    " █████╗ ██╗   ██╗██╗      ██████╗ ██████╗  █████╗ ",
    "██╔══██╗██║   ██║██║     ██╔═══██╗██╔══██╗██╔══██╗",
    "███████║██║   ██║██║     ██║   ██║██████╔╝███████║",
    "██╔══██║██║   ██║██║     ██║   ██║██╔══██╗██╔══██║",
    "██║  ██║╚██████╔╝███████╗╚██████╔╝██║  ██║██║  ██║",
    "╚═╝  ╚═╝ ╚═════╝ ╚══════╝ ╚═════╝ ╚═╝  ╚═╝╚═╝  ╚═╝",
];

/// The same letters in line drawing, 3 rows by 18, for narrow spaces.
pub const SMALL: [&str; 3] = ["╔═╗╦ ╦╦  ╔═╗╦═╗╔═╗", "╠═╣║ ║║  ║ ║╠╦╝╠═╣", "╩ ╩╚═╝╩═╝╚═╝╩╚═╩ ╩"];

/// The mark, before the name in the sidebar.
pub const MARK: &str = "◆";

pub const TAGLINE: &str = "every Claude session, on one screen";

/// Blend two colours; `f` 0 is `a`, 1 is `b`. Named colours can't blend,
/// so the nearer one is used.
fn mix(a: Color, b: Color, f: f32) -> Color {
    match (a, b) {
        (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => {
            let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * f).round() as u8;
            Color::Rgb(m(r1, r2), m(g1, g2), m(b1, b2))
        }
        _ if f < 0.5 => a,
        _ => b,
    }
}

/// The gradient's colour `p` of the way along (0 to 1): blue, cyan,
/// green, from the theme.
pub fn shade(t: &Theme, p: f32) -> Color {
    let stops = [t.blue, t.cyan, t.green];
    let seg = (p.clamp(0.0, 1.0) * 2.0).min(1.999);
    let i = seg as usize;
    mix(stops[i], stops[i + 1], seg - i as f32)
}

/// The gradient's colour at `x` of `width`, for one line of text.
pub fn gradient(t: &Theme, x: usize, width: usize) -> Color {
    shade(t, if width <= 1 { 0.0 } else { x as f32 / (width - 1) as f32 })
}

/// The banner that fits `width`, drawn on a known background `bg`: the
/// wordmark, the line-drawn one, or the mark and name. Always ends with
/// the tagline between diamonds, centred under the art.
pub fn banner(t: &Theme, width: u16, bg: Color) -> Vec<Line<'static>> {
    let (mut lines, w) = if width as usize >= WORDMARK[0].chars().count() {
        (art(t, &WORDMARK, bg), WORDMARK[0].chars().count())
    } else if width as usize >= SMALL[0].chars().count() {
        (art(t, &SMALL, bg), SMALL[0].chars().count())
    } else {
        return vec![Line::from(mark(t))];
    };
    lines.push(Line::raw(""));
    let caption = format!("◆  {TAGLINE}  ◆");
    let n = caption.chars().count();
    if n <= width as usize {
        let pad = w.saturating_sub(n) / 2;
        lines.push(Line::from(vec![
            Span::raw(" ".repeat(pad)),
            Span::styled("◆  ", Style::new().fg(shade(t, 1.0))),
            Span::styled(TAGLINE, Style::new().fg(t.dim)),
            Span::styled("  ◆", Style::new().fg(shade(t, 1.0))),
            Span::raw(" ".repeat(w.saturating_sub(n + pad))),
        ]));
    }
    lines
}

/// Draw art shaded top to bottom: solid strokes as cell backgrounds, line
/// drawing in a deeper shade of the same colour, like a shadow.
fn art(t: &Theme, rows: &[&str], bg: Color) -> Vec<Line<'static>> {
    let h = rows.len();
    rows.iter()
        .enumerate()
        .map(|(y, row)| {
            let c = shade(t, if h <= 1 { 0.0 } else { y as f32 / (h - 1) as f32 });
            let edge = mix(c, bg, 0.45);
            let spans: Vec<Span> = row
                .chars()
                .map(|ch| match ch {
                    '█' => Span::styled(" ", Style::new().bg(c)),
                    ' ' => Span::styled(" ", Style::new().bg(bg)),
                    // In the small version the lines are the letters.
                    _ if rows.len() < 6 => Span::styled(ch.to_string(), Style::new().fg(c).bg(bg)),
                    _ => Span::styled(ch.to_string(), Style::new().fg(edge).bg(bg)),
                })
                .collect();
            Line::from(spans)
        })
        .collect()
}

/// `◆ aulora`, for the sidebar's header: the mark, then the name in the
/// gradient.
pub fn mark(t: &Theme) -> Vec<Span<'static>> {
    let bold = |c: Color| Style::new().fg(c).add_modifier(Modifier::BOLD);
    let mut spans = vec![Span::styled(MARK, Style::new().fg(shade(t, 0.5))), Span::raw(" ")];
    for (i, c) in "aulora".chars().enumerate() {
        spans.push(Span::styled(c.to_string(), bold(gradient(t, i, 6))));
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_width::UnicodeWidthChar;

    #[test]
    fn the_art_is_even_and_one_cell_per_char() {
        for art in [&WORDMARK[..], &SMALL[..]] {
            let w = art[0].chars().count();
            for row in art {
                assert_eq!(row.chars().count(), w, "ragged row: {row:?}");
                assert!(row.chars().all(|c| c.width() == Some(1)), "a char isn't one cell wide in {row:?}");
            }
        }
    }

    #[test]
    fn solid_strokes_are_backgrounds_not_blocks() {
        let t = crate::theme::get();
        let lines = art(t, &WORDMARK, t.panel);
        let text: String = lines.iter().flat_map(|l| l.spans.iter().map(|s| s.content.to_string())).collect();
        assert!(!text.contains('█'));
        assert!(lines[0].spans.iter().any(|s| s.style.bg == Some(t.blue)), "the top row is blue");
        assert!(lines[5].spans.iter().all(|s| s.style.bg != Some(t.blue)));
    }

    #[test]
    fn the_banner_fits_the_width_it_is_given() {
        let t = crate::theme::get();
        for w in [10u16, 18, 30, 49, 50, 80] {
            for line in banner(t, w, t.panel) {
                assert!(line.width() <= w as usize, "{w}: {line:?}");
            }
        }
        assert_eq!(banner(t, 80, t.panel).len(), 8, "wordmark, blank, tagline");
    }
}
