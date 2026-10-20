use std::process::Command;

use aulora_core::model::GitInfo;

fn git(dir: &str, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("--no-optional-locks")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("LC_ALL", "C")
        .output()
        .ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).to_string())
}

/// The repository root containing `dir`, or `dir` itself outside git.
pub fn project_root(dir: &str) -> String {
    repo_root(dir).unwrap_or_else(|| dir.trim_end_matches('/').to_string())
}

/// The repository root containing `dir`, if it's inside one.
pub fn repo_root(dir: &str) -> Option<String> {
    git(dir, &["rev-parse", "--show-toplevel"]).map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

pub fn info(root: &str) -> Option<GitInfo> {
    let status = git(root, &["status", "--porcelain=v2", "--branch"])?;
    let mut g = GitInfo::default();
    for line in status.lines() {
        if let Some(head) = line.strip_prefix("# branch.head ") {
            if head == "(detached)" {
                g.detached = true;
            } else {
                g.branch = Some(head.to_string());
            }
        } else if let Some(ab) = line.strip_prefix("# branch.ab ") {
            for part in ab.split_whitespace() {
                if let Some(n) = part.strip_prefix('+') {
                    g.ahead = n.parse().unwrap_or(0);
                } else if let Some(n) = part.strip_prefix('-') {
                    g.behind = n.parse().unwrap_or(0);
                }
            }
        } else if line.starts_with("1 ") || line.starts_with("2 ") {
            let xy: Vec<char> = line[2..4].chars().collect();
            if xy[0] != '.' {
                g.staged += 1;
            }
            if xy[1] != '.' {
                g.modified += 1;
            }
        } else if line.starts_with("u ") {
            g.conflicts += 1;
        } else if line.starts_with("? ") {
            g.untracked += 1;
        }
    }
    if let Some(log) = git(root, &["log", "-1", "--format=%s%x1f%ct"]) {
        let mut it = log.trim().splitn(2, '\x1f');
        g.last_commit = it.next().map(str::to_string).filter(|s| !s.is_empty());
        g.last_commit_at = it.next().and_then(|t| t.parse::<i64>().ok()).map(|t| t * 1000);
    }
    if g.detached {
        g.branch = git(root, &["rev-parse", "--short", "HEAD"]).map(|s| s.trim().to_string());
    }
    Some(g)
}
