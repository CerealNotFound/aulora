//! Themes. Every color the dashboard draws comes from the active theme.
//!
//! The default, `catppuccin`, follows Herdr: it keeps the terminal's own
//! background and draws with Catppuccin Mocha on top. Values come from each
//! theme's published palette.

use std::sync::atomic::{AtomicUsize, Ordering};

use ratatui::style::{Color, Modifier, Style};

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub name: &'static str,
    pub light: bool,
    /// Main background. `Color::Reset` keeps the terminal's own.
    pub bg: Color,
    /// Sidebar background.
    pub surface: Color,
    /// Selected row.
    pub select: Color,
    /// Popups and key caps.
    pub raised: Color,
    pub text: Color,
    pub dim: Color,
    pub faint: Color,
    pub accent: Color,
    pub green: Color,
    pub yellow: Color,
    pub red: Color,
    /// For links and syntax highlighting.
    #[allow(dead_code)]
    pub blue: Color,
    pub cyan: Color,
    pub magenta: Color,
    #[allow(dead_code)]
    pub orange: Color,
    /// Header and footer bar.
    pub bar_bg: Color,
    pub bar_fg: Color,
    /// Tab bar, mode bar, modals and toasts (Herdr's panel_bg).
    pub panel: Color,
    /// The focused row while its region doesn't have focus (Herdr's active_row_bg).
    pub active: Color,
    /// Separators and scrollbar tracks (Herdr's surface_dim).
    pub rule: Color,
}

const fn rgb(hex: u32) -> Color {
    Color::Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

const R: Color = Color::Reset;

pub const THEMES: &[Theme] = &[
    // Catppuccin Mocha on the terminal's own background, like Herdr.
    Theme {
        name: "catppuccin",
        light: false,
        bg: R,
        surface: R,
        select: rgb(0x313244),
        raised: rgb(0x313244),
        text: rgb(0xcdd6f4),
        dim: rgb(0xa6adc8),
        faint: rgb(0x6c7086),
        accent: rgb(0x89b4fa),
        green: rgb(0xa6e3a1),
        yellow: rgb(0xf9e2af),
        red: rgb(0xf38ba8),
        blue: rgb(0x89b4fa),
        cyan: rgb(0x94e2d5),
        magenta: rgb(0xcba6f7),
        orange: rgb(0xfab387),
        bar_bg: R,
        bar_fg: rgb(0xcdd6f4),
        panel: rgb(0x181825),
        active: rgb(0x1e1e2e),
        rule: rgb(0x313244),
    },
    Theme {
        name: "catppuccin-latte",
        light: true,
        bg: rgb(0xeff1f5),
        surface: rgb(0xe6e9ef),
        select: rgb(0xccd0da),
        raised: rgb(0xdce0e8),
        text: rgb(0x4c4f69),
        dim: rgb(0x6c6f85),
        faint: rgb(0x9ca0b0),
        accent: rgb(0x8839ef),
        green: rgb(0x40a02b),
        yellow: rgb(0xdf8e1d),
        red: rgb(0xd20f39),
        blue: rgb(0x1e66f5),
        cyan: rgb(0x179299),
        magenta: rgb(0xea76cb),
        orange: rgb(0xfe640b),
        bar_bg: rgb(0xe6e9ef),
        bar_fg: rgb(0x4c4f69),
        panel: rgb(0xe6e9ef),
        active: rgb(0xdce0e8),
        rule: rgb(0xccd0da),
    },
    // The classic tmux look: your terminal's colors and a green status bar.
    Theme {
        name: "tmux",
        light: false,
        bg: R,
        surface: R,
        select: Color::Indexed(236),
        raised: Color::Indexed(236),
        text: R,
        dim: Color::Gray,
        faint: Color::DarkGray,
        accent: Color::Green,
        green: Color::Green,
        yellow: Color::Yellow,
        red: Color::Red,
        blue: Color::Blue,
        cyan: Color::Cyan,
        magenta: Color::Magenta,
        orange: Color::Yellow,
        bar_bg: Color::Green,
        bar_fg: Color::Black,
        panel: Color::Indexed(235),
        active: Color::Indexed(235),
        rule: Color::Indexed(238),
    },
    // Only the terminal's 16 ANSI colors, so it follows your terminal theme.
    Theme {
        name: "terminal",
        light: false,
        bg: R,
        surface: R,
        select: Color::DarkGray,
        raised: Color::DarkGray,
        text: R,
        dim: Color::Gray,
        faint: Color::DarkGray,
        accent: Color::Blue,
        green: Color::Green,
        yellow: Color::Yellow,
        red: Color::Red,
        blue: Color::Blue,
        cyan: Color::Cyan,
        magenta: Color::Magenta,
        orange: Color::LightRed,
        bar_bg: R,
        bar_fg: R,
        panel: Color::Black,
        active: Color::Black,
        rule: Color::DarkGray,
    },
    Theme {
        name: "github-dark",
        light: false,
        bg: rgb(0x0d1117),
        surface: rgb(0x010409),
        select: rgb(0x21262d),
        raised: rgb(0x161b22),
        text: rgb(0xe6edf3),
        dim: rgb(0x9198a1),
        faint: rgb(0x6e7681),
        accent: rgb(0x58a6ff),
        green: rgb(0x3fb950),
        yellow: rgb(0xd29922),
        red: rgb(0xf85149),
        blue: rgb(0x58a6ff),
        cyan: rgb(0x56d4dd),
        magenta: rgb(0xbc8cff),
        orange: rgb(0xdb6d28),
        bar_bg: rgb(0x010409),
        bar_fg: rgb(0xe6edf3),
        panel: rgb(0x161b22),
        active: rgb(0x161b22),
        rule: rgb(0x21262d),
    },
    Theme {
        name: "github-light",
        light: true,
        bg: rgb(0xffffff),
        surface: rgb(0xf6f8fa),
        select: rgb(0xddf4ff),
        raised: rgb(0xeff2f5),
        text: rgb(0x1f2328),
        dim: rgb(0x59636e),
        faint: rgb(0x818b98),
        accent: rgb(0x0969da),
        green: rgb(0x1a7f37),
        yellow: rgb(0x9a6700),
        red: rgb(0xd1242f),
        blue: rgb(0x0969da),
        cyan: rgb(0x1b7c83),
        magenta: rgb(0x8250df),
        orange: rgb(0xbc4c00),
        bar_bg: rgb(0xf6f8fa),
        bar_fg: rgb(0x1f2328),
        panel: rgb(0xf6f8fa),
        active: rgb(0xf6f8fa),
        rule: rgb(0xd1d9e0),
    },
    Theme {
        name: "tokyo-night",
        light: false,
        bg: rgb(0x1a1b26),
        surface: rgb(0x16161e),
        select: rgb(0x292e42),
        raised: rgb(0x24283b),
        text: rgb(0xc0caf5),
        dim: rgb(0xa9b1d6),
        faint: rgb(0x565f89),
        accent: rgb(0x7aa2f7),
        green: rgb(0x9ece6a),
        yellow: rgb(0xe0af68),
        red: rgb(0xf7768e),
        blue: rgb(0x7aa2f7),
        cyan: rgb(0x7dcfff),
        magenta: rgb(0xbb9af7),
        orange: rgb(0xff9e64),
        bar_bg: rgb(0x16161e),
        bar_fg: rgb(0xc0caf5),
        panel: rgb(0x16161e),
        active: rgb(0x1f2335),
        rule: rgb(0x292e42),
    },
    Theme {
        name: "gruvbox",
        light: false,
        bg: rgb(0x282828),
        surface: rgb(0x1d2021),
        select: rgb(0x3c3836),
        raised: rgb(0x3c3836),
        text: rgb(0xebdbb2),
        dim: rgb(0xbdae93),
        faint: rgb(0x928374),
        accent: rgb(0x8ec07c),
        green: rgb(0xb8bb26),
        yellow: rgb(0xfabd2f),
        red: rgb(0xfb4934),
        blue: rgb(0x83a598),
        cyan: rgb(0x8ec07c),
        magenta: rgb(0xd3869b),
        orange: rgb(0xfe8019),
        bar_bg: rgb(0x1d2021),
        bar_fg: rgb(0xebdbb2),
        panel: rgb(0x1d2021),
        active: rgb(0x32302f),
        rule: rgb(0x3c3836),
    },
    Theme {
        name: "nord",
        light: false,
        bg: rgb(0x2e3440),
        surface: rgb(0x292e39),
        select: rgb(0x3b4252),
        raised: rgb(0x3b4252),
        text: rgb(0xeceff4),
        dim: rgb(0xd8dee9),
        faint: rgb(0x7b88a1),
        accent: rgb(0x88c0d0),
        green: rgb(0xa3be8c),
        yellow: rgb(0xebcb8b),
        red: rgb(0xbf616a),
        blue: rgb(0x81a1c1),
        cyan: rgb(0x8fbcbb),
        magenta: rgb(0xb48ead),
        orange: rgb(0xd08770),
        bar_bg: rgb(0x292e39),
        bar_fg: rgb(0xeceff4),
        panel: rgb(0x272c36),
        active: rgb(0x353b49),
        rule: rgb(0x3b4252),
    },
    Theme {
        name: "dracula",
        light: false,
        bg: rgb(0x282a36),
        surface: rgb(0x21222c),
        select: rgb(0x44475a),
        raised: rgb(0x343746),
        text: rgb(0xf8f8f2),
        dim: rgb(0xbfbfd0),
        faint: rgb(0x6272a4),
        accent: rgb(0xbd93f9),
        green: rgb(0x50fa7b),
        yellow: rgb(0xf1fa8c),
        red: rgb(0xff5555),
        blue: rgb(0x8be9fd),
        cyan: rgb(0x8be9fd),
        magenta: rgb(0xff79c6),
        orange: rgb(0xffb86c),
        bar_bg: rgb(0x21222c),
        bar_fg: rgb(0xf8f8f2),
        panel: rgb(0x21222c),
        active: rgb(0x343746),
        rule: rgb(0x44475a),
    },
    Theme {
        name: "one-dark",
        light: false,
        bg: rgb(0x282c34),
        surface: rgb(0x21252b),
        select: rgb(0x2c313a),
        raised: rgb(0x2c313a),
        text: rgb(0xabb2bf),
        dim: rgb(0x9da5b4),
        faint: rgb(0x5c6370),
        accent: rgb(0x61afef),
        green: rgb(0x98c379),
        yellow: rgb(0xe5c07b),
        red: rgb(0xe06c75),
        blue: rgb(0x61afef),
        cyan: rgb(0x56b6c2),
        magenta: rgb(0xc678dd),
        orange: rgb(0xd19a66),
        bar_bg: rgb(0x21252b),
        bar_fg: rgb(0xabb2bf),
        panel: rgb(0x21252b),
        active: rgb(0x2c313a),
        rule: rgb(0x3e4451),
    },
    // aulora's first look, toned down.
    Theme {
        name: "aurora",
        light: false,
        bg: rgb(0x0e121b),
        surface: rgb(0x0b0e15),
        select: rgb(0x1b2234),
        raised: rgb(0x161c2b),
        text: rgb(0xd5dae6),
        dim: rgb(0x8f98b0),
        faint: rgb(0x5a6380),
        accent: rgb(0x6fd8c6),
        green: rgb(0x7dd8a0),
        yellow: rgb(0xe8be55),
        red: rgb(0xec8585),
        blue: rgb(0x8fadf0),
        cyan: rgb(0x7ccbe6),
        magenta: rgb(0xb59df0),
        orange: rgb(0xe8a070),
        bar_bg: rgb(0x0b0e15),
        bar_fg: rgb(0xd5dae6),
        panel: rgb(0x0b0e15),
        active: rgb(0x161c2b),
        rule: rgb(0x1b2234),
    },
];

static CURRENT: AtomicUsize = AtomicUsize::new(0);

pub fn get() -> &'static Theme {
    &THEMES[CURRENT.load(Ordering::Relaxed) % THEMES.len()]
}

pub fn index_of(name: &str) -> Option<usize> {
    let n = name.trim().to_lowercase().replace([' ', '_'], "-");
    let n = match n.as_str() {
        "catppuccin-mocha" | "mocha" => "catppuccin",
        "latte" | "light" => "catppuccin-latte",
        "github" => "github-dark",
        "tokyonight" => "tokyo-night",
        "onedark" => "one-dark",
        other => other,
    };
    THEMES.iter().position(|t| t.name == n)
}

pub fn set(index: usize) {
    CURRENT.store(index % THEMES.len(), Ordering::Relaxed);
}

pub fn current_index() -> usize {
    CURRENT.load(Ordering::Relaxed) % THEMES.len()
}

/// The default when nothing is configured: Catppuccin, light or dark to
/// match the terminal.
pub fn default_for(light_terminal: bool) -> usize {
    index_of(if light_terminal { "catppuccin-latte" } else { "catppuccin" }).unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Semantic colors and styles

const SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub fn spinner(now: i64) -> &'static str {
    SPINNER[((now / 90) as usize) % SPINNER.len()]
}

pub fn fg(c: Color) -> Style {
    Style::new().fg(c)
}
pub fn text() -> Style {
    fg(get().text)
}
pub fn dim() -> Style {
    fg(get().dim)
}
pub fn faint() -> Style {
    fg(get().faint)
}
pub fn bold(c: Color) -> Style {
    Style::new().fg(c).add_modifier(Modifier::BOLD)
}
/// Small section labels.
pub fn label() -> Style {
    Style::new().fg(get().faint).add_modifier(Modifier::BOLD)
}

/// Ask the terminal for its background color (OSC 11) and report whether
/// it is light. Must run in raw mode before the event reader starts.
pub fn terminal_is_light() -> Option<bool> {
    use std::io::Write;
    let mut out = std::io::stdout();
    out.write_all(b"\x1b]11;?\x1b\\").ok()?;
    out.flush().ok()?;
    // Read the reply straight from the descriptor: going through std's
    // buffered stdin could swallow keys meant for the event reader.
    let fd = 0;
    let mut buf = Vec::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(150);
    while std::time::Instant::now() < deadline {
        let mut p = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        if unsafe { libc::poll(&mut p, 1, 30) } <= 0 {
            continue;
        }
        let mut chunk = [0u8; 64];
        let n = unsafe { libc::read(fd, chunk.as_mut_ptr().cast(), chunk.len()) };
        if n <= 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n as usize]);
        if buf.ends_with(b"\x07") || buf.ends_with(b"\x1b\\") {
            break;
        }
    }
    parse_osc11(&String::from_utf8_lossy(&buf))
}

/// Whether an OSC 11 reply like `ESC]11;rgb:1e1e/1e1e/2e2e ESC\` is light.
fn parse_osc11(reply: &str) -> Option<bool> {
    let rgb = reply.split("rgb:").nth(1)?;
    let parts: Vec<u32> = rgb
        .split('/')
        .take(3)
        .filter_map(|p| {
            let p: String = p.chars().take_while(char::is_ascii_hexdigit).collect();
            let v = u32::from_str_radix(&p, 16).ok()?;
            // Scale 1–4 hex digits to 0–255.
            Some(match p.len() {
                1 => v * 17,
                2 => v,
                3 => v >> 4,
                _ => v >> 8,
            })
        })
        .collect();
    if parts.len() != 3 {
        return None;
    }
    let lum = 0.2126 * parts[0] as f32 + 0.7152 * parts[1] as f32 + 0.0722 * parts[2] as f32;
    Some(lum > 140.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_aliases() {
        assert_eq!(index_of("Catppuccin Mocha"), index_of("catppuccin"));
        assert_eq!(THEMES[index_of("github").unwrap()].name, "github-dark");
        assert!(index_of("nope").is_none());
        let mut names: Vec<&str> = THEMES.iter().map(|t| t.name).collect();
        names.dedup();
        assert_eq!(names.len(), THEMES.len());
    }

    #[test]
    fn background_replies() {
        assert_eq!(parse_osc11("\x1b]11;rgb:1e1e/1e1e/2e2e\x1b\\"), Some(false));
        assert_eq!(parse_osc11("\x1b]11;rgb:ffff/ffff/ffff\x07"), Some(true));
        assert_eq!(parse_osc11("\x1b]11;rgb:ef/f1/f5\x07"), Some(true));
        assert_eq!(parse_osc11("garbage"), None);
    }
}
