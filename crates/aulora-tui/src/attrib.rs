//! Which Claude session wrote which change.
//!
//! Claude Code records the exact patch of every Edit and Write in the
//! session's transcript (`toolUseResult.structuredPatch`, or the whole
//! `content` of a new file). A change in `git diff` belongs to the session
//! whose recorded lines it contains. Reading transcripts, rather than
//! logging hook events, also covers sessions from before aulora was
//! installed.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};

use serde_json::Value;

/// A line worth matching: not blank, not just a brace or a keyword.
pub fn significant(line: &str) -> Option<&str> {
    let t = line.trim();
    (t.chars().count() >= 4 && t.chars().any(|c| c.is_alphanumeric())).then_some(t)
}

#[derive(Default)]
struct Transcript {
    offset: u64,
    /// File (absolute) → lines this session added to it.
    added: HashMap<String, HashSet<String>>,
    /// File → when this session last wrote it.
    last: HashMap<String, i64>,
}

/// What every session in view has written, kept current by reading only
/// what was appended to each transcript since last time.
#[derive(Default)]
pub struct Index {
    transcripts: HashMap<String, (String, Transcript)>,
}

impl Index {
    /// Bring the index up to date with these sessions' transcripts.
    pub fn update(&mut self, sessions: &[(String, String)]) {
        for (id, path) in sessions {
            let entry = self.transcripts.entry(path.clone()).or_insert_with(|| (id.clone(), Transcript::default()));
            let _ = read_new(path, &mut entry.1);
        }
    }

    /// Sessions whose writes contain these lines, most lines first, then
    /// most recent. Only sessions in `among` count.
    pub fn attribute<'a>(
        &self,
        file: &str,
        lines: impl Iterator<Item = &'a str>,
        among: &HashSet<String>,
    ) -> Vec<String> {
        let lines: Vec<&str> = lines.filter_map(significant).collect();
        if lines.is_empty() {
            return vec![];
        }
        let mut scored: Vec<(usize, i64, &String)> = self
            .transcripts
            .values()
            .filter(|(id, _)| among.contains(id))
            .filter_map(|(id, t)| {
                let set = t.added.get(file)?;
                let hits = lines.iter().filter(|l| set.contains(**l)).count();
                (hits > 0).then(|| (hits, t.last.get(file).copied().unwrap_or(0), id))
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
        scored.into_iter().map(|(_, _, id)| id.clone()).collect()
    }
}

fn read_new(path: &str, t: &mut Transcript) -> std::io::Result<()> {
    let mut f = std::fs::File::open(path)?;
    let len = f.metadata()?.len();
    if len < t.offset {
        // Rewritten from scratch; start over.
        *t = Transcript::default();
    }
    if len == t.offset {
        return Ok(());
    }
    f.seek(SeekFrom::Start(t.offset))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    // Only whole lines; a half-written last line waits for next time.
    let Some(end) = buf.iter().rposition(|b| *b == b'\n') else { return Ok(()) };
    for line in buf[..end].split(|b| *b == b'\n') {
        if contains(line, b"\"toolUseResult\"") && contains(line, b"\"filePath\"") {
            record(line, t);
        }
    }
    t.offset += end as u64 + 1;
    Ok(())
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

fn record(line: &[u8], t: &mut Transcript) {
    let Ok(v) = serde_json::from_slice::<Value>(line) else { return };
    let r = &v["toolUseResult"];
    let Some(file) = r["filePath"].as_str() else { return };
    let at = v["timestamp"].as_str().and_then(aulora_core::util::parse_iso_ms).unwrap_or(0);
    let set = t.added.entry(file.to_string()).or_default();
    let mut any = false;
    for hunk in r["structuredPatch"].as_array().into_iter().flatten() {
        for l in hunk["lines"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            if let Some(added) = l.strip_prefix('+').and_then(significant) {
                set.insert(added.to_string());
                any = true;
            }
        }
    }
    if !any && r["type"].as_str() == Some("create") {
        for l in r["content"].as_str().unwrap_or("").lines().filter_map(significant) {
            set.insert(l.to_string());
            any = true;
        }
    }
    if any {
        let last = t.last.entry(file.to_string()).or_default();
        *last = (*last).max(at);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn reads_edits_and_new_files_incrementally() {
        let dir = std::env::temp_dir().join(format!("aulora-attrib-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.jsonl");
        let edit = r#"{"timestamp":"2026-10-20T10:00:00.000Z","toolUseResult":{"filePath":"/r/a.rs","structuredPatch":[{"lines":[" fn a() {","-    old();","+    new_call(1);","+    }"]}]}}"#;
        let create = r##"{"timestamp":"2026-10-20T10:01:00.000Z","toolUseResult":{"type":"create","filePath":"/r/b.md","content":"# Notes\nsecond line here\n","structuredPatch":[]}}"##;
        std::fs::write(&path, format!("{edit}\n")).unwrap();
        let mut idx = Index::default();
        let sessions = vec![("s1".to_string(), path.to_string_lossy().to_string())];
        idx.update(&sessions);
        let among: HashSet<String> = ["s1".to_string()].into();
        assert_eq!(idx.attribute("/r/a.rs", ["    new_call(1);", "}"].into_iter(), &among), ["s1"]);
        assert!(idx.attribute("/r/a.rs", ["    old();"].into_iter(), &among).is_empty());
        // Appended later, plus half a line still being written.
        let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        write!(f, "{create}\n{{\"half\":").unwrap();
        idx.update(&sessions);
        assert_eq!(idx.attribute("/r/b.md", ["second line here"].into_iter(), &among), ["s1"]);
        assert!(idx.attribute("/r/b.md", ["second line here"].into_iter(), &HashSet::new()).is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}
