//! Ctrl-P: one fuzzy box for everything (VS Code's quick open and command
//! palette, matched with nucleo as in helix). With no prefix it finds files
//! in the project (`path:line` opens at a line); `#` finds projects and
//! folders to open, `@` agents, `>` actions. Ctrl-O starts at `#`.

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{App, RowRef};
use crate::keymap::Action;

/// What a result stands for.
#[derive(Debug, Clone, PartialEq)]
pub enum Pick {
    File(String),
    /// A project aulora shows already.
    Project(String),
    /// A folder on disk to open as a project.
    Folder(String),
    Agent(RowRef),
    Action(Action),
}

#[derive(Debug, Clone)]
pub struct Item {
    pub pick: Pick,
    /// What the query is matched against, and shown.
    pub text: String,
    /// A second, quieter part: a folder, a key, a project name.
    pub note: String,
    /// Char positions in `text` that matched.
    pub idx: Vec<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Files,
    Projects,
    Agents,
    Actions,
}

impl Mode {
    pub fn of(query: &str, has_root: bool) -> (Mode, &str) {
        match query.chars().next() {
            Some('#') => (Mode::Projects, query[1..].trim_start()),
            Some('@') => (Mode::Agents, query[1..].trim_start()),
            Some('>') => (Mode::Actions, query[1..].trim_start()),
            _ if has_root => (Mode::Files, query),
            _ => (Mode::Projects, query),
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Mode::Files => "find a file",
            Mode::Projects => "open a project",
            Mode::Agents => "go to an agent",
            Mode::Actions => "run an action",
        }
    }
}

/// What choosing a result asks the caller to do.
pub enum Outcome {
    Run(Action),
    /// Open a folder as a project, with Claude when true.
    Open(String, bool),
}

pub struct Finder {
    pub root: Option<String>,
    pub query: String,
    pub selected: usize,
    pub results: Vec<Item>,
    /// How many matched in all.
    pub total: usize,
    pub dirty: bool,
}

/// Results shown at most.
const SHOWN: usize = 50;

/// "src/ui.rs:120" → ("src/ui.rs", Some(120)).
pub fn split_line(q: &str) -> (&str, Option<u32>) {
    match q.rsplit_once(':') {
        Some((path, n)) if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) => (path, n.parse().ok()),
        _ => (q, None),
    }
}

/// Rank candidates against a query: best first, shorter text on ties. With
/// no query, candidates keep their order.
pub fn rank(cands: Vec<(Pick, String, String)>, query: &str) -> (Vec<Item>, usize) {
    let q = query.trim();
    if q.is_empty() {
        let total = cands.len();
        let items = cands.into_iter().take(SHOWN).map(|(pick, text, note)| Item { pick, text, note, idx: vec![] });
        return (items.collect(), total);
    }
    let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
    let pattern = Pattern::parse(q, CaseMatching::Smart, Normalization::Smart);
    let mut buf = Vec::new();
    let mut scored: Vec<(u32, usize)> = cands
        .iter()
        .enumerate()
        .filter_map(|(i, c)| pattern.score(Utf32Str::new(&c.1, &mut buf), &mut matcher).map(|s| (s, i)))
        .collect();
    let total = scored.len();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(cands[a.1].1.len().cmp(&cands[b.1].1.len())));
    let keep: Vec<usize> = scored.into_iter().take(SHOWN).map(|(_, i)| i).collect();
    let mut cands: Vec<Option<(Pick, String, String)>> = cands.into_iter().map(Some).collect();
    let items = keep
        .into_iter()
        .filter_map(|i| {
            let (pick, text, note) = cands[i].take()?;
            let mut idx = Vec::new();
            pattern.indices(Utf32Str::new(&text, &mut buf), &mut matcher, &mut idx);
            idx.sort_unstable();
            idx.dedup();
            Some(Item { pick, text, note, idx })
        })
        .collect();
    (items, total)
}

/// Actions worth offering by name, with what they're called.
const ACTIONS: &[(Action, &str)] = &[
    (Action::OpenProject, "Open a project…"),
    (Action::NewClaude, "New Claude pane beside this one"),
    (Action::SplitRight, "Split right with a shell"),
    (Action::SplitDown, "Split down with a shell"),
    (Action::NewTab, "New tab"),
    (Action::OpenChanges, "Show Changes"),
    (Action::OpenFiles, "Show Files"),
    (Action::Zoom, "Zoom this pane"),
    (Action::Equalize, "Even out panes"),
    (Action::CloseLeaf, "Close this pane"),
    (Action::NextAlert, "Next agent that needs you"),
    (Action::Themes, "Choose a theme"),
    (Action::ToggleHistory, "Show or hide past sessions"),
    (Action::ToggleEnded, "Show or hide ended sessions"),
    (Action::DismissEnded, "Dismiss every ended session"),
    (Action::OpenEditor, "Open in your editor"),
    (Action::Reveal, "Show in Finder"),
    (Action::ToggleMouse, "Turn mouse capture on or off"),
    (Action::Help, "All keys"),
    (Action::Welcome, "Getting started"),
    (Action::Quit, "Quit"),
];

fn key_of(a: Action) -> String {
    crate::keymap::BINDINGS.iter().find(|b| b.action == a).map(|b| b.label.to_string()).unwrap_or_default()
}

impl App {
    pub fn open_finder(&mut self) {
        let root = self.selected_root();
        self.finder = Some(Finder { root, query: String::new(), selected: 0, results: vec![], total: 0, dirty: true });
    }

    /// Ctrl-O: straight to projects.
    pub fn open_project_picker(&mut self) {
        self.open_finder();
        if let Some(f) = self.finder.as_mut() {
            f.query = "#".into();
        }
    }

    pub fn finder_mode(&self) -> Option<Mode> {
        let f = self.finder.as_ref()?;
        Some(Mode::of(&f.query, f.root.is_some()).0)
    }

    fn candidates(&mut self, mode: Mode, query: &str) -> Option<Vec<(Pick, String, String)>> {
        let root = self.finder.as_ref()?.root.clone();
        Some(match mode {
            Mode::Files => {
                let root = root?;
                let index = self.file_index(&root)?;
                let changed: Vec<String> = self
                    .changes
                    .get(&root)
                    .map(|p| p.files.iter().map(|f| f.entry.path.clone()).collect())
                    .unwrap_or_default();
                let mut v: Vec<(Pick, String, String)> = Vec::with_capacity(index.len());
                // Changed files first when there's no query.
                for p in changed.iter().filter(|p| index.contains(p)) {
                    v.push((Pick::File(p.clone()), p.clone(), String::new()));
                }
                for p in index.iter().filter(|p| !changed.contains(p)) {
                    v.push((Pick::File(p.clone()), p.clone(), String::new()));
                }
                v
            }
            Mode::Projects => {
                let mut v = Vec::new();
                let mut seen = std::collections::HashSet::new();
                // A typed path opens even if no scan found it.
                let typed = query.trim();
                if typed.starts_with('/') || typed.starts_with('~') {
                    let path = crate::files::expand_home(typed);
                    if std::path::Path::new(&path).is_dir() {
                        seen.insert(path.clone());
                        v.push((
                            Pick::Folder(path.clone()),
                            aulora_core::paths::tildify(&path),
                            "open this folder".into(),
                        ));
                    }
                }
                let open: Vec<(String, String)> = self
                    .projects()
                    .iter()
                    .filter_map(|r| match r {
                        RowRef::Project(root) => Some((root.clone(), self.title(r))),
                        _ => None,
                    })
                    .collect();
                for (root, name) in open {
                    if seen.insert(root.clone()) {
                        v.push((
                            Pick::Project(root.clone()),
                            aulora_core::paths::tildify(&root),
                            format!("{name} · open"),
                        ));
                    }
                }
                // Recent projects from the last week's sessions.
                let recent: Vec<String> =
                    self.snap.iter().flat_map(|s| s.projects.iter().map(|p| p.root.clone())).collect();
                for root in recent {
                    if seen.insert(root.clone()) {
                        v.push((Pick::Folder(root.clone()), aulora_core::paths::tildify(&root), "recent".into()));
                    }
                }
                for root in self.known_folders(&mut seen) {
                    v.push((Pick::Folder(root.clone()), aulora_core::paths::tildify(&root), String::new()));
                }
                v
            }
            Mode::Agents => self
                .agents()
                .into_iter()
                .map(|r| {
                    let project = self.root_of(&r).map(|p| aulora_core::util::basename(&p)).unwrap_or_default();
                    let text = self.title(&r);
                    (Pick::Agent(r), text, project)
                })
                .collect(),
            Mode::Actions => {
                ACTIONS.iter().map(|(a, label)| (Pick::Action(*a), label.to_string(), key_of(*a))).collect()
            }
        })
    }

    /// Git repositories found on disk, for opening; scanned once, in the
    /// background.
    fn known_folders(&mut self, seen: &mut std::collections::HashSet<String>) -> Vec<String> {
        match &self.folders {
            Some(list) => list.iter().filter(|f| seen.insert((*f).clone())).cloned().collect(),
            None => {
                if !self.folders_requested {
                    self.folders_requested = true;
                    self.send_folders_job();
                }
                vec![]
            }
        }
    }

    /// Work out the results again if the query or the data changed.
    pub fn refresh_finder(&mut self) {
        let Some((query, has_root)) =
            self.finder.as_ref().filter(|f| f.dirty).map(|f| (f.query.clone(), f.root.is_some()))
        else {
            return;
        };
        let (mode, q) = Mode::of(&query, has_root);
        let q = if mode == Mode::Files { split_line(q.trim()).0.to_string() } else { q.to_string() };
        let Some(cands) = self.candidates(mode, &q) else { return };
        let Some(f) = self.finder.as_mut() else { return };
        (f.results, f.total) = rank(cands, &q);
        f.selected = f.selected.min(f.results.len().saturating_sub(1));
        f.dirty = false;
    }

    /// A key in the finder. Returns what the caller should do, if anything.
    pub fn finder_key(&mut self, key: KeyEvent) -> Option<Outcome> {
        let f = self.finder.as_mut()?;
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => self.finder = None,
            KeyCode::Enter | KeyCode::Tab => {
                let with_claude = key.code == KeyCode::Tab;
                let pick = f.results.get(f.selected).map(|r| r.pick.clone());
                let (_, line) = split_line(f.query.trim());
                let root = f.root.clone();
                self.finder = None;
                match pick? {
                    Pick::File(path) => {
                        if let Some(root) = root {
                            self.reveal_file(&root, &path, line);
                        }
                    }
                    Pick::Project(root) if !with_claude => {
                        self.select(RowRef::Project(root));
                    }
                    Pick::Project(root) | Pick::Folder(root) => return Some(Outcome::Open(root, with_claude)),
                    Pick::Agent(r) => {
                        self.select(r);
                        self.focus = crate::app::Focus::Main;
                    }
                    Pick::Action(a) => return Some(Outcome::Run(a)),
                }
            }
            KeyCode::Up => f.selected = f.selected.saturating_sub(1),
            KeyCode::Char('p') | KeyCode::Char('k') if ctrl => f.selected = f.selected.saturating_sub(1),
            KeyCode::Down => f.selected = (f.selected + 1).min(f.results.len().saturating_sub(1)),
            KeyCode::Char('n') | KeyCode::Char('j') if ctrl => {
                f.selected = (f.selected + 1).min(f.results.len().saturating_sub(1))
            }
            _ => {
                if crate::compose::edit_line(&mut f.query, &key) {
                    f.dirty = true;
                    f.selected = 0;
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(list: &[&str]) -> Vec<(Pick, String, String)> {
        list.iter().map(|s| (Pick::File(s.to_string()), s.to_string(), String::new())).collect()
    }

    #[test]
    fn ranks_fuzzily() {
        let c =
            files(&["README.md", "crates/aulora-tui/src/ui.rs", "crates/aulora-tui/src/input.rs", "docs/ui-notes.md"]);
        let (r, total) = rank(c.clone(), "tuiui");
        assert_eq!(total, 1);
        assert_eq!(r[0].text, "crates/aulora-tui/src/ui.rs");
        assert!(!r[0].idx.is_empty());
        assert_eq!(split_line("ui.rs:120"), ("ui.rs", Some(120)));
        let (r, total) = rank(c, "");
        assert_eq!((r.len(), total), (4, 4), "no query keeps everything, in order");
        assert_eq!(r[0].text, "README.md");
    }

    #[test]
    fn prefixes_pick_the_mode() {
        assert_eq!(Mode::of("# multi", true), (Mode::Projects, "multi"));
        assert_eq!(Mode::of(">theme", true), (Mode::Actions, "theme"));
        assert_eq!(Mode::of("@bill", true), (Mode::Agents, "bill"));
        assert_eq!(Mode::of("ui.rs", true), (Mode::Files, "ui.rs"));
        assert_eq!(Mode::of("ui.rs", false), (Mode::Projects, "ui.rs"), "no project selected: projects");
    }
}
