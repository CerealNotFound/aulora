//! The Changes tab: VS Code's Source Control tree on the left, the
//! selected file's diff on the right (delta's look), and who wrote it.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::{App, Focus, Hit, RowRef};
use crate::changes::{DiffState, FileChange, Node, Row};
use crate::git::{Kind, LineKind};
use crate::highlight::{Role, Styled};
use crate::theme::{self, bold, dim, faint, fg, spinner, text};
use crate::ui::{fit, lr, scrollbar, sp, w};

pub fn kind_color(k: Kind) -> Color {
    let t = theme::get();
    match k {
        Kind::Conflict | Kind::Deleted => t.red,
        Kind::Modified | Kind::TypeChange => t.yellow,
        Kind::Added | Kind::Untracked => t.green,
        Kind::Renamed => t.cyan,
    }
}

/// A steady color per session, so its dot means the same thing everywhere.
pub fn session_color(id: &str) -> Color {
    let t = theme::get();
    let palette = [t.accent, t.magenta, t.cyan, t.orange, t.green, t.yellow, t.blue];
    let h = id.bytes().fold(5381u32, |h, b| h.wrapping_mul(33) ^ b as u32);
    palette[h as usize % palette.len()]
}

fn role_color(r: Role) -> Color {
    let t = theme::get();
    match r {
        Role::Text => t.text,
        Role::Comment => t.faint,
        Role::Keyword => t.magenta,
        Role::String => t.green,
        Role::Constant => t.orange,
        Role::Function => t.blue,
        Role::Type => t.yellow,
        Role::Punct => t.dim,
        Role::Tag => t.red,
        Role::Property => t.cyan,
    }
}

/// `c` laid over the background at `amount`, when both are true colors.
fn tint(c: Color, amount: f32) -> Option<Color> {
    let t = theme::get();
    let base = match t.bg {
        Color::Rgb(r, g, b) => (r, g, b),
        _ if t.light => (255, 255, 255),
        _ => (30, 30, 46),
    };
    let Color::Rgb(r, g, b) = c else { return None };
    let mix = |a: u8, b: u8| (a as f32 * amount + b as f32 * (1.0 - amount)).round() as u8;
    Some(Color::Rgb(mix(r, base.0), mix(g, base.1), mix(b, base.2)))
}

struct Tints {
    add: Option<Color>,
    add_em: Option<Color>,
    del: Option<Color>,
    del_em: Option<Color>,
}

fn tints() -> Tints {
    let t = theme::get();
    let (soft, strong) = if t.light { (0.14, 0.32) } else { (0.13, 0.30) };
    Tints {
        add: tint(t.green, soft),
        add_em: tint(t.green, strong),
        del: tint(t.red, soft),
        del_em: tint(t.red, strong),
    }
}

pub fn session_label(app: &App, id: &str) -> String {
    app.session(id).map(|s| s.display_title()).unwrap_or_else(|| format!("session {}", &id[..id.len().min(8)]))
}

pub fn draw(f: &mut Frame, area: Rect, app: &mut App, root: &str) {
    let t = theme::get();
    app.refresh_changes(root);
    let (loaded, error) = app.changes.get(root).map(|p| (p.loaded, p.error.clone())).unwrap_or((false, None));
    if !loaded {
        let msg = Line::from(vec![sp(format!("  {} ", spinner(app.now)), fg(t.accent)), sp("reading changes…", dim())]);
        f.render_widget(Paragraph::new(vec![Line::raw(""), msg]), area);
        return;
    }
    if let Some(e) = error {
        let msg = vec![Line::raw(""), Line::from(sp(format!("  git: {e}"), fg(t.red)))];
        f.render_widget(Paragraph::new(msg), area);
        return;
    }

    // Whose changes: an agent's own, or everything in the project.
    let mut area = area;
    let filter = app.changes_filter(root);
    let agent = !matches!(app.selected, Some(RowRef::Project(_)) | None) && app.selected_session_id().is_some();
    if agent {
        let line = match &filter {
            Some(sid) => Line::from(vec![
                sp("  ● ", fg(session_color(sid))),
                sp(format!("{}'s changes", session_label(app, sid)), dim()),
                sp("   a all", faint()),
            ]),
            None => Line::from(vec![sp("  every change in the project", dim()), sp("   a this agent's", faint())]),
        };
        f.render_widget(Paragraph::new(line), Rect { height: 1, ..area });
        area = Rect { y: area.y + 2, height: area.height.saturating_sub(2), ..area };
    }

    let rows = app.change_rows(root);
    if rows.is_empty() {
        let (head, sub) = match &filter {
            Some(_) => ("Nothing this agent wrote is uncommitted.", "a shows every change in the project"),
            None => ("No changes.", "The working tree matches the last commit."),
        };
        let msg = vec![
            Line::raw(""),
            Line::from(sp(format!("  {head}"), text())),
            Line::from(sp(format!("  {sub}"), faint())),
        ];
        f.render_widget(Paragraph::new(msg), area);
        return;
    }

    let selected = app.change_selected(root, &rows);
    let wide = area.width >= 100;
    let focus_diff = app.view_mut(root).focus_diff;
    let (tree_area, diff_area) = if wide {
        let tw = ((area.width as u32 * 36) / 100).clamp(32, 56) as u16;
        let sep = Rect { x: area.x + tw, width: 1, ..area };
        let lines: Vec<Line> = (0..sep.height).map(|_| Line::from(sp("│", fg(t.rule)))).collect();
        f.render_widget(Paragraph::new(lines), sep);
        (
            Some(Rect { width: tw, ..area }),
            Some(Rect { x: area.x + tw + 2, width: area.width.saturating_sub(tw + 2), ..area }),
        )
    } else if focus_diff {
        (None, Some(area))
    } else {
        (Some(area), None)
    };

    if let Some(ta) = tree_area {
        tree(f, ta, app, root, &rows, selected);
    }
    if let Some(da) = diff_area {
        let file = selected.and_then(|i| app.change_file(root, &rows[i]));
        match file {
            Some(file) => diff(f, da, app, root, &file),
            None => {
                let row = selected.map(|i| &rows[i]);
                folder_summary(f, da, app, row);
            }
        }
    }
}

fn tree(f: &mut Frame, area: Rect, app: &mut App, root: &str, rows: &[Row], selected: Option<usize>) {
    let t = theme::get();
    let width = area.width.saturating_sub(1) as usize;
    let h = area.height as usize;
    let focused = app.focus == Focus::Main && !app.view_mut(root).focus_diff;
    let v = app.view_mut(root);
    v.tree_height = h;
    let sel_id = selected.map(|i| rows[i].id.clone());
    if let Some(i) = selected.filter(|_| v.shown != sel_id) {
        v.shown = sel_id;
        if i < v.scroll {
            v.scroll = i;
        } else if i >= v.scroll + h {
            v.scroll = i + 1 - h;
        }
    }
    v.scroll = v.scroll.min(rows.len().saturating_sub(h));
    let scroll = v.scroll;
    app.hits.push((area, Hit::ChangeTree));
    let mut lines = Vec::with_capacity(h);
    for (i, r) in rows.iter().enumerate().skip(scroll).take(h) {
        let is_sel = selected == Some(i);
        let bg = match (is_sel, focused) {
            (true, true) => Style::new().bg(t.select),
            (true, false) => Style::new().bg(t.active),
            _ => Style::new(),
        };
        let indent = "  ".repeat(r.depth);
        let line = match &r.node {
            Node::Group(_) => lr(
                vec![
                    sp(format!(" {} ", if r.expanded { "▾" } else { "▸" }), faint()),
                    sp(r.label.clone(), bold(t.dim)),
                ],
                vec![sp(format!("{} ", r.count), faint())],
                width,
            ),
            Node::Dir(..) => {
                let color = r.kind.map(kind_color).unwrap_or(t.dim);
                lr(
                    vec![
                        sp(format!(" {indent}{} ", if r.expanded { "▾" } else { "▸" }), faint()),
                        sp(r.label.clone(), if is_sel { bold(color) } else { fg(color) }),
                    ],
                    vec![],
                    width,
                )
            }
            Node::File(_) => {
                let k = r.kind.unwrap_or(Kind::Modified);
                let color = kind_color(k);
                let mut right = dots(&r.sessions);
                right.extend(counts(r.added, r.removed));
                right.push(sp(format!(" {} ", k.letter()), fg(color)));
                let name_style = match (is_sel, k) {
                    (_, Kind::Deleted) => fg(color).add_modifier(Modifier::CROSSED_OUT),
                    (true, _) => bold(color),
                    (false, _) => fg(color),
                };
                lr(vec![sp(format!(" {indent}  "), text()), sp(r.label.clone(), name_style)], right, width)
            }
        };
        let y = area.y + (i - scroll) as u16;
        app.hits.push((Rect { x: area.x, y, width: area.width, height: 1 }, Hit::ChangeRow(i)));
        lines.push(line.style(bg));
    }
    f.render_widget(Paragraph::new(lines), Rect { width: area.width.saturating_sub(1), ..area });
    scrollbar(f, Rect { x: area.right().saturating_sub(1), width: 1, ..area }, scroll, h, rows.len(), focused);
}

/// `+12 −4`, leaving out a side that's zero.
fn counts(added: u32, removed: u32) -> Vec<Span<'static>> {
    let t = theme::get();
    let mut v = Vec::new();
    if added > 0 {
        v.push(sp(format!("+{added}"), fg(t.green)));
    }
    if removed > 0 {
        v.push(sp(format!("{}−{removed}", if added > 0 { " " } else { "" }), fg(t.red)));
    }
    v
}

fn dots(sessions: &[String]) -> Vec<Span<'static>> {
    let mut v: Vec<Span<'static>> = sessions.iter().take(3).map(|s| sp("●", fg(session_color(s)))).collect();
    if !v.is_empty() {
        v.push(sp(" ", text()));
    }
    v
}

fn folder_summary(f: &mut Frame, area: Rect, app: &App, row: Option<&Row>) {
    let t = theme::get();
    let Some(r) = row else { return };
    let mut lines = vec![
        Line::from(vec![
            sp(r.label.clone(), bold(t.text)),
            sp(format!("   {} changed", crate::ui::plural(r.count, "file")), faint()),
        ]),
        Line::from(counts(r.added, r.removed)),
        Line::raw(""),
    ];
    for s in &r.sessions {
        lines.push(Line::from(vec![sp("● ", fg(session_color(s))), sp(session_label(app, s), dim())]));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(sp("⏎ open · space stage all", faint())));
    f.render_widget(Paragraph::new(lines), area);
}

fn diff(f: &mut Frame, area: Rect, app: &mut App, root: &str, file: &FileChange) {
    let t = theme::get();
    let width = area.width as usize;
    // Header: the path, what happened, who wrote it.
    let (dir, name) = match file.entry.path.rsplit_once('/') {
        Some((d, n)) => (format!("{d}/"), n.to_string()),
        None => (String::new(), file.entry.path.clone()),
    };
    let mut left = vec![sp(dir, faint()), sp(name, bold(t.text))];
    if let Some(o) = &file.entry.orig {
        left.push(sp(format!("  ← {o}"), faint()));
    }
    left.push(sp(format!("  {}", file.entry.kind.word()), fg(kind_color(file.entry.kind))));
    let mut right = counts(file.added, file.removed);
    right.push(sp(" ", text()));
    let mut header = vec![lr(left, right, width)];
    let mut who: Vec<Span<'static>> = Vec::new();
    for s in file.sessions.iter().take(3) {
        who.push(sp("● ", fg(session_color(s))));
        who.push(sp(format!("{}   ", fit(&session_label(app, s), 40)), dim()));
    }
    if who.is_empty() {
        who.push(sp("no Claude session from the last 7 days wrote this", faint()));
    }
    header.push(lr(who, vec![], width));
    f.render_widget(Paragraph::new(header), Rect { height: 2, ..area });
    let body = Rect { y: area.y + 3, height: area.height.saturating_sub(3), ..area };
    app.hits.push((body, Hit::Diff));

    let state = match app.diff_for(root, file) {
        Some(DiffState::Ready(d)) => Ok(d.clone()),
        Some(DiffState::Failed(e)) => Err(Some(e.clone())),
        _ => Err(None),
    };
    let d = match state {
        Ok(d) => d,
        Err(e) => {
            let msg = match e {
                Some(e) => Line::from(sp(format!("couldn't read the diff: {e}"), fg(t.red))),
                None => {
                    Line::from(vec![sp(format!("{} ", spinner(app.now)), fg(t.accent)), sp("reading diff…", dim())])
                }
            };
            f.render_widget(Paragraph::new(msg), body);
            return;
        }
    };
    if d.binary {
        f.render_widget(Paragraph::new(Line::from(sp("Binary file, not shown.", faint()))), body);
        return;
    }
    if d.hunks.is_empty() {
        f.render_widget(Paragraph::new(Line::from(sp("No content changes (mode or rename only).", faint()))), body);
        return;
    }
    let focused = app.focus == Focus::Main && app.view_mut(root).focus_diff;
    let total = d.rows();
    let h = body.height as usize;
    let v = app.view_mut(root);
    v.diff_height = h;
    v.diff_scroll = v.diff_scroll.min(total.saturating_sub(h));
    let scroll = v.diff_scroll;
    let label_hunks = file.sessions.len() > 1;
    let lines = diff_lines(app, &d, scroll, h, (body.width.saturating_sub(1)) as usize, label_hunks);
    f.render_widget(Paragraph::new(lines), Rect { width: body.width.saturating_sub(1), ..body });
    scrollbar(f, Rect { x: body.right().saturating_sub(1), width: 1, ..body }, scroll, h, total, focused);
}

/// Rows `top..top+h` of a diff: a header per hunk, then its lines.
fn diff_lines(app: &App, d: &Styled, top: usize, h: usize, width: usize, label_hunks: bool) -> Vec<Line<'static>> {
    let t = theme::get();
    let tn = tints();
    let max_no = d.hunks.iter().flat_map(|x| &x.lines).flat_map(|l| [l.old, l.new]).flatten().max().unwrap_or(1);
    let nw = max_no.to_string().len().max(2);
    // A conflicted file has one numbering; a diff has the old and the new.
    let two = !d.hunks.iter().flat_map(|x| &x.lines).any(|l| l.kind == LineKind::Marker);
    let mut out = Vec::with_capacity(h);
    let mut row = 0usize;
    for hunk in &d.hunks {
        if row + hunk.lines.len() < top {
            row += hunk.lines.len() + 1;
            continue;
        }
        if row >= top && out.len() < h {
            // ── 120 · fn sidebar ──────────── ● claude · billing
            let mut left = vec![sp("── ", fg(t.rule))];
            if hunk.new_start > 0 {
                left.push(sp(format!("{} ", hunk.new_start), faint()));
            }
            if !hunk.context.is_empty() {
                left.push(sp(format!("{} ", hunk.context), dim()));
            }
            let mut right = Vec::new();
            if let Some(s) = hunk.session.as_ref().filter(|_| label_hunks) {
                right.push(sp(" ● ", fg(session_color(s))));
                right.push(sp(format!("{} ", fit(&session_label(app, s), 36)), faint()));
            }
            let used = crate::ui::spans_width(&left) + crate::ui::spans_width(&right);
            left.push(sp("─".repeat(width.saturating_sub(used)), fg(t.rule)));
            out.push(Line::from(
                [crate::ui::fit_spans(left, width.saturating_sub(crate::ui::spans_width(&right))), right].concat(),
            ));
        }
        row += 1;
        for l in &hunk.lines {
            if row >= top && out.len() < h {
                out.push(code_line(l, nw, two, width, &tn));
            }
            row += 1;
        }
        if out.len() >= h {
            break;
        }
    }
    out
}

fn code_line(l: &crate::highlight::Line, nw: usize, two: bool, width: usize, tn: &Tints) -> Line<'static> {
    let t = theme::get();
    let (sign, sign_color, bg, em) = match l.kind {
        LineKind::Added => ("+", t.green, tn.add, tn.add_em),
        LineKind::Removed => ("-", t.red, tn.del, tn.del_em),
        LineKind::Marker => ("!", t.red, tint(t.red, 0.2), None),
        LineKind::Context => (" ", t.faint, None, None),
    };
    let base = bg.map(|b| Style::new().bg(b)).unwrap_or_default();
    let num = |n: Option<u32>| format!("{:>nw$} ", n.map(|n| n.to_string()).unwrap_or_default());
    let gutter = if two { format!("{}{}", num(l.old), num(l.new)) } else { num(l.new.or(l.old)) };
    let mut used = w(&gutter) + 2;
    let mut spans = vec![sp(gutter, faint().patch(base)), sp(format!("{sign} "), fg(sign_color).patch(base))];
    for seg in &l.segs {
        if used >= width {
            break;
        }
        let text = seg.text.replace('\t', "    ");
        let mut style = fg(role_color(seg.role)).patch(base);
        if seg.emph {
            style = match em {
                Some(c) => style.bg(c),
                None => style.add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            };
        }
        if l.kind == LineKind::Marker {
            style = style.fg(t.red).add_modifier(Modifier::BOLD);
        }
        let room = width - used;
        let piece = if w(&text) > room { fit(&text, room) } else { text };
        used += w(&piece);
        spans.push(Span::styled(piece, style));
    }
    if bg.is_some() && used < width {
        spans.push(sp(" ".repeat(width - used), base));
    }
    Line::from(spans)
}

/// Row offsets where each hunk starts, for `[` and `]`.
pub fn hunk_starts(d: &Styled) -> Vec<usize> {
    let mut v = Vec::with_capacity(d.hunks.len());
    let mut row = 0;
    for h in &d.hunks {
        v.push(row);
        row += h.lines.len() + 1;
    }
    v
}

/// The first changed line's number in the hunk at or after `row`, for
/// opening the editor there.
pub fn line_at(d: &Styled, row: usize) -> Option<u32> {
    let mut r = 0;
    for h in &d.hunks {
        if r + h.lines.len() + 1 > row {
            return h
                .lines
                .iter()
                .find(|l| l.kind != LineKind::Context)
                .and_then(|l| l.new.or(l.old))
                .or(Some(h.new_start));
        }
        r += h.lines.len() + 1;
    }
    None
}
