//! Incremental reader for Claude Code transcript files (JSONL).
//!
//! Hooks tell us *what* is happening; the transcript adds the AI title,
//! the model and token usage. Files can be many MB, so we remember a byte
//! offset and only parse new lines.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

use serde_json::Value;

use crate::model::Session;

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
    last_msg_id: Option<String>,
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
        let _ = std::fs::remove_dir_all(dir);
    }
}
