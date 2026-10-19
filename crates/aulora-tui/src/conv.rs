//! The Conversation tab: a session's whole transcript, readable and
//! scrollable.
//!
//! The dashboard reads the transcript file itself (it's on the same
//! machine) and keeps a byte offset, so a long session is parsed once and
//! then only new lines are read.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

use aulora_core::model::{Ms, Session};
use aulora_core::reduce::{tool_label, tool_target};
use aulora_core::util::{first_line, parse_iso_ms, squash, truncate};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;
use unicode_width::UnicodeWidthStr;

use crate::theme::{self, bold, dim, faint, fg, text};

#[derive(Debug, Clone, PartialEq)]
pub enum Entry {
    You {
        at: Option<Ms>,
        text: String,
    },
    Claude {
        at: Option<Ms>,
        text: String,
    },
    Tool {
        at: Option<Ms>,
        id: String,
        name: String,
        target: String,
        input: String,
        result: Option<String>,
        error: bool,
    },
    Note {
        at: Option<Ms>,
        text: String,
    },
}

impl Entry {
    pub fn is_prompt(&self) -> bool {
        matches!(self, Entry::You { .. })
    }

    /// Plain text for copying and searching.
    pub fn plain(&self) -> String {
        match self {
            Entry::You { text, .. } | Entry::Claude { text, .. } | Entry::Note { text, .. } => text.clone(),
            Entry::Tool { name, target, input, result, .. } => {
                format!("{name} {target}\n{input}\n{}", result.as_deref().unwrap_or(""))
            }
        }
    }
}

#[derive(Default)]
pub struct Conversation {
    pub path: Option<String>,
    offset: u64,
    partial: Vec<u8>,
    cwd: String,
    last_msg: Option<String>,
    pub entries: Vec<Entry>,
    tools: HashMap<String, usize>,
    /// Selected entry when the tab has focus.
    pub selected: usize,
    /// First rendered line shown.
    pub scroll: usize,
    /// Stick to the bottom as new messages arrive.
    pub follow: bool,
    pub expanded: HashSet<usize>,
    /// Entries that arrived while scrolled up.
    pub unseen: usize,
    cache: Option<(u16, usize, usize, Vec<Rendered>)>,
    pub search: String,
}

/// One rendered line and the entry it belongs to.
#[derive(Clone)]
pub struct Rendered {
    pub line: Line<'static>,
    pub entry: usize,
}

fn text_of(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| match p.get("type").and_then(Value::as_str) {
                Some("text") => p.get("text").and_then(Value::as_str).map(str::to_string),
                Some("image") => Some("[image]".into()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn short_input(name: &str, input: &Value) -> String {
    let pick = |k: &str| input.get(k).and_then(Value::as_str).map(str::to_string);
    let s = match name {
        "Bash" => pick("command"),
        "Edit" => Some(format!(
            "- {}\n+ {}",
            truncate(&squash(&pick("old_string").unwrap_or_default()), 300),
            truncate(&squash(&pick("new_string").unwrap_or_default()), 300)
        )),
        "Write" => pick("content").map(|c| truncate(&c, 600)),
        "Task" | "Agent" => pick("prompt"),
        _ => None,
    };
    s.unwrap_or_else(|| truncate(&input.to_string(), 600))
}

impl Conversation {
    pub fn new(path: Option<String>) -> Self {
        Conversation { path, follow: true, ..Default::default() }
    }

    /// Entries made up from what the snapshot knows, for sessions without a
    /// readable transcript (the demo, or before the file appears).
    pub fn from_session(s: &Session) -> Self {
        let mut c = Conversation::new(None);
        for p in &s.prompts {
            c.entries.push(Entry::You { at: Some(p.at), text: p.text.clone() });
        }
        if c.entries.is_empty()
            && let Some(p) = &s.last_prompt
        {
            c.entries.push(Entry::You { at: s.last_prompt_at, text: p.clone() });
        }
        for t in s.recent_tools.iter().rev() {
            c.entries.push(Entry::Tool {
                at: Some(t.started_at),
                id: t.id.clone().unwrap_or_default(),
                name: t.name.clone(),
                target: t.target.clone(),
                input: String::new(),
                result: t.ok.map(|_| String::new()),
                error: t.ok == Some(false),
            });
        }
        if let Some(text) = s.live_text.clone().or_else(|| s.last_reply.clone()) {
            c.entries.push(Entry::Claude { at: s.last_reply_at, text });
        }
        c.selected = c.entries.len().saturating_sub(1);
        c
    }

    /// Read whatever was appended since last time. Returns true if new
    /// entries arrived.
    pub fn refresh(&mut self) -> bool {
        let Some(path) = self.path.clone() else { return false };
        let Ok(mut f) = File::open(&path) else { return false };
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        if len < self.offset {
            *self = Conversation::new(Some(path));
        }
        if len == self.offset {
            return false;
        }
        if f.seek(SeekFrom::Start(self.offset)).is_err() {
            return false;
        }
        let mut buf = Vec::new();
        if f.take(len - self.offset).read_to_end(&mut buf).is_err() {
            return false;
        }
        self.offset += buf.len() as u64;
        let mut data = std::mem::take(&mut self.partial);
        data.extend_from_slice(&buf);
        let before = self.entries.len();
        let mut start = 0;
        for i in 0..data.len() {
            if data[i] == b'\n' {
                self.line(&data[start..i]);
                start = i + 1;
            }
        }
        self.partial = data[start..].to_vec();
        let added = self.entries.len() - before;
        if added > 0 {
            self.cache = None;
            if self.follow {
                self.selected = self.entries.len() - 1;
            } else {
                self.unseen += added;
            }
        }
        added > 0
    }

    fn line(&mut self, line: &[u8]) {
        let head = &line[..line.len().min(48)];
        if head.windows(11).any(|w| w == b"\"api-reques") {
            return;
        }
        let Ok(v) = serde_json::from_slice::<Value>(line) else { return };
        if v.get("isSidechain").and_then(Value::as_bool) == Some(true) {
            return;
        }
        if let Some(c) = v.get("cwd").and_then(Value::as_str) {
            self.cwd = c.to_string();
        }
        let at = v.get("timestamp").and_then(Value::as_str).and_then(parse_iso_ms);
        let Some(msg) = v.get("message") else {
            if v.get("type").and_then(Value::as_str) == Some("system")
                && v.get("subtype").and_then(Value::as_str) == Some("compact_boundary")
            {
                self.entries.push(Entry::Note { at, text: "context compacted".into() });
            }
            return;
        };
        match v.get("type").and_then(Value::as_str) {
            Some("user") => {
                if v.get("isMeta").and_then(Value::as_bool) == Some(true) {
                    return;
                }
                let content = msg.get("content").cloned().unwrap_or(Value::Null);
                if let Value::Array(parts) = &content {
                    let mut any_result = false;
                    for p in parts.iter().filter(|p| p.get("type").and_then(Value::as_str) == Some("tool_result")) {
                        any_result = true;
                        let id = p.get("tool_use_id").and_then(Value::as_str).unwrap_or("");
                        let out = text_of(p.get("content").unwrap_or(&Value::Null));
                        let err = p.get("is_error").and_then(Value::as_bool).unwrap_or(false);
                        if let Some(&i) = self.tools.get(id)
                            && let Some(Entry::Tool { result, error, .. }) = self.entries.get_mut(i)
                        {
                            *result = Some(truncate(&out, 4000));
                            *error = err;
                        }
                    }
                    if any_result {
                        return;
                    }
                }
                let t = text_of(&content);
                let t = t.trim();
                if t.is_empty() || t.starts_with("<command-") || t.starts_with("<local-command") {
                    return;
                }
                if t.starts_with("[Request interrupted") {
                    self.entries.push(Entry::Note { at, text: "interrupted".into() });
                    return;
                }
                self.entries.push(Entry::You { at, text: t.to_string() });
            }
            Some("assistant") => {
                let id = msg.get("id").and_then(Value::as_str).map(str::to_string);
                let same_msg = id.is_some() && id == self.last_msg;
                self.last_msg = id;
                let Some(blocks) = msg.get("content").and_then(Value::as_array) else { return };
                for b in blocks {
                    match b.get("type").and_then(Value::as_str) {
                        Some("text") => {
                            let t = b.get("text").and_then(Value::as_str).unwrap_or("").trim();
                            if t.is_empty() {
                                continue;
                            }
                            // Text split across lines of one message joins up.
                            if same_msg && let Some(Entry::Claude { text, .. }) = self.entries.last_mut() {
                                text.push_str("\n\n");
                                text.push_str(t);
                                continue;
                            }
                            self.entries.push(Entry::Claude { at, text: t.to_string() });
                        }
                        Some("tool_use") => {
                            let name = b.get("name").and_then(Value::as_str).unwrap_or("tool").to_string();
                            let input = b.get("input").cloned().unwrap_or(Value::Null);
                            let id = b.get("id").and_then(Value::as_str).unwrap_or("").to_string();
                            self.tools.insert(id.clone(), self.entries.len());
                            self.entries.push(Entry::Tool {
                                at,
                                id,
                                target: tool_target(&name, &input, &self.cwd),
                                input: short_input(&name, &input),
                                name,
                                result: None,
                                error: false,
                            });
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    // --- navigation -----------------------------------------------------

    pub fn select(&mut self, i: usize) {
        if self.entries.is_empty() {
            return;
        }
        self.selected = i.min(self.entries.len() - 1);
        self.follow = self.selected + 1 == self.entries.len();
        if self.follow {
            self.unseen = 0;
        }
    }

    pub fn move_by(&mut self, delta: i64) {
        let i = (self.selected as i64 + delta).max(0) as usize;
        self.select(i);
    }

    pub fn jump_prompt(&mut self, forward: bool) {
        let n = self.entries.len();
        let found = if forward {
            (self.selected + 1..n).find(|&i| self.entries[i].is_prompt())
        } else {
            (0..self.selected).rev().find(|&i| self.entries[i].is_prompt())
        };
        if let Some(i) = found {
            self.select(i);
        }
    }

    pub fn select_last(&mut self) {
        self.select(self.entries.len().saturating_sub(1));
    }

    pub fn toggle(&mut self) {
        if matches!(self.entries.get(self.selected), Some(Entry::Tool { .. })) {
            if !self.expanded.remove(&self.selected) {
                self.expanded.insert(self.selected);
            }
            self.cache = None;
        }
    }

    /// Select the next entry containing the search text.
    pub fn find_next(&mut self) -> bool {
        let q = self.search.to_lowercase();
        if q.is_empty() {
            return false;
        }
        let n = self.entries.len();
        for step in 1..=n {
            let i = (self.selected + step) % n;
            if self.entries[i].plain().to_lowercase().contains(&q) {
                self.select(i);
                return true;
            }
        }
        false
    }

    // --- rendering ------------------------------------------------------

    pub fn render(&mut self, width: u16) -> &[Rendered] {
        let key = (width, self.entries.len(), self.expanded.len());
        let stale = !matches!(&self.cache, Some((w, n, e, _)) if (*w, *n, *e) == key);
        if stale {
            let lines = render_entries(&self.entries, &self.expanded, width as usize);
            self.cache = Some((width, key.1, key.2, lines));
        }
        &self.cache.as_ref().expect("cache").3
    }
}

fn hm(ms: Option<Ms>) -> String {
    ms.and_then(chrono::DateTime::from_timestamp_millis)
        .map(|d| d.with_timezone(&chrono::Local).format("%H:%M").to_string())
        .unwrap_or_default()
}

/// Word-wrap to `width` columns, hard-breaking very long words.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(8);
    let mut out = Vec::new();
    for para in text.lines() {
        if para.trim().is_empty() {
            out.push(String::new());
            continue;
        }
        let indent: String = para.chars().take_while(|c| *c == ' ').collect();
        let mut line = indent.clone();
        for word in para.split_whitespace() {
            let lw = UnicodeWidthStr::width(line.as_str());
            let ww = UnicodeWidthStr::width(word);
            if lw > indent.len() && lw + 1 + ww > width {
                out.push(std::mem::replace(&mut line, indent.clone()));
            }
            if ww > width {
                let mut chunk = String::new();
                for ch in word.chars() {
                    if UnicodeWidthStr::width(chunk.as_str()) + 1 > width {
                        out.push(std::mem::take(&mut chunk));
                    }
                    chunk.push(ch);
                }
                line.push_str(&chunk);
                continue;
            }
            if UnicodeWidthStr::width(line.as_str()) > indent.len() {
                line.push(' ');
            }
            line.push_str(word);
        }
        out.push(line);
    }
    while out.last().is_some_and(|l| l.trim().is_empty()) {
        out.pop();
    }
    out
}

/// **bold** and `code` inside one line of text.
fn inline(line: &str, base: Style) -> Vec<Span<'static>> {
    let t = theme::get();
    let mut spans = Vec::new();
    let mut rest = line;
    let mut bold_on = false;
    while !rest.is_empty() {
        let next_bold = rest.find("**");
        let next_code = rest.find('`');
        match (next_bold, next_code) {
            (Some(b), c) if c.is_none_or(|c| b < c) => {
                if b > 0 {
                    spans.push(Span::styled(
                        rest[..b].to_string(),
                        if bold_on { base.add_modifier(Modifier::BOLD) } else { base },
                    ));
                }
                bold_on = !bold_on;
                rest = &rest[b + 2..];
            }
            (_, Some(c)) => {
                if c > 0 {
                    spans.push(Span::styled(
                        rest[..c].to_string(),
                        if bold_on { base.add_modifier(Modifier::BOLD) } else { base },
                    ));
                }
                let after = &rest[c + 1..];
                match after.find('`') {
                    Some(e) => {
                        spans.push(Span::styled(after[..e].to_string(), Style::new().fg(t.cyan)));
                        rest = &after[e + 1..];
                    }
                    None => {
                        spans.push(Span::styled(format!("`{after}"), base));
                        rest = "";
                    }
                }
            }
            _ => {
                spans.push(Span::styled(
                    rest.to_string(),
                    if bold_on { base.add_modifier(Modifier::BOLD) } else { base },
                ));
                rest = "";
            }
        }
    }
    spans
}

/// Light markdown: headings, bullets, code blocks, **bold**, `code`.
fn markdown(body: &str, width: usize, indent: &str) -> Vec<Line<'static>> {
    let t = theme::get();
    let mut out = Vec::new();
    let mut in_code = false;
    for raw in body.lines() {
        let trimmed = raw.trim_start();
        if trimmed.starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code {
            let code_style = Style::new().fg(t.dim).bg(t.raised);
            for chunk in wrap(raw, width.saturating_sub(indent.len() + 2)) {
                let pad = width.saturating_sub(indent.len() + UnicodeWidthStr::width(chunk.as_str()) + 1);
                out.push(Line::from(vec![
                    Span::raw(indent.to_string()),
                    Span::styled(format!(" {chunk}{}", " ".repeat(pad)), code_style),
                ]));
            }
            continue;
        }
        if let Some(h) = trimmed.strip_prefix('#') {
            let h = h.trim_start_matches('#').trim();
            for chunk in wrap(h, width.saturating_sub(indent.len())) {
                out.push(Line::from(vec![Span::raw(indent.to_string()), Span::styled(chunk, bold(t.text))]));
            }
            continue;
        }
        let (bullet, body) = if let Some(b) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")) {
            ("• ", b)
        } else {
            ("", raw)
        };
        let lead = raw.len() - trimmed.len();
        let sub_indent = format!(
            "{indent}{}{}",
            " ".repeat(if bullet.is_empty() { 0 } else { lead }),
            if bullet.is_empty() { "" } else { "  " }
        );
        let chunks = wrap(body, width.saturating_sub(sub_indent.len()).max(8));
        if chunks.is_empty() {
            out.push(Line::raw(""));
        }
        for (i, chunk) in chunks.into_iter().enumerate() {
            let mut spans = vec![Span::raw(if i == 0 && !bullet.is_empty() {
                format!("{indent}{}{bullet}", " ".repeat(lead))
            } else if bullet.is_empty() {
                indent.to_string()
            } else {
                sub_indent.clone()
            })];
            spans.extend(inline(chunk.trim_start_matches(' '), text()));
            out.push(Line::from(spans));
        }
    }
    out
}

fn render_entries(entries: &[Entry], expanded: &HashSet<usize>, width: usize) -> Vec<Rendered> {
    let t = theme::get();
    let mut out: Vec<Rendered> = Vec::new();
    let mut push = |line: Line<'static>, entry: usize| out.push(Rendered { line, entry });
    let mut prev_was_claude = false;
    for (i, e) in entries.iter().enumerate() {
        match e {
            Entry::You { at, text: body } => {
                if i > 0 {
                    push(Line::raw(""), i);
                }
                push(
                    Line::from(vec![
                        Span::styled(" YOU", bold(t.accent)),
                        Span::styled(format!("  {}", hm(*at)), faint()),
                    ]),
                    i,
                );
                for l in markdown(body, width.saturating_sub(1), "  ") {
                    push(l, i);
                }
                prev_was_claude = false;
            }
            Entry::Claude { at, text: body } => {
                if !prev_was_claude {
                    push(Line::raw(""), i);
                    push(
                        Line::from(vec![
                            Span::styled(" CLAUDE", bold(t.magenta)),
                            Span::styled(format!("  {}", hm(*at)), faint()),
                        ]),
                        i,
                    );
                } else {
                    push(Line::raw(""), i);
                }
                for l in markdown(body, width.saturating_sub(1), "  ") {
                    push(l, i);
                }
                prev_was_claude = true;
            }
            Entry::Tool { name, target, input, result, error, .. } => {
                if !prev_was_claude {
                    push(Line::raw(""), i);
                    push(Line::from(Span::styled(" CLAUDE", bold(t.magenta))), i);
                }
                let open = expanded.contains(&i);
                let (mark, mark_style) = match (result, error) {
                    (_, true) => ("✗", fg(t.red)),
                    (Some(_), false) => ("✓", fg(t.green)),
                    (None, false) => ("…", faint()),
                };
                let summary = result.as_deref().map(first_line).map(|l| truncate(l, 60)).unwrap_or_default();
                let head = vec![
                    Span::styled(if open { "   ▾ " } else { "   ▸ " }, faint()),
                    Span::styled(format!("{:<6}", tool_label(name)), fg(t.cyan)),
                    Span::styled(format!(" {}", truncate(target, width.saturating_sub(30))), dim()),
                    Span::styled(format!("  {mark}"), mark_style),
                    Span::styled(if summary.is_empty() { String::new() } else { format!(" {summary}") }, faint()),
                ];
                push(Line::from(head), i);
                if open {
                    for (label, body) in
                        [("in ", input.as_str()), ("out", result.as_deref().unwrap_or("(no output yet)"))]
                    {
                        for (j, l) in wrap(body, width.saturating_sub(12)).into_iter().take(30).enumerate() {
                            let lead = if j == 0 { format!("       {label} ") } else { "           ".to_string() };
                            push(Line::from(vec![Span::styled(lead, faint()), Span::styled(l, dim())]), i);
                        }
                    }
                }
                prev_was_claude = true;
            }
            Entry::Note { text: body, .. } => {
                push(Line::from(Span::styled(format!("   ─ {body} ─"), faint())), i);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn reads_turns_tools_and_results() {
        let dir = std::env::temp_dir().join(format!("aulora-conv-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.jsonl");
        let mut f = File::create(&path).unwrap();
        let lines = [
            r#"{"type":"user","timestamp":"2026-10-19T10:00:00Z","cwd":"/p","message":{"role":"user","content":"fix the **bug**"}}"#,
            r#"{"type":"assistant","message":{"id":"m1","content":[{"type":"text","text":"Looking."}]}}"#,
            r#"{"type":"assistant","message":{"id":"m1","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"pytest -q"}}]}}"#,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"3 passed","is_error":false}]}}"#,
            r#"{"type":"assistant","message":{"id":"m2","content":[{"type":"text","text":"Fixed."}]}}"#,
            r#"{"type":"user","isSidechain":true,"message":{"role":"user","content":"ignored"}}"#,
        ];
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
        f.flush().unwrap();
        let mut c = Conversation::new(Some(path.to_string_lossy().to_string()));
        assert!(c.refresh());
        assert_eq!(c.entries.len(), 4);
        assert!(matches!(&c.entries[2], Entry::Tool { result: Some(r), error: false, .. } if r == "3 passed"));
        assert_eq!(c.selected, 3, "follows the latest entry");

        c.select(0);
        assert!(!c.follow);
        writeln!(f, r#"{{"type":"user","message":{{"role":"user","content":"thanks"}}}}"#).unwrap();
        f.flush().unwrap();
        assert!(c.refresh());
        assert_eq!(c.unseen, 1);
        c.jump_prompt(true);
        assert_eq!(c.selected, 4);

        let rendered = c.render(60).len();
        c.selected = 2;
        c.toggle();
        assert!(c.render(60).len() > rendered, "expanding a tool shows its input and output");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn wraps_and_marks_up() {
        assert_eq!(wrap("aaa bbb ccc", 8), vec!["aaa bbb", "ccc"]);
        let spans = inline("a **b** `c`", text());
        let texts: Vec<&str> = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(texts, ["a ", "b", " ", "c"]);
    }
}
