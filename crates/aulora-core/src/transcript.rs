//! Incremental reader for Claude Code transcript files (JSONL).
//!
//! Hooks tell us *what* is happening; the transcript adds the AI title,
//! the model and token usage. Files can be many MB, so we remember a byte
//! offset and only parse new lines.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

use serde_json::Value;

use crate::model::{FileTouch, Ms, Origin, PromptEntry, Session, SessionState};
use crate::util::{parse_iso_ms, rel_path, truncate};

#[derive(Debug, Clone, Default)]
pub struct Cursor {
    pub offset: u64,
    partial: Vec<u8>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Summary {
    pub ai_title: Option<String>,
    pub last_prompt: Option<String>,
    pub model: Option<String>,
    pub git_branch: Option<String>,
    /// Context size at the latest assistant message.
    pub context_tokens: Option<u64>,
    pub output_tokens: u64,
    pub last_assistant_text: Option<String>,
    pub last_turn_ms: Option<u64>,
    /// Assistant text written since the latest human prompt.
    pub turn_text: Option<String>,
    /// Working directory recorded on transcript lines.
    pub cwd: Option<String>,
    pub first_at: Option<Ms>,
    pub last_at: Option<Ms>,
    /// Human prompts, oldest first (last few only).
    pub prompts: Vec<PromptEntry>,
    pub prompt_count: u32,
    pub tool_calls: u32,
    /// Files edited, by absolute path.
    pub files: Vec<FileTouch>,
    last_msg_id: Option<String>,
}

const MAX_PROMPTS: usize = 12;

fn lines_of(v: Option<&Value>) -> u32 {
    v.and_then(Value::as_str).map(|s| s.lines().count() as u32).unwrap_or(0)
}

fn record_edit(sum: &mut Summary, name: &str, input: &Value, at: Ms) {
    let path = input.get("file_path").or_else(|| input.get("notebook_path")).and_then(Value::as_str);
    let Some(path) = path else { return };
    let (added, removed) = match name {
        "Write" => (lines_of(input.get("content")), 0),
        "MultiEdit" => input.get("edits").and_then(Value::as_array).map_or((0, 0), |edits| {
            edits
                .iter()
                .fold((0, 0), |(a, r), e| (a + lines_of(e.get("new_string")), r + lines_of(e.get("old_string"))))
        }),
        "NotebookEdit" => (lines_of(input.get("new_source")), 0),
        _ => (lines_of(input.get("new_string")), lines_of(input.get("old_string"))),
    };
    match sum.files.iter_mut().find(|f| f.path == path) {
        Some(f) => {
            f.edits += 1;
            f.added += added;
            f.removed += removed;
            f.last_at = f.last_at.max(at);
        }
        None => sum.files.push(FileTouch { path: path.to_string(), edits: 1, added, removed, last_at: at }),
    }
}

/// Read anything appended since the last call. Returns true if something changed.
pub fn read_new(path: &str, cur: &mut Cursor, sum: &mut Summary) -> std::io::Result<bool> {
    let mut f = File::open(path)?;
    let len = f.metadata()?.len();
    if len < cur.offset {
        // Rewritten file: start over.
        *cur = Cursor::default();
        *sum = Summary::default();
    }
    if len == cur.offset {
        return Ok(false);
    }
    f.seek(SeekFrom::Start(cur.offset))?;
    let mut buf = Vec::with_capacity((len - cur.offset) as usize);
    f.take(len - cur.offset).read_to_end(&mut buf)?;
    cur.offset += buf.len() as u64;

    let mut data = std::mem::take(&mut cur.partial);
    data.extend_from_slice(&buf);
    let before = sum.clone();
    let mut start = 0;
    for i in 0..data.len() {
        if data[i] == b'\n' {
            process_line(&data[start..i], sum);
            start = i + 1;
        }
    }
    cur.partial = data[start..].to_vec();
    Ok(*sum != before)
}

fn process_line(line: &[u8], sum: &mut Summary) {
    if line.is_empty() {
        return;
    }
    // Skip the big request records without parsing them.
    let head = &line[..line.len().min(48)];
    if head.windows(12).any(|w| w == b"api-request\"" || w == b"api-request-")
        || head.windows(11).any(|w| w == b"\"api-reques")
    {
        return;
    }
    let Ok(v) = serde_json::from_slice::<Value>(line) else { return };
    if v.get("isSidechain").and_then(Value::as_bool) == Some(true) {
        return;
    }
    if let Some(b) = v.get("gitBranch").and_then(Value::as_str).filter(|b| !b.is_empty()) {
        sum.git_branch = Some(b.to_string());
    }
    if let Some(c) = v.get("cwd").and_then(Value::as_str).filter(|c| !c.is_empty()) {
        sum.cwd = Some(c.to_string());
    }
    let at = v.get("timestamp").and_then(Value::as_str).and_then(parse_iso_ms);
    if let Some(at) = at {
        sum.first_at = Some(sum.first_at.map_or(at, |f| f.min(at)));
        sum.last_at = Some(sum.last_at.map_or(at, |l| l.max(at)));
    }
    let at = at.or(sum.last_at).unwrap_or(0);
    match v.get("type").and_then(Value::as_str).unwrap_or("") {
        "ai-title" => {
            if let Some(t) = v.get("aiTitle").and_then(Value::as_str) {
                sum.ai_title = Some(t.to_string());
            }
        }
        "custom-title" => {
            if let Some(t) = v.get("customTitle").and_then(Value::as_str) {
                sum.ai_title = Some(t.to_string());
            }
        }
        "last-prompt" => {
            if let Some(t) = v.get("lastPrompt").and_then(Value::as_str) {
                sum.last_prompt = Some(t.to_string());
            }
        }
        "user" => {
            let human = v.get("message").and_then(|m| m.get("content")).is_some_and(Value::is_string)
                || v.get("origin").and_then(|o| o.get("kind")).and_then(Value::as_str) == Some("human");
            let meta = v.get("isMeta").and_then(Value::as_bool).unwrap_or(false);
            if human && !meta {
                sum.turn_text = None;
                let text = v.get("message").and_then(|m| m.get("content")).and_then(Value::as_str).unwrap_or("");
                // Slash commands and their output are recorded as tagged text.
                if !text.trim().is_empty() && !crate::util::is_injected(text) {
                    sum.prompt_count += 1;
                    sum.prompts.push(PromptEntry { at, text: truncate(text, 600) });
                    if sum.prompts.len() > MAX_PROMPTS {
                        sum.prompts.remove(0);
                    }
                }
            }
        }
        "system" => {
            if v.get("subtype").and_then(Value::as_str) == Some("turn_duration") {
                sum.last_turn_ms = v.get("durationMs").and_then(Value::as_u64);
            }
        }
        "assistant" => {
            let Some(msg) = v.get("message") else { return };
            if let Some(m) = msg.get("model").and_then(Value::as_str).filter(|m| !m.starts_with('<')) {
                sum.model = Some(m.to_string());
            }
            let id = msg.get("id").and_then(Value::as_str).map(str::to_string);
            let new_msg = id.is_none() || id != sum.last_msg_id;
            if let Some(u) = msg.get("usage") {
                let g = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
                let ctx = g("input_tokens") + g("cache_creation_input_tokens") + g("cache_read_input_tokens");
                if ctx > 0 {
                    sum.context_tokens = Some(ctx + g("output_tokens"));
                }
                if new_msg {
                    sum.output_tokens += g("output_tokens");
                }
            }
            if let Some(blocks) = msg.get("content").and_then(Value::as_array) {
                for b in blocks.iter().filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_use")) {
                    sum.tool_calls += 1;
                    let name = b.get("name").and_then(Value::as_str).unwrap_or("");
                    if matches!(name, "Edit" | "MultiEdit" | "Write" | "NotebookEdit")
                        && let Some(input) = b.get("input")
                    {
                        record_edit(sum, name, input, at);
                    }
                }
            }
            let text: Vec<&str> = msg
                .get("content")
                .and_then(Value::as_array)
                .map(|c| {
                    c.iter()
                        .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                        .filter_map(|b| b.get("text").and_then(Value::as_str))
                        .collect()
                })
                .unwrap_or_default();
            if !text.is_empty() {
                let joined = text.join("\n");
                match (&mut sum.last_assistant_text, new_msg) {
                    (Some(prev), false) => {
                        prev.push('\n');
                        prev.push_str(&joined);
                    }
                    _ => sum.last_assistant_text = Some(joined.clone()),
                }
                sum.turn_text = Some(match sum.turn_text.take() {
                    Some(prev) => format!("{prev}\n\n{joined}"),
                    None => joined,
                });
            }
            if id.is_some() {
                sum.last_msg_id = id;
            }
        }
        _ => {}
    }
}

impl Summary {
    /// Fill a session that no hook has described, from the transcript alone.
    pub fn apply_history(&self, sess: &mut Session) {
        self.apply_to(sess);
        if let Some(at) = self.first_at {
            sess.started_at = at;
        }
        if let Some(at) = self.last_at {
            sess.last_event_at = sess.last_event_at.max(at);
            if sess.origin == Origin::History {
                sess.state_since = at;
            }
        }
        sess.turns = self.prompt_count;
        sess.tool_calls = self.tool_calls;
        sess.prompts = self.prompts.clone();
        if let Some(p) = self.prompts.last() {
            sess.last_prompt = Some(p.text.clone());
            sess.last_prompt_at = Some(p.at);
        }
        if let Some(t) = &self.last_assistant_text {
            sess.last_reply = Some(t.clone());
            sess.last_reply_at = self.last_at;
        }
        let root = sess.project_root.clone();
        let mut files: Vec<FileTouch> =
            self.files.iter().map(|f| FileTouch { path: rel_path(&f.path, &root), ..f.clone() }).collect();
        files.sort_by_key(|f| -f.last_at);
        sess.files_touched = files;
        if sess.origin == Origin::History {
            sess.state = SessionState::Ended { reason: "history".into() };
        }
    }

    /// Fill in what hooks don't tell us.
    pub fn apply_to(&self, sess: &mut Session) {
        if let Some(t) = &self.ai_title {
            sess.title = Some(t.clone());
        }
        if let Some(m) = &self.model {
            sess.model = Some(m.clone());
        }
        if let Some(b) = &self.git_branch {
            sess.git_branch = Some(b.clone());
        }
        if self.context_tokens.is_some() {
            sess.context_tokens = self.context_tokens;
        }
        sess.output_tokens = self.output_tokens;
        if sess.last_prompt.is_none() {
            sess.last_prompt = self.last_prompt.clone();
        }
        if sess.last_reply.is_none() {
            sess.last_reply = self.last_assistant_text.clone();
        }
        sess.live_text = self.turn_text.clone();
        if sess.last_turn_ms.is_none() {
            sess.last_turn_ms = self.last_turn_ms;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn incremental() {
        let dir = std::env::temp_dir().join(format!("aulora-tx-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.jsonl");
        let mut f = File::create(&path).unwrap();
        writeln!(f, r#"{{"type":"ai-title","aiTitle":"Fix the bug","sessionId":"x"}}"#).unwrap();
        writeln!(f, r#"{{"type":"assistant","gitBranch":"main","message":{{"id":"m1","model":"claude-opus-5-5","content":[{{"type":"thinking","thinking":""}}],"usage":{{"input_tokens":2,"cache_creation_input_tokens":100,"cache_read_input_tokens":900,"output_tokens":50}}}}}}"#).unwrap();
        writeln!(f, r#"{{"type":"assistant","message":{{"id":"m1","model":"claude-opus-5-5","content":[{{"type":"text","text":"Done."}}],"usage":{{"input_tokens":2,"cache_creation_input_tokens":100,"cache_read_input_tokens":900,"output_tokens":50}}}}}}"#).unwrap();
        write!(f, r#"{{"type":"last-prompt","lastPr"#).unwrap();
        f.flush().unwrap();

        let (mut c, mut s) = (Cursor::default(), Summary::default());
        assert!(read_new(path.to_str().unwrap(), &mut c, &mut s).unwrap());
        assert_eq!(s.ai_title.as_deref(), Some("Fix the bug"));
        assert_eq!(s.context_tokens, Some(1052));
        assert_eq!(s.output_tokens, 50, "same message id counted once");
        assert_eq!(s.last_assistant_text.as_deref(), Some("Done."));
        assert!(s.last_prompt.is_none(), "partial line not parsed yet");

        writeln!(f, r#"ompt":"hello"}}"#).unwrap();
        f.flush().unwrap();
        assert!(read_new(path.to_str().unwrap(), &mut c, &mut s).unwrap());
        assert_eq!(s.last_prompt.as_deref(), Some("hello"));
        assert!(!read_new(path.to_str().unwrap(), &mut c, &mut s).unwrap());

        writeln!(f, r#"{{"type":"user","timestamp":"2026-10-19T10:00:00.000Z","cwd":"/p","message":{{"role":"user","content":"fix calc"}}}}"#).unwrap();
        writeln!(f, r#"{{"type":"user","timestamp":"2026-10-19T10:00:01.000Z","message":{{"role":"user","content":"<command-name>/clear</command-name>"}}}}"#).unwrap();
        writeln!(f, r#"{{"type":"assistant","timestamp":"2026-10-19T10:00:05.000Z","message":{{"id":"m2","content":[{{"type":"tool_use","id":"t1","name":"Edit","input":{{"file_path":"/p/calc.py","old_string":"a","new_string":"b\nc"}}}}]}}}}"#).unwrap();
        f.flush().unwrap();
        read_new(path.to_str().unwrap(), &mut c, &mut s).unwrap();
        assert_eq!(s.cwd.as_deref(), Some("/p"));
        assert_eq!(s.prompt_count, 1, "slash commands are not prompts");
        assert_eq!(s.tool_calls, 1);
        assert_eq!((s.files[0].added, s.files[0].removed), (2, 1));
        let mut sess = Session::new("x", "/p", "/p", 0);
        sess.origin = Origin::History;
        s.apply_history(&mut sess);
        assert_eq!(sess.files_touched[0].path, "calc.py");
        assert_eq!(sess.last_prompt.as_deref(), Some("fix calc"));
        assert!(!sess.state.is_live());
        let _ = std::fs::remove_dir_all(dir);
    }
}
