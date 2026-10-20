//! Slash commands for the prompt box, as Claude Code's own `/` menu: the
//! built-in commands, your commands in ~/.claude/commands, the project's in
//! .claude/commands, and skills.

use std::path::Path;

/// Claude Code's built-in commands.
const BUILT_IN: &[(&str, &str)] = &[
    ("add-dir", "Add a working directory"),
    ("agents", "Manage subagents"),
    ("clear", "Clear the conversation and start fresh"),
    ("compact", "Compact the conversation, keeping a summary"),
    ("config", "Open settings"),
    ("context", "Show what's using the context window"),
    ("cost", "Show this session's cost and duration"),
    ("doctor", "Check the Claude Code installation"),
    ("exit", "Exit Claude Code"),
    ("export", "Export the conversation"),
    ("help", "Show help"),
    ("hooks", "Manage hooks"),
    ("init", "Write a CLAUDE.md for this project"),
    ("mcp", "Manage MCP servers"),
    ("memory", "Edit memory files"),
    ("model", "Choose the model"),
    ("permissions", "Manage tool permissions"),
    ("plugin", "Manage plugins"),
    ("resume", "Resume another conversation"),
    ("review", "Review a pull request"),
    ("rewind", "Rewind the conversation or code"),
    ("status", "Show account and session status"),
    ("todos", "Show the todo list"),
    ("usage", "Show plan usage"),
];

/// The description in a command's front matter, or its first line.
fn describe(text: &str) -> String {
    let mut lines = text.lines();
    if text.starts_with("---") {
        lines.next();
        for l in lines.by_ref() {
            if l.trim() == "---" {
                break;
            }
            if let Some(d) = l.strip_prefix("description:") {
                return d.trim().trim_matches('"').to_string();
            }
        }
    }
    lines.map(str::trim).find(|l| !l.is_empty()).unwrap_or("").trim_start_matches('#').trim().to_string()
}

fn commands_in(dir: &Path, prefix: &str, out: &mut Vec<(String, String)>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let path = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            commands_in(&path, &format!("{prefix}{name}:"), out);
        } else if let Some(stem) = name.strip_suffix(".md") {
            let desc = std::fs::read_to_string(&path).map(|t| describe(&t)).unwrap_or_default();
            out.push((format!("{prefix}{stem}"), desc));
        }
    }
}

fn skills_in(dir: &Path, out: &mut Vec<(String, String)>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let skill = e.path().join("SKILL.md");
        if let Ok(text) = std::fs::read_to_string(&skill) {
            let name = text
                .lines()
                .find_map(|l| l.strip_prefix("name:"))
                .map(|n| n.trim().trim_matches('"').to_string())
                .unwrap_or_else(|| e.file_name().to_string_lossy().into_owned());
            out.push((name, describe(&text)));
        }
    }
}

/// Every command available in a project, built-ins first.
pub fn commands(project: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = BUILT_IN.iter().map(|(n, d)| (n.to_string(), d.to_string())).collect();
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_default();
    let mut mine = Vec::new();
    commands_in(&Path::new(project).join(".claude/commands"), "", &mut mine);
    commands_in(&home.join(".claude/commands"), "", &mut mine);
    skills_in(&Path::new(project).join(".claude/skills"), &mut mine);
    skills_in(&home.join(".claude/skills"), &mut mine);
    for c in mine {
        if !out.iter().any(|(n, _)| *n == c.0) {
            out.push(c);
        }
    }
    out
}

/// Commands matching what's typed after the `/`: names starting with it
/// first, then names containing it.
pub fn matching<'a>(all: &'a [(String, String)], typed: &str) -> Vec<&'a (String, String)> {
    let t = typed.to_lowercase();
    let mut starts: Vec<&(String, String)> = all.iter().filter(|(n, _)| n.to_lowercase().starts_with(&t)).collect();
    let contains = all.iter().filter(|(n, _)| !n.to_lowercase().starts_with(&t) && n.to_lowercase().contains(&t));
    starts.extend(contains);
    starts
}

/// The command being typed, if the prompt is one: "/comp" → "comp".
pub fn typing(text: &str) -> Option<&str> {
    let rest = text.strip_prefix('/')?;
    (!rest.contains(char::is_whitespace)).then_some(rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_and_matches_commands() {
        let dir = std::env::temp_dir().join(format!("aulora-slash-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".claude/commands/git")).unwrap();
        std::fs::write(dir.join(".claude/commands/ship.md"), "---\ndescription: Ship it\n---\nRun the release")
            .unwrap();
        std::fs::write(dir.join(".claude/commands/git/sync.md"), "# Pull and push\n").unwrap();
        let all = commands(&dir.to_string_lossy());
        assert!(all.iter().any(|(n, d)| n == "ship" && d == "Ship it"));
        assert!(all.iter().any(|(n, d)| n == "git:sync" && d == "Pull and push"));
        let m = matching(&all, "comp");
        assert_eq!(m[0].0, "compact");
        assert_eq!(typing("/comp"), Some("comp"));
        assert_eq!(typing("/compact now"), None);
        assert_eq!(typing("hello"), None);
        let _ = std::fs::remove_dir_all(dir);
    }
}
