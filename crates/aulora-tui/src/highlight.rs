//! Syntax-highlighted diffs with changed words marked (delta).
//!
//! syntect parses the grammar; colors come from aulora's own theme, so a
//! diff matches whichever theme is chosen. To get there, a syntect theme
//! is built whose colors encode a role number instead of a color, and the
//! renderer maps roles to the current theme.

use std::str::FromStr;
use std::sync::OnceLock;

use syntect::easy::HighlightLines;
use syntect::highlighting::{Color, ScopeSelectors, StyleModifier, Theme, ThemeItem, ThemeSettings};
use syntect::parsing::{SyntaxReference, SyntaxSet};

use crate::git::{FileDiff, LineKind};

/// What a piece of code is, for coloring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Role {
    Text = 0,
    Comment,
    Keyword,
    String,
    Constant,
    Function,
    Type,
    Punct,
    Tag,
    Property,
}

impl Role {
    fn from_u8(n: u8) -> Role {
        use Role::*;
        [Text, Comment, Keyword, String, Constant, Function, Type, Punct, Tag, Property]
            .get(n as usize)
            .copied()
            .unwrap_or(Text)
    }
}

/// TextMate scopes to roles. More specific selectors win, as in any theme.
const SCOPES: &[(&str, Role)] = &[
    ("comment, punctuation.definition.comment", Role::Comment),
    ("keyword, storage, keyword.control, storage.type, storage.modifier", Role::Keyword),
    ("keyword.operator, punctuation, meta.brace", Role::Punct),
    ("keyword.operator.word, keyword.operator.new, keyword.operator.expression", Role::Keyword),
    ("string, markup.raw, punctuation.definition.string", Role::String),
    ("constant.character.escape, string.regexp", Role::Property),
    ("constant.numeric, constant.language, constant.other, support.constant, variable.language", Role::Constant),
    ("entity.name.function, support.function, variable.function, meta.function-call.generic", Role::Function),
    (
        "entity.name.type, entity.name.class, entity.name.struct, entity.name.enum, entity.name.trait, support.type, support.class, entity.other.inherited-class, storage.type.primitive, storage.type.numeric, storage.type.string",
        Role::Type,
    ),
    ("entity.name.tag, markup.heading", Role::Tag),
    (
        "entity.other.attribute-name, support.type.property-name, meta.object-literal.key, variable.other.property, variable.other.member, meta.mapping.key string",
        Role::Property,
    ),
];

struct Engine {
    syntaxes: SyntaxSet,
    theme: Theme,
}

fn engine() -> &'static Engine {
    static ENGINE: OnceLock<Engine> = OnceLock::new();
    ENGINE.get_or_init(|| {
        let role = |r: Role| Color { r: r as u8, g: 0, b: 0, a: 0xff };
        let scopes = SCOPES
            .iter()
            .filter_map(|(sel, r)| {
                Some(ThemeItem {
                    scope: ScopeSelectors::from_str(sel).ok()?,
                    style: StyleModifier { foreground: Some(role(*r)), background: None, font_style: None },
                })
            })
            .collect();
        let theme = Theme {
            name: Some("aulora roles".into()),
            author: None,
            settings: ThemeSettings { foreground: Some(role(Role::Text)), ..Default::default() },
            scopes,
        };
        Engine { syntaxes: two_face::syntax::extra_newlines(), theme }
    })
}

/// Load the grammars now (about 50 ms), so the first diff doesn't wait.
pub fn warm_up() {
    engine();
}

fn syntax_for<'a>(ss: &'a SyntaxSet, path: &str, first_line: &str) -> Option<&'a SyntaxReference> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let ext = name.rsplit_once('.').map(|(_, e)| e).unwrap_or(name);
    ss.find_syntax_by_extension(ext)
        .or_else(|| ss.find_syntax_by_extension(name))
        .or_else(|| ss.find_syntax_by_first_line(first_line))
        .filter(|s| s.name != "Plain Text")
}

/// A run of text with one role; `emph` marks words that changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seg {
    pub text: String,
    pub role: Role,
    pub emph: bool,
}

#[derive(Debug, Clone)]
pub struct Line {
    pub kind: LineKind,
    pub old: Option<u32>,
    pub new: Option<u32>,
    pub segs: Vec<Seg>,
}

#[derive(Debug, Clone)]
pub struct Hunk {
    pub new_start: u32,
    pub context: String,
    /// The session that wrote most of this hunk.
    pub session: Option<String>,
    pub lines: Vec<Line>,
}

#[derive(Debug, Clone, Default)]
pub struct Styled {
    pub binary: bool,
    pub hunks: Vec<Hunk>,
}

impl Styled {
    pub fn rows(&self) -> usize {
        self.hunks.iter().map(|h| h.lines.len() + 1).sum()
    }
}

/// Lines longer than this aren't worth highlighting (minified files).
const LONG_LINE: usize = 1000;
/// Past this many lines a diff is shown plain.
const MAX_HIGHLIGHT: usize = 20_000;

pub fn style(diff: &FileDiff, sessions: impl Fn(&crate::git::Hunk) -> Option<String>) -> Styled {
    let e = engine();
    let first = diff.hunks.first().and_then(|h| h.lines.first()).map(|l| l.text.as_str()).unwrap_or("");
    let total: usize = diff.hunks.iter().map(|h| h.lines.len()).sum();
    let syntax = if total <= MAX_HIGHLIGHT { syntax_for(&e.syntaxes, &diff.path, first) } else { None };
    let mut out = Styled { binary: diff.binary, hunks: Vec::new() };
    for h in &diff.hunks {
        // Two parsers: the old side sees context and removals, the new side
        // context and additions, so each reads like real code (delta).
        let mut old_hl = syntax.map(|s| HighlightLines::new(s, &e.theme));
        let mut new_hl = syntax.map(|s| HighlightLines::new(s, &e.theme));
        let mut lines: Vec<Line> = Vec::with_capacity(h.lines.len());
        for l in &h.lines {
            let segs = match l.kind {
                LineKind::Context | LineKind::Marker => {
                    let segs = highlight(new_hl.as_mut(), &l.text, &e.syntaxes);
                    let _ = highlight(old_hl.as_mut(), &l.text, &e.syntaxes);
                    segs
                }
                LineKind::Added => highlight(new_hl.as_mut(), &l.text, &e.syntaxes),
                LineKind::Removed => highlight(old_hl.as_mut(), &l.text, &e.syntaxes),
            };
            lines.push(Line { kind: l.kind, old: l.old, new: l.new, segs });
        }
        mark_changed_words(&mut lines);
        out.hunks.push(Hunk { new_start: h.new_start, context: h.context.clone(), session: sessions(h), lines });
    }
    out
}

fn highlight(hl: Option<&mut HighlightLines>, text: &str, ss: &SyntaxSet) -> Vec<Seg> {
    let plain = || vec![Seg { text: text.to_string(), role: Role::Text, emph: false }];
    let Some(hl) = hl else { return plain() };
    if text.len() > LONG_LINE {
        // Keep the parser's state moving without the cost.
        let _ = hl.highlight_line("\n", ss);
        return plain();
    }
    let line = format!("{text}\n");
    let Ok(ranges) = hl.highlight_line(&line, ss) else { return plain() };
    let mut segs: Vec<Seg> = Vec::new();
    for (style, piece) in ranges {
        let piece = piece.trim_end_matches('\n');
        if piece.is_empty() {
            continue;
        }
        let role = Role::from_u8(style.foreground.r);
        match segs.last_mut() {
            Some(last) if last.role == role => last.text.push_str(piece),
            _ => segs.push(Seg { text: piece.to_string(), role, emph: false }),
        }
    }
    segs
}

/// Words, runs of spaces, and single symbols.
fn tokens(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut prev: Option<u8> = None;
    for (i, c) in s.char_indices() {
        let class = if c.is_alphanumeric() || c == '_' {
            0
        } else if c.is_whitespace() {
            1
        } else {
            2
        };
        if let Some(p) = prev
            && (p != class || class == 2)
        {
            out.push(&s[start..i]);
            start = i;
        }
        prev = Some(class);
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

/// Within each block of removed-then-added lines, pair them up and mark the
/// words that differ, when the lines are similar enough for that to help.
fn mark_changed_words(lines: &mut [Line]) {
    let mut i = 0;
    while i < lines.len() {
        if lines[i].kind != LineKind::Removed {
            i += 1;
            continue;
        }
        let del_start = i;
        while i < lines.len() && lines[i].kind == LineKind::Removed {
            i += 1;
        }
        let add_start = i;
        while i < lines.len() && lines[i].kind == LineKind::Added {
            i += 1;
        }
        let pairs = (add_start - del_start).min(i - add_start);
        for k in 0..pairs {
            let (a, b) = (del_start + k, add_start + k);
            let old: String = lines[a].segs.iter().map(|s| s.text.as_str()).collect();
            let new: String = lines[b].segs.iter().map(|s| s.text.as_str()).collect();
            if let Some((old_r, new_r)) = word_diff(&old, &new) {
                emphasize(&mut lines[a].segs, &old_r);
                emphasize(&mut lines[b].segs, &new_r);
            }
        }
    }
}

type Ranges = Vec<std::ops::Range<usize>>;

/// Byte ranges that changed on each side, or None when the lines share too
/// little for word marks to mean anything.
fn word_diff(old: &str, new: &str) -> Option<(Ranges, Ranges)> {
    use similar::{Algorithm, DiffTag, capture_diff_slices};
    let (a, b) = (tokens(old), tokens(new));
    let offsets = |t: &[&str]| {
        let mut v = Vec::with_capacity(t.len() + 1);
        let mut n = 0;
        v.push(0);
        for s in t {
            n += s.len();
            v.push(n);
        }
        v
    };
    let (oa, ob) = (offsets(&a), offsets(&b));
    let mut same = 0;
    let (mut ra, mut rb) = (Vec::new(), Vec::new());
    for op in capture_diff_slices(Algorithm::Myers, &a, &b) {
        let (tag, x, y) = op.as_tag_tuple();
        match tag {
            DiffTag::Equal => same += oa[x.end] - oa[x.start],
            _ => {
                if !x.is_empty() {
                    ra.push(oa[x.start]..oa[x.end]);
                }
                if !y.is_empty() {
                    rb.push(ob[y.start]..ob[y.end]);
                }
            }
        }
    }
    let longest = old.len().max(new.len()).max(1);
    let similar_enough = same * 10 >= longest * 4;
    (similar_enough && (!ra.is_empty() || !rb.is_empty())).then_some((ra, rb))
}

/// Split segments at range edges and mark the parts inside.
fn emphasize(segs: &mut Vec<Seg>, ranges: &Ranges) {
    if ranges.is_empty() {
        return;
    }
    let mut out = Vec::with_capacity(segs.len() + ranges.len() * 2);
    let mut pos = 0;
    for seg in segs.drain(..) {
        let end = pos + seg.text.len();
        let mut cuts = vec![pos, end];
        for r in ranges {
            for c in [r.start, r.end] {
                if c > pos && c < end && seg.text.is_char_boundary(c - pos) {
                    cuts.push(c);
                }
            }
        }
        cuts.sort_unstable();
        cuts.dedup();
        for w in cuts.windows(2) {
            let (s, e) = (w[0], w[1]);
            let emph = ranges.iter().any(|r| r.start <= s && e <= r.end);
            out.push(Seg { text: seg.text[s - pos..e - pos].to_string(), role: seg.role, emph });
        }
        pos = end;
    }
    *segs = out;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::parse_diff;

    #[test]
    fn highlights_by_role() {
        let d = &parse_diff(
            "diff --git a/x.rs b/x.rs\n--- a/x.rs\n+++ b/x.rs\n@@ -1 +1 @@\n-fn a() {}\n+fn b() { \"s\" } // hi\n",
        )[0];
        let s = style(d, |_| None);
        let added = &s.hunks[0].lines[1];
        let role_of = |text: &str| added.segs.iter().find(|g| g.text.contains(text)).map(|g| g.role);
        assert_eq!(role_of("fn"), Some(Role::Keyword));
        assert_eq!(role_of("\"s\""), Some(Role::String));
        assert_eq!(role_of("hi"), Some(Role::Comment));
    }

    #[test]
    fn marks_only_the_words_that_changed() {
        let (a, b) = word_diff("let total = price * qty;", "let total = price * quantity;").unwrap();
        assert_eq!(&"let total = price * qty;"[a[0].clone()], "qty");
        assert_eq!(&"let total = price * quantity;"[b[0].clone()], "quantity");
        assert!(word_diff("completely different", "nothing alike at all here").is_none());
    }

    #[test]
    fn emphasis_splits_segments() {
        let mut segs = vec![
            Seg { text: "let ".into(), role: Role::Keyword, emph: false },
            Seg { text: "abc".into(), role: Role::Text, emph: false },
        ];
        let ranges: Ranges = std::iter::once(5..7).collect();
        emphasize(&mut segs, &ranges);
        let parts: Vec<(&str, bool)> = segs.iter().map(|s| (s.text.as_str(), s.emph)).collect();
        assert_eq!(parts, [("let ", false), ("a", false), ("bc", true)]);
    }
}
