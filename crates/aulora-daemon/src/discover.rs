//! Find Claude Code sessions that hooks haven't told us about: ones that
//! were already running when the hooks were installed, and past ones.
//!
//! Claude Code keeps a file per running session in `~/.claude/sessions`
//! and writes every conversation to `~/.claude/projects/<dir>/<id>.jsonl`.
//! Neither is a documented interface, so everything here is best effort:
//! anything that doesn't parse is skipped, and hooks always take priority.

use std::path::PathBuf;
use std::time::UNIX_EPOCH;

use aulora_core::model::Ms;
use serde::Deserialize;

pub fn claude_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(d);
    }
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join(".claude")
}

/// One entry of Claude Code's running-session list.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Running {
    pub pid: u32,
    pub session_id: String,
    pub cwd: String,
    #[serde(default)]
    pub started_at: Option<Ms>,
    /// "busy" or "idle" as seen so far.
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub status_updated_at: Option<Ms>,
}

pub fn running() -> Vec<Running> {
    let Ok(dir) = std::fs::read_dir(claude_dir().join("sessions")) else { return vec![] };
    dir.filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| std::fs::read(e.path()).ok())
        .filter_map(|b| serde_json::from_slice::<Running>(&b).ok())
        .filter(|r| !r.session_id.is_empty())
        .collect()
}

fn mtime_ms(meta: &std::fs::Metadata) -> Ms {
    meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_millis() as Ms).unwrap_or(0)
}

/// Top-level transcripts changed since `since`: (session id, path, mtime).
/// Subagent transcripts live in subdirectories and are left out.
pub fn transcripts(since: Ms) -> Vec<(String, PathBuf, Ms)> {
    let Ok(projects) = std::fs::read_dir(claude_dir().join("projects")) else { return vec![] };
    let mut out = Vec::new();
    for project in projects.filter_map(Result::ok) {
        let Ok(files) = std::fs::read_dir(project.path()) else { continue };
        for f in files.filter_map(Result::ok) {
            let path = f.path();
            if path.extension().is_none_or(|x| x != "jsonl") {
                continue;
            }
            let Ok(meta) = f.metadata() else { continue };
            let mtime = mtime_ms(&meta);
            if !meta.is_file() || mtime < since {
                continue;
            }
            if let Some(id) = path.file_stem().map(|s| s.to_string_lossy().to_string()) {
                out.push((id, path, mtime));
            }
        }
    }
    out.sort_by_key(|(_, _, m)| -m);
    out
}

/// Where a session's transcript is, if it exists.
pub fn find_transcript(session_id: &str) -> Option<PathBuf> {
    let projects = std::fs::read_dir(claude_dir().join("projects")).ok()?;
    let name = format!("{session_id}.jsonl");
    projects.filter_map(Result::ok).map(|p| p.path().join(&name)).find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_running_entries() {
        let r: Running = serde_json::from_str(
            r#"{"pid":4126,"sessionId":"e8d9","cwd":"/p","startedAt":1,"status":"busy","statusUpdatedAt":2,"kind":"interactive","extra":true}"#,
        )
        .unwrap();
        assert_eq!((r.pid, r.session_id.as_str(), r.status.as_deref()), (4126, "e8d9", Some("busy")));
        // Missing optional fields are fine.
        assert!(serde_json::from_str::<Running>(r#"{"pid":1,"sessionId":"x","cwd":"/"}"#).is_ok());
    }
}
