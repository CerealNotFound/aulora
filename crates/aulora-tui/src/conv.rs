//! The Conversation tab: a session's whole transcript, drawn the way Claude
//! Code draws it, and scrolled by line.
//!
//! Each entry is wrapped once per width and cached; a prefix sum of row
//! counts maps rows to entries, so a frame only touches the visible lines
//! (codex, gitui). Positions are anchored to content (entry, row), so new
//! messages never move what you're reading.

use std::collections::HashSet;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

use aulora_core::model::{FeedEvent, FeedKind, Ms, Session};
use aulora_core::reduce::{tool_label, tool_target};
use aulora_core::util::{parse_iso_ms, squash, truncate};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;
use unicode_width::UnicodeWidthStr;

use crate::theme::{self, bold, dim, faint, fg, text};

const PREVIEW_LINES: usize = 3;
const EXPANDED_LINES: usize = 400;

#[derive(Debug, Clone, PartialEq)]
pub struct Tool {
    pub id: String,
    pub name: String,
    pub target: String,
    /// What it ran: a command, a prompt…
    pub input: String,
    /// For edits: the text replaced and the replacement.
    pub old: String,
    pub new: String,
    pub result: Option<String>,
    pub error: bool,
    pub expanded: bool,
    /// Pieced together without a transcript: no output to show.
    pub sparse: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    You(String),
    Claude(String),
    Tool(Box<Tool>),
    Note(FeedKind, String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub at: Option<Ms>,
    pub body: Body,
    /// Bumped whenever the entry changes, so its wrapped lines are redone.
    rev: u32,
}

impl Entry {
    fn new(at: Option<Ms>, body: Body) -> Self {
        Entry { at, body, rev: 0 }
    }

    pub fn is_prompt(&self) -> bool {
        matches!(self.body, Body::You(_))
    }

    /// Plain text, for copying and searching.
    pub fn plain(&self) -> String {
        match &self.body {
            Body::You(t) | Body::Claude(t) | Body::Note(_, t) => t.clone(),
            Body::Tool(tool) => {
                format!("{}({})\n{}\n{}", tool.name, tool.target, tool.input, tool.result.as_deref().unwrap_or(""))
            }
        }
    }
}

/// A position tied to content: an entry and a row inside it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Anchor {
    pub entry: usize,
    pub row: usize,
}

#[derive(Default)]
pub struct Conversation {
    pub path: Option<String>,
    offset: u64,
    partial: Vec<u8>,
    cwd: String,
    last_msg: Option<String>,
    pub entries: Vec<Entry>,
    notes_seen: HashSet<(Ms, String)>,

    // Layout.
    width: u16,
    /// Wrapped lines, keyed by the entry's rev and whether it ends a turn.
    wrapped: Vec<Option<(u64, Vec<Line<'static>>)>>,
    /// prefix[i] = rows before entry i; len = entries + 1 when valid.
    prefix: Vec<usize>,

    // View.
    pub top: Anchor,
    pub cursor: Anchor,
    /// Stick to the bottom as new messages arrive.
    pub follow: bool,
    /// Entries that arrived while scrolled up.
    pub unseen: usize,
    pub search: String,
    /// Claude is mid-turn, so the last entry doesn't end a turn yet.
    pub working: bool,
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

fn input_summary(name: &str, input: &Value) -> String {
    let pick = |k: &str| input.get(k).and_then(Value::as_str).map(str::to_string);
    match name {
        "Bash" | "PowerShell" => pick("command"),
        "Task" | "Agent" => pick("prompt"),
        "Write" => pick("content"),
        "WebSearch" => pick("query"),
        "WebFetch" => pick("prompt"),
        _ => None,
    }
    .map(|s| truncate(&s, 4000))
    .unwrap_or_else(|| truncate(&input.to_string(), 600))
}

impl Conversation {
    pub fn new(path: Option<String>) -> Self {
        Conversation { path, follow: true, ..Default::default() }
    }

    /// Entries pieced together from the snapshot, for sessions without a
    /// readable transcript (the demo, or before the file appears).
    pub fn from_session(s: &Session) -> Self {
        let mut c = Conversation::new(None);
        for p in &s.prompts {
            c.push(Entry::new(Some(p.at), Body::You(p.text.clone())));
        }
        if c.entries.is_empty()
            && let Some(p) = &s.last_prompt
        {
            c.push(Entry::new(s.last_prompt_at, Body::You(p.clone())));
        }
        for t in s.recent_tools.iter().rev() {
            c.push(Entry::new(
                Some(t.started_at),
                Body::Tool(Box::new(Tool {
                    id: t.id.clone().unwrap_or_default(),
                    name: t.name.clone(),
                    target: t.target.clone(),
                    input: t.target.trim_start_matches("$ ").to_string(),
                    old: String::new(),
                    new: String::new(),
                    result: t.ok.map(|_| String::new()),
                    error: t.ok == Some(false),
                    expanded: false,
                    sparse: true,
                })),
            ));
        }
        if let Some(text) = s.live_text.clone().or_else(|| s.last_reply.clone()) {
            c.push(Entry::new(s.last_reply_at, Body::Claude(text)));
        }
        c
    }

    fn push(&mut self, e: Entry) {
        self.entries.push(e);
        self.wrapped.push(None);
    }

    /// Keep the view position and expanded items of an older copy.
    pub fn adopt_view(&mut self, old: &Conversation) {
        self.top = old.top;
        self.cursor = old.cursor;
        self.follow = old.follow;
        for (e, o) in self.entries.iter_mut().zip(&old.entries) {
            if let (Body::Tool(a), Body::Tool(b)) = (&mut e.body, &o.body) {
                a.expanded = b.expanded;
            }
        }
    }

    /// Read whatever was appended since last time. Returns true if entries changed.
    pub fn refresh(&mut self) -> bool {
        let Some(path) = self.path.clone() else { return false };
        let Ok(mut f) = File::open(&path) else { return false };
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        if len < self.offset {
            *self = Conversation::new(Some(path));
        }
        if len == self.offset || f.seek(SeekFrom::Start(self.offset)).is_err() {
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
        let mut changed = false;
        let mut start = 0;
        for i in 0..data.len() {
            if data[i] == b'\n' {
                changed |= self.line(&data[start..i]);
                start = i + 1;
            }
        }
        self.partial = data[start..].to_vec();
        if !self.follow {
            self.unseen += self.entries.len() - before;
        }
        changed
    }

    /// Fold hook events (permissions, subagents, compaction…) into the
    /// conversation at the time they happened.
    pub fn add_notes(&mut self, events: &[&FeedEvent]) {
        for e in events {
            if !matches!(
                e.kind,
                FeedKind::Permission
                    | FeedKind::Question
                    | FeedKind::Agent
                    | FeedKind::Compact
                    | FeedKind::Start
                    | FeedKind::End
                    | FeedKind::Error
            ) {
                continue;
            }
            if !self.notes_seen.insert((e.at, e.text.clone())) {
                continue;
            }
            // After every entry that happened at or before it.
            let pos = self.entries.iter().rposition(|x| x.at.is_none_or(|a| a <= e.at)).map(|i| i + 1).unwrap_or(0);
            let at_end = pos == self.entries.len();
            self.entries.insert(pos, Entry::new(Some(e.at), Body::Note(e.kind, e.text.clone())));
            self.wrapped.insert(pos, None);
            self.prefix.truncate(pos + 1);
            if !at_end {
                for a in [&mut self.top, &mut self.cursor] {
                    if a.entry >= pos {
                        a.entry += 1;
                    }
                }
            }
            // The entry after a note may change its spacing.
            if let Some(w) = self.wrapped.get_mut(pos + 1) {
                *w = None;
            }
        }
    }

    fn tool_mut(&mut self, id: &str) -> Option<&mut Entry> {
        self.entries.iter_mut().rev().find(|e| matches!(&e.body, Body::Tool(t) if t.id == id))
    }

    /// Parse one transcript line. Returns true if entries changed.
    fn line(&mut self, line: &[u8]) -> bool {
        let head = &line[..line.len().min(48)];
        if head.windows(11).any(|w| w == b"\"api-reques") {
            return false;
        }
        let Ok(v) = serde_json::from_slice::<Value>(line) else { return false };
        if v.get("isSidechain").and_then(Value::as_bool) == Some(true) {
            return false;
        }
        if let Some(c) = v.get("cwd").and_then(Value::as_str) {
            self.cwd = c.to_string();
        }
        let at = v.get("timestamp").and_then(Value::as_str).and_then(parse_iso_ms);
        let Some(msg) = v.get("message") else {
            if v.get("type").and_then(Value::as_str) == Some("system")
                && v.get("subtype").and_then(Value::as_str) == Some("compact_boundary")
            {
                self.push(Entry::new(at, Body::Note(FeedKind::Compact, "context compacted".into())));
                return true;
            }
            return false;
        };
        match v.get("type").and_then(Value::as_str) {
            Some("user") => {
                if v.get("isMeta").and_then(Value::as_bool) == Some(true) {
                    return false;
                }
                let content = msg.get("content").cloned().unwrap_or(Value::Null);
                if let Value::Array(parts) = &content {
                    let mut any = false;
                    for p in parts.iter().filter(|p| p.get("type").and_then(Value::as_str) == Some("tool_result")) {
                        any = true;
                        let id = p.get("tool_use_id").and_then(Value::as_str).unwrap_or("").to_string();
                        let out = text_of(p.get("content").unwrap_or(&Value::Null));
                        let err = p.get("is_error").and_then(Value::as_bool).unwrap_or(false);
                        if let Some(e) = self.tool_mut(&id) {
                            if let Body::Tool(t) = &mut e.body {
                                t.result = Some(truncate(&out, 20_000));
                                t.error = err;
                            }
                            e.rev += 1;
                        }
                    }
                    if any {
                        return true;
                    }
                }
                let t = text_of(&content);
                let t = t.trim();
                if t.is_empty() || t.starts_with("<command-") || t.starts_with("<local-command") {
                    return false;
                }
                if t.starts_with("[Request interrupted") {
                    self.push(Entry::new(at, Body::Note(FeedKind::Info, "interrupted by you".into())));
                } else {
                    self.push(Entry::new(at, Body::You(t.to_string())));
                }
                true
            }
            Some("assistant") => {
                let id = msg.get("id").and_then(Value::as_str).map(str::to_string);
                let same_msg = id.is_some() && id == self.last_msg;
                self.last_msg = id;
                let Some(blocks) = msg.get("content").and_then(Value::as_array) else { return false };
                let mut changed = false;
                for b in blocks {
                    match b.get("type").and_then(Value::as_str) {
                        Some("text") => {
                            let t = b.get("text").and_then(Value::as_str).unwrap_or("").trim();
                            if t.is_empty() {
                                continue;
                            }
                            changed = true;
                            if same_msg
                                && let Some(last) = self.entries.last_mut()
                                && let Body::Claude(text) = &mut last.body
                            {
                                text.push_str("\n\n");
                                text.push_str(t);
                                last.rev += 1;
                                continue;
                            }
                            self.push(Entry::new(at, Body::Claude(t.to_string())));
                        }
                        Some("tool_use") => {
                            changed = true;
                            let name = b.get("name").and_then(Value::as_str).unwrap_or("tool").to_string();
                            let input = b.get("input").cloned().unwrap_or(Value::Null);
                            let s = |k: &str| input.get(k).and_then(Value::as_str).unwrap_or("").to_string();
                            let (old, new) = match name.as_str() {
                                "Edit" => (s("old_string"), s("new_string")),
                                "Write" => (String::new(), s("content")),
                                _ => (String::new(), String::new()),
                            };
                            self.push(Entry::new(
                                at,
                                Body::Tool(Box::new(Tool {
                                    id: b.get("id").and_then(Value::as_str).unwrap_or("").to_string(),
                                    target: tool_target(&name, &input, &self.cwd),
                                    input: input_summary(&name, &input),
                                    old: truncate(&old, 8_000),
                                    new: truncate(&new, 8_000),
                                    name,
                                    result: None,
                                    error: false,
                                    expanded: false,
                                    sparse: false,
                                })),
                            ));
                        }
                        _ => {}
                    }
                }
                changed
            }
            _ => false,
        }
    }

    // --- layout -----------------------------------------------------------

    /// Wrap whatever changed and rebuild row offsets. Cheap when nothing did.
    pub fn layout(&mut self, width: u16) {
        if width != self.width {
            self.width = width;
            self.wrapped.iter_mut().for_each(|w| *w = None);
            self.prefix.clear();
        }
        let mut first_dirty = self.prefix.len().saturating_sub(1).min(self.entries.len());
        for i in 0..self.entries.len() {
            let ends_turn = match self.entries.get(i + 1) {
                Some(next) => next.is_prompt(),
                None => !self.working,
            };
            let key = ((self.entries[i].rev as u64) << 1) | ends_turn as u64;
            let stale = !matches!(&self.wrapped[i], Some((r, _)) if *r == key);
            if stale {
                let mut lines = render_entry(&self.entries[i], i, &self.entries, width as usize);
                if ends_turn {
                    lines.extend(recap(&self.entries[..=i]));
                }
                self.wrapped[i] = Some((key, lines));
                first_dirty = first_dirty.min(i);
            }
        }
        if self.prefix.len() != self.entries.len() + 1 || first_dirty < self.entries.len() {
            self.prefix.truncate(first_dirty + 1);
            if self.prefix.is_empty() {
                self.prefix.push(0);
            }
            for i in self.prefix.len() - 1..self.entries.len() {
                let rows = self.wrapped[i].as_ref().map(|w| w.1.len()).unwrap_or(0);
                let next = self.prefix[i] + rows;
                self.prefix.push(next);
            }
        }
    }

    pub fn total_rows(&self) -> usize {
        self.prefix.last().copied().unwrap_or(0)
    }

    fn row(&self, a: Anchor) -> usize {
        let total = self.total_rows();
        let base = self.prefix.get(a.entry).copied().unwrap_or(total);
        let len = self.wrapped.get(a.entry).and_then(|w| w.as_ref()).map(|w| w.1.len()).unwrap_or(1);
        (base + a.row.min(len.saturating_sub(1))).min(total.saturating_sub(1))
    }

    fn anchor(&self, row: usize) -> Anchor {
        if self.entries.is_empty() || self.prefix.len() < 2 {
            return Anchor::default();
        }
        let entry = self.prefix.partition_point(|&p| p <= row).saturating_sub(1).min(self.entries.len() - 1);
        Anchor { entry, row: row.saturating_sub(self.prefix[entry]) }
    }

    /// Fix up the view for a viewport of `height` rows; returns the top row.
    pub fn settle(&mut self, height: usize) -> usize {
        let total = self.total_rows();
        let max_top = total.saturating_sub(height);
        if self.follow {
            self.top = self.anchor(max_top);
            self.cursor = self.anchor(total.saturating_sub(1));
            self.unseen = 0;
            return max_top;
        }
        let top = self.row(self.top).min(max_top);
        self.top = self.anchor(top);
        top
    }

    /// Scroll the viewport; the cursor stays on screen.
    pub fn scroll_by(&mut self, delta: isize, height: usize) {
        let total = self.total_rows();
        let max_top = total.saturating_sub(height);
        let top = if self.follow { max_top } else { self.row(self.top) };
        let next = (top as isize + delta).clamp(0, max_top as isize) as usize;
        self.follow = next >= max_top && delta > 0;
        self.top = self.anchor(next);
        let cur = self.row(self.cursor).clamp(next, (next + height).saturating_sub(1));
        self.cursor = self.anchor(cur);
        if self.follow {
            self.unseen = 0;
        }
    }

    /// Move the cursor; the viewport follows with a small margin (vim's scrolloff).
    pub fn move_cursor(&mut self, delta: isize, height: usize) {
        let total = self.total_rows();
        if total == 0 {
            return;
        }
        let cur = if self.follow { total - 1 } else { self.row(self.cursor) };
        let next = (cur as isize + delta).clamp(0, total as isize - 1) as usize;
        self.set_cursor_row(next, height);
    }

    fn set_cursor_row(&mut self, row: usize, height: usize) {
        let total = self.total_rows();
        let max_top = total.saturating_sub(height);
        let margin = 3.min(height / 4);
        let mut top = if self.follow { max_top } else { self.row(self.top) };
        if row < top + margin {
            top = row.saturating_sub(margin);
        } else if row + margin >= top + height {
            top = (row + margin + 1).saturating_sub(height);
        }
        top = top.min(max_top);
        self.cursor = self.anchor(row);
        self.top = self.anchor(top);
        self.follow = row + 1 >= total && top >= max_top;
        if self.follow {
            self.unseen = 0;
        }
    }

    pub fn go_top(&mut self, height: usize) {
        self.follow = false;
        self.set_cursor_row(0, height);
    }

    pub fn go_bottom(&mut self) {
        self.follow = true;
        self.unseen = 0;
    }

    /// Put the cursor on screen row `y` (a click).
    pub fn click(&mut self, y: usize, height: usize) {
        let top = if self.follow { self.total_rows().saturating_sub(height) } else { self.row(self.top) };
        self.follow = false;
        self.top = self.anchor(top);
        let row = (top + y).min(self.total_rows().saturating_sub(1));
        self.cursor = self.anchor(row);
    }

    pub fn jump_prompt(&mut self, forward: bool, height: usize) {
        let cur = self.cursor.entry;
        let n = self.entries.len();
        let found = if forward {
            (cur + 1..n).find(|&i| self.entries[i].is_prompt())
        } else {
            (0..cur).rev().find(|&i| self.entries[i].is_prompt())
        };
        if let Some(i) = found {
            self.follow = false;
            let row = self.prefix.get(i).copied().unwrap_or(0) + usize::from(i > 0);
            self.set_cursor_row(row.min(self.total_rows().saturating_sub(1)), height);
        }
    }

    /// Expand or collapse the tool call at the cursor.
    pub fn toggle_at_cursor(&mut self) -> bool {
        let i = self.cursor.entry;
        if let Some(e) = self.entries.get_mut(i)
            && let Body::Tool(t) = &mut e.body
        {
            t.expanded = !t.expanded;
            e.rev += 1;
            self.cursor.row = self.cursor.row.min(1);
            self.follow = false;
            return true;
        }
        false
    }

    /// Move to the next entry containing the search text.
    pub fn find_next(&mut self, height: usize) -> bool {
        let q = self.search.to_lowercase();
        let n = self.entries.len();
        if q.is_empty() || n == 0 {
            return false;
        }
        for step in 1..=n {
            let i = (self.cursor.entry + step) % n;
            if self.entries[i].plain().to_lowercase().contains(&q) {
                self.follow = false;
                let row = self.prefix.get(i).copied().unwrap_or(0) + usize::from(i > 0);
                self.set_cursor_row(row.min(self.total_rows().saturating_sub(1)), height);
                return true;
            }
        }
        false
    }

    /// The rows from `top`, at most `height`, with the entry each belongs to.
    pub fn visible(&self, top: usize, height: usize) -> Vec<(usize, &Line<'static>)> {
        let mut out = Vec::with_capacity(height);
        if self.entries.is_empty() {
            return out;
        }
        let mut a = self.anchor(top);
        while out.len() < height && a.entry < self.entries.len() {
            if let Some((_, lines)) = &self.wrapped[a.entry] {
                for l in lines.iter().skip(a.row) {
                    if out.len() == height {
                        break;
                    }
                    out.push((a.entry, l));
                }
            }
            a = Anchor { entry: a.entry + 1, row: 0 };
        }
        out
    }

    pub fn cursor_row(&self) -> usize {
        self.row(self.cursor)
    }
}

// ---------------------------------------------------------------------------
// Rendering one entry

/// Word-wrap to `width` columns, keeping leading indentation and
/// hard-breaking very long words.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(8);
    let mut out = Vec::new();
    for para in text.lines() {
        let para = para.replace('\t', "    ");
        if para.trim().is_empty() {
            out.push(String::new());
            continue;
        }
        let indent: String = para.chars().take_while(|c| *c == ' ').collect();
        let indent = if indent.len() > width / 2 { String::new() } else { indent };
        let mut line = indent.clone();
        for word in para.split_whitespace() {
            let lw = UnicodeWidthStr::width(line.as_str());
            let ww = UnicodeWidthStr::width(word);
            if lw > indent.len() && lw + 1 + ww > width {
                out.push(std::mem::replace(&mut line, indent.clone()));
            }
            if ww > width.saturating_sub(indent.len()) {
                for ch in word.chars() {
                    if UnicodeWidthStr::width(line.as_str()) + 1 > width {
                        out.push(std::mem::replace(&mut line, indent.clone()));
                    }
                    line.push(ch);
                }
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
    let mut on = false;
    let style = |on: bool| if on { base.add_modifier(Modifier::BOLD) } else { base };
    while !rest.is_empty() {
        let b = rest.find("**");
        let c = rest.find('`');
        match (b, c) {
            (Some(b), c) if c.is_none_or(|c| b < c) => {
                if b > 0 {
                    spans.push(Span::styled(rest[..b].to_string(), style(on)));
                }
                on = !on;
                rest = &rest[b + 2..];
            }
            (_, Some(c)) => {
                if c > 0 {
                    spans.push(Span::styled(rest[..c].to_string(), style(on)));
                }
                let after = &rest[c + 1..];
                match after.find('`') {
                    Some(e) => {
                        spans.push(Span::styled(after[..e].to_string(), Style::new().fg(t.cyan)));
                        rest = &after[e + 1..];
                    }
                    None => {
                        spans.push(Span::styled(format!("`{after}"), style(on)));
                        rest = "";
                    }
                }
            }
            _ => {
                spans.push(Span::styled(rest.to_string(), style(on)));
                rest = "";
            }
        }
    }
    spans
}

/// Light markdown with a first-line prefix and an indent for the rest.
fn markdown(body: &str, width: usize, first: Span<'static>, rest: &'static str) -> Vec<Line<'static>> {
    let t = theme::get();
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut in_code = false;
    let inner = width.saturating_sub(rest.len()).max(8);
    let lead = |out: &Vec<Line<'static>>| if out.is_empty() { first.clone() } else { Span::raw(rest) };
    for raw in body.lines() {
        let trimmed = raw.trim_start();
        if trimmed.starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code {
            for chunk in wrap(raw, inner.saturating_sub(1)) {
                let pad = inner.saturating_sub(UnicodeWidthStr::width(chunk.as_str()) + 1);
                let l = lead(&out);
                let code = Span::styled(format!(" {chunk}{}", " ".repeat(pad)), Style::new().fg(t.dim).bg(t.raised));
                out.push(Line::from(vec![l, code]));
            }
            continue;
        }
        if let Some(h) = trimmed.strip_prefix('#') {
            let h = h.trim_start_matches('#').trim();
            for chunk in wrap(h, inner) {
                let l = lead(&out);
                out.push(Line::from(vec![l, Span::styled(chunk, bold(t.text))]));
            }
            continue;
        }
        let (bullet, body) = match trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")) {
            Some(b) => (true, b),
            None => (false, raw),
        };
        let depth = if bullet { (raw.len() - trimmed.len()).min(8) } else { 0 };
        let chunks = wrap(body, inner.saturating_sub(if bullet { depth + 2 } else { 0 }));
        if chunks.is_empty() {
            out.push(Line::raw(""));
        }
        for (i, chunk) in chunks.into_iter().enumerate() {
            let mut spans = vec![lead(&out)];
            if bullet {
                spans.push(Span::raw(format!("{}{}", " ".repeat(depth), if i == 0 { "• " } else { "  " })));
            }
            spans.extend(inline(chunk.trim_start_matches(' '), text()));
            out.push(Line::from(spans));
        }
    }
    if out.is_empty() {
        out.push(Line::from(first));
    }
    out
}

fn line_count(s: &str) -> usize {
    s.lines().count()
}

fn gutter(first: bool) -> Span<'static> {
    Span::styled(if first { "   ⎿  " } else { "      " }, faint())
}

/// Wrapped text under a tool call, at most `max` rows, then a "+n" marker.
fn push_text(out: &mut Vec<Line<'static>>, s: &str, style: Style, width: usize, max: usize) {
    let rows: Vec<String> = s.lines().flat_map(|l| wrap(l, width)).collect();
    let n = rows.len();
    for l in rows.into_iter().take(max) {
        let g = gutter(out.is_empty());
        out.push(Line::from(vec![g, Span::styled(l, style)]));
    }
    if n > max {
        out.push(Line::from(vec![gutter(false), Span::styled(format!("… +{} lines (⏎ to expand)", n - max), faint())]));
    }
}

/// The lines under a tool call, Claude Code style: `⎿` and a short preview.
fn tool_lines(tool: &Tool, width: usize) -> Vec<Line<'static>> {
    let t = theme::get();
    let inner = width.saturating_sub(7).max(10);
    let mut out: Vec<Line<'static>> = Vec::new();
    let max = if tool.expanded { EXPANDED_LINES } else { PREVIEW_LINES };
    let Some(result) = &tool.result else {
        out.push(Line::from(vec![gutter(true), Span::styled("running…", faint())]));
        return out;
    };
    if tool.sparse {
        out.push(Line::from(vec![gutter(true), Span::styled(if tool.error { "failed" } else { "done" }, faint())]));
        return out;
    }
    let result_style = if tool.error { fg(t.red) } else { dim() };
    match tool.name.as_str() {
        "Edit" | "MultiEdit" if !tool.error => {
            let (a, r) = (line_count(&tool.new), line_count(&tool.old));
            out.push(Line::from(vec![
                gutter(true),
                Span::styled("Updated with ", dim()),
                Span::styled(format!("+{a}"), fg(t.green)),
                Span::styled(format!(" −{r}"), fg(t.red)),
            ]));
            let budget = if tool.expanded { EXPANDED_LINES } else { 4 };
            let mut diff: Vec<(char, &str)> = tool.old.lines().map(|l| ('-', l)).collect();
            diff.extend(tool.new.lines().map(|l| ('+', l)));
            let n = diff.len();
            for (sign, l) in diff.into_iter().take(budget) {
                let style = if sign == '-' { fg(t.red) } else { fg(t.green) };
                let shown = truncate(&l.replace('\t', "    "), inner.saturating_sub(2));
                out.push(Line::from(vec![gutter(false), Span::styled(format!("{sign} {shown}"), style)]));
            }
            if n > budget {
                let more = format!("… +{} lines (⏎ to expand)", n - budget);
                out.push(Line::from(vec![gutter(false), Span::styled(more, faint())]));
            }
        }
        "Write" if !tool.error => {
            out.push(Line::from(vec![
                gutter(true),
                Span::styled(format!("Wrote {} lines", line_count(&tool.new)), dim()),
            ]));
            if tool.expanded {
                push_text(&mut out, &tool.new, fg(t.green), inner, EXPANDED_LINES);
            }
        }
        "Read" if !tool.error => {
            out.push(Line::from(vec![gutter(true), Span::styled(format!("Read {} lines", line_count(result)), dim())]));
        }
        "TodoWrite" => out.push(Line::from(vec![gutter(true), Span::styled("Updated the todo list", dim())])),
        _ => {
            if tool.expanded && !tool.input.is_empty() && !matches!(tool.name.as_str(), "Bash" | "PowerShell") {
                push_text(&mut out, &tool.input, faint(), inner, 20);
            }
            let shown = if result.trim().is_empty() { "(no output)" } else { result.trim_end() };
            push_text(&mut out, shown, result_style, inner, max);
        }
    }
    out
}

fn tool_title(tool: &Tool) -> (String, String) {
    let name = match tool.name.as_str() {
        "Edit" | "MultiEdit" => "Update".to_string(),
        "Grep" => "Search".to_string(),
        "Glob" => "Find".to_string(),
        "Task" | "Agent" => "Agent".to_string(),
        other => tool_label(other),
    };
    let target = match tool.name.as_str() {
        "Bash" | "PowerShell" if !tool.input.is_empty() => squash(&tool.input),
        _ => tool.target.clone(),
    };
    (name, target)
}

fn note_style(kind: FeedKind) -> (&'static str, Style) {
    let t = theme::get();
    match kind {
        FeedKind::Permission | FeedKind::Question => ("◆", fg(t.red)),
        FeedKind::Error => ("✗", fg(t.red)),
        FeedKind::Agent => ("⑂", fg(t.magenta)),
        FeedKind::Compact => ("◌", fg(t.magenta)),
        FeedKind::Start | FeedKind::End => ("─", faint()),
        _ => ("·", faint()),
    }
}

fn hm(ms: Option<Ms>) -> String {
    ms.and_then(chrono::DateTime::from_timestamp_millis)
        .map(|d| d.with_timezone(&chrono::Local).format("%H:%M").to_string())
        .unwrap_or_default()
}

/// "45s", "3m 12s", "1h 5m", as Claude Code writes them.
pub fn long_duration(secs: i64) -> String {
    match secs {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m {}s", s / 60, s % 60),
        s => format!("{}h {}m", s / 3600, (s % 3600) / 60),
    }
}

/// Claude Code's line after a finished turn ("✻ Worked for 3m 12s"),
/// plus what the turn did: files edited and commands run.
fn recap(entries: &[Entry]) -> Vec<Line<'static>> {
    let t = theme::get();
    let Some(start) = entries.iter().rposition(Entry::is_prompt) else { return vec![] };
    let turn = &entries[start..];
    let (Some(from), Some(to)) = (turn[0].at, turn.iter().rev().find_map(|e| e.at)) else { return vec![] };
    let secs = (to - from) / 1000;
    if secs < 1 || turn.len() < 2 {
        return vec![];
    }
    let mut files: Vec<&str> = Vec::new();
    let (mut added, mut removed, mut commands, mut failed) = (0usize, 0usize, 0, 0);
    for e in turn {
        let Body::Tool(tool) = &e.body else { continue };
        match tool.name.as_str() {
            "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => {
                if !files.contains(&tool.target.as_str()) {
                    files.push(&tool.target);
                }
                added += tool.new.lines().count();
                removed += tool.old.lines().count();
            }
            "Bash" => {
                commands += 1;
                failed += tool.error as usize;
            }
            _ => {}
        }
    }
    let mut parts =
        vec![Span::styled(" ✻ ", fg(t.orange)), Span::styled(format!("Worked for {}", long_duration(secs)), dim())];
    if !files.is_empty() {
        let n = files.len();
        parts.push(Span::styled(format!(" · edited {n} file{}", if n == 1 { "" } else { "s" }), faint()));
        if added > 0 {
            parts.push(Span::styled(format!(" +{added}"), fg(t.green)));
        }
        if removed > 0 {
            parts.push(Span::styled(format!(" −{removed}"), fg(t.red)));
        }
    }
    if commands > 0 {
        parts.push(Span::styled(format!(" · ran {commands} command{}", if commands == 1 { "" } else { "s" }), faint()));
        if failed > 0 {
            parts.push(Span::styled(format!(", {failed} failed"), fg(t.red)));
        }
    }
    vec![Line::raw(""), Line::from(parts)]
}

fn render_entry(e: &Entry, i: usize, all: &[Entry], width: usize) -> Vec<Line<'static>> {
    let t = theme::get();
    let mut out = Vec::new();
    // One blank line between blocks, as Claude Code does; notes stack.
    let prev_note = i > 0 && matches!(all[i - 1].body, Body::Note(..));
    let is_note = matches!(e.body, Body::Note(..));
    if i > 0 && !(is_note && prev_note) {
        out.push(Line::raw(""));
    }
    match &e.body {
        Body::You(body) => {
            let band = Style::new().bg(t.raised);
            let time = hm(e.at);
            let inner = width.saturating_sub(4 + time.len() + 1).max(8);
            for (j, l) in wrap(body, inner).into_iter().enumerate() {
                let lead =
                    if j == 0 { Span::styled(" > ", bold(t.dim).bg(t.raised)) } else { Span::styled("   ", band) };
                let used = 3 + UnicodeWidthStr::width(l.as_str());
                let right = if j == 0 { format!("{time} ") } else { String::new() };
                let pad = width.saturating_sub(used + right.len());
                out.push(Line::from(vec![
                    lead,
                    Span::styled(l, Style::new().fg(t.text).bg(t.raised)),
                    Span::styled(" ".repeat(pad), band),
                    Span::styled(right, Style::new().fg(t.faint).bg(t.raised)),
                ]));
            }
        }
        Body::Claude(body) => out.extend(markdown(body, width, Span::styled(" ● ", bold(t.text)), "   ")),
        Body::Tool(tool) => {
            let dot = match (&tool.result, tool.error) {
                (_, true) => fg(t.red),
                (Some(_), false) => fg(t.green),
                (None, _) => fg(t.yellow),
            };
            let (name, target) = tool_title(tool);
            let room = width.saturating_sub(UnicodeWidthStr::width(name.as_str()) + 6);
            out.push(Line::from(vec![
                Span::styled(" ● ", dot),
                Span::styled(name, bold(t.text)),
                Span::styled(format!("({})", truncate(&target, room)), dim()),
            ]));
            out.extend(tool_lines(tool, width));
        }
        Body::Note(kind, body) => {
            let (glyph, style) = note_style(*kind);
            out.push(Line::from(vec![
                Span::styled(format!("   {glyph} "), style),
                Span::styled(truncate(body, width.saturating_sub(12)), style),
                Span::styled(format!("  {}", hm(e.at)), faint()),
            ]));
        }
    }
    out
}

/// Hook events about one session, for `add_notes`.
pub fn feed_for<'a>(feed: &'a [FeedEvent], session_id: &str) -> Vec<&'a FeedEvent> {
    feed.iter().filter(|e| e.session_id == session_id).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn transcript(tag: &str, lines: &[&str]) -> (std::path::PathBuf, Conversation) {
        let dir = std::env::temp_dir().join(format!("aulora-conv-{}-{tag}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.jsonl");
        let mut f = File::create(&path).unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
        let mut c = Conversation::new(Some(path.to_string_lossy().to_string()));
        c.refresh();
        (dir, c)
    }

    const LINES: &[&str] = &[
        r#"{"type":"user","timestamp":"2026-10-19T10:00:00Z","cwd":"/p","message":{"role":"user","content":"fix the **bug**"}}"#,
        r#"{"type":"assistant","timestamp":"2026-10-19T10:00:01Z","message":{"id":"m1","content":[{"type":"text","text":"Looking."}]}}"#,
        r#"{"type":"assistant","timestamp":"2026-10-19T10:00:02Z","message":{"id":"m1","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"pytest -q"}}]}}"#,
        r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"a\nb\nc\nd\ne\n3 passed","is_error":false}]}}"#,
        r#"{"type":"assistant","timestamp":"2026-10-19T10:00:05Z","message":{"id":"m2","content":[{"type":"text","text":"Fixed."}]}}"#,
        r#"{"type":"user","isSidechain":true,"message":{"role":"user","content":"ignored"}}"#,
    ];

    #[test]
    fn reads_entries_and_pairs_results() {
        let (dir, c) = transcript("pairs", LINES);
        assert_eq!(c.entries.len(), 4);
        assert!(matches!(&c.entries[2].body, Body::Tool(t) if t.result.as_deref() == Some("a\nb\nc\nd\ne\n3 passed")));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn scrolls_by_line_and_keeps_its_place() {
        let (dir, mut c) = transcript("scroll", LINES);
        c.layout(60);
        let total = c.total_rows();
        assert!(total > 10, "{total}");
        assert_eq!(c.settle(5), total - 5, "follows the bottom");

        c.scroll_by(-3, 5);
        assert!(!c.follow);
        let top = c.settle(5);
        assert_eq!(top, total - 8, "three lines up");

        // A new event arrives at the end: the view stays where it was.
        let note = FeedEvent {
            at: i64::MAX,
            session_id: "s".into(),
            project_root: "/p".into(),
            kind: FeedKind::Agent,
            text: "Explore agent started".into(),
        };
        c.add_notes(&[&note]);
        c.layout(60);
        assert_eq!(c.settle(5), top);

        c.scroll_by(1000, 5);
        assert!(c.follow, "scrolling past the bottom follows again");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn previews_and_expands_tool_output() {
        let (dir, mut c) = transcript("expand", LINES);
        c.layout(60);
        let before = c.total_rows();
        c.cursor = Anchor { entry: 2, row: 0 };
        assert!(c.toggle_at_cursor());
        c.layout(60);
        // Six output lines instead of three plus a "+3 lines" marker.
        assert_eq!(c.total_rows(), before + 2);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn notes_land_in_time_order() {
        let (dir, mut c) = transcript("notes", LINES);
        let at = parse_iso_ms("2026-10-19T10:00:03Z").unwrap();
        let note = FeedEvent {
            at,
            session_id: "s".into(),
            project_root: "/p".into(),
            kind: FeedKind::Permission,
            text: "needs permission: Bash".into(),
        };
        c.add_notes(&[&note]);
        assert!(matches!(c.entries[3].body, Body::Note(FeedKind::Permission, _)));
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
