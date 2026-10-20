//! aulora's wordmark and mark. The art is kept as plain text; colour is
//! applied when drawing, as a gradient through the theme's green, cyan,
//! blue and magenta, so it follows every theme.
//!
//! Half-block art is drawn in "cells mode": each glyph becomes a cell whose
//! background carries the stroke, so the gap some terminals (Terminal.app)
//! leave between rows never shows as a seam.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::theme::Theme;

/// The welcome banner, lowercase, 5 rows.
pub const HALO: [&str; 5] = [
    "                ██                       ",
    "                ██                       ",
    " ▀▀▀█▄  ██  ██  ██   ▄█▀▀█▄  ▄█▀▀▀  ▀▀▀█▄",
    "▄█▀▀██  ██  ██  ██   ██  ██  ██    ▄█▀▀██",
    "▀█▄▄██  ▀█▄▄██  ▀█▄  ▀█▄▄█▀  ██    ▀█▄▄██",
];

/// The same letters in line drawing, for narrow spaces.
pub const HAIRLINE: [&str; 4] =
    ["        ╷            ", "╶─╮ ╷ ╷ │ ╭─╮ ╭─╴ ╶─╮", "╭─┤ │ │ │ │ │ │   ╭─┤", "╰─┴ ╰─┴ ╰╴╰─╯ ╵   ╰─┴"];

/// The mark: an arch, an arc of light over every workspace.
pub const MARK: [&str; 2] = ["▛", "▜"];

pub const TAGLINE: &str = "every Claude session, on one screen";

/// The gradient's colour at `x` of `width`.
pub fn gradient(t: &Theme, x: usize, width: usize) -> Color {
    let stops = [t.green, t.cyan, t.blue, t.magenta];
    let p = if width <= 1 { 0.0 } else { x as f32 / (width - 1) as f32 };
    let rgb: Option<Vec<(u8, u8, u8)>> = stops
        .iter()
        .map(|c| match c {
            Color::Rgb(r, g, b) => Some((*r, *g, *b)),
            _ => None,
        })
        .collect();
    match rgb {
        Some(rgb) => {
            let seg = (p * 3.0).min(2.999);
            let i = seg as usize;
            let f = seg - i as f32;
            let (a, b) = (rgb[i], rgb[i + 1]);
            let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * f).round() as u8;
            Color::Rgb(mix(a.0, b.0), mix(a.1, b.1), mix(a.2, b.2))
        }
        // Named or indexed colours can't blend: step through them, so the
        // terminal's own palette stays in charge.
        None => stops[((p * 4.0) as usize).min(3)],
    }
}

/// The banner that fits `width`, drawn on a known background `bg`:
/// the half-block wordmark, the line-drawn one, or one line.
pub fn banner(t: &Theme, width: u16, bg: Color) -> Vec<Line<'static>> {
    if width >= 43 {
        return cells(t, &HALO, bg);
    }
    if width >= 23 {
        let w = HAIRLINE[0].chars().count();
        return HAIRLINE
            .iter()
            .map(|row| {
                Line::from(
                    row.chars()
                        .enumerate()
                        .map(|(x, c)| Span::styled(c.to_string(), Style::new().fg(gradient(t, x, w)).bg(bg)))
                        .collect::<Vec<_>>(),
                )
            })
            .collect();
    }
    vec![Line::from(mark(t))]
}

/// Half-block art in cells mode: a cell's lit halves come from its
/// background and a lower half block, never from a full block glyph.
fn cells(t: &Theme, art: &[&str], bg: Color) -> Vec<Line<'static>> {
    let w = art.iter().map(|r| r.chars().count()).max().unwrap_or(0);
    art.iter()
        .map(|row| {
            let spans: Vec<Span> = row
                .chars()
                .enumerate()
                .map(|(x, ch)| {
                    let c = gradient(t, x, w);
                    let (top, bottom) = match ch {
                        '█' => (true, true),
                        '▀' => (true, false),
                        '▄' => (false, true),
                        _ => (false, false),
                    };
                    match (top, bottom) {
                        (true, true) => Span::styled(" ", Style::new().bg(c)),
                        (true, false) => Span::styled("▄", Style::new().fg(bg).bg(c)),
                        (false, true) => Span::styled("▄", Style::new().fg(c).bg(bg)),
                        (false, false) => Span::styled(" ", Style::new().bg(bg)),
                    }
                })
                .collect();
            Line::from(spans)
        })
        .collect()
}

/// `▛▜ aulora`, for the sidebar's header: the mark, then the name in the
/// gradient.
pub fn mark(t: &Theme) -> Vec<Span<'static>> {
    let bold = |c: Color| Style::new().fg(c).add_modifier(Modifier::BOLD);
    let mut spans = vec![
        Span::styled(MARK[0], Style::new().fg(gradient(t, 0, 2))),
        Span::styled(MARK[1], Style::new().fg(gradient(t, 1, 2))),
        Span::raw(" "),
    ];
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
        for art in [&HALO[..], &HAIRLINE[..], &MARK[..]] {
            let w = art[0].chars().count();
            for row in art {
                assert_eq!(row.chars().count(), w, "ragged row: {row:?}");
                assert!(row.chars().all(|c| c.width() == Some(1)), "a char isn't one cell wide in {row:?}");
            }
        }
    }

    #[test]
    fn cells_mode_never_uses_a_full_block() {
        let t = crate::theme::get();
        let lines = cells(t, &HALO, t.panel);
        assert_eq!(lines.len(), 5);
        let text: String = lines.iter().flat_map(|l| l.spans.iter().map(|s| s.content.to_string())).collect();
        assert!(!text.contains('█') && !text.contains('▀'), "only spaces and lower half blocks");
    }

    #[test]
    fn the_gradient_runs_from_green_to_magenta() {
        let t = crate::theme::get();
        assert_eq!(gradient(t, 0, 10), t.green);
        assert_eq!(gradient(t, 9, 10), t.magenta);
    }
}
