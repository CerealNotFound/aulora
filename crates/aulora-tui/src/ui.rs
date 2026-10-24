//! Drawing, Herdr-style: a borderless sidebar (projects above agents), a
//! tab bar, one view at a time, and a mode bar. Every clickable area is
//! recorded in `app.hits` as it's drawn.

use aulora_core::model::*;
use aulora_core::paths::tildify;
use aulora_core::reduce::tool_label;
use aulora_core::util::{ago, basename, fmt_tokens, short_duration, short_model};
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::{App, Conn, Focus, Hit, RowRef, Status};
use crate::keymap::{self, BINDINGS, Scope};
use crate::theme::{self, bold, dim, faint, fg, label, spinner, text};

// ---------------------------------------------------------------------------
// Helpers

pub fn w(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// Cut to `max` display columns, with an ellipsis if cut.
pub fn fit(s: &str, max: usize) -> String {
    if w(s) <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if used + cw > max - 1 {
            break;
        }
        out.push(c);
        used += cw;
    }
    out.push('…');
    out
}

pub(crate) fn spans_width(spans: &[Span]) -> usize {
    spans.iter().map(|s| w(&s.content)).sum()
}

/// Drop whole spans from the end before cutting text (Herdr).
pub(crate) fn fit_spans(spans: Vec<Span<'static>>, max: usize) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    let mut used = 0;
    for s in spans {
        let sw = w(&s.content);
        if used + sw <= max {
            used += sw;
            out.push(s);
        } else {
            let rest = max.saturating_sub(used);
            if rest > 3 {
                out.push(Span::styled(fit(&s.content, rest), s.style));
            }
            break;
        }
    }
    out
}

/// Wrap styled text over lines of the given widths, breaking at spaces,
/// with an ellipsis only if it still doesn't fit in all of them.
pub(crate) fn wrap_spans(spans: &[Span<'static>], widths: &[usize]) -> Vec<Vec<Span<'static>>> {
    let cells: Vec<(char, Style)> = spans.iter().flat_map(|s| s.content.chars().map(move |c| (c, s.style))).collect();
    let mut lines: Vec<Vec<(char, Style)>> = Vec::new();
    let mut start = 0;
    for (n, &width) in widths.iter().enumerate() {
        while start < cells.len() && cells[start].0 == ' ' {
            start += 1;
        }
        if start >= cells.len() || width == 0 {
            break;
        }
        let (mut end, mut used, mut space) = (start, 0, None);
        while end < cells.len() {
            let cw = cells[end].0.width().unwrap_or(0);
            if used + cw > width {
                break;
            }
            if cells[end].0 == ' ' {
                space = Some(end);
            }
            used += cw;
            end += 1;
        }
        let last = n + 1 == widths.len();
        // Back up to the last space, unless the line ends right at one.
        if end < cells.len()
            && !last
            && cells[end].0 != ' '
            && let Some(s) = space.filter(|s| *s > start)
        {
            end = s;
        }
        let mut line = cells[start..end].to_vec();
        if last && end < cells.len() {
            // Out of room: end with an ellipsis.
            while line.iter().map(|(c, _)| c.width().unwrap_or(0)).sum::<usize>() + 1 > width && !line.is_empty() {
                line.pop();
            }
            let style = line.last().map(|(_, s)| *s).unwrap_or_default();
            line.push(('…', style));
        }
        lines.push(line);
        start = end;
    }
    lines
        .into_iter()
        .map(|line| {
            let mut out: Vec<Span<'static>> = Vec::new();
            for (c, style) in line {
                match out.last_mut() {
                    Some(last) if last.style == style => last.content.to_mut().push(c),
                    _ => out.push(Span::styled(c.to_string(), style)),
                }
            }
            out
        })
        .collect()
}

/// Left and right content on one line, padded to `width`.
pub(crate) fn lr(left: Vec<Span<'static>>, right: Vec<Span<'static>>, width: usize) -> Line<'static> {
    let rw = spans_width(&right);
    let left = fit_spans(left, width.saturating_sub(rw + usize::from(rw > 0)));
    let lw = spans_width(&left);
    let mut spans = left;
    spans.push(Span::raw(" ".repeat(width.saturating_sub(lw + rw))));
    spans.extend(right);
    Line::from(spans)
}

/// Text made safe to draw on one line: control characters become spaces.
pub(crate) fn clean(s: &str) -> String {
    if s.chars().any(|c| c.is_control()) {
        s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect()
    } else {
        s.to_string()
    }
}

/// A span of one line of text. Line breaks and tabs in text from
/// prompts, titles or commands would move the terminal's cursor mid-frame
/// and break the layout, so they become spaces.
pub(crate) fn sp(s: impl Into<String>, style: Style) -> Span<'static> {
    let s: String = s.into();
    if s.chars().any(|c| c.is_control()) {
        let clean: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
        return Span::styled(clean, style);
    }
    Span::styled(s, style)
}

fn clock(ms: Ms) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|d| d.with_timezone(&chrono::Local).format("%H:%M").to_string())
        .unwrap_or_default()
}

pub(crate) fn plural(n: usize, word: &str) -> String {
    if n == 1 { format!("1 {word}") } else { format!("{n} {word}s") }
}

pub(crate) fn fill(f: &mut Frame, area: Rect, bg: Color) {
    if bg != Color::Reset {
        f.render_widget(Block::new().style(Style::new().bg(bg)), area);
    }
}

/// Text on a pill filled with a color.
pub(crate) fn on_fill() -> Color {
    let t = theme::get();
    match t.bg {
        Color::Reset if t.light => Color::White,
        Color::Reset => Color::Black,
        c => c,
    }
}

fn editor_name(app: &App) -> String {
    match app.editor.as_str() {
        "code" | "code-insiders" => "VS Code".into(),
        "cursor" => "Cursor".into(),
        "zed" => "Zed".into(),
        other => other.to_string(),
    }
}

pub fn status_color(s: Status) -> Color {
    let t = theme::get();
    match s {
        Status::Blocked => t.red,
        Status::Done => t.cyan,
        Status::Working => t.yellow,
        Status::Idle => t.green,
        Status::Ended => t.faint,
    }
}

pub fn status_glyph(s: Status) -> &'static str {
    match s {
        Status::Idle => "○",
        Status::Ended => "·",
        _ => "●",
    }
}

/// Dim everything already drawn, behind a modal (Herdr).
fn dim_behind(buf: &mut Buffer) {
    let area = buf.area;
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if let Some(c) = buf.cell_mut((x, y)) {
                c.modifier.insert(Modifier::DIM);
            }
        }
    }
}

pub(crate) fn scrollbar(f: &mut Frame, area: Rect, top: usize, view: usize, total: usize, focused: bool) {
    if total <= view || area.height == 0 {
        return;
    }
    let t = theme::get();
    let h = area.height as usize;
    let thumb = ((view * h) / total).clamp(1, h);
    let pos = ((top * (h - thumb)) / (total - view).max(1)).min(h - thumb);
    let buf = f.buffer_mut();
    for i in 0..h {
        let on = i >= pos && i < pos + thumb;
        let (sym, color) = match (on, focused) {
            (true, true) => ("▐", t.dim),
            (true, false) => ("▕", t.faint),
            (false, _) => ("▕", t.rule),
        };
        if let Some(c) = buf.cell_mut((area.x, area.y + i as u16)) {
            c.set_symbol(sym).set_fg(color);
        }
    }
}

// ---------------------------------------------------------------------------
// Frame

pub fn draw(f: &mut Frame, app: &mut App) {
    let t = theme::get();
    let area = f.area();
    app.hits.clear();
    app.view_sizes.clear();
    app.cut_rows.clear();
    f.render_widget(Clear, area);
    fill(f, area, t.bg);

    let rows = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(area);
    if app.snap.is_some() && app.rows().is_empty() && app.filter.is_empty() {
        empty_state(f, rows[0], app);
    } else {
        body(f, rows[0], app);
    }
    mode_bar(f, rows[1], app);
    app.frame = Some(f.buffer_mut().clone());

    if app.help.is_some() {
        dim_behind(f.buffer_mut());
        help(f, area, app);
    } else if app.theme_picker.is_some() {
        dim_behind(f.buffer_mut());
        theme_picker(f, area, app);
    } else if app.confirm.is_some() {
        dim_behind(f.buffer_mut());
        confirm(f, area, app);
    }
    if app.finder.is_some() {
        crate::view_files::finder(f, area, app);
    }
    if app.away.is_some() && app.welcome.is_none() {
        crate::away::draw(f, area, app);
    }
    if app.welcome.is_some() {
        dim_behind(f.buffer_mut());
        crate::welcome::draw(f, area, app);
    }
    menu(f, area, app);
    toast(f, rows[0], app);
    app.hover_url = if app.menu.is_none() && app.finder.is_none() {
        crate::links::hover(f.buffer_mut(), app.mouse_pos, wrapped_line(app))
    } else {
        None
    };
    if let Some(s) = &app.selection {
        s.paint(f.buffer_mut());
    }
}

/// Where the text under the pointer goes on past its row: the rows of a
/// terminal line that wrapped, or of a word a conversation cut to fit, so
/// a long URL is one link.
fn wrapped_line(app: &App) -> Option<crate::links::Run> {
    let (col, row) = app.mouse_pos?;
    let cut = |y: u16| app.cut_rows.iter().find(|(r, cols, _)| *r == y && cols.contains(&col));
    if cut(row).is_some() || row.checked_sub(1).and_then(cut).is_some() {
        let (mut first, mut last) = (row, row);
        while first > 0 && cut(first - 1).is_some() {
            first -= 1;
        }
        while cut(last).is_some() {
            last += 1;
        }
        let cols = cut(first).map(|(_, c, _)| c.clone())?;
        let rows = (first..=last).map(|y| (y, cols.start..cut(y).map_or(cols.end, |(_, _, at)| *at))).collect();
        return Some(crate::links::Run { rows, indented: true });
    }
    let (area, id) = app.hits.iter().find_map(|(r, h)| match h {
        Hit::Terminal(id) if col >= r.x && col < r.right() && row >= r.y && row < r.bottom() => Some((*r, id)),
        _ => None,
    })?;
    let screen = app.terms.get(id)?.parser.screen();
    let (rows, cols) = screen.size();
    // Drawn cut short (not yet resized to fit): its wrapping isn't what shows.
    if cols.get() != area.width {
        return None;
    }
    let last = rows.get().min(area.height).saturating_sub(1);
    let at = row - area.y;
    let (mut first, mut end) = (at, at);
    while first > 0 && screen.row_wrapped(first - 1) {
        first -= 1;
    }
    while end < last && screen.row_wrapped(end) {
        end += 1;
    }
    let rows = (area.y + first..=area.y + end).map(|y| (y, area.x..area.right())).collect();
    Some(crate::links::Run { rows, indented: false })
}

fn body(f: &mut Frame, area: Rect, app: &mut App) {
    let t = theme::get();
    // Narrow, or asked for (`\`): the sidebar shrinks to glyphs (Herdr's
    // compact sidebar), so the panes keep the room.
    let compact = match app.sidebar_compact {
        Some(c) => c,
        None => area.width < 100,
    };
    if compact {
        let side_w = 5;
        let cols =
            Layout::horizontal([Constraint::Length(side_w), Constraint::Length(1), Constraint::Min(0)]).split(area);
        fill(f, cols[0], t.surface);
        sidebar_compact(f, cols[0], app);
        let sep: Vec<Line> = (0..cols[1].height).map(|_| Line::from(sp("│", fg(t.rule)))).collect();
        f.render_widget(Paragraph::new(sep), cols[1]);
        main(f, cols[2], app);
        return;
    }
    let side_w = ((area.width as u32 * 26) / 100).clamp(30, 44) as u16;
    let cols = Layout::horizontal([Constraint::Length(side_w), Constraint::Length(1), Constraint::Min(0)]).split(area);
    fill(f, cols[0], t.surface);
    sidebar(f, cols[0], app);
    let sep: Vec<Line> = (0..cols[1].height).map(|_| Line::from(sp("│", fg(t.rule)))).collect();
    f.render_widget(Paragraph::new(sep), cols[1]);
    main(f, cols[2], app);
}

// ---------------------------------------------------------------------------
// Sidebar

/// The sidebar as a strip of glyphs: each project's state and first
/// letter, then a dot per agent. Click to select; `\\` to widen.
fn sidebar_compact(f: &mut Frame, area: Rect, app: &mut App) {
    let t = theme::get();
    let rows = app.rows();
    let mut lines = Vec::new();
    lines.push(Line::from(
        std::iter::once(sp(" ", text())).chain(crate::brand::mark(t).into_iter().take(2)).collect::<Vec<_>>(),
    ));
    lines.push(Line::raw(""));
    app.hits.push((area, Hit::Sidebar));
    for r in rows.iter() {
        let y = area.y + lines.len() as u16;
        if y + 1 >= area.bottom() {
            break;
        }
        let selected = app.selected.as_ref() == Some(r);
        let status = app.status(r);
        let bg = match (selected, app.focus) {
            (true, Focus::Sidebar) => Style::new().bg(t.select),
            (true, Focus::Main) => Style::new().bg(t.active),
            _ => Style::new(),
        };
        let line = match r {
            RowRef::Project(_) => {
                let initial: String =
                    app.title(r).chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
                if lines.len() > 2 {
                    lines.push(Line::raw(""));
                }
                Line::from(vec![
                    sp(format!(" {} ", status_glyph(status)), fg(status_color(status))),
                    sp(initial, bold(t.text)),
                ])
            }
            _ => Line::from(vec![sp("  ", text()), sp(status_glyph(status), fg(status_color(status)))]),
        };
        let y = area.y + lines.len() as u16;
        app.hits.push((Rect { x: area.x, y, width: area.width, height: 1 }, Hit::Row(r.clone())));
        lines.push(line.style(bg));
    }
    f.render_widget(Paragraph::new(lines), area);
    let foot = Rect { y: area.bottom().saturating_sub(1), height: 1, ..area };
    f.render_widget(Paragraph::new(Line::from(sp(" \\ ", faint()))), foot);
    app.hits.push((foot, Hit::Action(crate::keymap::Action::ToggleSidebar)));
}

/// One sidebar entry: its lines and what it stands for.
struct Item {
    lines: Vec<Line<'static>>,
    row: Option<RowRef>,
    /// A button at the right end of the first line (× or +).
    button: Option<Hit>,
}

fn git_line(g: &GitInfo, selected: bool) -> Vec<Span<'static>> {
    let t = theme::get();
    let mut v = vec![sp(g.branch.clone().unwrap_or_default(), if selected { fg(t.magenta) } else { faint() })];
    if g.ahead > 0 {
        v.push(sp(format!(" ↑{}", g.ahead), fg(t.green)));
    }
    if g.behind > 0 {
        v.push(sp(format!(" ↓{}", g.behind), fg(t.red)));
    }
    let changes = g.staged + g.modified + g.untracked;
    if g.conflicts > 0 {
        v.push(sp(format!(" ✗{}", g.conflicts), fg(t.red)));
    } else if changes > 0 {
        v.push(sp(format!(" ●{changes}"), fg(t.yellow)));
    }
    v
}

/// Markdown out of a reply, so a line under an agent reads as text.
fn plain_text(s: &str) -> String {
    aulora_core::util::squash(&s.replace("**", "").replace('`', "").replace("## ", "").replace("# ", ""))
}

/// The newest thing that happened in a session: Claude's last reply, or
/// your last prompt if you've sent one since. The title can be hours old;
/// this is what's true now.
fn latest(s: &Session) -> Vec<Span<'static>> {
    let t = theme::get();
    let reply_newer = match (s.last_reply_at, s.last_prompt_at) {
        (Some(r), Some(p)) => r >= p,
        _ => s.last_reply.is_some(),
    };
    match (&s.last_reply, &s.last_prompt) {
        (Some(r), _) if reply_newer && !r.trim().is_empty() => vec![sp(plain_text(r), faint())],
        (_, Some(p)) if !p.trim().is_empty() => vec![sp("› ", fg(t.accent)), sp(plain_text(p), dim())],
        _ => vec![],
    }
}

/// What a session that's waiting is waiting with.
fn waiting(s: &Session) -> Vec<Span<'static>> {
    let l = latest(s);
    if !l.is_empty() {
        return l;
    }
    if s.turns == 0 && s.transcript_path.is_none() {
        vec![sp("waiting for a first prompt", faint())]
    } else {
        vec![sp("idle", faint())]
    }
}

/// The lines under an agent: what it's doing, or the last thing that
/// happened.
fn agent_detail(app: &App, r: &RowRef) -> Vec<Span<'static>> {
    let t = theme::get();
    let now = app.now;
    let session = match r {
        RowRef::Session(id) => app.session(id),
        RowRef::Pane(id) => app.pane_session(id).filter(|s| s.state.is_live()),
        RowRef::Project(_) => None,
    };
    if let Some(s) = session {
        return match &s.state {
            SessionState::NeedsYou { reason, detail } => {
                vec![sp(format!("{}: ", reason.label()), fg(t.red)), sp(detail.clone(), dim())]
            }
            SessionState::Errored { detail } => vec![sp(detail.clone(), fg(t.red))],
            SessionState::Working | SessionState::Compacting => match (&s.current_tool, &s.live_text) {
                (Some(c), _) => vec![sp(tool_label(&c.name), fg(t.cyan)), sp(format!(" {}", c.target), faint())],
                (None, Some(text)) if !text.trim().is_empty() => vec![sp(plain_text(text), faint())],
                _ => {
                    // Thinking: say about what, from your prompt.
                    let mut v = vec![sp("thinking", faint())];
                    if let Some(p) = s.last_prompt.as_deref().filter(|p| !p.trim().is_empty()) {
                        v.push(sp(" about › ", faint()));
                        v.push(sp(plain_text(p), dim()));
                    }
                    v
                }
            },
            SessionState::YourTurn | SessionState::Idle => waiting(s),
            SessionState::Ended { .. } => {
                let mut v = vec![sp(format!("ended {} · ", ago(now, s.last_event_at)), faint())];
                v.extend(latest(s));
                v
            }
        };
    }
    match r {
        RowRef::Pane(id) => match app.pane(id) {
            Some(p) if p.exit.is_some() => vec![sp("exited · r restart · x close", faint())],
            Some(p) => {
                let what = p.title.clone().unwrap_or_else(|| match p.kind {
                    PaneKind::Command => p.args.join(" "),
                    _ => "shell".into(),
                });
                vec![sp(what, faint())]
            }
            None => vec![],
        },
        _ => vec![],
    }
}

fn sidebar(f: &mut Frame, area: Rect, app: &mut App) {
    let t = theme::get();
    let width = area.width as usize;
    let now = app.now;
    let row_bg = |selected: bool| -> Style {
        match (selected, app.focus) {
            (true, Focus::Sidebar) => Style::new().bg(t.select),
            (true, Focus::Main) => Style::new().bg(t.active),
            _ => Style::new(),
        }
    };

    let mut items: Vec<Item> = Vec::new();
    let header = |title: &str, right: Vec<Span<'static>>| Item {
        lines: vec![lr(vec![sp(format!(" {title}"), label())], right, width)],
        row: None,
        button: None,
    };
    let blank = || Item { lines: vec![Line::raw("")], row: None, button: None };

    let status = match &app.conn {
        Conn::Offline(_) => sp("daemon offline", fg(t.red)),
        Conn::Connecting => sp(spinner(now), faint()),
        Conn::Demo => sp("demo", fg(t.magenta)),
        Conn::Live => sp("", text()),
    };
    let date = chrono::DateTime::from_timestamp_millis(now)
        .map(|d| d.with_timezone(&chrono::Local).format("%a %-d %b").to_string())
        .unwrap_or_default();
    // The wordmark across the top, with the connection, date and clock
    // under it; just the mark and name if the sidebar is ever too narrow.
    let lines = match crate::brand::side(t, area.width, t.surface) {
        Some(art) => {
            let mut lines = vec![Line::raw("")];
            lines.extend(art.into_iter().map(|l| Line::from([vec![sp(" ", text())], l.spans].concat())));
            lines.push(lr(
                vec![sp(" ", text()), status],
                vec![sp(format!("{date}  "), faint()), sp(format!("{} ", clock(now)), bold(t.text))],
                width,
            ));
            lines
        }
        None => {
            let mut brand = vec![sp(" ", text())];
            brand.extend(crate::brand::mark(t));
            brand.extend([sp("  ", text()), status]);
            vec![lr(brand, vec![sp(format!("{} ", clock(now)), faint())], width)]
        }
    };
    items.push(Item { lines, row: None, button: None });
    items.push(blank());

    if app.filtering || !app.filter.is_empty() {
        items.push(Item {
            lines: vec![Line::from(vec![
                sp(" / ", bold(t.accent)),
                sp(app.filter.clone(), text()),
                sp(if app.filtering { "▏" } else { "" }, fg(t.accent)),
            ])],
            row: None,
            button: None,
        });
        items.push(blank());
    }

    // One tree: each project, then its agents (most urgent first).
    let hover = app.hover.clone();
    let projects = app.projects();
    items.push(header("workspaces", vec![sp(format!("{} ", projects.len()), faint())]));
    items.push(blank());
    if projects.is_empty() && !app.filter.is_empty() {
        items.push(Item { lines: vec![Line::from(sp(" nothing matches", faint()))], row: None, button: None });
    }
    for (pi, p) in projects.iter().enumerate() {
        let RowRef::Project(root) = p else { continue };
        let Some(proj) = app.project(root) else { continue };
        if pi > 0 {
            items.push(blank());
        }
        let selected = app.selected.as_ref() == Some(p);
        let hot = selected || hover.as_ref() == Some(p);
        let status = app.status(p);
        let agents = app.agents_sorted(root);
        let name_style = if selected { bold(t.text) } else { bold(t.dim) };
        let right = if hot {
            vec![sp(" + ", bold(t.accent))]
        } else if agents.is_empty() {
            vec![]
        } else {
            vec![sp(format!("{} ", agents.len()), faint())]
        };
        let l1 = lr(
            vec![
                sp(format!(" {} ", status_glyph(status)), fg(status_color(status))),
                sp(proj.name.clone(), name_style),
            ],
            right,
            width,
        );
        let mut second = vec![sp("   ", text())];
        if let Some(g) = &proj.git {
            second.extend(git_line(g, selected));
        } else {
            second.push(sp(tildify(root), faint()));
        }
        let l2 = lr(second, vec![], width);
        let bg = row_bg(selected);
        items.push(Item {
            lines: vec![l1.style(bg), l2.style(bg)],
            row: Some(p.clone()),
            button: hot.then(|| Hit::Add(root.clone())),
        });

        let n = agents.len();
        for (i, a) in agents.iter().enumerate() {
            let last = i + 1 == n;
            let (branch, stem) = if last { (" └ ", "   ") } else { (" ├ ", " │ ") };
            let selected = app.selected.as_ref() == Some(a);
            let hot = selected || hover.as_ref() == Some(a);
            let status = app.status(a);
            let title_style = match (selected, status) {
                (_, Status::Ended) => faint(),
                (true, _) => bold(t.text),
                (false, _) => fg(t.dim),
            };
            let closable = app.closable(a);
            let right = if hot && closable {
                vec![sp(" × ", fg(t.red))]
            } else {
                match status {
                    Status::Ended => vec![],
                    _ => vec![sp(format!("{} ", short_duration(now - app.since(a))), faint())],
                }
            };
            // The title and what's happening, each on up to two lines:
            // cutting them short hides exactly what you came to read.
            let indent = 5;
            let right_w = spans_width(&right) + 1;
            let title = wrap_spans(
                &[sp(app.title(a), title_style)],
                &[width.saturating_sub(indent + right_w), width.saturating_sub(indent + 1)],
            );
            let detail = wrap_spans(&agent_detail(app, a), &[width.saturating_sub(indent + 1); 2]);
            let mut lines = Vec::new();
            for (k, part) in title.into_iter().enumerate() {
                let mut left = if k == 0 {
                    vec![sp(branch, faint()), sp(format!("{} ", status_glyph(status)), fg(status_color(status)))]
                } else {
                    vec![sp(stem, faint()), sp("  ", text())]
                };
                left.extend(part);
                lines.push(lr(left, if k == 0 { right.clone() } else { vec![] }, width));
            }
            if lines.is_empty() {
                lines.push(lr(vec![sp(branch, faint())], right.clone(), width));
            }
            for part in detail {
                let mut left = vec![sp(stem, faint()), sp("  ", text())];
                left.extend(part);
                lines.push(lr(left, vec![], width));
            }
            let bg = row_bg(selected);
            items.push(Item {
                lines: lines.into_iter().map(|l| l.style(bg)).collect(),
                row: Some(a.clone()),
                button: (hot && closable).then(|| Hit::Close(a.clone())),
            });
        }
    }
    if projects.is_empty() {
        items.push(Item { lines: vec![Line::from(sp("   nothing running", faint()))], row: None, button: None });
    }

    // Lay out, keeping the selection in view, with the buttons pinned below.
    let body_h = area.height.saturating_sub(2) as usize;
    let total: usize = items.iter().map(|i| i.lines.len()).sum();
    let mut offsets = Vec::with_capacity(items.len());
    let mut y = 0;
    for it in &items {
        offsets.push(y);
        y += it.lines.len();
    }
    let selected = items.iter().position(|it| it.row.is_some() && it.row == app.selected);
    let span = selected.map(|i| (offsets[i], offsets[i] + items[i].lines.len()));
    // Follow the selection only when it changes (keys, a click, a pane
    // focused), so scrolling with the wheel isn't undone next frame.
    let moved = app.sidebar_followed != app.selected;
    app.sidebar_followed = app.selected.clone();
    app.sidebar_scroll = sidebar_scroll(app.sidebar_scroll, span.filter(|_| moved), body_h, total);
    let scroll = app.sidebar_scroll;

    app.hits.push((area, Hit::Sidebar));
    let mut lines = Vec::with_capacity(body_h);
    for (i, it) in items.iter().enumerate() {
        for (j, l) in it.lines.iter().enumerate() {
            let abs = offsets[i] + j;
            if abs < scroll || abs >= scroll + body_h {
                continue;
            }
            let screen_y = area.y + (abs - scroll) as u16;
            if let Some(r) = &it.row {
                app.hits.push((Rect { x: area.x, y: screen_y, width: area.width, height: 1 }, Hit::Row(r.clone())));
            }
            if let (0, Some(b)) = (j, &it.button) {
                // Leave the scrollbar column alone.
                let x = area.x + area.width.saturating_sub(4);
                app.hits.push((Rect { x, y: screen_y, width: 3, height: 1 }, b.clone()));
            }
            lines.push(l.clone());
        }
    }
    let body_area = Rect { height: body_h as u16, ..area };
    f.render_widget(Paragraph::new(lines), body_area);
    scrollbar(
        f,
        Rect { x: area.x + area.width.saturating_sub(1), width: 1, ..body_area },
        scroll,
        body_h,
        total,
        false,
    );

    // Herdr's "new" and "menu" at the bottom.
    let foot = Rect { y: area.y + area.height.saturating_sub(1), height: 1, ..area };
    let line = lr(vec![sp(" new", fg(t.accent))], vec![sp("menu ", fg(t.dim))], width);
    f.render_widget(Paragraph::new(line), foot);
    app.hits.push((Rect { x: foot.x, width: 5, ..foot }, Hit::NewMenu));
    app.hits.push((Rect { x: foot.right().saturating_sub(6), width: 6, ..foot }, Hit::AppMenu));
}

/// Where the sidebar's list starts: `scroll`, moved just enough to show
/// `follow` (a selection's first and last line) if given, and never past
/// the end.
fn sidebar_scroll(scroll: usize, follow: Option<(usize, usize)>, body_h: usize, total: usize) -> usize {
    let scroll = match follow {
        Some((start, _)) if start < scroll => start.saturating_sub(1),
        Some((_, end)) if end > scroll + body_h => end - body_h,
        _ => scroll,
    };
    scroll.min(total.saturating_sub(body_h))
}

// ---------------------------------------------------------------------------
// Main area

fn main(f: &mut Frame, area: Rect, app: &mut App) {
    let area = Rect { x: area.x + 1, width: area.width.saturating_sub(1), ..area };
    app.hits.push((area, Hit::Main));
    app.follow_selection();
    app.ensure_panel(area.width);
    let Some(root) = app.space_root() else { return };
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(area);
    top_bar(f, rows[0], app, &root);
    let space = app.layouts.space(&root).clone();
    let placed = crate::layout::place(&space, rows[1]);
    app.part_rects = placed.parts.clone();
    app.main_rect = rows[1];
    for (part, r) in &placed.parts {
        let Some(view) = space.view(*part).cloned() else { continue };
        let focused = app.focus == Focus::Main && *part == space.focus;
        draw_leaf(f, *r, app, &root, *part, &view, focused);
    }
    // The panel's edge can be dragged; it sits on top.
    if let Some(edge) = placed.edge {
        app.hits.push((edge, Hit::PanelEdge));
    }
}

/// Above the main area: the workspace's agents as tabs, `+` for a new
/// one, then the Changes and Files buttons and how the selected agent is
/// doing.
fn top_bar(f: &mut Frame, area: Rect, app: &mut App, root: &str) {
    let t = theme::get();
    let now = app.now;
    fill(f, area, t.panel);
    let panel = app.layouts.space(root).panel.clone();
    // Changes and Files, one click away (VS Code's activity bar).
    let mut buttons: Vec<(String, crate::keymap::Action, bool)> = Vec::new();
    if app.has_git(root) {
        let n = app.change_count(root).filter(|n| *n > 0).map(|n| format!(" {n}")).unwrap_or_default();
        let on = matches!(panel, Some(crate::layout::View::Changes { .. }));
        buttons.push((format!(" d changes{n} "), crate::keymap::Action::OpenChanges, on));
    }
    let on = panel == Some(crate::layout::View::Files);
    buttons.push((" f files ".to_string(), crate::keymap::Action::OpenFiles, on));
    let mut right = Vec::new();
    if let Some(sel) = app.selected.clone().filter(|s| !s.is_project()) {
        let status = app.status(&sel);
        let glyph = if status == Status::Working { spinner(now) } else { status_glyph(status) };
        right.push(sp(format!("{glyph} {} ", status.word()), bold(status_color(status))));
        if status != Status::Ended {
            right.push(sp(format!("{} ", short_duration(now - app.since(&sel))), faint()));
        }
    }
    // Buttons first, then the status; hits from the right edge.
    let mut all: Vec<Span<'static>> = Vec::new();
    for (label, _, on) in &buttons {
        let style = if *on { Style::new().bg(t.select).fg(t.text) } else { Style::new().bg(t.panel).fg(t.dim) };
        all.push(sp(label.clone(), style));
        all.push(sp(" ", Style::new().bg(t.panel)));
    }
    all.extend(right);
    let total = spans_width(&all) as u16;
    let mut bx = area.right().saturating_sub(total);
    for (label, action, _) in &buttons {
        let wd = w(label) as u16;
        app.hits.push((Rect { x: bx, y: area.y, width: wd, height: 1 }, Hit::Action(*action)));
        bx += wd + 1;
    }
    let budget = area.width.saturating_sub(total + 1) as usize;
    let left = agent_tabs(app, area, root, budget);
    f.render_widget(Paragraph::new(lr(left, all, area.width as usize)).style(Style::new().bg(t.panel)), area);
}

/// The workspace's agents as tabs, the shown one highlighted (VS Code's
/// open editors). When they don't all fit, the strip scrolls: the wheel
/// over it, or `‹n` and `n›`, which count the tabs out of view. Showing
/// another tab brings it into view.
fn agent_tabs(app: &mut App, area: Rect, root: &str, budget: usize) -> Vec<Span<'static>> {
    let t = theme::get();
    let tabs = app.tabs(root);
    let shown = app.shown_tab();
    let labels: Vec<(String, Status)> = tabs
        .iter()
        .map(|r| {
            let status = app.status(r);
            let glyph = if status == Status::Working { spinner(app.now) } else { status_glyph(status) };
            (format!(" {glyph} {} ", fit(&app.title(r), 24)), status)
        })
        .collect();
    let widths: Vec<usize> = labels.iter().map(|(l, _)| w(l) + 1).collect();
    let n = tabs.len();
    let active = shown.as_ref().and_then(|s| tabs.iter().position(|r| r == s));
    // Room for `+`, and for the counts when some tabs are left out.
    let budget = budget.saturating_sub(4);
    let fits = |a: usize, b: usize| {
        widths[a..b].iter().sum::<usize>() + if a > 0 { 5 } else { 0 } + if b < n { 5 } else { 0 } <= budget
    };
    let same = app.tab_scroll.0 == root;
    let mut start = if same { app.tab_scroll.1.min(n.saturating_sub(1)) } else { 0 };
    if let Some(a) = active
        && (!same || app.tab_revealed != shown)
    {
        start = start.min(a);
        while start < a && !fits(start, a + 1) {
            start += 1;
        }
    }
    // Never scrolled past the end: earlier tabs fill any room left.
    while start > 0 && fits(start - 1, n) {
        start -= 1;
    }
    let mut end = start;
    while end < n && fits(start, end + 1) {
        end += 1;
    }
    let end = if end == start { (start + 1).min(n) } else { end };
    app.tab_scroll = (root.to_string(), start);
    app.tab_revealed = shown.clone();
    // The whole strip takes the wheel; the tabs on it take clicks.
    let strip = budget.saturating_add(4).min(area.width as usize) as u16;
    app.hits.push((Rect { width: strip, ..area }, Hit::TabBar));
    // Each piece: its text, its style, and what a click on it does. A
    // tab's glyph keeps its state's color, except on the highlighted tab.
    let gap = Style::new().bg(t.panel);
    let mut pieces: Vec<(String, Style, Option<Hit>)> = Vec::new();
    if start > 0 {
        pieces.push((format!(" ‹{start} "), faint().bg(t.panel), Some(Hit::TabScroll(-1))));
    }
    for i in start..end {
        let (label, status) = &labels[i];
        let on = Some(i) == active;
        let style = if on {
            Style::new().bg(t.accent).fg(on_fill()).add_modifier(Modifier::BOLD)
        } else {
            Style::new().bg(t.select).fg(t.dim)
        };
        let split = label.char_indices().nth(2).map_or(label.len(), |(b, _)| b);
        let (glyph, name) = label.split_at(split);
        let hit = Some(Hit::AgentTab(tabs[i].clone()));
        pieces.push((glyph.to_string(), if on { style } else { style.fg(status_color(*status)) }, hit.clone()));
        pieces.push((name.to_string(), style, hit));
        pieces.push((" ".into(), gap, None));
    }
    if end < n {
        pieces.push((format!(" {}› ", n - end), faint().bg(t.panel), Some(Hit::TabScroll(1))));
    }
    pieces.push((" + ".into(), bold(t.accent).bg(t.panel), Some(Hit::NewMenu)));
    let mut x = area.x;
    let mut spans = Vec::new();
    for (text, style, hit) in pieces {
        let wd = w(&text) as u16;
        if let Some(hit) = hit {
            app.hits.push((Rect { x, y: area.y, width: wd, height: 1 }, hit));
        }
        x += wd;
        spans.push(sp(text, style));
    }
    spans
}

/// What a leaf's frame says: what it shows and how it's doing.
fn leaf_title(app: &App, view: &crate::layout::View, focused: bool) -> Vec<Span<'static>> {
    use crate::layout::View;
    let t = theme::get();
    let name_style = if focused { bold(t.text) } else { fg(t.dim) };
    let mut v = vec![sp(" ", text())];
    match view {
        View::Pane { id } => {
            let row = RowRef::Pane(id.clone());
            let status = app.status(&row);
            let glyph = if status == Status::Working { spinner(app.now) } else { status_glyph(status) };
            v.push(sp(format!("{glyph} "), fg(status_color(status))));
            v.push(sp(app.title(&row), name_style));
            if let Some(m) = app.pane_session(id).filter(|s| s.state.is_live()).and_then(|s| s.model.clone()) {
                v.push(sp(format!(" · {}", short_model(&m)), faint()));
            }
            if let Some(p) = app.pane(id).filter(|p| p.exit.is_some()) {
                v.push(sp(format!(" · exited {}", p.exit.as_ref().map(|e| e.code).unwrap_or(0)), fg(t.red)));
            }
        }
        View::Changes { filter } => {
            v.push(sp("changes", name_style));
            if let Some(n) = app.space_root().and_then(|r| app.change_count(&r)).filter(|n| *n > 0) {
                v.push(sp(format!(" {n}"), faint()));
            }
            if let Some(sid) = filter {
                v.push(sp(format!(" · {}", fit(&crate::view_changes::session_label(app, sid), 30)), faint()));
            }
        }
        View::Files => v.push(sp("files", name_style)),
        View::Conversation { session } => {
            let row = RowRef::Session(session.clone());
            let status = app.status(&row);
            v.push(sp(format!("{} ", status_glyph(status)), fg(status_color(status))));
            v.push(sp(app.title(&row), name_style));
            if let Some(a) = app.session(session).and_then(|s| s.terminal.app_name()) {
                v.push(sp(format!(" · in {a}"), faint()));
            }
        }
        View::Overview => v.push(sp("overview", name_style)),
    }
    v.push(sp(" ", text()));
    v
}

fn draw_leaf(
    f: &mut Frame,
    r: Rect,
    app: &mut App,
    root: &str,
    part: crate::layout::Part,
    view: &crate::layout::View,
    focused: bool,
) {
    use crate::layout::View;
    let t = theme::get();
    let border = if focused { t.accent } else { t.rule };
    let mut block = Block::bordered()
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(fg(border))
        .title(Line::from(crate::ui::fit_spans(leaf_title(app, view, focused), r.width.saturating_sub(4) as usize)));
    if focused && app.typing && view.is_terminal() {
        // Typing here: how to get the keys back.
        block = block.title_top(Line::from(sp(" ctrl-q gives the keys back ", faint())).right_aligned());
    } else if focused
        && !app.typing
        && matches!(view, View::Pane { id } if app.pane(id).is_some_and(|p| p.is_running()))
    {
        block = block.title_top(Line::from(sp(" ⏎ type ", faint())).right_aligned());
    }
    let panel = view.is_panel();
    // Every pane and panel can be closed from its frame.
    let close = match view {
        View::Pane { id } => Some(Hit::Close(RowRef::Pane(id.clone()))),
        _ if panel => Some(Hit::Action(crate::keymap::Action::ClosePanel)),
        _ => None,
    };
    if close.is_some() {
        block = block.title_top(Line::from(sp(" × ", fg(t.dim))).right_aligned());
    }
    let inner = block.inner(r);
    f.render_widget(block, r);
    app.hits.push((r, Hit::Leaf(part)));
    if let Some(hit) = close {
        app.hits.push((Rect { x: r.right().saturating_sub(4), y: r.y, width: 3, height: 1 }, hit));
    }
    app.leaf_focused = focused;
    match view {
        View::Pane { id } => pane_view(f, inner, app, id),
        View::Changes { filter } => crate::view_changes::draw(f, inner, app, root, filter.clone()),
        View::Files => crate::view_files::draw(f, inner, app, root),
        View::Conversation { session } => {
            let mut area = inner;
            if app.session(session).is_some() {
                resume_bar(f, Rect { height: 1, ..area }, app, session);
                area = Rect { y: area.y + 2, height: area.height.saturating_sub(2), ..area };
            }
            conversation(f, area, app, session);
        }
        View::Overview => project_overview(f, inner, app, root),
    }
    app.leaf_focused = false;
}

/// Above a session that isn't one of our panes: open it as a terminal.
fn resume_bar(f: &mut Frame, area: Rect, app: &mut App, id: &str) {
    let t = theme::get();
    let Some(s) = app.session(id).cloned() else { return };
    fill(f, area, t.panel);
    let place = s.terminal.app_name().unwrap_or_else(|| "another terminal".into());
    let in_editor = matches!(place.as_str(), "VS Code" | "Cursor" | "VSCodium" | "Windsurf");
    let (label, note) = if let Some(editor) = s.terminal.editor.clone().filter(|_| s.state.is_live()) {
        (" i  type here ".to_string(), format!("  running in {editor} · r fork a copy here"))
    } else if s.state.is_live() && in_editor {
        (
            " r  fork here ".to_string(),
            format!("  running in {place} · aulora vscode install to type into it from here"),
        )
    } else if s.state.is_live() {
        (" r  fork here ".to_string(), format!("  running in {place} · this is a read-only view"))
    } else {
        (" r  resume in a terminal ".to_string(), format!("  claude --resume, in {}", basename(&session_dir_name(&s))))
    };
    let style = Style::new().bg(t.accent).fg(on_fill()).add_modifier(Modifier::BOLD);
    let bw = w(&label) as u16;
    let line = Line::from(vec![sp(" ", Style::new().bg(t.panel)), sp(label, style), sp(note, faint().bg(t.panel))]);
    f.render_widget(Paragraph::new(line), area);
    let hit = if s.terminal.editor.is_some() && s.state.is_live() {
        Hit::Compose(id.to_string())
    } else {
        Hit::Resume(id.to_string())
    };
    app.hits.push((Rect { x: area.x + 1, width: bw, ..area }, hit));
}

fn session_dir_name(s: &Session) -> String {
    if s.cwd.is_empty() { s.project_root.clone() } else { s.cwd.clone() }
}

fn pane_view(f: &mut Frame, area: Rect, app: &mut App, id: &str) {
    let t = theme::get();
    let term_area = area;
    app.view_sizes.push((id.to_string(), term_area.width, term_area.height));
    app.hits.push((area, Hit::Terminal(id.to_string())));
    let Some(term) = app.terms.get(id) else {
        let msg = Line::from(vec![sp(format!(" {} ", spinner(app.now)), fg(t.accent)), sp("attaching…", dim())]);
        f.render_widget(Paragraph::new(msg), area);
        return;
    };
    let cursor = crate::term::render(term, term_area, f.buffer_mut());
    if app.typing
        && app.here()
        && let Some(pos) = cursor
    {
        f.set_cursor_position(pos);
    }
    let screen = term.parser.screen();
    let scrolled = screen.scrollback();
    let history = term.history();
    if !screen.alternate_screen() && history > 0 {
        let view = term_area.height as usize;
        let total = history + view;
        let top = history - scrolled.min(history);
        // On the frame's right edge.
        scrollbar(f, Rect { x: area.right(), width: 1, ..area }, top, view, total, app.here());
    }
    let pill = Style::new().fg(t.text).bg(t.panel);
    if scrolled > 0 {
        let msg = format!(" ↑ {scrolled} lines · G live ");
        let wd = (w(&msg) as u16).min(term_area.width);
        let r = Rect { x: term_area.x + term_area.width - wd, y: term_area.y, width: wd, height: 1 };
        f.render_widget(Paragraph::new(Line::from(sp(msg, pill))), r);
    }
    if let Some(e) = app.pane(id).and_then(|p| p.exit.clone()) {
        let msg = format!(" exited with code {} · r restart · x close ", e.code);
        let wd = (w(&msg) as u16).min(term_area.width);
        let r = Rect { x: term_area.x, y: term_area.y + term_area.height.saturating_sub(1), width: wd, height: 1 };
        f.render_widget(Paragraph::new(Line::from(sp(msg, pill))), r);
    }
}

/// One line above the conversation: what's happening and the numbers.
fn status_strip(app: &App, s: &Session, width: usize) -> Line<'static> {
    let t = theme::get();
    let now = app.now;
    let left: Vec<Span<'static>> = match &s.state {
        SessionState::NeedsYou { reason, detail } => {
            vec![sp(" ● ", bold(t.red)), sp(format!("{}: ", reason.label()), bold(t.red)), sp(detail.clone(), text())]
        }
        SessionState::Working => match &s.current_tool {
            Some(c) => vec![
                sp(format!(" {} ", spinner(now)), bold(t.yellow)),
                sp(tool_label(&c.name), fg(t.cyan)),
                sp(format!(" {}", c.target), dim()),
                sp(format!("  {}", short_duration(now - c.started_at)), faint()),
            ],
            None => vec![sp(format!(" {} thinking", spinner(now)), fg(t.yellow))],
        },
        SessionState::YourTurn => vec![sp(" ● ", fg(t.cyan)), sp("finished, your turn", fg(t.cyan))],
        SessionState::Errored { detail } => vec![sp(" ✗ ", bold(t.red)), sp(detail.clone(), fg(t.red))],
        SessionState::Compacting => vec![sp(" ◌ compacting context", fg(t.magenta))],
        SessionState::Idle if s.turns == 0 && s.transcript_path.is_none() => {
            vec![sp(" ○ waiting for a first prompt", faint())]
        }
        SessionState::Idle => vec![sp(" ○ idle", faint())],
        SessionState::Ended { .. } if s.origin == Origin::History => {
            vec![sp(" · past session · resume with ", faint()), sp(format!("claude --resume {}", s.id), text())]
        }
        SessionState::Ended { reason } => vec![sp(format!(" · ended ({reason})"), faint())],
    };
    let added: u32 = s.files_touched.iter().map(|f| f.added).sum();
    let removed: u32 = s.files_touched.iter().map(|f| f.removed).sum();
    let right = vec![
        sp(format!("{} · {} · ", plural(s.turns as usize, "turn"), plural(s.tool_calls as usize, "tool")), faint()),
        sp(format!("{} ", plural(s.files_touched.len(), "file")), faint()),
        sp(format!("+{added}"), fg(t.green)),
        sp(format!(" −{removed} "), fg(t.red)),
    ];
    lr(left, right, width)
}

/// Lines of a prompt wrapped to `width`, and where the cursor lands.
pub(crate) fn wrap_prompt(text: &str, cursor: usize, width: usize) -> (Vec<String>, (usize, usize)) {
    let width = width.max(1);
    let mut lines = vec![String::new()];
    let mut col = 0;
    let mut at = (0, 0);
    for (i, ch) in text.char_indices() {
        if i == cursor {
            at = (lines.len() - 1, col);
        }
        if ch == '\n' {
            lines.push(String::new());
            col = 0;
            continue;
        }
        let cw = ch.width().unwrap_or(0);
        if col + cw > width {
            lines.push(String::new());
            col = 0;
        }
        lines.last_mut().expect("a line").push(ch);
        col += cw;
    }
    if cursor >= text.len() {
        at = (lines.len() - 1, col);
        if col >= width {
            lines.push(String::new());
            at = (lines.len() - 1, 0);
        }
    }
    (lines, at)
}

/// The prompt box under a conversation, for sessions aulora can type into
/// (Claude Code's input box).
fn prompt_box(f: &mut Frame, area: Rect, app: &mut App, s: &Session) -> u16 {
    let t = theme::get();
    let editor = s.terminal.editor.clone().unwrap_or_default();
    let inner_w = area.width.saturating_sub(6) as usize;
    let composing = app.compose.as_ref().filter(|c| c.session == s.id);
    let blocked = matches!(s.state, SessionState::NeedsYou { .. });
    let (lines, cursor) = match composing {
        Some(c) => wrap_prompt(&c.text, c.cursor, inner_w),
        None => (vec![], (0, 0)),
    };
    let shown = lines.len().clamp(1, 8);
    let top_line = cursor.0.saturating_sub(shown - 1);
    let height = shown as u16 + 2;
    let boxed = Rect { y: area.bottom().saturating_sub(height), height, ..area };
    let border = if composing.is_some() { t.accent } else { t.rule };
    let block =
        ratatui::widgets::Block::bordered().border_type(ratatui::widgets::BorderType::Rounded).border_style(fg(border));
    let inner = block.inner(boxed);
    f.render_widget(block, boxed);
    app.hits.push((boxed, Hit::Compose(s.id.clone())));
    let body: Vec<Line> = match composing {
        Some(c) => {
            // Everything selected (Cmd-A, Ctrl-A twice) shows highlighted.
            let style = if c.all { text().add_modifier(ratatui::style::Modifier::REVERSED) } else { text() };
            lines
                .iter()
                .enumerate()
                .skip(top_line)
                .take(shown)
                .map(|(i, l)| {
                    Line::from(vec![sp(if i == 0 { " > " } else { "   " }, fg(t.accent)), sp(l.clone(), style)])
                })
                .collect()
        }
        None if blocked => {
            vec![Line::from(sp(format!(" Claude is waiting for an answer in {editor}; answer it there"), faint()))]
        }
        None => {
            let draft =
                app.drafts.get(&s.id).map(|d| crate::ui::fit(&d.replace('\n', " ⏎ "), inner_w.saturating_sub(12)));
            match draft {
                Some(d) => vec![Line::from(vec![sp(" > ", faint()), sp(d, dim()), sp("  (draft)", faint())])],
                None => vec![Line::from(vec![
                    sp(" > ", faint()),
                    sp(format!("i  write a prompt · it's typed into {editor}"), faint()),
                ])],
            }
        }
    };
    f.render_widget(Paragraph::new(body), inner);
    if composing.is_some() {
        f.set_cursor_position((inner.x + 3 + cursor.1 as u16, inner.y + (cursor.0 - top_line) as u16));
        slash_menu(f, boxed, app);
    }
    height
}

/// Tips under the working line: aulora's own keys, so waiting teaches
/// something (Claude Code shows tips the same way).
const TIPS: &[&str] = &[
    "n jumps to the next agent that needs you",
    "right-click any row for what you can do with it",
    "d opens Changes: what this agent wrote, ready to commit",
    "[ and ] jump between the prompts you sent",
    "f shows or hides the Files panel",
    "t picks one of twelve themes",
    "r on an ended session resumes it in a terminal here",
    "aulora send <session> \"…\" prompts a session from a script",
    "F12 hands the mouse back so your terminal can select text",
    "/ searches this conversation",
    "C starts Claude in the selected workspace's folder",
    "y copies the message at the cursor",
];

/// Claude Code's whimsical verbs, for when no tool says what's going on.
const VERBS: &[&str] = &[
    "Thinking",
    "Pondering",
    "Brewing",
    "Cooking",
    "Noodling",
    "Percolating",
    "Mulling",
    "Tinkering",
    "Crafting",
    "Conjuring",
    "Wrangling",
    "Simmering",
];

/// The line Claude Code shows while it works: a spinner, a verb, the time
/// so far and what it's doing, with a tip underneath.
fn working_line(app: &App, s: &Session, width: usize) -> Vec<Line<'static>> {
    let t = theme::get();
    let now = app.now;
    let frames = ["·", "✢", "✳", "✶", "✻", "✽", "✻", "✶", "✳", "✢"];
    let glyph = frames[((now / 120) as usize) % frames.len()];
    let started = s.turn_started_at.unwrap_or(s.state_since);
    let verb = match (&s.state, s.current_tool.as_ref().map(|c| c.name.as_str())) {
        (SessionState::Compacting, _) => "Compacting".to_string(),
        (_, Some("Bash")) => "Running".into(),
        (_, Some("Edit" | "MultiEdit" | "Write" | "NotebookEdit")) => "Editing".into(),
        (_, Some("Read")) => "Reading".into(),
        (_, Some("Grep" | "Glob")) => "Searching".into(),
        (_, Some("WebFetch" | "WebSearch")) => "Researching".into(),
        (_, Some("Task" | "Agent")) => "Delegating".into(),
        (_, Some("TodoWrite")) => "Planning".into(),
        _ => VERBS[(started / 1000).unsigned_abs() as usize % VERBS.len()].to_string(),
    };
    let mut detail = vec![crate::conv::long_duration((now - started).max(0) / 1000)];
    if let Some(c) = &s.current_tool
        && !c.target.is_empty()
    {
        detail.push(c.target.clone());
    }
    if let Some(ctx) = s.context_tokens {
        detail.push(format!("ctx {}", fmt_tokens(ctx)));
    }
    let head = vec![
        sp(format!(" {glyph} "), fg(t.orange)),
        sp(format!("{verb}… "), bold(t.orange)),
        sp(format!("({})", detail.join(" · ")), faint()),
    ];
    let seed = s.id.bytes().fold(0usize, |h, b| h.wrapping_mul(31).wrapping_add(b as usize));
    let tip = TIPS[(seed + (now / 12_000) as usize) % TIPS.len()];
    vec![
        Line::from(fit_spans(head, width)),
        Line::from(vec![sp("   ⎿  ", faint()), sp(fit(&format!("Tip: {tip}"), width.saturating_sub(6)), faint())]),
    ]
}

/// Claude's question picker, as Claude Code draws it in its terminal: a
/// rule, the question, its numbered choices. Keys and clicks here are
/// pressed in that terminal (see picker.rs).
fn answer_card(f: &mut Frame, area: Rect, app: &mut App, s: &Session, ask: &Ask) -> u16 {
    let t = theme::get();
    let own = matches!(s.state, SessionState::NeedsYou { reason: aulora_core::model::Attention::Question, .. });
    let fresh = app.picker.as_ref().is_none_or(|p| p.session != s.id || p.sig != crate::picker::sig(ask));
    if fresh {
        app.picker = Some(crate::picker::Picker::new(&s.id, ask));
    }
    let Some(p) = app.picker.clone() else { return 0 };
    let w = (area.width as usize).saturating_sub(2);
    let (mut lines, rows) = crate::picker::lines(&p, ask, own, w);
    if s.terminal.editor.is_none() {
        lines.push(Line::from(sp("answer it in the terminal it runs in", faint())));
    }
    let height = (lines.len() as u16 + 1).min(area.height.saturating_sub(3).max(4));
    let r = Rect { y: area.bottom().saturating_sub(height), height, ..area };
    f.render_widget(Clear, r);
    f.render_widget(
        Paragraph::new(Line::from(sp("─".repeat(r.width as usize), fg(t.accent)))),
        Rect { height: 1, ..r },
    );
    let body = Rect { x: r.x + 1, y: r.y + 1, width: r.width.saturating_sub(1), height: height - 1 };
    f.render_widget(Paragraph::new(lines), body);
    for (line, row) in rows {
        if (line as u16) < body.height {
            app.hits.push((Rect { y: body.y + line as u16, height: 1, ..body }, Hit::AnswerRow(row)));
        }
    }
    height
}

/// Claude Code's `/` menu, above the prompt box while a command is typed.
fn slash_menu(f: &mut Frame, boxed: Rect, app: &mut App) {
    let t = theme::get();
    let menu = app.slash_menu();
    if menu.is_empty() {
        return;
    }
    let sel = app.compose.as_ref().map_or(0, |c| c.menu.min(menu.len() - 1));
    let rows = menu.len().min(8);
    let start = sel.saturating_sub(rows - 1);
    let h = rows as u16;
    let r = Rect { y: boxed.y.saturating_sub(h), height: h, ..boxed };
    f.render_widget(Clear, r);
    let width = r.width as usize;
    let name_w = menu.iter().map(|(n, _)| w(n) + 1).max().unwrap_or(8).min(width / 2);
    let lines: Vec<Line> = menu
        .iter()
        .enumerate()
        .skip(start)
        .take(rows)
        .map(|(i, (name, desc))| {
            let style = if i == sel { Style::new().bg(t.select) } else { Style::new().bg(t.panel) };
            let left = format!("  /{name:<name_w$}");
            Line::from(vec![
                sp(fit(&left, width / 2 + 3), if i == sel { bold(t.accent) } else { fg(t.text) }),
                sp(format!("  {}", fit(desc, width.saturating_sub(name_w + 7))), faint()),
            ])
            .style(style)
        })
        .collect();
    f.render_widget(Paragraph::new(lines).style(Style::new().bg(t.panel)), r);
}

fn conversation(f: &mut Frame, area: Rect, app: &mut App, session_id: &str) {
    let t = theme::get();
    let Some(s) = app.session(session_id).cloned() else { return };
    let mut area = area;
    if let (SessionState::NeedsYou { .. }, Some(ask)) = (&s.state, s.ask.clone()) {
        let h = answer_card(f, area, app, &s, &ask);
        area.height = area.height.saturating_sub(h);
    } else if app.can_compose(session_id) {
        let h = prompt_box(f, area, app, &s);
        area.height = area.height.saturating_sub(h);
    }
    // Room for the working line, drawn right under the last message, and
    // the messages queued behind it (Claude Code shows them the same way).
    let working = matches!(s.state, SessionState::Working | SessionState::Compacting) && area.height > 8;
    let queued: Vec<String> = app.conversations.get(session_id).map(|(c, _)| c.queued.clone()).unwrap_or_default();
    let queued_rows = queued.len().min(4) as u16;
    let full = area;
    if working {
        area.height -= 3 + queued_rows;
    } else if queued_rows > 0 {
        area.height = area.height.saturating_sub(queued_rows + 1);
    }
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Min(0)]).split(area);
    f.render_widget(Paragraph::new(status_strip(app, &s, area.width as usize)), rows[0]);
    f.render_widget(Paragraph::new(Line::from(sp("─".repeat(area.width as usize), fg(t.rule)))), rows[1]);
    let view = rows[2];
    let text_area = Rect { width: view.width.saturating_sub(1), ..view };
    let height = view.height as usize;
    app.conv_height = height;
    app.hits.push((view, Hit::Conversation(session_id.to_string())));
    let focused = app.focus == Focus::Main;
    let Some(conv) = app.conversation(session_id) else { return };
    conv.layout(text_area.width);
    let top = conv.settle(height);
    let total = conv.total_rows();
    let cursor = conv.cursor_row();
    let (follow, unseen) = (conv.follow, conv.unseen);
    if total == 0 {
        let msg = "  No messages yet. They appear here as Claude works.";
        f.render_widget(Paragraph::new(Line::from(sp(msg, faint()))), text_area);
        return;
    }
    let buf = f.buffer_mut();
    let mut cut_rows = Vec::new();
    for (i, (_, line)) in conv.visible(top, height).into_iter().enumerate() {
        let y = text_area.y + i as u16;
        if focused && top + i == cursor {
            let bg = Style::new().bg(t.active);
            buf.set_style(Rect { x: text_area.x, y, width: text_area.width, height: 1 }, bg);
        }
        buf.set_line(text_area.x, y, line, text_area.width);
        if let Some(w) = crate::conv::cut_at(line) {
            cut_rows.push((y, text_area.x..text_area.right(), text_area.x + w));
        }
    }
    app.cut_rows.extend(cut_rows);
    scrollbar(f, Rect { x: view.x + view.width - 1, width: 1, ..view }, top, height, total, focused);
    let shown = total.saturating_sub(top).min(height) as u16;
    let mut y = (text_area.y + shown + 1).min(full.bottom().saturating_sub(2 + queued_rows));
    if working {
        let lines = working_line(app, &s, text_area.width as usize);
        f.render_widget(Paragraph::new(lines), Rect { y, height: 2, ..text_area });
        y += 2;
    }
    if queued_rows > 0 {
        let w = text_area.width as usize;
        let mut lines: Vec<Line> = queued
            .iter()
            .take(queued_rows as usize)
            .map(|q| {
                Line::from(vec![
                    sp(" › ", faint()),
                    sp(fit(&clean(q), w.saturating_sub(12)), dim()),
                    sp("  queued", faint()),
                ])
            })
            .collect();
        if queued.len() > queued_rows as usize
            && let Some(last) = lines.last_mut()
        {
            *last = Line::from(sp(format!(" … {} more queued", queued.len() - queued_rows as usize + 1), faint()));
        }
        f.render_widget(Paragraph::new(lines), Rect { y, height: queued_rows, ..text_area });
    }
    if unseen > 0 && !follow {
        let msg = format!(" ↓ {} · G latest ", plural(unseen, "new message"));
        let wd = (w(&msg) as u16).min(text_area.width);
        let r = Rect {
            x: text_area.x + text_area.width - wd,
            y: view.y + view.height.saturating_sub(1),
            width: wd,
            height: 1,
        };
        f.render_widget(Paragraph::new(Line::from(sp(msg, Style::new().fg(on_fill()).bg(t.accent)))), r);
    }
}

fn section(title: &str, right: Option<String>, width: usize) -> Line<'static> {
    lr(
        vec![sp(format!(" {title}"), label())],
        right.map(|r| vec![sp(format!("{r} "), faint())]).unwrap_or_default(),
        width,
    )
}

fn project_overview(f: &mut Frame, area: Rect, app: &mut App, root: &str) {
    let t = theme::get();
    let now = app.now;
    let width = area.width as usize;
    let Some(p) = app.project(root).cloned() else { return };
    let mut lines: Vec<Line> = vec![Line::raw("")];
    lines.push(section("git", None, width));
    match &p.git {
        Some(g) => {
            let mut parts = Vec::new();
            for (n, word) in [
                (g.staged, "staged"),
                (g.modified, "modified"),
                (g.untracked, "untracked"),
                (g.conflicts, "conflicted"),
            ] {
                if n > 0 {
                    parts.push(format!("{n} {word}"));
                }
            }
            let state = if parts.is_empty() { "clean".to_string() } else { parts.join(" · ") };
            let mut l = vec![sp(format!("   {}", g.branch.clone().unwrap_or_default()), fg(t.magenta))];
            l.push(sp(format!("   {state}"), if parts.is_empty() { fg(t.green) } else { fg(t.yellow) }));
            if g.ahead + g.behind > 0 {
                l.push(sp(format!("   ↑{} ↓{}", g.ahead, g.behind), faint()));
            }
            lines.push(Line::from(l));
            if let Some(m) = &g.last_commit {
                lines.push(Line::from(vec![
                    sp("   last commit  ", faint()),
                    sp(m.clone(), dim()),
                    sp(g.last_commit_at.map(|a| format!("  {}", ago(now, a))).unwrap_or_default(), faint()),
                ]));
            }
        }
        None => lines.push(Line::from(sp("   not a git repository", faint()))),
    }
    lines.push(Line::raw(""));
    let agents = app.agents_in(root);
    lines.push(section("agents", Some(format!("{}", agents.len())), width));
    for a in &agents {
        let st = app.status(a);
        let mut l = vec![sp(format!("   {} ", status_glyph(st)), fg(status_color(st))), sp(app.title(a), text())];
        l.push(sp(format!("  {}", st.word()), faint()));
        lines.push(lr(l, vec![], width));
    }
    lines.push(Line::raw(""));
    let mut files: Vec<FileTouch> = Vec::new();
    for s in app.snap.iter().flat_map(|snap| snap.sessions.iter()).filter(|s| s.project_root == root) {
        for x in &s.files_touched {
            match files.iter_mut().find(|y| y.path == x.path) {
                Some(y) => {
                    y.added += x.added;
                    y.removed += x.removed;
                    y.last_at = y.last_at.max(x.last_at);
                }
                None => files.push(x.clone()),
            }
        }
    }
    files.sort_by_key(|x| -x.last_at);
    lines.push(section("files claude edited", Some(plural(files.len(), "file")), width));
    for x in files.iter().take(area.height.saturating_sub(lines.len() as u16) as usize) {
        let dir = std::path::Path::new(&x.path).parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
        let mut left = vec![sp("   ", text())];
        if !dir.is_empty() {
            left.push(sp(format!("{dir}/"), faint()));
        }
        left.push(sp(basename(&x.path), text()));
        lines.push(lr(
            left,
            vec![sp(format!("+{}", x.added), fg(t.green)), sp(format!(" −{} ", x.removed), fg(t.red))],
            width,
        ));
    }
    f.render_widget(Paragraph::new(lines), area);
}

// ---------------------------------------------------------------------------
// Mode bar, overlays

/// Bottom row: a mode pill and key hints for the focused region, built
/// from the keymap and shortened to fit (zellij, Herdr's mode bar).
fn mode_bar(f: &mut Frame, area: Rect, app: &App) {
    let t = theme::get();
    let width = area.width as usize;
    // A theme with its own status-bar colors (tmux) uses them here.
    let (bar_bg, bar_fg) = if t.name == "tmux" { (t.bar_bg, Some(t.bar_fg)) } else { (t.panel, None) };
    let pill =
        |word: &str, c: Color| sp(format!(" {word} "), Style::new().bg(c).fg(on_fill()).add_modifier(Modifier::BOLD));
    let key = |k: &str| sp(format!(" {k}"), bold(bar_fg.unwrap_or(t.accent)));
    let lbl = |l: &str| sp(format!(" {l} "), bar_fg.map(fg).unwrap_or_else(faint));
    let mut left: Vec<Span<'static>> = Vec::new();
    if app.typing {
        let name = app.selected_pane().map(|p| p.name.clone()).unwrap_or_default();
        left.extend([
            pill("TERMINAL", t.accent),
            lbl(&format!("keys go to {name}")),
            key("ctrl-q"),
            lbl("back to aulora"),
        ]);
    } else if app.commit_edit.is_some() {
        left.extend([
            pill("COMMIT", t.accent),
            key("⏎"),
            lbl("commit"),
            key("⇧⏎"),
            lbl("new line"),
            key("esc"),
            lbl("keep as draft"),
        ]);
    } else if let Some(c) = &app.compose {
        let editor = app.session(&c.session).and_then(|s| s.terminal.editor.clone()).unwrap_or_default();
        left.extend([
            pill("PROMPT", t.accent),
            lbl(&format!("typed into {editor}")),
            key("⏎"),
            lbl("send"),
            key("⇧⏎"),
            lbl("new line"),
            key("esc"),
            lbl("keep as draft"),
        ]);
    } else if app.filtering {
        left.extend([
            pill("FILTER", t.magenta),
            key("type"),
            lbl("to filter"),
            key("⏎"),
            lbl("keep"),
            key("esc"),
            lbl("clear"),
        ]);
    } else if app.searching {
        left.extend([
            pill("SEARCH", t.magenta),
            sp(format!(" {}", app.query), text()),
            sp("▏", fg(t.accent)),
            key("⏎"),
            lbl("find"),
            key("esc"),
            lbl("cancel"),
        ]);
    } else {
        let scope = app.scope();
        let hints = keymap::hints(scope);
        // Full labels, then fewer hints, keeping "? help" last.
        let mut shown: Vec<(&str, &str)> = hints.clone();
        let help = shown.iter().position(|h| h.1 == "help").map(|i| shown.remove(i));
        let size = |v: &[(&str, &str)]| v.iter().map(|(k, l)| w(k) + w(l) + 3).sum::<usize>() + 12;
        while size(&shown) > width.saturating_sub(24) && shown.len() > 2 {
            shown.pop();
        }
        if let Some(h) = help {
            shown.push(h);
        }
        for (k, l) in shown {
            left.push(key(k));
            left.push(lbl(l));
        }
    }
    let mut right = Vec::new();
    let alerts = app.alerts();
    if let Some(a) = alerts.first() {
        right.push(sp(format!("● {} needs you ", basename(&a.project_root)), bold(t.red)));
        if alerts.len() > 1 {
            right.push(sp(format!("+{} ", alerts.len() - 1), fg(t.red)));
        }
        right.push(sp("n ", bold(t.accent)));
    }
    if !app.mouse {
        right.push(sp("mouse off · F12 ", fg(t.yellow)));
    }
    f.render_widget(Paragraph::new(lr(left, right, width)).style(Style::new().bg(bar_bg)), area);
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let w_ = width.min(area.width);
    let h_ = height.min(area.height);
    Rect { x: area.x + (area.width - w_) / 2, y: area.y + (area.height - h_) / 2, width: w_, height: h_ }
}

/// Herdr's modal: accent border on the panel color.
fn modal(f: &mut Frame, r: Rect, title: &str, border: Color) -> Rect {
    let t = theme::get();
    f.render_widget(Clear, r);
    let block = Block::bordered()
        .border_style(fg(border))
        .style(Style::new().bg(t.panel).fg(t.text))
        .title(Line::from(sp(format!(" {title} "), bold(border))));
    let inner = block.inner(r);
    f.render_widget(block, r);
    inner
}

fn button(label: &str, primary: bool, danger: bool) -> Span<'static> {
    let t = theme::get();
    let bg = match (primary, danger) {
        (true, true) => t.red,
        (true, false) => t.accent,
        _ => t.select,
    };
    let fg_ = if primary { on_fill() } else { t.text };
    sp(
        format!(" {label} "),
        Style::new().bg(bg).fg(fg_).add_modifier(if primary { Modifier::BOLD } else { Modifier::empty() }),
    )
}

/// Help built from the keymap: filter by typing, ⏎ runs the binding (lazygit).
pub fn help_rows(query: &str) -> Vec<(Scope, &'static str, &'static str, keymap::Action)> {
    let q = query.to_lowercase();
    let order = [
        Scope::Sidebar,
        Scope::Pane,
        Scope::Changes,
        Scope::Files,
        Scope::Conversation,
        Scope::Overview,
        Scope::Global,
    ];
    order
        .iter()
        .flat_map(|s| BINDINGS.iter().filter(move |b| b.scope == *s))
        .filter(|b| q.is_empty() || b.desc.to_lowercase().contains(&q) || b.label.to_lowercase().contains(&q))
        .map(|b| (b.scope, b.label, b.desc, b.action))
        .collect()
}

fn help(f: &mut Frame, area: Rect, app: &App) {
    let t = theme::get();
    let Some(h) = &app.help else { return };
    let inner = modal(f, centered(area, 76, 26), "keys", t.accent);
    let rows = help_rows(&h.query);
    let mut lines = vec![Line::from(vec![
        sp(" / ", bold(t.accent)),
        sp(h.query.clone(), text()),
        sp("▏", fg(t.accent)),
        sp(if h.query.is_empty() { "  type to filter" } else { "" }, faint()),
    ])];
    let room = inner.height.saturating_sub(3) as usize;
    let start = h.selected.saturating_sub(room.saturating_sub(4));
    let mut last_scope = None;
    for (i, (scope, keys, desc, _)) in rows.iter().enumerate().skip(start) {
        if lines.len() >= room {
            break;
        }
        if last_scope != Some(*scope) {
            lines.push(Line::from(sp(format!(" {}", scope.title()), bold(t.accent))));
            last_scope = Some(*scope);
        }
        let style = if i == h.selected { Style::new().bg(t.select) } else { Style::new() };
        lines.push(
            Line::from(vec![sp(format!("   {keys:<12}"), bold(t.magenta)), sp(desc.to_string(), text())]).style(style),
        );
    }
    if rows.is_empty() {
        lines.push(Line::from(sp("   no matching keys", faint())));
    }
    f.render_widget(Paragraph::new(lines), inner);
    let foot = Rect { y: inner.y + inner.height.saturating_sub(1), height: 1, ..inner };
    let l = Line::from(vec![
        sp(" ↑↓ select  ", faint()),
        button("↵ run", true, false),
        sp(" ", text()),
        button("esc close", false, false),
    ]);
    f.render_widget(Paragraph::new(l), foot);
}

fn theme_picker(f: &mut Frame, area: Rect, app: &App) {
    let t = theme::get();
    let Some(sel) = app.theme_picker else { return };
    let h = theme::THEMES.len() as u16 + 4;
    let inner = modal(f, centered(area, 50, h), "theme", t.accent);
    let mut lines = Vec::new();
    for (i, th) in theme::THEMES.iter().enumerate() {
        let selected = i == sel;
        let mut l = vec![
            sp(if selected { " ▸ " } else { "   " }, bold(t.accent)),
            sp(format!("{:<18}", th.name), if selected { bold(t.text) } else { dim() }),
        ];
        for c in [th.accent, th.red, th.yellow, th.cyan, th.green, th.magenta] {
            l.push(sp("● ", fg(c)));
        }
        if th.light {
            l.push(sp("light", faint()));
        }
        lines.push(Line::from(l).style(if selected { Style::new().bg(t.select) } else { Style::new() }));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(vec![
        sp("   ", text()),
        button("↵ keep", true, false),
        sp(" ", text()),
        button("esc cancel", false, false),
    ]));
    f.render_widget(Paragraph::new(lines), inner);
}

fn confirm(f: &mut Frame, area: Rect, app: &App) {
    let t = theme::get();
    let Some(c) = &app.confirm else { return };
    let body = crate::conv::wrap(&c.body, 54);
    let color = if matches!(c.action, crate::ConfirmAction::ClosePane(_) | crate::ConfirmAction::EndSession(_)) {
        t.red
    } else {
        t.accent
    };
    let inner = modal(f, centered(area, 60, 6 + body.len() as u16), &c.title, color);
    let mut lines = vec![Line::raw("")];
    lines.extend(body.into_iter().map(|l| Line::from(sp(format!(" {l}"), text()))));
    lines.extend([
        Line::raw(""),
        Line::from(vec![
            sp(" ", text()),
            button("esc cancel", !c.on_action, false),
            sp("  ", text()),
            // ↵ picks whichever is highlighted; y always means yes.
            button(&format!("{} {}", if c.on_action { "↵" } else { "y" }, c.action.verb()), c.on_action, true),
            sp("    ←→ choose", faint()),
        ]),
    ]);
    f.render_widget(Paragraph::new(lines), inner);
}

/// The right-click menu, opened at the mouse (or under the row for `m`).
fn menu(f: &mut Frame, area: Rect, app: &mut App) {
    let t = theme::get();
    let Some(m) = &app.menu else { return };
    let inner_w = m.items.iter().map(|i| w(i.label())).max().unwrap_or(0) as u16 + 4;
    let (width, height) = (inner_w + 2, m.items.len() as u16 + 2);
    let x = m.x.min(area.right().saturating_sub(width));
    let y = if m.y + height > area.bottom() { m.y.saturating_sub(height + 1) } else { m.y };
    let r = Rect { x, y, width: width.min(area.width), height: height.min(area.height) };
    f.render_widget(Clear, r);
    let block = Block::bordered().border_style(fg(t.rule)).style(Style::new().bg(t.panel));
    let inner = block.inner(r);
    f.render_widget(block, r);
    let mut lines = Vec::new();
    let mut hits = Vec::new();
    for (i, item) in m.items.iter().enumerate() {
        let selected = i == m.selected;
        let color = if item.danger() { t.red } else { t.text };
        let style = if selected { Style::new().bg(t.select).fg(color).add_modifier(Modifier::BOLD) } else { fg(color) };
        let label = format!("  {:<w$}", item.label(), w = inner_w as usize - 2);
        lines.push(Line::from(sp(label, style)));
        hits.push((Rect { x: inner.x, y: inner.y + i as u16, width: inner.width, height: 1 }, Hit::MenuItem(i)));
    }
    f.render_widget(Paragraph::new(lines), inner);
    // A click anywhere else closes the menu.
    app.hits.push((area, Hit::Dismiss));
    app.hits.extend(hits);
}

fn toast(f: &mut Frame, area: Rect, app: &App) {
    let t = theme::get();
    let Some(toast) = &app.toast else { return };
    if app.now.saturating_sub(toast.at) > toast.ttl {
        return;
    }
    let color = if toast.error { t.red } else { t.green };
    let msg = format!(" ● {} ", toast.msg);
    let width = (w(&msg) as u16 + 2).min(area.width);
    let r = Rect {
        x: area.x + area.width.saturating_sub(width + 1),
        y: area.y + area.height.saturating_sub(3),
        width,
        height: 3,
    };
    f.render_widget(Clear, r);
    let block = Block::bordered().border_style(fg(color)).style(Style::new().bg(t.panel));
    let inner = block.inner(r);
    f.render_widget(block, r);
    f.render_widget(
        Paragraph::new(Line::from(vec![sp(" ●", fg(color)), sp(format!(" {} ", toast.msg), text())])),
        inner,
    );
}

fn empty_state(f: &mut Frame, area: Rect, app: &mut App) {
    let t = theme::get();
    app.hits.push((area, Hit::Main));
    let r = centered(area, 72, 16);
    let mut lines: Vec<Line> = crate::brand::banner(t, r.width, t.bg).into_iter().map(Line::centered).collect();
    lines.push(Line::raw(""));
    let msg = match &app.conn {
        Conn::Offline(why) => vec![sp(format!("can't reach the daemon: {why} · retrying"), fg(t.red))],
        Conn::Connecting => vec![sp(format!("{} starting the daemon…", spinner(app.now)), dim())],
        _ if !app.hooks_installed => {
            vec![sp("run ", faint()), sp("aulora hooks install", bold(t.text)), sp(" to connect Claude Code", faint())]
        }
        _ => vec![sp("nothing running yet", faint())],
    };
    lines.push(Line::from(msg).centered());
    lines.push(Line::raw(""));
    lines.push(
        Line::from(vec![
            sp("C", bold(t.accent)),
            sp(" start Claude Code here    ", faint()),
            sp("c", bold(t.accent)),
            sp(" open a shell    ", faint()),
            sp("h", bold(t.accent)),
            sp(format!(" past sessions ({})", app.history_count()), faint()),
        ])
        .centered(),
    );
    f.render_widget(Paragraph::new(lines), r);
    let _ = editor_name(app);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_at_words_and_ellipsizes_only_at_the_end() {
        let text = |lines: &[Vec<Span<'static>>]| -> Vec<String> {
            lines.iter().map(|l| l.iter().map(|s| s.content.to_string()).collect()).collect()
        };
        let spans = [sp("Creative project and portfolio inspiration", Style::new())];
        assert_eq!(text(&wrap_spans(&spans, &[20, 24])), ["Creative project and", "portfolio inspiration"]);
        assert_eq!(text(&wrap_spans(&spans, &[20, 12])), ["Creative project and", "portfolio i…"]);
        let styled = [sp("› ", fg(Color::Red)), sp("fix the bug", Style::new())];
        let w = wrap_spans(&styled, &[30, 30]);
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].len(), 2, "styles survive wrapping");
    }

    #[test]
    fn the_sidebar_scrolls_freely_and_follows_a_new_selection() {
        // 10 lines shown of 40.
        assert_eq!(sidebar_scroll(25, None, 10, 40), 25, "the wheel's scroll stays");
        assert_eq!(sidebar_scroll(35, None, 10, 40), 30, "but not past the end");
        assert_eq!(sidebar_scroll(25, Some((2, 4)), 10, 40), 1, "a selection above scrolls up to it");
        assert_eq!(sidebar_scroll(0, Some((14, 16)), 10, 40), 6, "and one below scrolls down to it");
        assert_eq!(sidebar_scroll(5, Some((8, 10)), 10, 40), 5, "one in view leaves it alone");
    }

    #[test]
    fn helpers() {
        assert_eq!(fit("abcdef", 4), "abc…");
        let l = lr(vec![sp("left side text", text())], vec![sp("R", text())], 10);
        assert_eq!(spans_width(&l.spans), 10);
    }
}
