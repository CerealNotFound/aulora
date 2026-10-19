//! The "aurora" palette. Deep night background, teal → violet → pink
//! accents, and amber reserved for anything that needs you.

use aulora_core::model::{FeedKind, SessionState};
use ratatui::style::{Color, Modifier, Style};

pub const BG: Color = Color::Rgb(10, 13, 20);
pub const PANEL: Color = Color::Rgb(14, 18, 27);
pub const RAISED: Color = Color::Rgb(20, 25, 37);
pub const SELECT: Color = Color::Rgb(27, 34, 52);
pub const BORDER: Color = Color::Rgb(36, 43, 61);
pub const BORDER_HI: Color = Color::Rgb(74, 88, 124);

pub const TEXT: Color = Color::Rgb(218, 223, 234);
pub const DIM: Color = Color::Rgb(136, 145, 168);
pub const FAINT: Color = Color::Rgb(84, 93, 117);

pub const TEAL: Color = Color::Rgb(94, 234, 212);
pub const CYAN: Color = Color::Rgb(103, 210, 249);
pub const GREEN: Color = Color::Rgb(110, 231, 150);
pub const VIOLET: Color = Color::Rgb(167, 139, 250);
pub const PINK: Color = Color::Rgb(244, 114, 182);
pub const BLUE: Color = Color::Rgb(125, 166, 255);
pub const AMBER: Color = Color::Rgb(251, 191, 36);
pub const AMBER_DIM: Color = Color::Rgb(146, 104, 18);
pub const AMBER_BG: Color = Color::Rgb(44, 33, 10);
pub const RED: Color = Color::Rgb(248, 113, 113);

/// Colors that identify projects (no amber/red: those mean state).
const PROJECT_HUES: &[Color] = &[
    TEAL,
    VIOLET,
    PINK,
    BLUE,
    Color::Rgb(134, 239, 172),
    Color::Rgb(253, 164, 175),
    Color::Rgb(147, 197, 253),
    Color::Rgb(216, 180, 254),
];

pub fn project_color(root: &str) -> Color {
    // FNV-1a: stable across runs.
    let mut h: u32 = 0x811c9dc5;
    for b in root.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x01000193);
    }
    PROJECT_HUES[(h as usize) % PROJECT_HUES.len()]
}

pub fn lerp(a: Color, b: Color, t: f32) -> Color {
    match (a, b) {
        (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => {
            let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
            Color::Rgb(m(r1, r2), m(g1, g2), m(b1, b2))
        }
        _ => a,
    }
}

/// Teal → violet → pink.
pub fn aurora(t: f32) -> Color {
    if t < 0.5 { lerp(TEAL, VIOLET, t * 2.0) } else { lerp(VIOLET, PINK, (t - 0.5) * 2.0) }
}

/// Smooth 0..1..0 pulse with the given period.
pub fn pulse(now_ms: i64, period_ms: i64) -> f32 {
    let phase = (now_ms.rem_euclid(period_ms)) as f32 / period_ms as f32;
    0.5 - 0.5 * (phase * std::f32::consts::TAU).cos()
}

pub fn state_color(state: &SessionState) -> Color {
    match state {
        SessionState::NeedsYou { .. } => AMBER,
        SessionState::Errored { .. } => RED,
        SessionState::YourTurn => GREEN,
        SessionState::Working => CYAN,
        SessionState::Compacting => VIOLET,
        SessionState::Idle => DIM,
        SessionState::Ended { .. } => FAINT,
    }
}

/// Pulsing color for states that need you.
pub fn live_state_color(state: &SessionState, now: i64) -> Color {
    match state {
        SessionState::NeedsYou { .. } => lerp(AMBER, Color::Rgb(255, 236, 170), pulse(now, 1400)),
        other => state_color(other),
    }
}

const SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub fn spinner(now: i64) -> &'static str {
    SPINNER[((now / 80) as usize) % SPINNER.len()]
}

pub fn state_glyph(state: &SessionState, now: i64) -> &'static str {
    match state {
        SessionState::NeedsYou { .. } => "◆",
        SessionState::Errored { .. } => "✗",
        SessionState::YourTurn => "✓",
        SessionState::Working => spinner(now),
        SessionState::Compacting => "◌",
        SessionState::Idle => "○",
        SessionState::Ended { .. } => "·",
    }
}

pub fn feed_style(kind: FeedKind) -> (&'static str, Color) {
    match kind {
        FeedKind::Permission => ("◆", AMBER),
        FeedKind::Question => ("?", AMBER),
        FeedKind::Done => ("✓", GREEN),
        FeedKind::Error => ("✗", RED),
        FeedKind::Edit => ("✎", VIOLET),
        FeedKind::Tool => ("$", CYAN),
        FeedKind::Prompt => ("›", TEXT),
        FeedKind::Agent => ("⑂", PINK),
        FeedKind::Compact => ("◌", VIOLET),
        FeedKind::Start => ("▸", TEAL),
        FeedKind::End => ("■", FAINT),
        FeedKind::Info => ("i", DIM),
    }
}

pub fn text() -> Style {
    Style::new().fg(TEXT)
}
pub fn dim() -> Style {
    Style::new().fg(DIM)
}
pub fn faint() -> Style {
    Style::new().fg(FAINT)
}
pub fn bold(c: Color) -> Style {
    Style::new().fg(c).add_modifier(Modifier::BOLD)
}
pub fn label() -> Style {
    Style::new().fg(FAINT).add_modifier(Modifier::BOLD)
}
