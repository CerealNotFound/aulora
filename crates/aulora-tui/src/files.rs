//! The Files tab's data: folders listed a level at a time (gitignore-aware,
//! as VS Code's explorer with `files.exclude` of ignored files), every file
//! for Ctrl-P, a file's text with change markers, and content search with
//! ripgrep's engine. All of it runs on the worker thread.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use crate::git::{self, LineKind};
use crate::highlight::{self, Styled};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
    /// Git ignores it (shown faded).
    pub ignored: bool,
}

/// Never listed, as in VS Code's default `files.exclude`.
pub const EXCLUDED: &[&str] = &[".git", ".DS_Store", "Thumbs.db", ".svn", ".hg"];

/// A folder's children, folders first, by name. What git ignores is
/// listed too but marked, so it can be shown faded (VS Code's explorer).
pub fn list_dir(root: &str, rel: &str) -> Result<Vec<DirEntry>, String> {
    let dir = if rel.is_empty() { Path::new(root).to_path_buf() } else { Path::new(root).join(rel) };
    let walker = ignore::WalkBuilder::new(&dir)
        .max_depth(Some(1))
        .hidden(false)
        .git_global(true)
        .require_git(false)
        .filter_entry(|e| e.file_name() != ".git")
        .build();
    let kept: std::collections::HashSet<std::ffi::OsString> =
        walker.flatten().filter(|e| e.depth() == 1).map(|e| e.file_name().to_os_string()).collect();
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir).map_err(|e| e.to_string())?.flatten() {
        let name = entry.file_name();
        if EXCLUDED.iter().any(|x| name == *x) {
            continue;
        }
        // Follow links, so a linked folder opens like a folder.
        let is_dir = std::fs::metadata(entry.path()).is_ok_and(|m| m.is_dir());
        out.push(DirEntry { ignored: !kept.contains(&name), name: name.to_string_lossy().into_owned(), is_dir });
    }
    out.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    Ok(out)
}

/// Past this many files, Ctrl-P stops indexing (a home directory, say).
pub const INDEX_LIMIT: usize = 200_000;

/// Every file that isn't ignored, relative to the root.
pub fn index(root: &str) -> Vec<String> {
    let mut out = Vec::new();
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .require_git(false)
        .filter_entry(|e| e.file_name() != ".git")
        .build();
    for entry in walker.flatten() {
        if entry.file_type().is_some_and(|t| t.is_file())
            && let Ok(rel) = entry.path().strip_prefix(root)
        {
            out.push(rel.to_string_lossy().into_owned());
            if out.len() >= INDEX_LIMIT {
                break;
            }
        }
    }
    out.sort();
    out
}

/// VS Code's quick-diff marks in the gutter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    Added,
    Modified,
    /// Lines were removed just above this one.
    Removed,
}

/// A file ready to show.
#[derive(Debug, Clone, Default)]
pub struct Text {
    pub styled: Option<Arc<Styled>>,
    pub binary: bool,
    pub lines: usize,
    /// By line number, from 1.
    pub marks: HashMap<u32, Mark>,
    pub modified: Option<std::time::SystemTime>,
    pub truncated: bool,
    /// Gone from disk: this is the text of the last commit.
    pub deleted: bool,
}

/// The file's text, highlighted, with marks for what changed since the
/// last commit.
pub fn read(root: &str, rel: &str) -> Result<Text, String> {
    let path = Path::new(root).join(rel);
    let meta = match std::fs::metadata(&path) {
        Ok(meta) => meta,
        // Deleted since the last commit (opened from the changes list):
        // what it was, as VS Code shows a deleted file.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let (repo, inside) = git::locate(root, rel);
            let Ok(old) = git::run(&repo, &["show", &format!("HEAD:./{inside}")]) else { return Err(e.to_string()) };
            if old.iter().take(8000).any(|b| *b == 0) {
                return Ok(Text { binary: true, deleted: true, ..Default::default() });
            }
            let text = shown(rel, &String::from_utf8_lossy(&old));
            return Ok(Text { deleted: true, ..text });
        }
        Err(e) => return Err(e.to_string()),
    };
    let modified = meta.modified().ok();
    let Some(content) = git::read_text(&path)? else {
        return Ok(Text { binary: true, modified, ..Default::default() });
    };
    let truncated = meta.len() > content.len() as u64;
    Ok(Text { marks: marks(root, rel, &content), modified, truncated, ..shown(rel, &content) })
}

/// A file's text, highlighted, without marks.
fn shown(rel: &str, content: &str) -> Text {
    let lines: Vec<git::DiffLine> = content
        .lines()
        .enumerate()
        .map(|(i, l)| git::DiffLine {
            kind: LineKind::Context,
            old: None,
            new: Some(i as u32 + 1),
            text: l.to_string(),
        })
        .collect();
    let n = lines.len();
    let diff = git::FileDiff {
        path: rel.to_string(),
        hunks: vec![git::Hunk { old_start: 0, new_start: 1, context: String::new(), lines }],
        binary: false,
    };
    let styled = highlight::style(&diff, |_| None);
    Text { styled: Some(Arc::new(styled)), lines: n, ..Default::default() }
}

/// Lines changed since HEAD, from `git diff -U0 HEAD`.
fn marks(root: &str, rel: &str, content: &str) -> HashMap<u32, Mark> {
    let mut out = HashMap::new();
    // In a folder of repositories, the file's own repository.
    let (repo, rel) = git::locate(root, rel);
    // A renamed file is compared with what it was. git pairs the two only
    // while they're still alike, and otherwise every line is marked new.
    if let Some(orig) = renamed_from(&repo, &rel) {
        return match git::run(&repo, &["show", &format!("HEAD:./{orig}")]) {
            Ok(old) => marks_between(&String::from_utf8_lossy(&old), content),
            Err(_) => out,
        };
    }
    let Ok(text) = git::run(&repo, &["diff", "--no-color", "--no-ext-diff", "-U0", "HEAD", "--", &rel]) else {
        return out;
    };
    for d in git::parse_diff(&String::from_utf8_lossy(&text)) {
        for h in d.hunks {
            let added: Vec<u32> = h.lines.iter().filter(|l| l.kind == LineKind::Added).filter_map(|l| l.new).collect();
            let removed = h.lines.iter().filter(|l| l.kind == LineKind::Removed).count();
            if added.is_empty() && removed > 0 {
                out.insert(h.new_start.max(1) + u32::from(h.new_start > 0), Mark::Removed);
            }
            for (i, n) in added.iter().enumerate() {
                out.insert(*n, if i < removed { Mark::Modified } else { Mark::Added });
            }
        }
    }
    out
}

/// The same marks, from two texts.
fn marks_between(old: &str, new: &str) -> HashMap<u32, Mark> {
    use similar::{DiffOp, TextDiff};
    let mut out = HashMap::new();
    for op in TextDiff::from_lines(old, new).ops() {
        let (old_len, new_index, new_len) = match *op {
            DiffOp::Equal { .. } => continue,
            DiffOp::Delete { old_len, new_index, .. } => (old_len, new_index, 0),
            DiffOp::Insert { new_index, new_len, .. } => (0, new_index, new_len),
            DiffOp::Replace { old_len, new_index, new_len, .. } => (old_len, new_index, new_len),
        };
        if new_len == 0 {
            out.insert(new_index as u32 + 1, Mark::Removed);
        }
        for i in 0..new_len {
            out.insert((new_index + i) as u32 + 1, if i < old_len { Mark::Modified } else { Mark::Added });
        }
    }
    out
}

/// Where a file renamed in the index was renamed from.
fn renamed_from(repo: &str, rel: &str) -> Option<String> {
    let args = ["diff", "--cached", "--find-renames", "--relative", "--name-status", "-z", "HEAD"];
    let out = git::run(repo, &args).ok()?;
    let out = String::from_utf8_lossy(&out);
    let mut fields = out.split('\0');
    while let Some(status) = fields.next() {
        if status.starts_with('R') || status.starts_with('C') {
            let (from, to) = (fields.next()?, fields.next()?);
            if to == rel {
                return Some(from.to_string());
            }
        } else {
            fields.next();
        }
    }
    None
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    pub path: String,
    pub line: u32,
    pub text: String,
}

/// Stop collecting after this many matches.
pub const SEARCH_LIMIT: usize = 2000;

/// Search file contents like ripgrep: gitignore-aware, the query taken
/// literally, case-insensitive unless it has a capital (smart case).
pub fn search(root: &str, query: &str) -> Result<(Vec<Match>, bool), String> {
    use grep_regex::RegexMatcherBuilder;
    use grep_searcher::sinks::UTF8;
    use grep_searcher::{BinaryDetection, SearcherBuilder};
    let matcher =
        RegexMatcherBuilder::new().case_smart(true).fixed_strings(true).build(query).map_err(|e| e.to_string())?;
    let mut searcher = SearcherBuilder::new().line_number(true).binary_detection(BinaryDetection::quit(0)).build();
    let mut out = Vec::new();
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .require_git(false)
        .sort_by_file_name(|a, b| a.cmp(b))
        .filter_entry(|e| e.file_name() != ".git")
        .build();
    for entry in walker.flatten() {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let rel = entry.path().strip_prefix(root).unwrap_or(entry.path()).to_string_lossy().into_owned();
        let mut full = false;
        let _ = searcher.search_path(
            &matcher,
            entry.path(),
            UTF8(|line, text| {
                out.push(Match { path: rel.clone(), line: line as u32, text: text.trim_end().to_string() });
                full = out.len() >= SEARCH_LIMIT;
                Ok(!full)
            }),
        );
        if full {
            return Ok((out, true));
        }
    }
    Ok((out, false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_indexes_reads_and_searches() {
        let dir = std::env::temp_dir().join(format!("aulora-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::create_dir_all(dir.join("target")).unwrap();
        let root = dir.to_string_lossy().to_string();
        let git = |args: &[&str]| {
            assert!(
                std::process::Command::new("git").arg("-C").arg(&root).args(args).output().unwrap().status.success()
            )
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        std::fs::write(dir.join(".gitignore"), "target/\n").unwrap();
        std::fs::write(dir.join("target/big.bin"), "ignored").unwrap();
        std::fs::write(dir.join("src/main.rs"), "fn main() {\n    println!(\"hi\");\n}\n").unwrap();
        std::fs::write(dir.join("README.md"), "# Demo\nSay Hi.\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "init"]);
        std::fs::write(dir.join("src/main.rs"), "fn main() {\n    println!(\"hello\");\n    run();\n}\n").unwrap();

        let top: Vec<(String, bool, bool)> =
            list_dir(&root, "").unwrap().into_iter().map(|e| (e.name, e.is_dir, e.ignored)).collect();
        // Ignored folders are listed, marked, as VS Code shows them faded;
        // .git never is.
        assert_eq!(
            top,
            [
                ("src".into(), true, false),
                ("target".into(), true, true),
                (".gitignore".into(), false, false),
                ("README.md".into(), false, false)
            ]
        );
        assert_eq!(index(&root), [".gitignore", "README.md", "src/main.rs"]);

        let t = read(&root, "src/main.rs").unwrap();
        assert_eq!(t.lines, 4);
        assert_eq!(t.marks.get(&2), Some(&Mark::Modified));
        assert_eq!(t.marks.get(&3), Some(&Mark::Added));

        let (hits, more) = search(&root, "hi").unwrap();
        assert!(!more);
        let found: Vec<(&str, u32)> = hits.iter().map(|m| (m.path.as_str(), m.line)).collect();
        assert_eq!(found, [("README.md", 2)], "smart case: \"hi\" matches Hi; ignored files are skipped");
        let (hits, _) = search(&root, "Hi").unwrap();
        assert_eq!(hits.len(), 1);
        // A renamed one only marks what changed in it.
        git(&["mv", "src/main.rs", "src/app.rs"]);
        let marks = read(&root, "src/app.rs").unwrap().marks;
        assert_eq!(marks, HashMap::from([(2, Mark::Modified), (3, Mark::Added)]));
        // A deleted file shows what it was.
        std::fs::remove_file(dir.join("README.md")).unwrap();
        let t = read(&root, "README.md").unwrap();
        assert!(t.deleted);
        assert_eq!(t.lines, 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

// ---------------------------------------------------------------------------
// State kept by the app

use std::collections::HashSet;

use aulora_core::model::Ms;

use crate::app::{App, Conn};
use crate::changes::Job;

pub enum Loaded<T> {
    Loading,
    Ready(T, Ms),
    Failed(String),
}

#[derive(Default)]
pub struct Tree {
    pub dirs: HashMap<String, Loaded<Vec<DirEntry>>>,
    pub index: Option<Arc<Vec<String>>>,
    pub indexing: bool,
    /// File text by path, with when it was last checked for changes.
    pub texts: HashMap<String, Loaded<Arc<Text>>>,
}

impl Tree {
    /// Whether `rel` is a file, going by its folder's listing (or, before
    /// that's loaded, by not being a listed folder itself).
    pub fn is_file(&self, rel: &str) -> bool {
        let (dir, name) = rel.rsplit_once('/').unwrap_or(("", rel));
        match self.dirs.get(dir) {
            Some(Loaded::Ready(entries, _)) => entries.iter().any(|e| e.name == name && !e.is_dir),
            _ => !self.dirs.contains_key(rel),
        }
    }
}

pub struct Search {
    pub query: String,
    pub editing: bool,
    /// The query the results are for.
    pub searched: String,
    pub results: Option<Result<(Vec<Match>, bool), String>>,
    pub selected: usize,
    pub scroll: usize,
}

#[derive(Default)]
pub struct View {
    /// Open folders (relative paths); the root is always open.
    pub expanded: HashSet<String>,
    pub selected: Option<String>,
    pub scroll: usize,
    pub shown: Option<String>,
    pub file_scroll: usize,
    pub focus_file: bool,
    /// A line to point at (a search result, a Ctrl-P `file:line`).
    pub mark_line: Option<u32>,
    /// Point at the file's first change once its text is read: a changed
    /// file clicked before then can't know where that is yet.
    pub to_first_change: bool,
    pub tree_height: usize,
    pub file_height: usize,
    pub search: Option<Search>,
}

#[derive(Debug, Clone)]
pub struct Row {
    pub rel: String,
    pub name: String,
    pub depth: usize,
    pub is_dir: bool,
    pub expanded: bool,
    pub loading: bool,
    /// Git ignores it, or a folder it's in.
    pub ignored: bool,
}

/// How long a folder listing or a file's text is trusted before checking.
const FRESH: Ms = 3000;

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() { name.to_string() } else { format!("{dir}/{name}") }
}

impl App {
    pub fn send_folders_job(&mut self) {
        if self.conn == Conn::Demo {
            self.folders = Some(vec![]);
            return;
        }
        self.send_job(Job::Folders);
    }

    fn send_job(&mut self, job: Job) {
        if self.worker.is_none() {
            self.worker = crate::changes::Worker::spawn();
        }
        if let Some(w) = self.worker.as_mut() {
            w.send(job);
        }
    }

    pub fn files_view(&mut self, root: &str) -> &mut View {
        self.files_view.entry(root.to_string()).or_default()
    }

    /// The tree as shown, asking for folders that aren't loaded or are stale.
    pub fn file_rows(&mut self, root: &str) -> Vec<Row> {
        let expanded = self.files_view(root).expanded.clone();
        let now = self.now;
        let mut wanted = Vec::new();
        let mut rows = Vec::new();
        let tree = self.files.entry(root.to_string()).or_default();
        fn walk(
            tree: &Tree,
            expanded: &HashSet<String>,
            dir: &str,
            // How deep, and whether a folder above is ignored.
            (depth, ignored): (usize, bool),
            rows: &mut Vec<Row>,
            wanted: &mut Vec<String>,
            now: Ms,
        ) {
            match tree.dirs.get(dir) {
                Some(Loaded::Ready(entries, at)) => {
                    if now - at > FRESH {
                        wanted.push(dir.to_string());
                    }
                    for e in entries {
                        let rel = join(dir, &e.name);
                        let open = e.is_dir && expanded.contains(&rel);
                        let loading = open && !matches!(tree.dirs.get(&rel), Some(Loaded::Ready(..)));
                        let ignored = ignored || e.ignored;
                        rows.push(Row {
                            rel: rel.clone(),
                            name: e.name.clone(),
                            depth,
                            is_dir: e.is_dir,
                            expanded: open,
                            loading,
                            ignored,
                        });
                        if open {
                            walk(tree, expanded, &rel, (depth + 1, ignored), rows, wanted, now);
                        }
                    }
                }
                Some(Loaded::Loading) => {}
                _ => wanted.push(dir.to_string()),
            }
        }
        walk(tree, &expanded, "", (0, false), &mut rows, &mut wanted, now);
        if self.conn != Conn::Demo {
            for rel in wanted {
                let tree = self.files.entry(root.to_string()).or_default();
                if !matches!(tree.dirs.get(&rel), Some(Loaded::Loading)) {
                    // Keep showing the old listing while the new one loads.
                    if !tree.dirs.contains_key(&rel) {
                        tree.dirs.insert(rel.clone(), Loaded::Loading);
                    } else if let Some(Loaded::Ready(_, at)) = tree.dirs.get_mut(&rel) {
                        *at = now;
                    }
                    self.send_job(Job::Dir { root: root.to_string(), rel });
                }
            }
        } else {
            let tree = self.files.entry(root.to_string()).or_default();
            if tree.dirs.is_empty() {
                for (dir, entries) in crate::demo::dirs(root) {
                    tree.dirs.insert(dir, Loaded::Ready(entries, Ms::MAX / 2));
                }
                return self.file_rows(root);
            }
        }
        rows
    }

    /// A file's text, asking for it (or for a fresher copy) as needed.
    pub fn file_text(&mut self, root: &str, rel: &str) -> Option<&Loaded<Arc<Text>>> {
        let now = self.now;
        let demo = self.conn == Conn::Demo;
        let tree = self.files.entry(root.to_string()).or_default();
        let job = match tree.texts.get_mut(rel) {
            None if demo => {
                let text = crate::demo::file_text(root, rel);
                tree.texts.insert(rel.to_string(), Loaded::Ready(Arc::new(text), Ms::MAX / 2));
                None
            }
            None => {
                tree.texts.insert(rel.to_string(), Loaded::Loading);
                Some(Job::Read { root: root.to_string(), rel: rel.to_string(), known: None })
            }
            Some(Loaded::Ready(text, at)) if now - *at > FRESH && !demo => {
                *at = now;
                Some(Job::Read { root: root.to_string(), rel: rel.to_string(), known: text.modified })
            }
            _ => None,
        };
        if let Some(job) = job {
            self.send_job(job);
        }
        self.files.get(root)?.texts.get(rel)
    }

    /// The first changed line of a file, if its text is already read.
    pub fn first_change_line(&self, root: &str, rel: &str) -> Option<u32> {
        match self.files.get(root)?.texts.get(rel)? {
            Loaded::Ready(text, _) => text.marks.keys().min().copied(),
            _ => None,
        }
    }

    /// Every file in the project, for Ctrl-P; None while it's being read.
    pub fn file_index(&mut self, root: &str) -> Option<Arc<Vec<String>>> {
        if self.conn == Conn::Demo {
            let tree = self.files.entry(root.to_string()).or_default();
            if tree.index.is_none() {
                tree.index = Some(Arc::new(crate::demo::index(root)));
            }
        }
        let tree = self.files.entry(root.to_string()).or_default();
        if tree.index.is_none() && !tree.indexing {
            tree.indexing = true;
            self.send_job(Job::Index { root: root.to_string() });
        }
        self.files.get(root)?.index.clone()
    }

    pub fn start_search(&mut self, root: &str, query: String) {
        let demo = self.conn == Conn::Demo;
        let v = self.files_view(root);
        let Some(s) = v.search.as_mut() else { return };
        s.searched = query.clone();
        s.results = None;
        s.selected = 0;
        s.scroll = 0;
        if demo {
            s.results = Some(Ok((crate::demo::search(root, &query), false)));
            return;
        }
        self.send_job(Job::Search { root: root.to_string(), query });
    }

    pub(crate) fn on_dir(&mut self, root: String, rel: String, result: Result<Vec<DirEntry>, String>) {
        let now = self.now;
        let tree = self.files.entry(root).or_default();
        tree.dirs.insert(
            rel,
            match result {
                Ok(entries) => Loaded::Ready(entries, now),
                Err(e) => Loaded::Failed(e),
            },
        );
    }

    pub(crate) fn on_index(&mut self, root: String, files: Arc<Vec<String>>) {
        let tree = self.files.entry(root).or_default();
        tree.index = Some(files);
        tree.indexing = false;
        if let Some(f) = self.finder.as_mut() {
            f.dirty = true;
        }
    }

    pub(crate) fn on_read(&mut self, root: String, rel: String, result: Option<Result<Text, String>>) {
        let now = self.now;
        let tree = self.files.entry(root).or_default();
        match result {
            None => {}
            Some(Ok(text)) => {
                tree.texts.insert(rel, Loaded::Ready(Arc::new(text), now));
            }
            Some(Err(e)) => {
                tree.texts.insert(rel, Loaded::Failed(e));
            }
        }
        if tree.texts.len() > 64 {
            tree.texts.retain(|_, t| matches!(t, Loaded::Ready(_, at) if now - *at < 60_000));
        }
    }

    pub(crate) fn on_search(&mut self, root: String, query: String, result: Result<(Vec<Match>, bool), String>) {
        if let Some(s) = self.files_view(&root).search.as_mut()
            && s.searched == query
        {
            s.results = Some(result);
        }
    }

    /// Show a file in the Files tab: open its folders, select it, and point
    /// at a line if given.
    pub fn reveal_file(&mut self, root: &str, rel: &str, line: Option<u32>) {
        let v = self.files_view(root);
        let mut dir = String::new();
        let parts: Vec<&str> = rel.split('/').collect();
        for part in &parts[..parts.len().saturating_sub(1)] {
            dir = join(&dir, part);
            v.expanded.insert(dir.clone());
        }
        v.selected = Some(rel.to_string());
        v.focus_file = true;
        v.mark_line = line;
        v.to_first_change = false;
        let height = v.file_height.max(10);
        v.file_scroll = line.map_or(0, |l| (l as usize).saturating_sub(height / 3));
        self.open_view(root, crate::layout::View::Files, true);
        self.focus = crate::app::Focus::Main;
    }
}

/// `~/x` → `/Users/you/x`.
pub fn expand_home(p: &str) -> String {
    match (p.strip_prefix('~'), std::env::var("HOME")) {
        (Some(rest), Ok(home)) if rest.is_empty() || rest.starts_with('/') => format!("{home}{rest}"),
        _ => p.to_string(),
    }
}

/// Git repositories in the usual places under the home folder, for opening
/// as projects: up to three levels deep, not inside other repositories,
/// skipping hidden folders and the ones that hold media or apps.
pub fn find_repos() -> Vec<String> {
    let Ok(home) = std::env::var("HOME") else { return vec![] };
    const SKIP: &[&str] =
        &["node_modules", "Library", "Applications", "Pictures", "Music", "Movies", "target", "vendor"];
    let mut out = Vec::new();
    let mut stack: Vec<(std::path::PathBuf, usize)> = vec![(std::path::PathBuf::from(&home), 0)];
    while let Some((dir, depth)) = stack.pop() {
        if out.len() >= 2000 {
            break;
        }
        if depth > 0 && dir.join(".git").exists() {
            out.push(dir.to_string_lossy().into_owned());
            continue;
        }
        if depth >= 3 {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || SKIP.contains(&name.as_str()) {
                continue;
            }
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                stack.push((e.path(), depth + 1));
            }
        }
    }
    out.sort();
    out
}
