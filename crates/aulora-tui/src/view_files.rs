//! The Files tab (VS Code's explorer and a read-only editor) and the
//! Ctrl-P finder.

use std::collections::HashMap;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};

use crate::app::{App, Hit};
use crate::files::{Loaded, Mark, Row};
use crate::git::Kind;
use crate::theme::{self, bold, dim, faint, fg, spinner, text};
use crate::ui::{fit, lr, scrollbar, sp, w};
use crate::view_changes::{kind_color, session_color};

/// Git status and sessions by path, with folders rolled up.
struct Decor {
    kinds: HashMap<String, Kind>,
    sessions: HashMap<String, Vec<String>>,
    /// Lines added and removed in each file, staged and not.
    counts: HashMap<String, Size>,
}

/// How big a file's change is.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Size {
    added: u32,
    removed: u32,
    binary: bool,
}

impl Size {
    fn of(f: &crate::changes::FileChange) -> Size {
        Size { added: f.added, removed: f.removed, binary: f.binary }
    }

    fn add(&mut self, o: Size) {
        self.added += o.added;
        self.removed += o.removed;
        self.binary |= o.binary;
    }

    /// `+12 −3` in green and red (only the sides that changed), or `bin`.
    fn spans(self) -> Vec<Span<'static>> {
        let t = theme::get();
        if self.binary {
            return vec![sp(" bin", faint())];
        }
        let mut v = Vec::new();
        if self.added > 0 {
            v.push(sp(format!(" +{}", self.added), fg(t.green)));
        }
        if self.removed > 0 {
            v.push(sp(format!(" −{}", self.removed), fg(t.red)));
        }
        v
    }
}

fn decor(app: &App, root: &str) -> Decor {
    let mut d = Decor { kinds: HashMap::new(), sessions: HashMap::new(), counts: HashMap::new() };
    let Some(p) = app.changes.get(root) else { return d };
    for f in &p.files {
        d.counts.entry(f.entry.path.clone()).or_default().add(Size::of(f));
        let path = f.entry.path.as_str();
        let mut keys = vec![path.to_string()];
        let mut dir = path;
        while let Some((parent, _)) = dir.rsplit_once('/') {
            keys.push(parent.to_string());
            dir = parent;
        }
        for k in keys {
            let e = d.kinds.entry(k.clone()).or_insert(f.entry.kind);
            *e = (*e).min(f.entry.kind);
            if k == path {
                let s = d.sessions.entry(k).or_default();
                for id in &f.sessions {
                    if !s.contains(id) {
                        s.push(id.clone());
                    }
                }
            }
        }
    }
    d
}

pub fn draw(f: &mut Frame, area: Rect, app: &mut App, root: &str) {
    let t = theme::get();
    // Also for a folder of repositories, whose changes show under each.
    app.refresh_changes(root);
    // Top line: the search box, or where you are.
    let width = area.width as usize;
    let searching = app.files_view(root).search.as_ref().map(|s| (s.query.clone(), s.editing));
    let top = match &searching {
        Some((q, editing)) => lr(
            vec![sp(" / ", bold(t.accent)), sp(q.clone(), text()), sp(if *editing { "▏" } else { "" }, fg(t.accent))],
            vec![sp(if *editing { "⏎ search · esc close " } else { "/ edit · esc close " }, faint())],
            width,
        ),
        None => {
            let sel = app.files_view(root).selected.clone().unwrap_or_default();
            lr(
                vec![sp(format!(" {}", if sel.is_empty() { "/".into() } else { sel }), dim())],
                vec![sp("/ search · ctrl-p find file ", faint())],
                width,
            )
        }
    };
    f.render_widget(Paragraph::new(top), Rect { height: 1, ..area });
    let area = Rect { y: area.y + 2, height: area.height.saturating_sub(2), ..area };

    let wide = area.width >= 100;
    let focus_file = app.files_view(root).focus_file;
    let (left, right) = if wide {
        let tw = ((area.width as u32 * 34) / 100).clamp(30, 52) as u16;
        let sep: Vec<Line> = (0..area.height).map(|_| Line::from(sp("│", fg(t.rule)))).collect();
        f.render_widget(Paragraph::new(sep), Rect { x: area.x + tw, width: 1, ..area });
        (
            Some(Rect { width: tw, ..area }),
            Some(Rect { x: area.x + tw + 2, width: area.width.saturating_sub(tw + 2), ..area }),
        )
    } else if focus_file {
        (None, Some(area))
    } else {
        (Some(area), None)
    };

    let show_results = app.files_view(root).search.as_ref().is_some_and(|s| !s.editing && s.results.is_some());
    if let Some(l) = left {
        if show_results {
            results(f, l, app, root);
        } else {
            tree(f, l, app, root);
        }
    }
    if let Some(r) = right {
        let sel = app.files_view(root).selected.clone();
        let is_file = sel.as_ref().is_some_and(|s| app.files.get(root).is_some_and(|tr| tr.is_file(s)));
        match sel.filter(|_| is_file) {
            Some(rel) => file(f, r, app, root, &rel),
            None => {
                let msg = vec![Line::raw(""), Line::from(sp("Pick a file, or ctrl-p to find one by name.", faint()))];
                f.render_widget(Paragraph::new(msg), r);
            }
        }
    }
}

/// What changed, listed above the tree (VS Code's explorer colours the
/// files; this also lists them, so they're visible without digging).
/// Returns the rows used.
fn changed(f: &mut Frame, area: Rect, app: &mut App, root: &str) -> u16 {
    let t = theme::get();
    let Some(files) = app.changes.get(root).filter(|p| p.loaded).map(|p| p.files.clone()) else { return 0 };
    if files.is_empty() || area.height < 12 {
        return 0;
    }
    let max = (area.height / 3).max(3) as usize;
    let width = area.width.saturating_sub(1) as usize;
    let selected = app.files_view(root).selected.clone();
    let mut lines = vec![lr(
        vec![sp(" CHANGES ", bold(t.dim)), sp(files.len().to_string(), fg(t.yellow))],
        vec![sp("d review ", faint())],
        width,
    )];
    let shown = if files.len() > max { max - 1 } else { files.len() };
    for (i, file) in files.iter().take(shown).enumerate() {
        let path = &file.entry.path;
        let (dir, name) = path.rsplit_once('/').map_or(("", path.as_str()), |(d, n)| (d, n));
        let is_sel = selected.as_deref() == Some(path.as_str());
        let color = kind_color(file.entry.kind);
        let mut right: Vec<Span> = file.sessions.iter().take(3).map(|s| sp("●", fg(session_color(s)))).collect();
        right.extend(Size::of(file).spans());
        right.push(sp(format!(" {} ", file.entry.kind.letter()), fg(color)));
        let right_w: usize = right.iter().map(|s| w(&s.content)).sum();
        let room = width.saturating_sub(name.chars().count() + 4 + right_w);
        let mut left = vec![sp("   ", text()), sp(name.to_string(), if is_sel { bold(color) } else { fg(color) })];
        if !dir.is_empty() && room > 4 {
            left.push(sp(format!(" {}", fit(dir, room)), faint()));
        }
        let mut line = lr(left, right, width);
        if is_sel {
            line = line.style(Style::new().bg(t.active));
        }
        lines.push(line);
        app.hits.push((
            Rect { x: area.x, y: area.y + 1 + i as u16, width: area.width, height: 1 },
            Hit::ChangedFile(path.clone()),
        ));
    }
    if shown < files.len() {
        lines.push(Line::from(sp(format!("   … {} more · d shows all", files.len() - shown), faint())));
    }
    let used = lines.len() as u16;
    lines.push(Line::from(sp(" FILES", bold(t.dim))));
    f.render_widget(Paragraph::new(lines), Rect { height: used + 1, ..area });
    used + 1
}

fn tree(f: &mut Frame, area: Rect, app: &mut App, root: &str) {
    let t = theme::get();
    let top = changed(f, area, app, root);
    let area = Rect { y: area.y + top, height: area.height.saturating_sub(top), ..area };
    let rows = app.file_rows(root);
    let decor = decor(app, root);
    let h = area.height as usize;
    let width = area.width.saturating_sub(1) as usize;
    let focused = app.here() && !app.files_view(root).focus_file;
    let v = app.files_view(root);
    if v.selected.is_none() {
        v.selected = rows.first().map(|r| r.rel.clone());
    }
    let sel = v.selected.as_ref().and_then(|s| rows.iter().position(|r| &r.rel == s));
    v.tree_height = h;
    if let Some(i) = sel.filter(|_| v.shown != v.selected) {
        v.shown = v.selected.clone();
        if i < v.scroll {
            v.scroll = i;
        } else if i >= v.scroll + h {
            v.scroll = i + 1 - h;
        }
    }
    v.scroll = v.scroll.min(rows.len().saturating_sub(h));
    let scroll = v.scroll;
    app.hits.push((area, Hit::FileTree));
    if rows.is_empty() {
        let msg = if app.files.get(root).is_some_and(|tr| matches!(tr.dirs.get(""), Some(Loaded::Failed(_)))) {
            "  can't read this folder"
        } else {
            "  reading…"
        };
        f.render_widget(Paragraph::new(Line::from(sp(msg, faint()))), area);
        return;
    }
    let mut lines = Vec::with_capacity(h);
    for (i, r) in rows.iter().enumerate().skip(scroll).take(h) {
        let is_sel = sel == Some(i);
        let bg = match (is_sel, focused) {
            (true, true) => Style::new().bg(t.select),
            (true, false) => Style::new().bg(t.active),
            _ => Style::new(),
        };
        lines.push(tree_line(r, &decor, is_sel, width, app.now).style(bg));
        app.hits
            .push((Rect { x: area.x, y: area.y + (i - scroll) as u16, width: area.width, height: 1 }, Hit::FileRow(i)));
    }
    f.render_widget(Paragraph::new(lines), Rect { width: area.width.saturating_sub(1), ..area });
    scrollbar(f, Rect { x: area.right().saturating_sub(1), width: 1, ..area }, scroll, h, rows.len(), focused);
}

fn tree_line(r: &Row, decor: &Decor, is_sel: bool, width: usize, now: i64) -> Line<'static> {
    let t = theme::get();
    let indent = "  ".repeat(r.depth);
    let kind = decor.kinds.get(&r.rel).copied();
    // VS Code's explorer: changes in their colour, ignored paths faded.
    let plain = if r.ignored {
        t.faint
    } else if r.is_dir {
        t.dim
    } else {
        t.text
    };
    let color = kind.map(kind_color).unwrap_or(plain);
    let name_style = if is_sel { bold(color) } else { fg(color) };
    let mut left = vec![sp(format!(" {indent}"), text())];
    if r.is_dir {
        let arrow = if r.loading {
            spinner(now)
        } else if r.expanded {
            "▾"
        } else {
            "▸"
        };
        left.push(sp(format!("{arrow} "), faint()));
    } else {
        left.push(sp("  ", text()));
    }
    left.push(sp(r.name.clone(), name_style));
    let mut right = Vec::new();
    if !r.is_dir {
        for s in decor.sessions.get(&r.rel).into_iter().flatten().take(3) {
            right.push(sp("●", fg(session_color(s))));
        }
        if let Some(c) = decor.counts.get(&r.rel) {
            right.extend(c.spans());
        }
        if let Some(k) = kind {
            right.push(sp(format!(" {} ", k.letter()), fg(kind_color(k))));
        }
    } else if kind.is_some() {
        right.push(sp("● ", fg(color)));
    }
    lr(left, right, width)
}

fn results(f: &mut Frame, area: Rect, app: &mut App, root: &str) {
    let t = theme::get();
    let h = area.height as usize;
    let width = area.width.saturating_sub(1) as usize;
    let focused = app.here() && !app.files_view(root).focus_file;
    let now = app.now;
    let Some(s) = app.files_view(root).search.as_mut() else { return };
    let (matches, more) = match &s.results {
        Some(Ok((m, more))) => (m.clone(), *more),
        Some(Err(e)) => {
            let e = e.clone();
            f.render_widget(Paragraph::new(Line::from(sp(format!("  {e}"), fg(t.red)))), area);
            return;
        }
        None => {
            let msg = Line::from(vec![sp(format!("  {} ", spinner(now)), fg(t.accent)), sp("searching…", dim())]);
            f.render_widget(Paragraph::new(msg), area);
            return;
        }
    };
    if matches.is_empty() {
        let q = s.searched.clone();
        f.render_widget(Paragraph::new(Line::from(sp(format!("  no matches for \"{q}\""), faint()))), area);
        return;
    }
    // Rows: a header per file, then its matches.
    let mut rows: Vec<(Option<usize>, Line<'static>)> = Vec::new();
    let files = {
        let mut seen = Vec::new();
        for m in &matches {
            if !seen.contains(&m.path) {
                seen.push(m.path.clone());
            }
        }
        seen.len()
    };
    let summary = format!(
        " {} in {}{}",
        crate::ui::plural(matches.len(), "match"),
        crate::ui::plural(files, "file"),
        if more { ", stopped there" } else { "" }
    );
    rows.push((None, Line::from(sp(summary, faint()))));
    let q = s.searched.clone();
    let mut last = String::new();
    for (i, m) in matches.iter().enumerate() {
        if m.path != last {
            last = m.path.clone();
            rows.push((None, Line::from(sp(format!(" {}", fit(&m.path, width.saturating_sub(1))), bold(t.text)))));
        }
        let num = format!("   {:>4}  ", m.line);
        let body = m.text.trim_start();
        let room = width.saturating_sub(w(&num));
        let mut spans = vec![sp(num, faint())];
        spans.extend(highlight_query(&fit(body, room), &q));
        rows.push((Some(i), Line::from(spans)));
    }
    s.selected = s.selected.min(matches.len() - 1);
    let sel_row = rows.iter().position(|(i, _)| *i == Some(s.selected)).unwrap_or(0);
    if sel_row < s.scroll {
        s.scroll = sel_row.saturating_sub(1);
    } else if sel_row >= s.scroll + h {
        s.scroll = sel_row + 1 - h;
    }
    s.scroll = s.scroll.min(rows.len().saturating_sub(h));
    let scroll = s.scroll;
    let selected = s.selected;
    let mut lines = Vec::new();
    for (k, (i, l)) in rows.iter().enumerate().skip(scroll).take(h) {
        let y = area.y + (k - scroll) as u16;
        if let Some(i) = i {
            app.hits.push((Rect { x: area.x, y, width: area.width, height: 1 }, Hit::FileResult(*i)));
        }
        let bg = match (*i == Some(selected), focused) {
            (true, true) => Style::new().bg(t.select),
            (true, false) => Style::new().bg(t.active),
            _ => Style::new(),
        };
        lines.push(l.clone().style(bg));
    }
    app.hits.push((area, Hit::FileTree));
    f.render_widget(Paragraph::new(lines), Rect { width: area.width.saturating_sub(1), ..area });
    scrollbar(f, Rect { x: area.right().saturating_sub(1), width: 1, ..area }, scroll, h, rows.len(), focused);
}

/// Spans with every occurrence of `q` marked (case-insensitive unless `q`
/// has a capital, as the search does).
fn highlight_query(s: &str, q: &str) -> Vec<Span<'static>> {
    let t = theme::get();
    let smart = q.chars().any(char::is_uppercase);
    let (hay, needle) = if smart { (s.to_string(), q.to_string()) } else { (s.to_lowercase(), q.to_lowercase()) };
    let mut out = Vec::new();
    let mut pos = 0;
    if !needle.is_empty() && hay.len() == s.len() {
        while let Some(i) = hay[pos..].find(&needle) {
            let start = pos + i;
            out.push(sp(s[pos..start].to_string(), dim()));
            out.push(sp(s[start..start + needle.len()].to_string(), bold(t.accent).add_modifier(Modifier::UNDERLINED)));
            pos = start + needle.len();
        }
    }
    out.push(sp(s[pos..].to_string(), dim()));
    out
}

fn file(f: &mut Frame, area: Rect, app: &mut App, root: &str, rel: &str) {
    let t = theme::get();
    let focused = app.here() && app.files_view(root).focus_file;
    let now = app.now;
    let text = match app.file_text(root, rel) {
        Some(Loaded::Ready(text, _)) => text.clone(),
        Some(Loaded::Failed(e)) => {
            let e = e.clone();
            f.render_widget(Paragraph::new(Line::from(sp(format!("couldn't read it: {e}"), fg(t.red)))), area);
            return;
        }
        _ => {
            let msg = Line::from(vec![sp(format!("{} ", spinner(now)), fg(t.accent)), sp("reading…", dim())]);
            f.render_widget(Paragraph::new(msg), area);
            return;
        }
    };
    let width = area.width as usize;
    let (dir, name) = match rel.rsplit_once('/') {
        Some((d, n)) => (format!("{d}/"), n.to_string()),
        None => (String::new(), rel.to_string()),
    };
    let changed = text.marks.len();
    let mut right = vec![sp(format!("{} ", crate::ui::plural(text.lines, "line")), faint())];
    if changed > 0 {
        right.insert(0, sp(format!("{changed} changed · "), fg(t.yellow)));
    }
    let header = lr(vec![sp(dir, faint()), sp(name, bold(t.text))], right, width);
    let hint = Line::from(sp("e edit here · o open in your editor · [ ] changes", faint()));
    f.render_widget(Paragraph::new(vec![header, hint]), Rect { height: 2, ..area });
    let body = Rect { y: area.y + 3, height: area.height.saturating_sub(3), ..area };
    app.hits.push((body, Hit::FileView));
    if text.binary {
        f.render_widget(Paragraph::new(Line::from(sp("Binary file, not shown.", faint()))), body);
        return;
    }
    let Some(styled) = text.styled.clone() else { return };
    let lines = &styled.hunks.first().map(|h| h.lines.as_slice()).unwrap_or(&[]);
    let h = body.height as usize;
    let v = app.files_view(root);
    v.file_height = h;
    v.file_scroll = v.file_scroll.min(lines.len().saturating_sub(h));
    let scroll = v.file_scroll;
    let mark_line = v.mark_line;
    let nw = lines.len().to_string().len().max(3);
    let code_w = (body.width as usize).saturating_sub(nw + 4);
    let mut out = Vec::with_capacity(h);
    for l in lines.iter().skip(scroll).take(h) {
        let n = l.new.unwrap_or(0);
        let (bar, bar_color) = match text.marks.get(&n) {
            Some(Mark::Added) => ("▎", t.green),
            Some(Mark::Modified) => ("▎", t.blue),
            Some(Mark::Removed) => ("▔", t.red),
            None => (" ", t.faint),
        };
        let here = mark_line == Some(n);
        let base = if here { Style::new().bg(t.active) } else { Style::new() };
        let mut spans =
            vec![sp(bar, fg(bar_color)), sp(format!("{n:>nw$}  "), if here { bold(t.accent) } else { faint() })];
        let mut used = 0;
        for seg in &l.segs {
            if used >= code_w {
                break;
            }
            let piece = crate::ui::clean(&seg.text.replace('\t', "    "));
            let room = code_w - used;
            let piece = if w(&piece) > room { fit(&piece, room) } else { piece };
            used += w(&piece);
            spans.push(Span::styled(piece, crate::view_changes::role_style(seg.role)));
        }
        out.push(Line::from(spans).style(base));
    }
    if text.truncated && scroll + h >= lines.len() {
        out.push(Line::from(sp("   … the file is longer; only the first 2 MB is shown", faint())));
    }
    f.render_widget(Paragraph::new(out), Rect { width: body.width.saturating_sub(1), ..body });
    scrollbar(f, Rect { x: body.right().saturating_sub(1), width: 1, ..body }, scroll, h, lines.len(), focused);
}

/// The Ctrl-P overlay, near the top as in VS Code.
pub fn finder(f: &mut Frame, area: Rect, app: &mut App) {
    use crate::finder::{Mode, Pick};
    let t = theme::get();
    app.refresh_finder();
    let mode = app.finder_mode().unwrap_or(Mode::Files);
    let waiting = app.finder.as_ref().is_some_and(|fi| match mode {
        Mode::Files => fi.root.as_ref().is_none_or(|r| app.files.get(r).is_none_or(|tr| tr.index.is_none())),
        Mode::Projects => app.folders.is_none(),
        _ => false,
    });
    let now = app.now;
    let Some(fi) = &app.finder else { return };
    let width = area.width.saturating_sub(4).min(100);
    let shown = fi.results.len().min(14) as u16;
    let height = (shown + 5).min(area.height.saturating_sub(2));
    let r = Rect { x: area.x + (area.width - width) / 2, y: area.y + 2, width, height };
    f.render_widget(Clear, r);
    let place = match (mode, &fi.root) {
        (Mode::Files, Some(root)) => format!(" in {}", aulora_core::util::basename(root)),
        _ => String::new(),
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(fg(t.accent))
        .style(Style::new().bg(t.panel))
        .title(sp(format!(" {}{place} ", mode.title()), faint()));
    let inner = block.inner(r);
    f.render_widget(block, r);
    let iw = inner.width as usize;
    let count = if waiting && fi.results.is_empty() {
        format!("{} looking…", spinner(now))
    } else if waiting {
        format!("{} {} so far ", spinner(now), fi.total)
    } else {
        format!("{} ", fi.total)
    };
    let mut lines = vec![lr(
        vec![sp(" > ", bold(t.accent)), sp(fi.query.clone(), text()), sp("▏", fg(t.accent))],
        vec![sp(count, faint())],
        iw,
    )];
    let hint = match mode {
        Mode::Projects => " ⏎ open with a shell · ⇥ open with Claude · type a path to open any folder",
        _ if fi.query.is_empty() => " files · # workspaces · @ agents · > actions",
        _ => "",
    };
    lines.push(Line::from(sp(fit(hint, iw), faint())));
    lines.push(Line::from(sp("─".repeat(iw), fg(t.rule))));
    let mut hits = Vec::new();
    let start = fi.selected.saturating_sub(shown.saturating_sub(1) as usize);
    for (i, item) in fi.results.iter().enumerate().skip(start).take(shown as usize) {
        let selected = i == fi.selected;
        let mut spans = vec![sp(if selected { " › " } else { "   " }, fg(t.accent))];
        // Files lead with their name, then the folder (VS Code).
        let (lead_at, glyph) = match &item.pick {
            Pick::File(p) => (p.rfind('/').map_or(0, |x| x + 1), ""),
            Pick::Project(_) => (0, "● "),
            Pick::Folder(_) => (0, "○ "),
            Pick::Agent(_) => (0, "◆ "),
            Pick::Action(_) => (0, "› "),
        };
        if !glyph.is_empty() {
            spans.push(sp(glyph, faint()));
        }
        let mut lead = Vec::new();
        let mut rest = Vec::new();
        for (ci, (byte, ch)) in item.text.char_indices().enumerate() {
            let hit = item.idx.contains(&(ci as u32));
            let style = match (hit, byte >= lead_at) {
                (true, _) => bold(t.accent),
                (false, true) if selected => bold(t.text),
                (false, true) => fg(t.text),
                (false, false) => faint(),
            };
            let target = if byte >= lead_at { &mut lead } else { &mut rest };
            target.push(Span::styled(ch.to_string(), style));
        }
        spans.extend(lead);
        if !rest.is_empty() {
            spans.push(sp("  ", text()));
            spans.extend(rest);
        }
        let mut right = Vec::new();
        if !item.note.is_empty() {
            right.push(sp(format!("{} ", item.note), faint()));
        }
        let line = lr(crate::ui::fit_spans(spans, iw.saturating_sub(crate::ui::spans_width(&right) + 1)), right, iw);
        lines.push(if selected { line.style(Style::new().bg(t.select)) } else { line });
        hits.push((
            Rect { x: inner.x, y: inner.y + lines.len() as u16 - 1, width: inner.width, height: 1 },
            Hit::FinderItem(i),
        ));
    }
    if fi.results.is_empty() && !waiting {
        lines.push(Line::from(sp("   nothing matches", faint())));
    }
    f.render_widget(Paragraph::new(lines), inner);
    app.hits.push((area, Hit::Dismiss));
    app.hits.extend(hits);
}
