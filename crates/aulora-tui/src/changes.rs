//! The Changes tab's data: a worker thread that reads git and transcripts
//! off the UI thread, and the tree the tab shows (VS Code's Source Control).

use std::collections::{BTreeMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use aulora_core::model::Ms;

use crate::app::{App, Conn};
use crate::attrib;
use crate::git::{self, Entry, Group, Kind};
use crate::highlight::{self, Styled};

/// A changed file, as the tree shows it.
#[derive(Debug, Clone)]
pub struct FileChange {
    pub entry: Entry,
    pub added: u32,
    pub removed: u32,
    pub binary: bool,
    /// Sessions that wrote this change, most lines first.
    pub sessions: Vec<String>,
    /// Changes when the change does, so a shown diff knows it's stale.
    pub sig: u64,
}

impl FileChange {
    pub fn id(&self) -> String {
        format!("f/{}/{}", self.entry.group.key(), self.entry.path)
    }

    pub fn name(&self) -> &str {
        self.entry.path.rsplit('/').next().unwrap_or(&self.entry.path)
    }
}

/// (session id, transcript path) for every session in the project.
pub type Sessions = Vec<(String, String)>;

pub enum Job {
    Status {
        root: String,
        sessions: Sessions,
    },
    Diff {
        root: String,
        file: FileChange,
        sessions: Sessions,
    },
    Stage {
        root: String,
        paths: Vec<String>,
        unstage: bool,
    },
    Op {
        root: String,
        op: git::Op,
    },
    /// Git repositories on disk, for opening projects.
    Folders,
    /// List a folder for the Files tab.
    Dir {
        root: String,
        rel: String,
    },
    /// Every file, for Ctrl-P.
    Index {
        root: String,
    },
    /// A file to show; skipped when unchanged since `known`.
    Read {
        root: String,
        rel: String,
        known: Option<std::time::SystemTime>,
    },
    Search {
        root: String,
        query: String,
    },
}

pub enum Done {
    Folders(Vec<String>),
    Dir {
        root: String,
        rel: String,
        result: Result<Vec<crate::files::DirEntry>, String>,
    },
    Index {
        root: String,
        files: Arc<Vec<String>>,
    },
    Read {
        root: String,
        rel: String,
        /// None when the file hasn't changed.
        result: Option<Result<crate::files::Text, String>>,
    },
    Search {
        root: String,
        query: String,
        result: Result<(Vec<crate::files::Match>, bool), String>,
    },
    Op {
        root: String,
        op: git::Op,
        result: Result<String, String>,
    },
    Status {
        root: String,
        result: Result<Vec<FileChange>, String>,
    },
    Diff {
        root: String,
        id: String,
        sig: u64,
        result: Result<Arc<Styled>, String>,
    },
    Staged {
        root: String,
        unstage: bool,
        result: Result<(), String>,
    },
    /// A job replaced by a newer one of the same kind before it ran.
    Skipped,
}

pub struct Worker {
    tx: Sender<Job>,
    rx: Receiver<Done>,
    pending: usize,
}

impl Worker {
    pub fn spawn() -> Option<Worker> {
        let (tx, jobs) = channel();
        let (done, rx) = channel();
        std::thread::Builder::new().name("aulora-changes".into()).spawn(move || work(jobs, done)).ok()?;
        Some(Worker { tx, rx, pending: 0 })
    }

    pub fn send(&mut self, job: Job) {
        if self.tx.send(job).is_ok() {
            self.pending += 1;
        }
    }

    pub fn try_recv(&mut self) -> Option<Done> {
        let d = self.rx.try_recv().ok()?;
        self.pending = self.pending.saturating_sub(1);
        Some(d)
    }

    pub fn busy(&self) -> bool {
        self.pending > 0
    }
}

fn work(jobs: Receiver<Job>, done: Sender<Done>) {
    highlight::warm_up();
    let mut index = attrib::Index::default();
    while let Ok(first) = jobs.recv() {
        let mut batch = vec![first];
        batch.extend(jobs.try_iter());
        // Only the newest status per project and the newest diff matter.
        let mut keep = vec![true; batch.len()];
        let mut seen_status = HashSet::new();
        let mut seen_diff = false;
        let mut seen_once = HashSet::new();
        for (i, job) in batch.iter().enumerate().rev() {
            match job {
                // The newest of each kind wins; folders and indexes once each.
                Job::Read { .. } => keep[i] = seen_once.insert("read".to_string()),
                Job::Search { .. } => keep[i] = seen_once.insert("search".to_string()),
                Job::Dir { root, rel } => keep[i] = seen_once.insert(format!("dir\n{root}\n{rel}")),
                Job::Index { root } => keep[i] = seen_once.insert(format!("index\n{root}")),
                Job::Folders => keep[i] = seen_once.insert("folders".to_string()),
                Job::Status { root, .. } => keep[i] = seen_status.insert(root.clone()),
                Job::Diff { .. } => {
                    keep[i] = !seen_diff;
                    seen_diff = true;
                }
                Job::Stage { .. } | Job::Op { .. } => {}
            }
        }
        for (job, keep) in batch.into_iter().zip(keep) {
            let out = if !keep {
                Done::Skipped
            } else {
                match job {
                    Job::Folders => Done::Folders(crate::files::find_repos()),
                    Job::Dir { root, rel } => {
                        let result = crate::files::list_dir(&root, &rel);
                        Done::Dir { root, rel, result }
                    }
                    Job::Index { root } => {
                        let files = Arc::new(crate::files::index(&root));
                        Done::Index { root, files }
                    }
                    Job::Read { root, rel, known } => {
                        let now =
                            std::fs::metadata(std::path::Path::new(&root).join(&rel)).and_then(|m| m.modified()).ok();
                        let result = (known.is_none() || now != known).then(|| crate::files::read(&root, &rel));
                        Done::Read { root, rel, result }
                    }
                    Job::Search { root, query } => {
                        let result = crate::files::search(&root, &query);
                        Done::Search { root, query, result }
                    }
                    Job::Op { root, op } => {
                        let result = git::perform(&root, &op);
                        Done::Op { root, op, result }
                    }
                    Job::Stage { root, paths, unstage } => {
                        let result = git::stage(&root, &paths, unstage);
                        Done::Staged { root, unstage, result }
                    }
                    Job::Status { root, sessions } => {
                        let result = status(&root, &sessions, &mut index);
                        Done::Status { root, result }
                    }
                    Job::Diff { root, file, sessions } => {
                        let result = diff(&root, &file, &sessions, &mut index);
                        Done::Diff { root, id: file.id(), sig: file.sig, result }
                    }
                }
            };
            if done.send(out).is_err() {
                return;
            }
        }
    }
}

fn hash_of(x: impl Hash) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    x.hash(&mut h);
    h.finish()
}

fn among(sessions: &Sessions) -> HashSet<String> {
    sessions.iter().map(|(id, _)| id.clone()).collect()
}

/// Untracked files past this many are listed without reading them.
const READ_UNTRACKED: usize = 400;

fn status(root: &str, sessions: &Sessions, index: &mut attrib::Index) -> Result<Vec<FileChange>, String> {
    index.update(sessions);
    let among = among(sessions);
    let out = git::run(root, &["status", "--porcelain=v2", "-z", "--untracked-files=all"])?;
    let entries = git::parse_status(&out);
    let unstaged = git::diffs(root, false)?;
    let staged = git::diffs(root, true)?;
    let mut untracked_read = 0;
    let mut files = Vec::with_capacity(entries.len());
    for entry in entries {
        let abs = format!("{root}/{}", entry.path);
        let mut f = FileChange { entry, added: 0, removed: 0, binary: false, sessions: vec![], sig: 0 };
        match f.entry.kind {
            Kind::Untracked => {
                untracked_read += 1;
                if untracked_read <= READ_UNTRACKED {
                    match git::read_text(std::path::Path::new(&abs)) {
                        Ok(Some(text)) => {
                            f.added = text.lines().count() as u32;
                            f.sessions = index.attribute(&abs, text.lines(), &among);
                            f.sig = hash_of(&text);
                        }
                        Ok(None) => f.binary = true,
                        Err(_) => {}
                    }
                }
            }
            Kind::Conflict => {
                let meta = std::fs::metadata(&abs).ok();
                f.sig = hash_of((meta.as_ref().map(|m| m.len()), meta.and_then(|m| m.modified().ok())));
            }
            _ => {
                let diffs = if f.entry.group == Group::Staged { &staged } else { &unstaged };
                if let Some(d) = diffs.get(&f.entry.path) {
                    (f.added, f.removed) = d.counts();
                    f.binary = d.binary;
                    f.sessions = index.attribute(&abs, d.added_lines(), &among);
                    f.sig = hash_of(&d.hunks);
                }
            }
        }
        files.push(f);
    }
    Ok(files)
}

fn diff(root: &str, file: &FileChange, sessions: &Sessions, index: &mut attrib::Index) -> Result<Arc<Styled>, String> {
    index.update(sessions);
    let among = among(sessions);
    let d = git::file_diff(root, &file.entry)?;
    let abs = format!("{root}/{}", file.entry.path);
    let styled = highlight::style(&d, |h| {
        let added = h.lines.iter().filter(|l| l.kind == git::LineKind::Added).map(|l| l.text.as_str());
        index.attribute(&abs, added, &among).into_iter().next()
    });
    Ok(Arc::new(styled))
}

// ---------------------------------------------------------------------------
// The tree

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    Group(Group),
    Dir(Group, String),
    /// Index into the project's files.
    File(usize),
}

#[derive(Debug, Clone)]
pub struct Row {
    pub id: String,
    pub node: Node,
    pub depth: usize,
    pub label: String,
    pub expanded: bool,
    /// The most important change inside (folders), or the file's own.
    pub kind: Option<Kind>,
    pub added: u32,
    pub removed: u32,
    pub count: usize,
    pub sessions: Vec<String>,
}

#[derive(Default)]
struct Dir {
    dirs: BTreeMap<String, Dir>,
    files: Vec<usize>,
}

impl Dir {
    fn rollup(&self, files: &[FileChange]) -> (u32, u32, usize, Option<Kind>, Vec<String>) {
        let mut r = (0, 0, 0, None::<Kind>, Vec::<String>::new());
        let mut add = |f: &FileChange| {
            r.0 += f.added;
            r.1 += f.removed;
            r.2 += 1;
            r.3 = Some(r.3.map_or(f.entry.kind, |k| k.min(f.entry.kind)));
            for s in &f.sessions {
                if !r.4.contains(s) {
                    r.4.push(s.clone());
                }
            }
        };
        let mut stack = vec![self];
        while let Some(d) = stack.pop() {
            d.files.iter().for_each(|&i| add(&files[i]));
            stack.extend(d.dirs.values());
        }
        r
    }
}

pub fn group_id(g: Group) -> String {
    format!("g/{}", g.key())
}

/// The rows to show: groups, then folders (compressed when they hold just
/// one folder) and files, folders first, each sorted by name.
pub fn tree(files: &[FileChange], visible: &[usize], collapsed: &HashSet<String>) -> Vec<Row> {
    let mut rows = Vec::new();
    for group in [Group::Conflicts, Group::Staged, Group::Changes] {
        let mut root = Dir::default();
        for &i in visible.iter().filter(|&&i| files[i].entry.group == group) {
            let mut d = &mut root;
            let parts: Vec<&str> = files[i].entry.path.split('/').collect();
            for part in &parts[..parts.len() - 1] {
                d = d.dirs.entry(part.to_string()).or_default();
            }
            d.files.push(i);
        }
        if root.files.is_empty() && root.dirs.is_empty() {
            continue;
        }
        let id = group_id(group);
        let expanded = !collapsed.contains(&id);
        let (added, removed, count, kind, sessions) = root.rollup(files);
        rows.push(Row {
            id,
            node: Node::Group(group),
            depth: 0,
            label: group.label().to_string(),
            expanded,
            kind,
            added,
            removed,
            count,
            sessions,
        });
        if expanded {
            emit(&root, group, "", 1, files, collapsed, &mut rows);
        }
    }
    rows
}

fn emit(
    dir: &Dir,
    group: Group,
    prefix: &str,
    depth: usize,
    files: &[FileChange],
    collapsed: &HashSet<String>,
    rows: &mut Vec<Row>,
) {
    for (name, mut sub) in &dir.dirs {
        let mut label = name.clone();
        let mut path = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
        while sub.files.is_empty() && sub.dirs.len() == 1 {
            let (n, s) = sub.dirs.iter().next().expect("one folder");
            label = format!("{label}/{n}");
            path = format!("{path}/{n}");
            sub = s;
        }
        let id = format!("d/{}/{path}", group.key());
        let expanded = !collapsed.contains(&id);
        let (added, removed, count, kind, sessions) = sub.rollup(files);
        rows.push(Row {
            id,
            node: Node::Dir(group, path.clone()),
            depth,
            label,
            expanded,
            kind,
            added,
            removed,
            count,
            sessions,
        });
        if expanded {
            emit(sub, group, &path, depth + 1, files, collapsed, rows);
        }
    }
    let mut own: Vec<usize> = dir.files.clone();
    own.sort_by_key(|&i| files[i].name().to_lowercase());
    for i in own {
        let f = &files[i];
        rows.push(Row {
            id: f.id(),
            node: Node::File(i),
            depth,
            label: f.name().to_string(),
            expanded: false,
            kind: Some(f.entry.kind),
            added: f.added,
            removed: f.removed,
            count: 1,
            sessions: f.sessions.clone(),
        });
    }
}

// ---------------------------------------------------------------------------
// State kept by the app

#[derive(Default)]
pub struct Project {
    pub files: Vec<FileChange>,
    pub loaded: bool,
    pub loading: bool,
    pub error: Option<String>,
    pub requested_at: Ms,
    /// What the daemon's last git poll said, when we last asked.
    pub git_sig: u64,
    /// A commit, push or pull under way.
    pub running: Option<git::Op>,
    /// What went wrong with the last one: (what, git's output).
    pub op_error: Option<(String, String)>,
}

#[derive(Default)]
pub struct View {
    pub selected: Option<String>,
    pub collapsed: HashSet<String>,
    pub scroll: usize,
    pub diff_scroll: usize,
    pub focus_diff: bool,
    pub tree_height: usize,
    pub diff_height: usize,
    /// The selection the tree last scrolled to; the wheel can move away from it.
    pub shown: Option<String>,
}

pub enum DiffState {
    Loading,
    Ready(Arc<Styled>),
    Failed(String),
}

/// How often the shown project re-reads git while you watch.
const REFRESH: Ms = 4000;

impl App {
    fn worker(&mut self) -> Option<&mut Worker> {
        if self.worker.is_none() {
            self.worker = Worker::spawn();
        }
        self.worker.as_mut()
    }

    pub fn changes_busy(&self) -> bool {
        self.worker.as_ref().is_some_and(Worker::busy)
    }

    fn project_sessions(&self, root: &str) -> Sessions {
        let Some(snap) = &self.snap else { return vec![] };
        snap.sessions
            .iter()
            .filter(|s| s.project_root == root)
            .filter_map(|s| Some((s.id.clone(), s.transcript_path.clone()?)))
            .collect()
    }

    /// How many files changed: what the tab read, or until it has, the
    /// daemon's last git poll (which counts an untracked folder once).
    pub fn change_count(&self, root: &str) -> Option<u32> {
        if let Some(p) = self.changes.get(root).filter(|p| p.loaded && p.error.is_none()) {
            return Some(p.files.len() as u32);
        }
        let g = self.project(root)?.git.as_ref()?;
        Some(g.staged + g.modified + g.untracked + g.conflicts)
    }

    /// Keep a project's changes current: on first look, when the daemon's
    /// git poll sees something new, and every few seconds while shown.
    pub fn refresh_changes(&mut self, root: &str) {
        let git_sig = hash_of(self.project(root).and_then(|p| p.git.as_ref()).map(|g| format!("{g:?}")));
        let now = self.now;
        if self.conn == Conn::Demo {
            let p = self.changes.entry(root.to_string()).or_default();
            if !p.loaded {
                p.files = crate::demo::changes(root);
                p.loaded = true;
            }
            return;
        }
        let p = self.changes.entry(root.to_string()).or_default();
        let due = !p.loaded || p.git_sig != git_sig || now - p.requested_at > REFRESH;
        if !due || p.loading {
            return;
        }
        p.loading = true;
        p.requested_at = now;
        p.git_sig = git_sig;
        let sessions = self.project_sessions(root);
        if let Some(w) = self.worker() {
            w.send(Job::Status { root: root.to_string(), sessions });
        }
    }

    /// Take in whatever the worker has finished. Returns true if anything did.
    pub fn poll_changes(&mut self) -> bool {
        let mut any = false;
        while let Some(done) = self.worker.as_mut().and_then(Worker::try_recv) {
            any = true;
            match done {
                Done::Status { root, result } => {
                    let p = self.changes.entry(root.clone()).or_default();
                    p.loading = false;
                    match result {
                        Ok(files) => {
                            p.files = files;
                            p.loaded = true;
                            p.error = None;
                        }
                        Err(e) => {
                            p.loaded = true;
                            p.error = Some(e);
                        }
                    }
                }
                Done::Diff { root, id, sig, result } => {
                    let state = match result {
                        Ok(d) => DiffState::Ready(d),
                        Err(e) => DiffState::Failed(e),
                    };
                    if self.diffs.len() > 256 {
                        self.diffs.clear();
                    }
                    self.diffs.insert(format!("{root}\n{id}"), (sig, state));
                }
                Done::Staged { root, unstage, result } => {
                    if let Err(e) = result {
                        self.error(format!("git {}: {e}", if unstage { "reset" } else { "add" }));
                    }
                    // See the result right away.
                    if let Some(p) = self.changes.get_mut(&root) {
                        p.requested_at = 0;
                    }
                }
                Done::Op { root, op, result } => {
                    let p = self.changes.entry(root.clone()).or_default();
                    p.running = None;
                    p.requested_at = 0;
                    match result {
                        Ok(summary) => {
                            p.op_error = None;
                            if matches!(op, git::Op::Commit { .. }) {
                                self.commit_drafts.remove(&root);
                            }
                            self.toast(summary);
                        }
                        Err(out) => {
                            if let git::Op::Commit { message, .. } = &op {
                                // Keep the message to fix and try again.
                                self.commit_drafts.insert(root.clone(), message.clone());
                            }
                            let p = self.changes.entry(root).or_default();
                            p.op_error = Some((format!("{} failed", op.doing()), out));
                        }
                    }
                }
                Done::Folders(list) => {
                    self.folders = Some(list);
                    if let Some(f) = self.finder.as_mut() {
                        f.dirty = true;
                    }
                }
                Done::Dir { root, rel, result } => self.on_dir(root, rel, result),
                Done::Index { root, files } => self.on_index(root, files),
                Done::Read { root, rel, result } => self.on_read(root, rel, result),
                Done::Search { root, query, result } => self.on_search(root, query, result),
                Done::Skipped => {}
            }
        }
        any
    }

    /// The rows of the focused Changes view.
    pub fn change_rows(&self, root: &str) -> Vec<Row> {
        self.change_rows_for(root, self.focused_filter().as_deref())
    }

    /// Sessions that wrote something still uncommitted, in order of
    /// appearance, for `a` to step through.
    pub fn change_authors(&self, root: &str) -> Vec<String> {
        let mut v: Vec<String> = Vec::new();
        for f in self.changes.get(root).map(|p| p.files.as_slice()).unwrap_or(&[]) {
            for s in &f.sessions {
                if !v.contains(s) {
                    v.push(s.clone());
                }
            }
        }
        v
    }

    pub fn change_rows_for(&self, root: &str, filter: Option<&str>) -> Vec<Row> {
        let Some(p) = self.changes.get(root) else { return vec![] };
        let visible: Vec<usize> =
            (0..p.files.len()).filter(|&i| filter.is_none_or(|s| p.files[i].sessions.iter().any(|x| x == s))).collect();
        let empty = HashSet::new();
        let collapsed = self.changes_view.get(root).map(|v| &v.collapsed).unwrap_or(&empty);
        tree(&p.files, &visible, collapsed)
    }

    pub fn view_mut(&mut self, root: &str) -> &mut View {
        self.changes_view.entry(root.to_string()).or_default()
    }

    /// The selected row, defaulting to the first file.
    pub fn change_selected(&mut self, root: &str, rows: &[Row]) -> Option<usize> {
        let v = self.view_mut(root);
        let found = v.selected.as_ref().and_then(|id| rows.iter().position(|r| &r.id == id));
        let i = found.or_else(|| {
            rows.iter().position(|r| matches!(r.node, Node::File(_))).or((!rows.is_empty()).then_some(0))
        })?;
        if found.is_none() {
            v.selected = Some(rows[i].id.clone());
            v.diff_scroll = 0;
        }
        Some(i)
    }

    pub fn change_file(&self, root: &str, row: &Row) -> Option<FileChange> {
        let Node::File(i) = row.node else { return None };
        self.changes.get(root)?.files.get(i).cloned()
    }

    /// The selected file's diff, asking for it if needed.
    pub fn diff_for(&mut self, root: &str, file: &FileChange) -> Option<&DiffState> {
        let key = format!("{root}\n{}", file.id());
        let fresh = self.diffs.get(&key).is_some_and(|(sig, _)| *sig == file.sig);
        if !fresh {
            if self.conn == Conn::Demo {
                let styled = crate::demo::diff(file);
                self.diffs.insert(key.clone(), (file.sig, DiffState::Ready(Arc::new(styled))));
            } else {
                // Keep showing the old one until the new one arrives.
                let old = self.diffs.remove(&key).map(|(_, s)| s);
                self.diffs.insert(key.clone(), (file.sig, old.unwrap_or(DiffState::Loading)));
                let sessions = self.project_sessions(root);
                if let Some(w) = self.worker() {
                    w.send(Job::Diff { root: root.to_string(), file: file.clone(), sessions });
                }
            }
        }
        self.diffs.get(&key).map(|(_, s)| s)
    }

    /// Stage or unstage the selected file or folder (VS Code's + and −).
    pub fn stage_selected(&mut self, root: &str) {
        let rows = self.change_rows(root);
        let Some(i) = self.change_selected(root, &rows) else { return };
        let row = &rows[i];
        let (group, paths): (Group, Vec<String>) = match &row.node {
            Node::File(f) => {
                let Some(file) = self.changes.get(root).and_then(|p| p.files.get(*f)) else { return };
                let mut paths = vec![file.entry.path.clone()];
                paths.extend(file.entry.orig.clone());
                (file.entry.group, paths)
            }
            Node::Dir(g, path) => (*g, vec![path.clone()]),
            Node::Group(g) => (*g, vec![".".into()]),
        };
        if group == Group::Conflicts {
            self.toast("Resolve the conflict in your editor, then stage it");
            return;
        }
        if self.conn == Conn::Demo {
            self.toast(if group == Group::Staged { "Would unstage (demo)" } else { "Would stage (demo)" });
            return;
        }
        let unstage = group == Group::Staged;
        if let Some(w) = self.worker() {
            w.send(Job::Stage { root: root.to_string(), paths, unstage });
        }
    }
}

// ---------------------------------------------------------------------------
// Commit, push, pull

impl App {
    /// Start a commit, push or pull in the background.
    pub fn start_op(&mut self, root: &str, op: git::Op) {
        if self.conn == Conn::Demo {
            self.toast(format!("Would be {} (demo)", op.doing()));
            return;
        }
        let p = self.changes.entry(root.to_string()).or_default();
        if let Some(r) = &p.running {
            let doing = r.doing();
            self.toast(format!("Still {doing}…"));
            return;
        }
        p.running = Some(op.clone());
        p.op_error = None;
        if let Some(w) = self.worker() {
            w.send(Job::Op { root: root.to_string(), op });
        }
    }

    pub fn start_commit_message(&mut self, root: &str) {
        let draft = self.commit_drafts.remove(root).unwrap_or_default();
        self.commit_edit = Some(crate::compose::Compose::new(root.to_string(), draft));
    }

    pub fn close_commit_message(&mut self) {
        if let Some(c) = self.commit_edit.take()
            && !c.text.trim().is_empty()
        {
            self.commit_drafts.insert(c.session, c.text);
        }
    }

    /// Commit with the message being written: what's staged, or after a
    /// yes, everything (VS Code's smart commit).
    pub fn commit(&mut self) {
        let Some(c) = self.commit_edit.take() else { return };
        let (root, message) = (c.session.clone(), c.text.trim().to_string());
        if message.is_empty() {
            self.commit_edit = Some(c);
            self.toast("Write a message first");
            return;
        }
        let files = self.changes.get(&root).map(|p| p.files.clone()).unwrap_or_default();
        let staged = files.iter().filter(|f| f.entry.group == Group::Staged).count();
        if staged > 0 {
            self.start_op(&root, git::Op::Commit { message, all: false });
        } else if files.is_empty() {
            self.commit_drafts.insert(root, message);
            self.toast("Nothing to commit");
        } else {
            self.commit_drafts.insert(root.clone(), message.clone());
            self.confirm = Some(crate::app::Confirm {
                title: "commit everything?".into(),
                body: format!(
                    "Nothing is staged. Stage all {} and commit them?",
                    crate::ui::plural(files.len(), "changed file")
                ),
                action: crate::ConfirmAction::CommitAll(root, message),
                on_action: true,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(group: Group, path: &str, added: u32) -> FileChange {
        FileChange {
            entry: Entry { group, kind: Kind::Modified, path: path.into(), orig: None },
            added,
            removed: 0,
            binary: false,
            sessions: vec![],
            sig: 0,
        }
    }

    #[test]
    fn reads_a_real_repository_and_attributes_changes() {
        let dir = std::env::temp_dir().join(format!("aulora-changes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        let root = dir.canonicalize().unwrap().to_string_lossy().to_string();
        let sh = |args: &[&str]| {
            let ok =
                std::process::Command::new("git").arg("-C").arg(&root).args(args).output().unwrap().status.success();
            assert!(ok, "git {args:?}");
        };
        sh(&["init", "-q"]);
        sh(&["config", "user.email", "t@t"]);
        sh(&["config", "user.name", "t"]);
        std::fs::write(dir.join("src/lib.rs"), "fn a() {}\nfn b() {}\n").unwrap();
        std::fs::write(dir.join("staged.txt"), "one\n").unwrap();
        sh(&["add", "."]);
        sh(&["commit", "-q", "-m", "init"]);
        std::fs::write(dir.join("src/lib.rs"), "fn a() {}\nfn better_b() -> u32 { 42 }\n").unwrap();
        std::fs::write(dir.join("staged.txt"), "one\ntwo more\n").unwrap();
        sh(&["add", "staged.txt"]);
        std::fs::write(dir.join("new.md"), "# hello there\n").unwrap();
        // A transcript in which session s1 wrote the lib.rs change.
        let t = dir.join("t.jsonl");
        let patch = format!(
            r#"{{"timestamp":"2026-10-20T10:00:00.000Z","toolUseResult":{{"filePath":"{root}/src/lib.rs","structuredPatch":[{{"lines":["-fn b() {{}}","+fn better_b() -> u32 {{ 42 }}"]}}]}}}}"#
        );
        std::fs::write(&t, format!("{patch}\n")).unwrap();
        let sessions = vec![("s1".to_string(), t.to_string_lossy().to_string())];
        let mut index = attrib::Index::default();
        let files = status(&root, &sessions, &mut index).unwrap();
        let find = |p: &str, g: Group| files.iter().find(|f| f.entry.path == p && f.entry.group == g).unwrap();
        let lib = find("src/lib.rs", Group::Changes);
        assert_eq!((lib.added, lib.removed, lib.sessions.clone()), (1, 1, vec!["s1".to_string()]));
        assert_eq!(find("staged.txt", Group::Staged).added, 1);
        let new = find("new.md", Group::Changes);
        assert_eq!((new.entry.kind, new.added), (Kind::Untracked, 1));
        assert!(new.sessions.is_empty());
        let d = diff(&root, lib, &sessions, &mut index).unwrap();
        assert_eq!(d.hunks[0].session.as_deref(), Some("s1"));
        assert_eq!(d.hunks[0].lines.len(), 3, "a context line, then the change");
        git::stage(&root, &["src/lib.rs".into()], false).unwrap();
        let files = status(&root, &sessions, &mut index).unwrap();
        assert!(files.iter().any(|f| f.entry.path == "src/lib.rs" && f.entry.group == Group::Staged));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn folders_compress_and_roll_up() {
        let files = vec![
            file(Group::Changes, "crates/tui/src/ui.rs", 3),
            file(Group::Changes, "crates/tui/src/app.rs", 2),
            file(Group::Changes, "README.md", 1),
            file(Group::Staged, "Cargo.toml", 4),
        ];
        let all: Vec<usize> = (0..files.len()).collect();
        let rows = tree(&files, &all, &HashSet::new());
        let labels: Vec<(usize, &str)> = rows.iter().map(|r| (r.depth, r.label.as_str())).collect();
        assert_eq!(
            labels,
            [
                (0, "Staged"),
                (1, "Cargo.toml"),
                (0, "Changes"),
                (1, "crates/tui/src"),
                (2, "app.rs"),
                (2, "ui.rs"),
                (1, "README.md")
            ]
        );
        assert_eq!((rows[3].added, rows[3].count), (5, 2));
        assert_eq!(rows[2].count, 3);
        let collapsed: HashSet<String> = [rows[3].id.clone()].into();
        assert_eq!(tree(&files, &all, &collapsed).len(), 5);
    }
}
