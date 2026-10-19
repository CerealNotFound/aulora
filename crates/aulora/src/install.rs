//! Add or remove aulora's hooks in a Claude Code settings file.
//!
//! The file is edited as JSON with key order preserved, backed up before
//! any change, and written atomically. Only handlers that run `aulora hook`
//! are touched.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};

/// Events aulora listens to.
pub const EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "PermissionRequest",
    "PermissionDenied",
    "Notification",
    "Stop",
    "StopFailure",
    "SubagentStart",
    "SubagentStop",
    "PreCompact",
    "PostCompact",
    "SessionEnd",
];

pub fn default_settings_path() -> PathBuf {
    dirs_home().join(".claude").join("settings.json")
}

fn dirs_home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// The hook command. `exec` replaces the shell so Claude is our direct parent.
pub fn hook_command(exe: &Path) -> String {
    format!("exec {} hook", shell_quote(&exe.to_string_lossy()))
}

fn is_ours(handler: &Value) -> bool {
    let cmd = handler.get("command").and_then(Value::as_str).unwrap_or("");
    let trimmed = cmd.trim_end();
    trimmed.ends_with("aulora' hook") || trimmed.ends_with("aulora hook") || trimmed == "aulora hook"
}

fn load(path: &Path) -> Result<Value> {
    if !path.exists() {
        return Ok(Value::Object(Map::new()));
    }
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    if text.trim().is_empty() {
        return Ok(Value::Object(Map::new()));
    }
    let v: Value = serde_json::from_str(&text).with_context(|| format!("{} is not valid JSON", path.display()))?;
    if !v.is_object() {
        bail!("{} must contain a JSON object", path.display());
    }
    Ok(v)
}

/// Remove our handlers everywhere; drop groups and events left empty.
fn strip(settings: &mut Value) -> usize {
    let mut removed = 0;
    let Some(hooks) = settings.get_mut("hooks").and_then(Value::as_object_mut) else { return 0 };
    for (_, groups) in hooks.iter_mut() {
        let Some(groups) = groups.as_array_mut() else { continue };
        for g in groups.iter_mut() {
            if let Some(hs) = g.get_mut("hooks").and_then(Value::as_array_mut) {
                let before = hs.len();
                hs.retain(|h| !is_ours(h));
                removed += before - hs.len();
            }
        }
        groups.retain(|g| g.get("hooks").and_then(Value::as_array).is_none_or(|h| !h.is_empty()));
    }
    hooks.retain(|_, g| g.as_array().is_none_or(|a| !a.is_empty()));
    if hooks.is_empty() {
        settings.as_object_mut().map(|o| o.remove("hooks"));
    }
    removed
}

pub fn installed_events(settings: &Value) -> Vec<String> {
    let Some(hooks) = settings.get("hooks").and_then(Value::as_object) else { return vec![] };
    hooks
        .iter()
        .filter(|(_, groups)| {
            groups.as_array().is_some_and(|gs| {
                gs.iter().any(|g| g.get("hooks").and_then(Value::as_array).is_some_and(|hs| hs.iter().any(is_ours)))
            })
        })
        .map(|(k, _)| k.clone())
        .collect()
}

pub fn installed_command(settings: &Value) -> Option<String> {
    settings.get("hooks")?.as_object()?.values().find_map(|groups| {
        groups.as_array()?.iter().find_map(|g| {
            g.get("hooks")?.as_array()?.iter().find(|h| is_ours(h))?.get("command")?.as_str().map(str::to_string)
        })
    })
}

fn apply_install(settings: &mut Value, exe: &Path) {
    strip(settings);
    let obj = settings.as_object_mut().expect("object");
    let hooks = obj.entry("hooks").or_insert_with(|| Value::Object(Map::new()));
    let hooks = hooks.as_object_mut().expect("hooks object");
    for ev in EVENTS {
        let groups = hooks.entry(*ev).or_insert_with(|| Value::Array(vec![]));
        if let Some(arr) = groups.as_array_mut() {
            arr.push(json!({
                "hooks": [{ "type": "command", "command": hook_command(exe), "timeout": 5 }]
            }));
        }
    }
}

fn write_atomic(path: &Path, value: &Value) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut text = serde_json::to_string_pretty(value)?;
    text.push('\n');
    let tmp = path.with_extension("json.aulora-tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn backup(path: &Path) -> Result<Option<PathBuf>> {
    if !path.exists() {
        return Ok(None);
    }
    let stamp = chrono_stamp();
    let dest = path.with_file_name(format!(
        "{}.aulora-backup-{stamp}",
        path.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default()
    ));
    std::fs::copy(path, &dest)?;
    Ok(Some(dest))
}

fn chrono_stamp() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    secs.to_string()
}

pub struct Outcome {
    pub changed: bool,
    pub backup: Option<PathBuf>,
    pub preview: String,
}

pub fn install(path: &Path, exe: &Path, dry_run: bool) -> Result<Outcome> {
    let original = load(path)?;
    let mut next = original.clone();
    apply_install(&mut next, exe);
    finish(path, &original, next, dry_run)
}

pub fn uninstall(path: &Path, dry_run: bool) -> Result<Outcome> {
    let original = load(path)?;
    let mut next = original.clone();
    strip(&mut next);
    finish(path, &original, next, dry_run)
}

fn finish(path: &Path, original: &Value, next: Value, dry_run: bool) -> Result<Outcome> {
    let preview = serde_json::to_string_pretty(next.get("hooks").unwrap_or(&Value::Null))?;
    if *original == next {
        return Ok(Outcome { changed: false, backup: None, preview });
    }
    if dry_run {
        return Ok(Outcome { changed: true, backup: None, preview });
    }
    let backup = backup(path)?;
    write_atomic(path, &next)?;
    Ok(Outcome { changed: true, backup, preview })
}

pub fn read(path: &Path) -> Result<Value> {
    load(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_is_idempotent_and_preserves_other_hooks() {
        let mut s = json!({
            "model": "opus",
            "hooks": { "PreToolUse": [ { "matcher": "Bash", "hooks": [ { "type": "command", "command": "./guard.sh" } ] } ] }
        });
        let exe = Path::new("/Users/me/.cargo/bin/aulora");
        apply_install(&mut s, exe);
        let once = s.clone();
        apply_install(&mut s, exe);
        assert_eq!(once, s, "second install changes nothing");
        assert_eq!(installed_events(&s).len(), EVENTS.len());
        assert_eq!(s["hooks"]["PreToolUse"][0]["hooks"][0]["command"], "./guard.sh");
        assert_eq!(s["hooks"]["PreToolUse"][1]["hooks"][0]["command"], "exec '/Users/me/.cargo/bin/aulora' hook");

        strip(&mut s);
        assert_eq!(
            s,
            json!({
                "model": "opus",
                "hooks": { "PreToolUse": [ { "matcher": "Bash", "hooks": [ { "type": "command", "command": "./guard.sh" } ] } ] }
            })
        );
    }

    #[test]
    fn uninstall_removes_empty_hooks_key() {
        let mut s = json!({ "theme": "dark" });
        apply_install(&mut s, Path::new("/x/aulora"));
        strip(&mut s);
        assert_eq!(s, json!({ "theme": "dark" }));
    }
}
