//! A repository's uncommitted changes: `git status` and `git diff`, parsed.
//! Runs on the changes worker thread, never on the UI thread.

use std::collections::HashMap;
use std::process::Command;

/// The groups of VS Code's Source Control view, in its order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Group {
    Conflicts,
    Staged,
    Changes,
}

impl Group {
    pub fn label(self) -> &'static str {
        match self {
            Group::Conflicts => "Merge conflicts",
            Group::Staged => "Staged",
            Group::Changes => "Changes",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Group::Conflicts => "conflicts",
            Group::Staged => "staged",
            Group::Changes => "changes",
        }
    }
}

/// Ordered by how much it matters, for rolling folders up (VS Code shows
/// a folder in the color of its most important change).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    Conflict,
    Modified,
    Renamed,
    TypeChange,
    Added,
    Untracked,
    Deleted,
}

impl Kind {
    /// VS Code's letters.
    pub fn letter(self) -> char {
        match self {
            Kind::Conflict => '!',
            Kind::Modified => 'M',
            Kind::Renamed => 'R',
            Kind::TypeChange => 'T',
            Kind::Added => 'A',
            Kind::Untracked => 'U',
            Kind::Deleted => 'D',
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            Kind::Conflict => "conflict",
            Kind::Modified => "modified",
            Kind::Renamed => "renamed",
            Kind::TypeChange => "type changed",
            Kind::Added => "added",
            Kind::Untracked => "untracked",
            Kind::Deleted => "deleted",
        }
    }

    fn from_code(c: char) -> Option<Kind> {
        Some(match c {
            'M' => Kind::Modified,
            'T' => Kind::TypeChange,
            'A' | 'C' => Kind::Added,
            'D' => Kind::Deleted,
            'R' => Kind::Renamed,
            _ => return None,
        })
    }
}

/// One entry of `git status`. A file staged and then edited again is two.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub group: Group,
    pub kind: Kind,
    /// Relative to the repository root, `/`-separated.
    pub path: String,
    /// Where a rename came from.
    pub orig: Option<String>,
}

/// Parse `git status --porcelain=v2 -z`.
pub fn parse_status(out: &[u8]) -> Vec<Entry> {
    let mut entries = Vec::new();
    let mut fields = out.split(|b| *b == 0).map(|f| String::from_utf8_lossy(f).into_owned());
    while let Some(rec) = fields.next() {
        let mut push = |group, kind, path: &str, orig: Option<String>| {
            entries.push(Entry { group, kind, path: path.to_string(), orig })
        };
        match rec.as_bytes().first() {
            Some(b'1') | Some(b'2') => {
                let renamed = rec.starts_with('2');
                let parts: Vec<&str> = rec.splitn(if renamed { 10 } else { 9 }, ' ').collect();
                let (Some(xy), Some(path)) = (parts.get(1), parts.last()) else { continue };
                let orig = if renamed { fields.next() } else { None };
                let mut xy = xy.chars();
                let (x, y) = (xy.next().unwrap_or('.'), xy.next().unwrap_or('.'));
                if let Some(k) = Kind::from_code(x) {
                    push(Group::Staged, k, path, orig.clone());
                }
                if let Some(k) = Kind::from_code(y) {
                    push(Group::Changes, k, path, if x == '.' { orig } else { None });
                }
            }
            Some(b'u') => {
                if let Some(path) = rec.splitn(11, ' ').nth(10) {
                    push(Group::Conflicts, Kind::Conflict, path, None);
                }
            }
            Some(b'?') => {
                if let Some(path) = rec.get(2..) {
                    push(Group::Changes, Kind::Untracked, path, None);
                }
            }
            _ => {}
        }
    }
    entries
}

/// The repositories a workspace shows changes from, relative to it: the
/// workspace itself ("") when it's one; otherwise the repositories up to
/// two folders down, as VS Code finds them in a folder that holds several.
pub fn repos_in(root: &str) -> Vec<String> {
    use std::path::Path;
    if run(root, &["rev-parse", "--show-toplevel"]).is_ok() {
        return vec![String::new()];
    }
    let folders = |dir: &Path| -> Vec<(String, std::path::PathBuf)> {
        let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
        let mut v: Vec<_> = rd
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
            .filter(|(n, _)| !n.starts_with('.') && n != "node_modules" && n != "target")
            .collect();
        v.sort();
        v.truncate(300);
        v
    };
    let is_repo = |p: &Path| p.join(".git").exists();
    let mut found = Vec::new();
    for (name, path) in folders(Path::new(root)) {
        if is_repo(&path) {
            found.push(name);
        } else {
            found.extend(folders(&path).into_iter().filter(|(_, p)| is_repo(p)).map(|(n, _)| format!("{name}/{n}")));
        }
    }
    found
}

/// The repository holding `rel` (a path under `root`), and the path
/// inside it: the nearest folder above it with a `.git`, else `root`.
pub fn locate(root: &str, rel: &str) -> (String, String) {
    if !rel.is_empty() && std::path::Path::new(root).join(rel).join(".git").exists() {
        return (format!("{root}/{rel}"), ".".into());
    }
    let mut dir = rel;
    while let Some((parent, _)) = dir.rsplit_once('/') {
        if std::path::Path::new(root).join(parent).join(".git").exists() {
            return (format!("{root}/{parent}"), rel[parent.len() + 1..].to_string());
        }
        dir = parent;
    }
    (root.to_string(), rel.to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LineKind {
    Context,
    Added,
    Removed,
    /// A conflict marker line (`<<<<<<<`, `=======`, `>>>>>>>`).
    Marker,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DiffLine {
    pub kind: LineKind,
    pub old: Option<u32>,
    pub new: Option<u32>,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct Hunk {
    pub old_start: u32,
    pub new_start: u32,
    /// The function or section git found above the hunk.
    pub context: String,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileDiff {
    pub path: String,
    pub hunks: Vec<Hunk>,
    pub binary: bool,
}

impl FileDiff {
    pub fn counts(&self) -> (u32, u32) {
        let mut c = (0, 0);
        for l in self.hunks.iter().flat_map(|h| &h.lines) {
            match l.kind {
                LineKind::Added => c.0 += 1,
                LineKind::Removed => c.1 += 1,
                _ => {}
            }
        }
        c
    }

    pub fn added_lines(&self) -> impl Iterator<Item = &str> {
        self.hunks.iter().flat_map(|h| &h.lines).filter(|l| l.kind == LineKind::Added).map(|l| l.text.as_str())
    }
}

/// A path as git prints it in diff headers: maybe quoted, maybe with a
/// trailing tab.
fn header_path(s: &str) -> String {
    let s = s.trim_end_matches('\t');
    let unquoted = if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        s[1..s.len() - 1].replace("\\\"", "\"").replace("\\\\", "\\").replace("\\t", "\t")
    } else {
        s.to_string()
    };
    unquoted.strip_prefix("a/").or_else(|| unquoted.strip_prefix("b/")).unwrap_or(&unquoted).to_string()
}

fn range_start(s: &str) -> u32 {
    s.split(',').next().and_then(|n| n.parse().ok()).unwrap_or(0)
}

/// Parse unified diff output covering any number of files. Combined diffs
/// of conflicted files (`diff --cc`) are skipped; those are read whole.
pub fn parse_diff(text: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = Vec::new();
    let mut skipping = false;
    let (mut old_no, mut new_no) = (0u32, 0u32);
    let mut in_header = false;
    for line in text.lines() {
        // Files with Windows line endings keep a \r on every line.
        let line = line.strip_suffix('\r').unwrap_or(line);
        if let Some(rest) = line.strip_prefix("diff --git ") {
            skipping = false;
            in_header = true;
            // Best guess until the ---/+++ lines say exactly.
            let guess = rest.rsplit_once(" b/").map(|(_, b)| b.to_string()).unwrap_or_default();
            files.push(FileDiff { path: guess, ..Default::default() });
            continue;
        }
        if line.starts_with("diff --cc ") || line.starts_with("diff --combined ") {
            skipping = true;
            continue;
        }
        if skipping {
            continue;
        }
        let Some(file) = files.last_mut() else { continue };
        if in_header {
            if let Some(p) = line.strip_prefix("+++ ") {
                if p != "/dev/null" {
                    file.path = header_path(p);
                }
                continue;
            }
            if let Some(p) = line.strip_prefix("--- ") {
                if p != "/dev/null" {
                    file.path = header_path(p);
                }
                continue;
            }
            if let Some(p) = line.strip_prefix("rename to ") {
                file.path = header_path(p);
                continue;
            }
            if line.starts_with("Binary files ") || line.starts_with("GIT binary patch") {
                file.binary = true;
                continue;
            }
        }
        if let Some(rest) = line.strip_prefix("@@ ") {
            in_header = false;
            let Some((ranges, context)) = rest.split_once(" @@") else { continue };
            let mut it = ranges.split_whitespace();
            let old = it.next().and_then(|s| s.strip_prefix('-')).map(range_start).unwrap_or(0);
            let new = it.next().and_then(|s| s.strip_prefix('+')).map(range_start).unwrap_or(0);
            old_no = old;
            new_no = new;
            file.hunks.push(Hunk {
                old_start: old,
                new_start: new,
                context: context.trim().to_string(),
                lines: Vec::new(),
            });
            continue;
        }
        let Some(hunk) = file.hunks.last_mut() else { continue };
        let (kind, text) = match line.as_bytes().first() {
            Some(b'+') => (LineKind::Added, &line[1..]),
            Some(b'-') => (LineKind::Removed, &line[1..]),
            Some(b' ') => (LineKind::Context, &line[1..]),
            Some(b'\\') => continue, // "\ No newline at end of file"
            None => (LineKind::Context, ""),
            _ => continue,
        };
        let (old, new) = match kind {
            LineKind::Added => (None, Some(new_no)),
            LineKind::Removed => (Some(old_no), None),
            _ => (Some(old_no), Some(new_no)),
        };
        if old.is_some() {
            old_no += 1;
        }
        if new.is_some() {
            new_no += 1;
        }
        hunk.lines.push(DiffLine { kind, old, new, text: text.to_string() });
    }
    files
}

/// A whole new file as a diff: every line added.
pub fn added_file(path: &str, content: &str) -> FileDiff {
    let lines: Vec<DiffLine> = content
        .lines()
        .enumerate()
        .map(|(i, l)| DiffLine { kind: LineKind::Added, old: None, new: Some(i as u32 + 1), text: l.to_string() })
        .collect();
    let hunks = if lines.is_empty() {
        vec![]
    } else {
        vec![Hunk { old_start: 0, new_start: 1, context: String::new(), lines }]
    };
    FileDiff { path: path.to_string(), hunks, binary: false }
}

/// A conflicted file, whole, with its two sides marked: ours as removed,
/// theirs as added, the markers between (VS Code's merge decorations).
pub fn conflict_file(path: &str, content: &str) -> FileDiff {
    #[derive(PartialEq)]
    enum Side {
        Outside,
        Ours,
        Base,
        Theirs,
    }
    let mut side = Side::Outside;
    let mut lines = Vec::new();
    for (i, l) in content.lines().enumerate() {
        let n = Some(i as u32 + 1);
        let marker = if l.starts_with("<<<<<<<") {
            side = Side::Ours;
            true
        } else if l.starts_with("|||||||") && side == Side::Ours {
            side = Side::Base;
            true
        } else if l.starts_with("=======") && side != Side::Outside {
            side = Side::Theirs;
            true
        } else if l.starts_with(">>>>>>>") && side != Side::Outside {
            side = Side::Outside;
            true
        } else {
            false
        };
        let kind = match (marker, &side) {
            (true, _) => LineKind::Marker,
            (false, Side::Ours) => LineKind::Removed,
            (false, Side::Theirs) => LineKind::Added,
            _ => LineKind::Context,
        };
        lines.push(DiffLine { kind, old: n, new: n, text: l.to_string() });
    }
    FileDiff { path: path.to_string(), hunks: vec![Hunk { lines, ..Default::default() }], binary: false }
}

pub fn run(root: &str, args: &[&str]) -> Result<Vec<u8>, String> {
    let out = Command::new("git")
        .args(["--no-optional-locks", "-c", "core.quotepath=off", "-C", root])
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("LC_ALL", "C")
        .output()
        .map_err(|e| format!("couldn't run git: {e}"))?;
    if out.status.success() { Ok(out.stdout) } else { Err(String::from_utf8_lossy(&out.stderr).trim().to_string()) }
}

const DIFF: &[&str] = &["diff", "--no-color", "--no-ext-diff", "--find-renames"];

/// Every unstaged (or staged) change, `-U0`, by path.
pub fn diffs(root: &str, staged: bool) -> Result<HashMap<String, FileDiff>, String> {
    let mut args = DIFF.to_vec();
    args.push("-U0");
    if staged {
        args.push("--cached");
    }
    let out = run(root, &args)?;
    Ok(parse_diff(&String::from_utf8_lossy(&out)).into_iter().map(|d| (d.path.clone(), d)).collect())
}

/// One file's diff with context, for showing.
pub fn file_diff(root: &str, e: &Entry) -> Result<FileDiff, String> {
    let full = std::path::Path::new(root).join(&e.path);
    match e.kind {
        Kind::Untracked => {
            let content = read_text(&full)?;
            Ok(match content {
                Some(c) => added_file(&e.path, &c),
                None => FileDiff { path: e.path.clone(), binary: true, ..Default::default() },
            })
        }
        Kind::Conflict => Ok(match read_text(&full)? {
            Some(c) => conflict_file(&e.path, &c),
            None => FileDiff { path: e.path.clone(), binary: true, ..Default::default() },
        }),
        _ => {
            // In a folder of repositories, asked of the file's own.
            let (repo, path) = locate(root, &e.path);
            let orig = e.orig.as_ref().map(|o| locate(root, o).1);
            let mut args = DIFF.to_vec();
            args.push("-U3");
            if e.group == Group::Staged {
                args.push("--cached");
            }
            args.push("--");
            if let Some(o) = &orig {
                args.push(o);
            }
            args.push(&path);
            let out = run(&repo, &args)?;
            let mut files = parse_diff(&String::from_utf8_lossy(&out));
            let mut d = files
                .iter()
                .position(|f| f.path == path)
                .map(|i| files.swap_remove(i))
                .or_else(|| files.pop())
                .unwrap_or_default();
            d.path = e.path.clone();
            Ok(d)
        }
    }
}

/// A text file's contents, or None for binary. Big files are cut short.
pub fn read_text(path: &std::path::Path) -> Result<Option<String>, String> {
    use std::io::Read;
    const MAX: u64 = 2 * 1024 * 1024;
    let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    f.take(MAX).read_to_end(&mut buf).map_err(|e| e.to_string())?;
    if buf.iter().take(8000).any(|b| *b == 0) {
        return Ok(None);
    }
    Ok(Some(String::from_utf8_lossy(&buf).into_owned()))
}

/// What the Changes tab can do to a repository besides staging.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    /// Commit what's staged, or everything when `all`.
    Commit {
        message: String,
        all: bool,
    },
    Push,
    Pull,
}

impl Op {
    pub fn doing(&self) -> &'static str {
        match self {
            Op::Commit { .. } => "committing",
            Op::Push => "pushing",
            Op::Pull => "pulling",
        }
    }
}

/// Run git for an op, in a new session with no controlling terminal: a
/// password or passphrase prompt then fails with a message instead of
/// writing over the dashboard and waiting for keys it will never get.
fn run_detached(root: &str, args: &[&str], input: Option<&str>) -> Result<String, String> {
    use std::io::Write;
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;
    let mut cmd = Command::new("git");
    cmd.args(["-C", root])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // SAFETY: setsid is async-signal-safe and touches no shared state.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let mut child = cmd.spawn().map_err(|e| format!("couldn't run git: {e}"))?;
    if let (Some(text), Some(mut stdin)) = (input, child.stdin.take()) {
        let _ = stdin.write_all(text.as_bytes());
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let text =
        format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)).trim().to_string();
    if out.status.success() { Ok(text) } else { Err(text) }
}

/// Carry out an op. Returns a one-line summary of what happened.
pub fn perform(root: &str, op: &Op) -> Result<String, String> {
    if run(root, &["rev-parse", "--git-dir"]).is_err() {
        return Err(format!(
            "This folder holds several repositories, so {} works on one at a time: \
             open that repository as its own workspace (ctrl-o).",
            op.doing()
        ));
    }
    match op {
        Op::Commit { message, all } => {
            if *all {
                run(root, &["add", "-A"])?;
            }
            let out = run_detached(root, &["commit", "-F", "-"], Some(message))?;
            // "[main 1a2b3c4] Subject" → "1a2b3c4 Subject"
            let first = out.lines().find(|l| l.starts_with('[')).unwrap_or("committed");
            Ok(first
                .split_once("] ")
                .map(|(head, subject)| {
                    let hash = head.rsplit(' ').next().unwrap_or("");
                    format!("Committed {hash} {subject}")
                })
                .unwrap_or_else(|| first.to_string()))
        }
        Op::Push => {
            let upstream = run(root, &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"]);
            match upstream {
                Ok(u) => {
                    run_detached(root, &["push"], None)?;
                    Ok(format!("Pushed to {}", String::from_utf8_lossy(&u).trim()))
                }
                Err(_) => {
                    // A new branch: publish it, as VS Code's "Publish Branch".
                    let branch =
                        String::from_utf8_lossy(&run(root, &["rev-parse", "--abbrev-ref", "HEAD"])?).trim().to_string();
                    let remotes = String::from_utf8_lossy(&run(root, &["remote"])?).to_string();
                    let remote = remotes
                        .lines()
                        .find(|r| *r == "origin")
                        .or_else(|| remotes.lines().next())
                        .ok_or("this repository has no remote to push to")?
                        .to_string();
                    run_detached(root, &["push", "-u", &remote, &branch], None)?;
                    Ok(format!("Published {branch} to {remote}"))
                }
            }
        }
        Op::Pull => {
            let out = run_detached(root, &["pull", "--ff-only"], None)?;
            Ok(if out.contains("Already up to date") { "Already up to date".into() } else { "Pulled".into() })
        }
    }
}

/// Throw away a file's unstaged changes. A new file goes to the Trash,
/// where it can be got back, rather than being deleted.
pub fn discard(root: &str, e: &Entry) -> Result<(), String> {
    let home = std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default());
    let trash = if cfg!(target_os = "macos") { home.join(".Trash") } else { home.join(".local/share/Trash/files") };
    discard_to(root, e, &trash)
}

fn discard_to(root: &str, e: &Entry, trash: &std::path::Path) -> Result<(), String> {
    if e.kind == Kind::Untracked {
        let path = std::path::Path::new(root).join(&e.path);
        std::fs::create_dir_all(trash).map_err(|x| x.to_string())?;
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let mut target = trash.join(&name);
        let mut n = 1;
        while target.exists() {
            target = trash.join(format!("{name} {n}"));
            n += 1;
        }
        return std::fs::rename(&path, &target).map_err(|x| x.to_string());
    }
    let (repo, path) = locate(root, &e.path);
    run(&repo, &["checkout", "--", &path]).map(|_| ())
}

/// Stage or unstage paths (files or folders).
pub fn stage(root: &str, paths: &[String], unstage: bool) -> Result<(), String> {
    for (repo, paths) in by_repo(root, paths) {
        let mut args: Vec<&str> = if unstage { vec!["reset", "-q", "--"] } else { vec!["add", "-A", "--"] };
        args.extend(paths.iter().map(String::as_str));
        run(&repo, &args)?;
    }
    Ok(())
}

/// Paths grouped by the repository holding them, as it sees them. In a
/// folder of repositories, a folder above them stands for each of them.
fn by_repo(root: &str, paths: &[String]) -> Vec<(String, Vec<String>)> {
    if run(root, &["rev-parse", "--git-dir"]).is_ok() {
        return vec![(root.to_string(), paths.to_vec())];
    }
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    let mut add = |repo: String, path: String| match out.iter_mut().find(|(r, _)| *r == repo) {
        Some((_, v)) => v.push(path),
        None => out.push((repo, vec![path])),
    };
    let repos = repos_in(root);
    for p in paths {
        let (repo, path) = locate(root, p);
        if repo != root {
            add(repo, path);
            continue;
        }
        for r in repos.iter().filter(|r| p.is_empty() || r.starts_with(&format!("{p}/"))) {
            add(format!("{root}/{r}"), ".".into());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_records() {
        let out = b"1 .M N... 100644 100644 100644 abc abc src/a.rs\x001 MM N... 100644 100644 100644 abc def b c.txt\x002 R. N... 100644 100644 100644 abc abc R100 new.rs\x00old.rs\x00u UU N... 100644 100644 100644 100644 a b c conf.rs\x00? notes/todo.md\x00";
        let e = parse_status(out);
        assert_eq!(e[0], Entry { group: Group::Changes, kind: Kind::Modified, path: "src/a.rs".into(), orig: None });
        assert_eq!((e[1].group, e[1].path.as_str()), (Group::Staged, "b c.txt"));
        assert_eq!((e[2].group, e[2].path.as_str()), (Group::Changes, "b c.txt"));
        assert_eq!(
            e[3],
            Entry { group: Group::Staged, kind: Kind::Renamed, path: "new.rs".into(), orig: Some("old.rs".into()) }
        );
        assert_eq!((e[4].group, e[4].kind, e[4].path.as_str()), (Group::Conflicts, Kind::Conflict, "conf.rs"));
        assert_eq!((e[5].kind, e[5].path.as_str()), (Kind::Untracked, "notes/todo.md"));
        assert_eq!(e.len(), 6);
    }

    #[test]
    fn unified_diffs() {
        let text = "diff --git a/src/a.rs b/src/a.rs\nindex 1..2 100644\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -10,3 +10,4 @@ fn main() {\n     let a = 1;\n-    let b = 2;\n+    let b = 3;\n+    let c = 4;\n }\n\\ No newline at end of file\ndiff --git a/gone.txt b/gone.txt\ndeleted file mode 100644\n--- a/gone.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-bye\ndiff --git a/img.png b/img.png\nBinary files a/img.png and b/img.png differ\n";
        let files = parse_diff(text);
        assert_eq!(files.len(), 3);
        let a = &files[0];
        assert_eq!(a.path, "src/a.rs");
        assert_eq!(a.hunks[0].context, "fn main() {");
        assert_eq!(a.counts(), (2, 1));
        let l = &a.hunks[0].lines;
        assert_eq!((l[1].kind, l[1].old, l[1].new), (LineKind::Removed, Some(11), None));
        assert_eq!((l[3].kind, l[3].new, l[3].text.as_str()), (LineKind::Added, Some(12), "    let c = 4;"));
        assert_eq!((l[4].old, l[4].new), (Some(12), Some(13)));
        assert_eq!(files[1].path, "gone.txt");
        assert_eq!(files[1].counts(), (0, 1));
        assert!(files[2].binary);
    }

    #[test]
    fn commits_publishes_pushes_and_pulls() {
        let base = std::env::temp_dir().join(format!("aulora-ops-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (remote, a, b) = (base.join("remote.git"), base.join("a"), base.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        let git = |dir: &std::path::Path, args: &[&str]| {
            let out = Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        git(&base, &["init", "-q", "--bare", "-b", "main", "remote.git"]);
        git(&a, &["init", "-q", "-b", "main"]);
        git(&a, &["config", "user.email", "t@t"]);
        git(&a, &["config", "user.name", "t"]);
        git(&a, &["remote", "add", "origin", remote.to_str().unwrap()]);
        let root = a.to_str().unwrap();
        std::fs::write(a.join("x.txt"), "one\n").unwrap();

        // Nothing staged: a staged-only commit fails with git's reason.
        assert!(perform(root, &Op::Commit { message: "first".into(), all: false }).is_err());
        let done = perform(root, &Op::Commit { message: "Add x.\n\nWhy it matters.".into(), all: true }).unwrap();
        assert!(done.starts_with("Committed ") && done.ends_with("Add x."), "{done}");
        // No upstream yet: published.
        assert_eq!(perform(root, &Op::Push).unwrap(), "Published main to origin");

        // Another clone pushes; pulling here fast-forwards.
        git(&base, &["clone", "-q", "-b", "main", remote.to_str().unwrap(), "b"]);
        git(&b, &["config", "user.email", "t@t"]);
        git(&b, &["config", "user.name", "t"]);
        std::fs::write(b.join("y.txt"), "two\n").unwrap();
        git(&b, &["add", "."]);
        git(&b, &["commit", "-q", "-m", "Add y."]);
        git(&b, &["push", "-q"]);
        assert_eq!(perform(root, &Op::Pull).unwrap(), "Pulled");
        assert!(a.join("y.txt").exists());
        assert_eq!(perform(root, &Op::Pull).unwrap(), "Already up to date");

        std::fs::write(a.join("x.txt"), "one more\n").unwrap();
        git(&a, &["add", "."]);
        perform(root, &Op::Commit { message: "Change x.".into(), all: false }).unwrap();
        assert_eq!(perform(root, &Op::Push).unwrap(), "Pushed to origin/main");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn discards_changes_and_trashes_new_files() {
        let base = std::env::temp_dir().join(format!("aulora-discard-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (repo, trash) = (base.join("r"), base.join("trash"));
        std::fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| {
            assert!(Command::new("git").arg("-C").arg(&repo).args(args).output().unwrap().status.success())
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        std::fs::write(repo.join("a.txt"), "one\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "init"]);
        std::fs::write(repo.join("a.txt"), "changed\n").unwrap();
        std::fs::write(repo.join("new.txt"), "fresh\n").unwrap();
        let root = repo.to_str().unwrap();
        let entry = |path: &str, kind| Entry { group: Group::Changes, kind, path: path.into(), orig: None };
        discard_to(root, &entry("a.txt", Kind::Modified), &trash).unwrap();
        assert_eq!(std::fs::read_to_string(repo.join("a.txt")).unwrap(), "one\n");
        discard_to(root, &entry("new.txt", Kind::Untracked), &trash).unwrap();
        assert!(!repo.join("new.txt").exists());
        assert_eq!(std::fs::read_to_string(trash.join("new.txt")).unwrap(), "fresh\n", "recoverable from the Trash");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn conflicts_mark_both_sides() {
        let d = conflict_file("x", "a\n<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> b\nz\n");
        let kinds: Vec<LineKind> = d.hunks[0].lines.iter().map(|l| l.kind).collect();
        use LineKind::*;
        assert_eq!(kinds, [Context, Marker, Removed, Marker, Added, Marker, Context]);
    }
}
