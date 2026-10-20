//! Ctrl-P: find a file in the project by fuzzy name (VS Code's quick open,
//! matched with nucleo as in helix). `path:line` opens at that line.

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::App;

pub struct Finder {
    pub root: String,
    pub query: String,
    pub selected: usize,
    /// Path and the char positions that matched.
    pub results: Vec<(String, Vec<u32>)>,
    /// How many files matched in all.
    pub total: usize,
    /// The results need working out again.
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

/// Rank `files` against `query`. With no query, `first` (changed files)
/// come before the rest.
pub fn rank(files: &[String], query: &str, first: &[String]) -> (Vec<(String, Vec<u32>)>, usize) {
    let (q, _) = split_line(query.trim());
    if q.is_empty() {
        let mut out: Vec<(String, Vec<u32>)> =
            first.iter().filter(|p| files.contains(p)).map(|p| (p.clone(), vec![])).take(SHOWN).collect();
        for p in files {
            if out.len() >= SHOWN {
                break;
            }
            if !first.contains(p) {
                out.push((p.clone(), vec![]));
            }
        }
        return (out, files.len());
    }
    let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
    let pattern = Pattern::parse(q, CaseMatching::Smart, Normalization::Smart);
    let mut buf = Vec::new();
    let mut scored: Vec<(u32, usize)> = files
        .iter()
        .enumerate()
        .filter_map(|(i, p)| pattern.score(Utf32Str::new(p, &mut buf), &mut matcher).map(|s| (s, i)))
        .collect();
    let total = scored.len();
    // Best first; among equals, shorter paths (closer to the root).
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(files[a.1].len().cmp(&files[b.1].len())));
    let out = scored
        .into_iter()
        .take(SHOWN)
        .map(|(_, i)| {
            let mut idx = Vec::new();
            pattern.indices(Utf32Str::new(&files[i], &mut buf), &mut matcher, &mut idx);
            idx.sort_unstable();
            idx.dedup();
            (files[i].clone(), idx)
        })
        .collect();
    (out, total)
}

impl App {
    pub fn open_finder(&mut self) {
        let Some(root) = self.selected_root() else {
            self.toast("Select a project first");
            return;
        };
        self.finder = Some(Finder { root, query: String::new(), selected: 0, results: vec![], total: 0, dirty: true });
    }

    /// Work out the results again if the query or the index changed.
    pub fn refresh_finder(&mut self) {
        let Some(root) = self.finder.as_ref().filter(|f| f.dirty).map(|f| f.root.clone()) else { return };
        let Some(index) = self.file_index(&root) else { return };
        let changed: Vec<String> =
            self.changes.get(&root).map(|p| p.files.iter().map(|f| f.entry.path.clone()).collect()).unwrap_or_default();
        let Some(f) = self.finder.as_mut() else { return };
        (f.results, f.total) = rank(&index, &f.query, &changed);
        f.selected = f.selected.min(f.results.len().saturating_sub(1));
        f.dirty = false;
    }

    pub fn finder_key(&mut self, key: KeyEvent) {
        let Some(f) = self.finder.as_mut() else { return };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => self.finder = None,
            KeyCode::Enter => {
                let pick = f.results.get(f.selected).map(|r| r.0.clone());
                let (_, line) = split_line(f.query.trim());
                let root = f.root.clone();
                self.finder = None;
                if let Some(path) = pick {
                    self.reveal_file(&root, &path, line);
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranks_paths_fuzzily() {
        let files: Vec<String> =
            ["README.md", "crates/aulora-tui/src/ui.rs", "crates/aulora-tui/src/input.rs", "docs/ui-notes.md"]
                .iter()
                .map(|s| s.to_string())
                .collect();
        let (r, total) = rank(&files, "tuiui", &[]);
        assert_eq!(total, 1);
        assert_eq!(r[0].0, "crates/aulora-tui/src/ui.rs");
        assert!(!r[0].1.is_empty());
        let (r, _) = rank(&files, "ui.rs:120", &[]);
        assert_eq!(r[0].0, "crates/aulora-tui/src/ui.rs");
        assert_eq!(split_line("ui.rs:120"), ("ui.rs", Some(120)));
        let (r, _) = rank(&files, "", &["docs/ui-notes.md".into()]);
        assert_eq!(r[0].0, "docs/ui-notes.md", "changed files first");
    }
}
