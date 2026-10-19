use crate::model::Ms;

/// Truncate to `max` characters, adding an ellipsis when cut.
pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// First non-empty line, trimmed.
pub fn first_line(s: &str) -> &str {
    s.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("")
}

/// Collapse all whitespace runs to single spaces.
pub fn squash(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// "12s", "4m 12s", "1h 03m", "3d 4h".
pub fn human_duration(ms: i64) -> String {
    let s = (ms.max(0) / 1000) as u64;
    match s {
        0..=59 => format!("{s}s"),
        60..=3599 => format!("{}m {:02}s", s / 60, s % 60),
        3600..=86399 => format!("{}h {:02}m", s / 3600, (s % 3600) / 60),
        _ => format!("{}d {}h", s / 86400, (s % 86400) / 3600),
    }
}

/// Compact form: "12s", "4m", "1h03", "3d".
pub fn short_duration(ms: i64) -> String {
    let s = (ms.max(0) / 1000) as u64;
    match s {
        0..=59 => format!("{s}s"),
        60..=3599 => format!("{}m", s / 60),
        3600..=86399 => format!("{}h{:02}", s / 3600, (s % 3600) / 60),
        _ => format!("{}d", s / 86400),
    }
}

pub fn ago(now: Ms, then: Ms) -> String {
    let d = now - then;
    if d < 5_000 { "just now".to_string() } else { format!("{} ago", short_duration(d)) }
}

/// "claude-opus-5-5" → "opus 5.5", "claude-sonnet-4-5-20250929" → "sonnet 4.5".
pub fn short_model(model: &str) -> String {
    let m = model.trim().trim_start_matches("claude-");
    let (m, suffix) = match m.find('[') {
        Some(i) => (&m[..i], m[i..].to_string()),
        None => (m, String::new()),
    };
    let mut name: Vec<&str> = Vec::new();
    let mut nums: Vec<&str> = Vec::new();
    for part in m.split('-') {
        if part.chars().all(|c| c.is_ascii_digit()) {
            if part.len() <= 2 {
                nums.push(part);
            }
        } else {
            name.push(part);
        }
    }
    let mut out = name.join("-");
    if !nums.is_empty() {
        out.push(' ');
        out.push_str(&nums.join("."));
    }
    out.push_str(&suffix);
    out
}

/// "142312" → "142.3k", "1200000" → "1.2M".
pub fn fmt_tokens(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{:.1}k", n as f64 / 1000.0),
        _ => format!("{:.2}M", n as f64 / 1_000_000.0),
    }
}

/// Path relative to `base` when it lives inside it.
pub fn rel_path(path: &str, base: &str) -> String {
    let base = base.trim_end_matches('/');
    if !base.is_empty()
        && let Some(rest) = path.strip_prefix(base)
        && let Some(r) = rest.strip_prefix('/')
    {
        return r.to_string();
    }
    crate::paths::tildify(path)
}

pub fn basename(path: &str) -> String {
    std::path::Path::new(path).file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn models() {
        assert_eq!(short_model("claude-opus-5-5"), "opus 5.5");
        assert_eq!(short_model("claude-sonnet-4-5-20250929"), "sonnet 4.5");
        assert_eq!(short_model("claude-haiku-5-5[1m]"), "haiku 5.5[1m]");
        assert_eq!(short_model("claude-fable-5-1"), "fable 5.1");
    }

    #[test]
    fn durations() {
        assert_eq!(human_duration(4_000), "4s");
        assert_eq!(human_duration(252_000), "4m 12s");
        assert_eq!(human_duration(3_780_000), "1h 03m");
        assert_eq!(short_duration(3_780_000), "1h03");
    }

    #[test]
    fn tokens() {
        assert_eq!(fmt_tokens(812), "812");
        assert_eq!(fmt_tokens(142_312), "142.3k");
        assert_eq!(fmt_tokens(1_200_000), "1.20M");
    }

    #[test]
    fn paths() {
        assert_eq!(rel_path("/a/b/c.rs", "/a/b"), "c.rs");
        assert_eq!(rel_path("/a/bc/c.rs", "/a/b"), "/a/bc/c.rs");
    }
}
