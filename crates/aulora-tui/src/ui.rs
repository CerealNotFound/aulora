//! Drawing. Layout: a slim header, a sidebar with projects and what runs in
//! them, and a main area that shows one view of the selection at a time
//! (Terminal, Conversation, Overview or Activity).

use aulora_core::model::*;
use aulora_core::paths::tildify;
use aulora_core::reduce::tool_label;
use aulora_core::util::{ago, basename, first_line, fmt_tokens, human_duration, short_duration, short_model};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::{App, Conn, Mode, RowRef, Tab};
use crate::theme::{self, bold, dim, faint, fg, label, project_color, spinner, state_color, state_glyph, text};

// ---------------------------------------------------------------------------
// Text helpers

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

fn spans_width(spans: &[Span]) -> usize {
    spans.iter().map(|s| w(&s.content)).sum()
}

fn fit_spans(spans: Vec<Span<'static>>, max: usize) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    let mut used = 0;
    for s in spans {
        let sw = w(&s.content);
        if used + sw <= max {
            used += sw;
            out.push(s);
        } else {
            let rest = max.saturating_sub(used);
            if rest > 0 {
                out.push(Span::styled(fit(&s.content, rest), s.style));
            }
            break;
        }
    }
    out
}

/// Left and right content on one line, padded to `width`.
fn lr(left: Vec<Span<'static>>, right: Vec<Span<'static>>, width: usize) -> Line<'static> {
    let rw = spans_width(&right);
    let left = fit_spans(left, width.saturating_sub(rw + if rw > 0 { 1 } else { 0 }));
    let lw = spans_width(&left);
    let mut spans = left;
    spans.push(Span::raw(" ".repeat(width.saturating_sub(lw + rw))));
    spans.extend(right);
    Line::from(spans)
}

fn sp(s: impl Into<String>, style: Style) -> Span<'static> {
    Span::styled(s.into(), style)
}

fn clock(ms: Ms) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|d| d.with_timezone(&chrono::Local).format("%H:%M").to_string())
        .unwrap_or_default()
}

fn dur_ms(ms: u64) -> String {
    match ms {
        0..=999 => format!("{ms}ms"),
        1_000..=59_999 => format!("{:.1}s", ms as f64 / 1000.0),
        _ => short_duration(ms as i64),
    }
}

fn spark(values: &[u64], width: usize) -> String {
    const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    if values.is_empty() || width == 0 {
        return String::new();
    }
    let vals = if values.len() > width { &values[values.len() - width..] } else { values };
    let max = (*vals.iter().max().unwrap_or(&1)).max(1);
    vals.iter().map(|v| BARS[((*v as f64 / max as f64) * 7.0).round() as usize]).collect()
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 { format!("1 {word}") } else { format!("{n} {word}s") }
}

fn fill(f: &mut Frame, area: Rect, bg: Color) {
    if bg != Color::Reset {
        f.render_widget(Block::new().style(Style::new().bg(bg)), area);
    }
}

/// Text color on a badge filled with an accent color.
fn on_accent() -> Color {
    let t = theme::get();
    match t.bg {
        Color::Reset if t.light => Color::White,
        Color::Reset => Color::Black,
        c => c,
    }
}

/// Minute buckets for the last hour across some sessions.
fn activity(sessions: &[&Session], now: Ms) -> (Vec<u64>, u64) {
    let minute = now / 60_000;
    let mut buckets = vec![0u64; 60];
    let mut total = 0;
    for s in sessions {
        for (m, n) in &s.activity {
            let age = minute - m;
            if (0..60).contains(&age) {
                buckets[59 - age as usize] += *n as u64;
                total += *n as u64;
            }
        }
    }
    (buckets, total)
}

// ---------------------------------------------------------------------------
// Entry point

pub fn draw(f: &mut Frame, app: &mut App) {
    let t = theme::get();
    let area = f.area();
    f.render_widget(Clear, area);
    fill(f, area, t.bg);

    let alert = !app.alerts().is_empty();
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(if alert { 1 } else { 0 }),
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .split(area);
    header(f, rows[0], app);
    if alert {
        alert_line(f, rows[1], app);
    }
    if app.rows().is_empty() {
        empty_state(f, rows[3], app);
    } else {
        body(f, rows[3], app);
    }
    footer(f, rows[4], app);
    if app.help {
        help(f, area);
    }
    if app.theme_picker.is_some() {
        theme_picker(f, area, app);
    }
    toast(f, rows[3], app);
}

fn header(f: &mut Frame, area: Rect, app: &App) {
    let t = theme::get();
    let now = app.now;
    let c = app.counts();
    // On a filled bar (tmux), everything uses the bar's own text color.
    let tint = |col: Color| if t.bar_bg != Color::Reset && t.name == "tmux" { t.bar_fg } else { col };
    let mut left = vec![sp(" aulora", bold(tint(t.accent)))];
    if let Some((project, item)) = app.breadcrumb() {
        left.push(sp("   ", text()));
        left.push(sp(project, fg(tint(t.dim))));
        if let Some(item) = item {
            left.push(sp(" › ", fg(tint(t.faint))));
            left.push(sp(item, fg(tint(t.text))));
        }
    }
    let mut right: Vec<Span<'static>> = Vec::new();
    for (n, glyph, color, word) in [
        (c.needs_you, "◆", t.yellow, "needs you"),
        (c.errored, "✗", t.red, "failed"),
        (c.working, spinner(now), t.blue, "working"),
        (c.your_turn, "✓", t.green, "your turn"),
    ] {
        if n > 0 {
            right.push(sp(format!("{glyph} {n} {word}   "), fg(tint(color))));
        }
    }
    match &app.conn {
        Conn::Offline(_) => right.push(sp("● daemon offline   ", bold(tint(t.red)))),
        Conn::Connecting => right.push(sp(format!("{} connecting   ", spinner(now)), fg(tint(t.dim)))),
        Conn::Demo => right.push(sp("demo   ", fg(tint(t.magenta)))),
        Conn::Live => {}
    }
    right.push(sp(format!("{} ", clock(now)), fg(tint(t.dim))));
    let bar = Style::new().bg(t.bar_bg).fg(t.bar_fg);
    f.render_widget(Paragraph::new(lr(left, right, area.width as usize)).style(bar), area);
}

fn alert_line(f: &mut Frame, area: Rect, app: &App) {
    let t = theme::get();
    let alerts = app.alerts();
    let Some(s) = alerts.first() else { return };
    let (reason, detail) = match &s.state {
        SessionState::NeedsYou { reason, detail } => (reason.label(), detail.clone()),
        _ => ("needs you", String::new()),
    };
    let left = vec![
        sp(format!(" {} ", state_glyph(&s.state, app.now)), bold(t.yellow)),
        sp(basename(&s.project_root), fg(project_color(&s.project_root))),
        sp(" › ", faint()),
        sp(s.display_title(), text()),
        sp(format!("  {reason}: "), fg(t.yellow)),
        sp(detail, dim()),
    ];
    let mut right = Vec::new();
    if alerts.len() > 1 {
        right.push(sp(format!("+{} more  ", alerts.len() - 1), fg(t.yellow)));
    }
    right.push(sp("n", bold(t.text)));
    right.push(sp(" jump ", faint()));
    f.render_widget(Paragraph::new(lr(left, right, area.width as usize)), area);
}

fn body(f: &mut Frame, area: Rect, app: &mut App) {
    let t = theme::get();
    let zoomed = app.zoom && app.selected_pane().is_some();
    let narrow = area.width < 90;
    let focused = app.mode != Mode::Navigate;
    if zoomed || (narrow && focused) {
        main(f, area, app);
        return;
    }
    if narrow {
        fill(f, area, t.surface);
        sidebar(f, area, app);
        return;
    }
    let side_w = ((area.width as u32 * 30) / 100).clamp(32, 52) as u16;
    let cols = Layout::horizontal([Constraint::Length(side_w), Constraint::Length(1), Constraint::Min(0)]).split(area);
    fill(f, cols[0], t.surface);
    sidebar(f, cols[0], app);
    let sep: Vec<Line> = (0..cols[1].height).map(|_| Line::from(sp("│", fg(t.border)))).collect();
    f.render_widget(Paragraph::new(sep), cols[1]);
    main(f, cols[2], app);
}

// ---------------------------------------------------------------------------
// Sidebar

fn git_brief(g: &GitInfo) -> Vec<Span<'static>> {
    let t = theme::get();
    let mut v = vec![sp(g.branch.clone().unwrap_or_default(), faint())];
    let changes = g.staged + g.modified + g.untracked;
    if g.conflicts > 0 {
        v.push(sp(format!(" ✗{}", g.conflicts), fg(t.red)));
    }
    if changes > 0 {
        v.push(sp(format!(" ●{changes}"), fg(t.yellow)));
    }
    if g.ahead > 0 {
        v.push(sp(format!(" ↑{}", g.ahead), fg(t.green)));
    }
    if g.behind > 0 {
        v.push(sp(format!(" ↓{}", g.behind), fg(t.red)));
    }
    v
}

/// The second line under a session or Claude pane.
fn session_detail(s: &Session, now: Ms) -> Vec<Span<'static>> {
    let t = theme::get();
    match &s.state {
        SessionState::NeedsYou { reason, detail } => {
            vec![sp(format!("{}: ", reason.label()), fg(t.yellow)), sp(detail.clone(), text())]
        }
        SessionState::Errored { detail } => vec![sp(detail.clone(), fg(t.red))],
        SessionState::Working | SessionState::Compacting => match &s.current_tool {
            Some(c) => vec![
                sp(tool_label(&c.name), fg(t.cyan)),
                sp(format!(" {}", c.target), dim()),
                sp(format!(" {}", short_duration(now - c.started_at)), faint()),
            ],
            None => {
                let what = if matches!(s.state, SessionState::Compacting) { "compacting…" } else { "thinking…" };
                vec![sp(what, faint())]
            }
        },
        SessionState::YourTurn => match &s.last_reply {
            Some(r) => vec![sp(first_line(r).replace("**", ""), dim())],
            None => vec![sp("done", faint())],
        },
        SessionState::Idle => vec![sp("waiting for a first prompt", faint())],
        SessionState::Ended { .. } if s.origin == Origin::History => {
            vec![sp(format!("{} turns · last active {}", s.turns, ago(now, s.last_event_at)), faint())]
        }
        SessionState::Ended { reason } => vec![sp(format!("ended · {reason}"), faint())],
    }
}

struct SideLine {
    line: Line<'static>,
    row: Option<usize>,
}

fn sidebar(f: &mut Frame, area: Rect, app: &mut App) {
    let t = theme::get();
    let now = app.now;
    let width = area.width as usize;
    let rows = app.rows();
    let mut out: Vec<SideLine> = Vec::new();
    let c = app.counts();
    let mut head = vec![sp(" PROJECTS", label())];
    if app.show_history {
        head.push(sp("  with history", fg(t.magenta)));
    }
    out.push(SideLine { line: lr(head, vec![sp(format!("{} ", c.projects), faint())], width), row: None });
    if !app.filter.is_empty() || app.filtering {
        out.push(SideLine {
            line: Line::from(vec![
                sp(" / ", bold(t.accent)),
                sp(app.filter.clone(), text()),
                sp(if app.filtering { "▏" } else { "" }, fg(t.accent)),
            ]),
            row: None,
        });
    }

    for (ri, r) in rows.iter().enumerate() {
        let selected = app.selected.as_ref() == Some(r);
        let bg = if selected { Style::new().bg(t.select) } else { Style::new() };
        match r {
            RowRef::Project(root) => {
                let Some(p) = app.project(root) else { continue };
                out.push(SideLine { line: Line::raw(""), row: None });
                let mut right = p.git.as_ref().map(git_brief).unwrap_or_default();
                right.push(sp(" ", text()));
                let name = vec![sp(format!(" {}", p.name), bold(project_color(root)))];
                out.push(SideLine { line: lr(name, right, width).style(bg), row: Some(ri) });
            }
            RowRef::Pane(id) => {
                let Some(x) = app.pane(id).cloned() else { continue };
                let linked = app.pane_session(&x.id).filter(|s| s.state.is_live()).cloned();
                let (glyph, gc, right) = match (&x.exit, &linked) {
                    (Some(e), _) => ("■".to_string(), t.faint, format!("exited {}", e.code)),
                    (None, Some(s)) => (
                        state_glyph(&s.state, now).to_string(),
                        state_color(&s.state),
                        short_duration(now - s.state_since),
                    ),
                    (None, None) => {
                        let active = now - x.last_output_at < 2_000;
                        let glyph = if x.kind == PaneKind::Command { "▶" } else { "❯" };
                        (glyph.to_string(), if active { t.cyan } else { t.faint }, String::new())
                    }
                };
                let title = match &linked {
                    Some(s) => format!("{} · {}", x.name, s.display_title()),
                    None => x.name.clone(),
                };
                let title_style = if x.exit.is_some() { faint() } else { text() };
                let l1 = lr(
                    vec![sp(format!("   {glyph} "), bold(gc)), sp(title, title_style)],
                    vec![sp(format!("{right} "), faint())],
                    width,
                );
                out.push(SideLine { line: l1.style(bg), row: Some(ri) });
                let attention = linked.as_ref().is_some_and(|s| matches!(s.state, SessionState::NeedsYou { .. }));
                if selected || attention {
                    let mut l2 = vec![sp("     ", text())];
                    match &linked {
                        Some(s) => l2.extend(session_detail(s, now)),
                        None => {
                            let what = x.title.clone().unwrap_or_else(|| {
                                if x.kind == PaneKind::Command { x.args.join(" ") } else { tildify(&x.cwd) }
                            });
                            l2.push(sp(what, faint()));
                        }
                    }
                    out.push(SideLine { line: lr(l2, vec![], width).style(bg), row: Some(ri) });
                }
            }
            RowRef::Session(id) => {
                let Some(s) = app.session(id).cloned() else { continue };
                let sc = state_color(&s.state);
                let title_style = if s.state.is_live() { text() } else { faint() };
                let right =
                    if s.state.is_live() { short_duration(now - s.state_since) } else { ago(now, s.last_event_at) };
                let l1 = lr(
                    vec![
                        sp(format!("   {} ", state_glyph(&s.state, now)), bold(sc)),
                        sp(s.display_title(), title_style),
                    ],
                    vec![sp(format!("{right} "), faint())],
                    width,
                );
                out.push(SideLine { line: l1.style(bg), row: Some(ri) });
                if selected || matches!(s.state, SessionState::NeedsYou { .. }) {
                    let mut l2 = vec![sp("     ", text())];
                    l2.extend(session_detail(&s, now));
                    out.push(SideLine { line: lr(l2, vec![], width).style(bg), row: Some(ri) });
                }
            }
        }
    }

    // Keep the selection in view.
    let h = area.height as usize;
    if let Some(sel) = app.selected.as_ref().and_then(|s| rows.iter().position(|r| r == s)) {
        let first = out.iter().position(|l| l.row == Some(sel)).unwrap_or(0);
        let last = out.iter().rposition(|l| l.row == Some(sel)).unwrap_or(first);
        if first < app.scroll {
            app.scroll = first.saturating_sub(1);
        } else if last >= app.scroll + h {
            app.scroll = last + 1 - h;
        }
    }
    app.scroll = app.scroll.min(out.len().saturating_sub(h));
    let visible: Vec<Line> = out.into_iter().skip(app.scroll).take(h).map(|l| l.line).collect();
    f.render_widget(Paragraph::new(visible), area);
}

// ---------------------------------------------------------------------------
// Main area

fn main(f: &mut Frame, area: Rect, app: &mut App) {
    let t = theme::get();
    let Some(sel) = app.selected.clone() else { return };
    let area = Rect { x: area.x + 1, width: area.width.saturating_sub(2), ..area };
    let width = area.width as usize;
    let now = app.now;
    let tabs = app.tabs();
    let tab = app.current_tab();
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .split(area);

    let (title, state_spans, meta) = match &sel {
        RowRef::Pane(id) => {
            let p = app.pane(id).cloned();
            let s = app.pane_session(id).filter(|s| s.state.is_live()).cloned();
            let title = match (&p, &s) {
                (Some(p), Some(s)) => format!("{} · {}", p.name, s.display_title()),
                (Some(p), None) => p.name.clone(),
                _ => String::new(),
            };
            let state = match (&p, &s) {
                (Some(p), _) if p.exit.is_some() => vec![sp("exited", faint())],
                (_, Some(s)) => state_words(s, now),
                _ => vec![sp("running", fg(t.cyan))],
            };
            let mut meta = Vec::new();
            if let Some(p) = &p {
                meta.push(sp(basename(&p.project_root), fg(project_color(&p.project_root))));
                meta.push(sp(format!("  {}", tildify(&p.cwd)), faint()));
                meta.push(sp(format!("  {}×{}", p.cols, p.rows), faint()));
            }
            if let Some(s) = &s {
                session_meta(&mut meta, s);
            }
            (title, state, meta)
        }
        RowRef::Session(id) => {
            let Some(s) = app.session(id).cloned() else { return };
            let mut meta = vec![sp(basename(&s.project_root), fg(project_color(&s.project_root)))];
            if let Some(b) = &s.git_branch {
                meta.push(sp(format!("  {b}"), faint()));
            }
            session_meta(&mut meta, &s);
            if let Some(a) = s.terminal.app_name() {
                meta.push(sp(format!("  in {a}"), faint()));
            }
            (s.display_title(), state_words(&s, now), meta)
        }
        RowRef::Project(root) => {
            let Some(p) = app.project(root).cloned() else { return };
            let mut meta = vec![sp(tildify(&p.root), faint())];
            if let Some(g) = &p.git {
                meta.push(sp("  ", text()));
                meta.extend(git_brief(g));
            }
            (p.name.clone(), vec![], meta)
        }
    };
    f.render_widget(Paragraph::new(lr(vec![sp(title, bold(t.text))], state_spans, width)), rows[0]);
    f.render_widget(Paragraph::new(lr(meta, vec![], width)), rows[1]);

    let mut spans = Vec::new();
    for (i, tb) in tabs.iter().enumerate() {
        let name = format!("{} {}", i + 1, tb.label());
        if *tb == tab {
            spans.push(sp(name, bold(t.accent).add_modifier(Modifier::UNDERLINED)));
        } else {
            spans.push(sp(name, faint()));
        }
        spans.push(sp("    ", text()));
    }
    let badge = |word: &str, c: Color| {
        vec![sp(format!(" {word} "), Style::new().fg(on_accent()).bg(c).add_modifier(Modifier::BOLD))]
    };
    let mode = match app.mode {
        Mode::Terminal => badge("typing", t.accent),
        Mode::Reading => badge("reading", t.magenta),
        Mode::Navigate => vec![],
    };
    f.render_widget(Paragraph::new(lr(spans, mode, width)), rows[2]);
    f.render_widget(Paragraph::new(Line::from(sp("─".repeat(width), fg(t.border)))), rows[3]);

    let content = rows[4];
    match (tab, &sel) {
        (Tab::Terminal, RowRef::Pane(id)) => pane_terminal(f, content, app, &id.clone()),
        (Tab::Conversation, _) => {
            if let Some(sid) = app.selected_session_id() {
                conversation(f, content, app, &sid);
            }
        }
        (Tab::Overview, RowRef::Project(root)) => project_overview(f, content, app, &root.clone()),
        (Tab::Overview, _) => {
            if let Some(s) = app.selected_session_id().and_then(|id| app.session(&id).cloned()) {
                session_overview(f, content, app, &s);
            }
        }
        (Tab::Activity, _) => activity_tab(f, content, app),
        _ => {}
    }
}

fn state_words(s: &Session, now: Ms) -> Vec<Span<'static>> {
    let c = state_color(&s.state);
    let word = match &s.state {
        SessionState::NeedsYou { reason, .. } => reason.label().to_string(),
        SessionState::Ended { .. } if s.origin == Origin::History => "not running".into(),
        other => other.label().to_lowercase(),
    };
    vec![
        sp(format!("{} ", state_glyph(&s.state, now)), bold(c)),
        sp(word, bold(c)),
        sp(format!(" · {}", short_duration(now - s.state_since)), faint()),
    ]
}

fn session_meta(meta: &mut Vec<Span<'static>>, s: &Session) {
    let t = theme::get();
    if let Some(m) = &s.model {
        meta.push(sp(format!("  {}", short_model(m)), fg(t.magenta)));
    }
    if let Some(c) = s.context_tokens {
        meta.push(sp(format!("  ctx {}", fmt_tokens(c)), faint()));
    }
    if let Some(m) = &s.permission_mode
        && m != "default"
    {
        meta.push(sp(format!("  {m}"), fg(if m == "bypassPermissions" { t.red } else { t.dim })));
    }
}

fn pane_terminal(f: &mut Frame, area: Rect, app: &mut App, id: &str) {
    let t = theme::get();
    let now = app.now;
    app.view_size = Some((id.to_string(), area.width, area.height));
    let Some(term) = app.terms.get(id) else {
        let msg = Line::from(vec![sp(format!("{} ", spinner(now)), fg(t.accent)), sp("attaching…", dim())]);
        f.render_widget(Paragraph::new(msg), area);
        return;
    };
    let cursor = crate::term::render(term, area, f.buffer_mut());
    if app.mode == Mode::Terminal
        && let Some(pos) = cursor
    {
        f.set_cursor_position(pos);
    }
    let scrolled = term.scrolled();
    let raised = Style::new().fg(t.text).bg(t.raised);
    if scrolled > 0 {
        let msg = format!(" ↑ {scrolled} lines up · PgDn or End to return ");
        let wd = (w(&msg) as u16).min(area.width);
        let r = Rect { x: area.x + area.width - wd, y: area.y, width: wd, height: 1 };
        f.render_widget(Paragraph::new(Line::from(sp(msg, raised))), r);
    }
    if let Some(e) = app.pane(id).and_then(|p| p.exit.clone()) {
        let msg = format!(" exited with code {} · r restart · x close ", e.code);
        let wd = (w(&msg) as u16).min(area.width);
        let r = Rect { x: area.x, y: area.y + area.height.saturating_sub(1), width: wd, height: 1 };
        f.render_widget(Paragraph::new(Line::from(sp(msg, raised))), r);
    }
}

fn section(title: &str, right: Option<String>, width: usize) -> Line<'static> {
    lr(vec![sp(title.to_string(), label())], right.map(|r| vec![sp(r, faint())]).unwrap_or_default(), width)
}

fn session_overview(f: &mut Frame, area: Rect, app: &App, s: &Session) {
    let t = theme::get();
    let now = app.now;
    let width = area.width as usize;
    let mut lines: Vec<Line> = vec![Line::raw("")];

    lines.push(section("NOW", None, width));
    let now_line: Vec<Span<'static>> = match &s.state {
        SessionState::NeedsYou { reason, detail } => vec![
            sp(format!("  {} ", state_glyph(&s.state, now)), bold(t.yellow)),
            sp(format!("{}: ", reason.label()), bold(t.yellow)),
            sp(detail.clone(), text()),
            sp(format!("  · {}", human_duration(now - s.state_since)), faint()),
        ],
        SessionState::Working => match &s.current_tool {
            Some(c) => vec![
                sp(format!("  {} ", spinner(now)), bold(t.blue)),
                sp(tool_label(&c.name), fg(t.cyan)),
                sp(format!("  {}", c.target), text()),
                sp(format!("  · {}", human_duration(now - c.started_at)), faint()),
            ],
            None => vec![sp(format!("  {} thinking", spinner(now)), fg(t.blue))],
        },
        SessionState::YourTurn => vec![
            sp("  ✓ ", bold(t.green)),
            sp("finished, your turn", fg(t.green)),
            sp(s.last_turn_ms.map(|m| format!("  · took {}", human_duration(m as i64))).unwrap_or_default(), faint()),
        ],
        SessionState::Errored { detail } => vec![sp("  ✗ ", bold(t.red)), sp(detail.clone(), fg(t.red))],
        SessionState::Compacting => vec![sp("  ◌ compacting context", fg(t.magenta))],
        SessionState::Idle => vec![sp("  ○ waiting for a first prompt", dim())],
        SessionState::Ended { .. } if s.origin == Origin::History => vec![
            sp("  not running · last active ", faint()),
            sp(ago(now, s.last_event_at), dim()),
            sp("  ·  resume with ", faint()),
            sp(format!("claude --resume {}", s.id), text()),
        ],
        SessionState::Ended { reason } => {
            vec![sp(format!("  ended ({reason}) · {}", ago(now, s.state_since)), faint())]
        }
    };
    lines.push(lr(now_line, vec![], width));
    if s.origin == Origin::Running && s.state.is_live() {
        lines.push(Line::from(sp(
            "  found in Claude's session list; more detail arrives with its next hook event",
            faint(),
        )));
    }
    lines.push(Line::raw(""));

    let added: u32 = s.files_touched.iter().map(|x| x.added).sum();
    let removed: u32 = s.files_touched.iter().map(|x| x.removed).sum();
    let mut nums = vec![
        sp(format!("  {}", plural(s.turns as usize, "turn")), text()),
        sp(" · ", faint()),
        sp(plural(s.tool_calls as usize, "tool call"), text()),
        sp(" · ", faint()),
        sp(plural(s.files_touched.len(), "file"), text()),
        sp(format!(" +{added}"), fg(t.green)),
        sp(format!(" −{removed}"), fg(t.red)),
    ];
    if s.tool_errors > 0 {
        nums.push(sp(" · ", faint()));
        nums.push(sp(plural(s.tool_errors as usize, "error"), fg(t.red)));
    }
    if s.subagents_active > 0 {
        nums.push(sp(" · ", faint()));
        nums.push(sp(format!("{} agents running", s.subagents_active), fg(t.magenta)));
    }
    let (buckets, total) = activity(&[s], now);
    lines.push(section("AT A GLANCE", Some(format!("{total} events in the last hour")), width));
    lines.push(lr(nums, vec![], width));
    let mut graphs = vec![sp("  activity ", faint()), sp(spark(&buckets, 30), fg(t.blue))];
    if s.context_history.len() > 1 {
        graphs.push(sp("   context ", faint()));
        graphs.push(sp(spark(&s.context_history, 20), fg(t.accent)));
    }
    lines.push(lr(graphs, vec![], width));
    lines.push(Line::raw(""));

    let body_w = width.saturating_sub(4);
    if let Some(p) = &s.last_prompt {
        lines.push(section("YOU ASKED", s.last_prompt_at.map(|a| ago(now, a)), width));
        for l in crate::conv::wrap(p, body_w).into_iter().take(3) {
            lines.push(Line::from(vec![sp("  ", text()), sp(l, text())]));
        }
        lines.push(Line::raw(""));
    }
    let busy = s.state.is_live() && !matches!(s.state, SessionState::YourTurn | SessionState::Idle);
    let reply = if busy { s.live_text.clone() } else { s.last_reply.clone() };
    if let Some(r) = reply {
        lines.push(section("CLAUDE SAID", Some("the rest is in Conversation".into()), width));
        for l in crate::conv::wrap(&r.replace("**", ""), body_w).into_iter().take(5) {
            lines.push(Line::from(vec![sp("  ", text()), sp(l, dim())]));
        }
        lines.push(Line::raw(""));
    }

    let used = (lines.len() as u16).min(area.height);
    f.render_widget(Paragraph::new(lines), area);
    let rest = Rect { y: area.y + used, height: area.height - used, ..area };
    if rest.height < 3 {
        return;
    }
    let (tools_area, files_area) = if rest.width >= 100 {
        let c = Layout::horizontal([Constraint::Percentage(55), Constraint::Length(2), Constraint::Min(0)]).split(rest);
        (c[0], c[2])
    } else {
        let c = Layout::vertical([Constraint::Percentage(55), Constraint::Min(0)]).split(rest);
        (c[0], c[1])
    };
    tool_list(f, tools_area, now, s);
    file_list(f, files_area, &s.files_touched);
}

fn tool_list(f: &mut Frame, area: Rect, now: Ms, s: &Session) {
    let t = theme::get();
    let width = area.width as usize;
    let mut lines = vec![section("RECENT TOOLS", Some(format!("{} total", s.tool_calls)), width)];
    let mut calls: Vec<(&ToolCall, bool)> = Vec::new();
    if let Some(c) = &s.current_tool {
        calls.push((c, true));
    }
    calls.extend(s.recent_tools.iter().map(|c| (c, false)));
    if calls.is_empty() {
        lines.push(Line::from(sp("  none yet", faint())));
    }
    for (c, running) in calls.into_iter().take(area.height.saturating_sub(1) as usize) {
        let (g, gc) = if running {
            (spinner(now).to_string(), t.blue)
        } else if c.ok == Some(false) {
            ("✗".into(), t.red)
        } else {
            ("✓".into(), t.green)
        };
        let dur =
            if running { short_duration(now - c.started_at) } else { c.duration_ms.map(dur_ms).unwrap_or_default() };
        lines.push(lr(
            vec![
                sp(format!("  {g} "), fg(gc)),
                sp(format!("{:<8}", fit(&tool_label(&c.name), 8)), fg(t.cyan)),
                sp(c.target.clone(), dim()),
            ],
            vec![sp(dur, faint())],
            width,
        ));
    }
    f.render_widget(Paragraph::new(lines), area);
}

fn file_list(f: &mut Frame, area: Rect, files: &[FileTouch]) {
    let t = theme::get();
    let width = area.width as usize;
    let mut lines = vec![section("FILES EDITED", Some(plural(files.len(), "file")), width)];
    if files.is_empty() {
        lines.push(Line::from(sp("  none yet", faint())));
    }
    for x in files.iter().take(area.height.saturating_sub(1) as usize) {
        let dir = std::path::Path::new(&x.path).parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
        let mut left = vec![sp("  ", text())];
        if !dir.is_empty() {
            left.push(sp(format!("{dir}/"), faint()));
        }
        left.push(sp(basename(&x.path), text()));
        let right = vec![sp(format!("+{}", x.added), fg(t.green)), sp(format!(" −{}", x.removed), fg(t.red))];
        lines.push(lr(left, right, width));
    }
    f.render_widget(Paragraph::new(lines), area);
}

fn project_overview(f: &mut Frame, area: Rect, app: &App, root: &str) {
    let t = theme::get();
    let now = app.now;
    let width = area.width as usize;
    let Some(p) = app.project(root).cloned() else { return };
    let mut lines: Vec<Line> = vec![Line::raw("")];
    lines.push(section("GIT", None, width));
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
            let ab = match (g.ahead, g.behind) {
                (0, 0) => String::new(),
                (a, b) => format!("  ↑{a} ↓{b}"),
            };
            lines.push(Line::from(vec![
                sp(format!("  {}", g.branch.clone().unwrap_or_default()), text()),
                sp(format!("  {state}"), if parts.is_empty() { fg(t.green) } else { fg(t.yellow) }),
                sp(ab, faint()),
            ]));
            if let Some(m) = &g.last_commit {
                lines.push(Line::from(vec![
                    sp("  last commit ", faint()),
                    sp(m.clone(), dim()),
                    sp(g.last_commit_at.map(|a| format!("  {}", ago(now, a))).unwrap_or_default(), faint()),
                ]));
            }
        }
        None => lines.push(Line::from(sp("  not a git repository", faint()))),
    }
    lines.push(Line::raw(""));

    let panes: Vec<Pane> = app.visible_panes(root).into_iter().cloned().collect();
    let sessions: Vec<Session> = app.visible_sessions(root).into_iter().cloned().collect();
    let counts = format!("{} · {}", plural(panes.len(), "pane"), plural(sessions.len(), "session"));
    lines.push(section("HERE", Some(counts), width));
    for x in &panes {
        let mut l = vec![sp(format!("  ❯ {}", x.name), text())];
        if let Some(s) = app.pane_session(&x.id).filter(|s| s.state.is_live()) {
            l.push(sp("  ", text()));
            l.extend(state_words(s, now));
        }
        lines.push(lr(l, vec![], width));
    }
    for s in &sessions {
        let l = vec![
            sp(format!("  {} ", state_glyph(&s.state, now)), bold(state_color(&s.state))),
            sp(s.display_title(), text()),
            sp(format!("  {}", s.state.label().to_lowercase()), faint()),
        ];
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
    let used = (lines.len() as u16).min(area.height);
    f.render_widget(Paragraph::new(lines), area);
    let rest = Rect { y: area.y + used, height: area.height - used, ..area };
    file_list(f, rest, &files);
}

fn activity_tab(f: &mut Frame, area: Rect, app: &App) {
    let t = theme::get();
    let now = app.now;
    let width = area.width as usize;
    let Some(snap) = &app.snap else { return };
    let (root, session) = match &app.selected {
        Some(RowRef::Project(r)) => (Some(r.clone()), None),
        Some(_) => (app.selected_root(), app.selected_session_id()),
        None => (None, None),
    };
    let sessions: Vec<&Session> = snap
        .sessions
        .iter()
        .filter(|s| match (&session, &root) {
            (Some(id), _) => &s.id == id,
            (None, Some(r)) => &s.project_root == r,
            _ => true,
        })
        .collect();
    let (buckets, total) = activity(&sessions, now);
    let mut lines = vec![
        Line::raw(""),
        section("LAST HOUR", Some(format!("{total} events")), width),
        Line::from(vec![sp("  ", text()), sp(spark(&buckets, width.saturating_sub(4)), fg(t.blue))]),
        Line::raw(""),
        section("EVENTS", None, width),
    ];
    let ids: std::collections::HashSet<&str> = sessions.iter().map(|s| s.id.as_str()).collect();
    let room = area.height.saturating_sub(lines.len() as u16) as usize;
    let events: Vec<&FeedEvent> =
        snap.feed.iter().rev().filter(|e| ids.contains(e.session_id.as_str())).take(room).collect();
    if events.is_empty() {
        lines.push(Line::from(sp("  nothing yet", faint())));
    }
    for e in events {
        let (g, c) = theme::feed_style(e.kind);
        let style = match e.kind {
            FeedKind::Permission | FeedKind::Question => fg(t.yellow),
            FeedKind::Error => fg(t.red),
            FeedKind::Start | FeedKind::End | FeedKind::Info => faint(),
            _ => dim(),
        };
        let l =
            vec![sp(format!("  {}  ", clock(e.at)), faint()), sp(format!("{g} "), fg(c)), sp(e.text.clone(), style)];
        lines.push(lr(l, vec![sp(ago(now, e.at), faint())], width));
    }
    f.render_widget(Paragraph::new(lines), area);
}

fn conversation(f: &mut Frame, area: Rect, app: &mut App, session_id: &str) {
    let t = theme::get();
    let reading = app.mode == Mode::Reading;
    let width = area.width;
    let height = area.height as usize;
    let Some(conv) = app.conversation(session_id) else {
        f.render_widget(Paragraph::new(Line::from(sp("  no conversation yet", faint()))), area);
        return;
    };
    let (selected, follow, unseen, mut scroll) = (conv.selected, conv.follow, conv.unseen, conv.scroll);
    let rendered = conv.render(width.saturating_sub(1)).to_vec();
    if rendered.is_empty() {
        f.render_widget(Paragraph::new(Line::from(sp("  no messages yet", faint()))), area);
        return;
    }
    let total = rendered.len();
    if follow {
        scroll = total.saturating_sub(height);
    } else {
        let first = rendered.iter().position(|r| r.entry == selected).unwrap_or(0);
        let last = rendered.iter().rposition(|r| r.entry == selected).unwrap_or(first);
        if first < scroll {
            scroll = first.saturating_sub(1);
        } else if last >= scroll + height {
            scroll = (last + 1).saturating_sub(height).min(first);
        }
    }
    scroll = scroll.min(total.saturating_sub(height));
    if let Some(c) = app.conversation(session_id) {
        c.scroll = scroll;
    }
    let lines: Vec<Line> = rendered
        .iter()
        .skip(scroll)
        .take(height)
        .map(|r| {
            let mut l = r.line.clone();
            if reading && r.entry == selected {
                let pad = (width as usize).saturating_sub(l.width());
                l.spans.push(Span::raw(" ".repeat(pad)));
                l = l.style(Style::new().bg(t.select));
            }
            l
        })
        .collect();
    f.render_widget(Paragraph::new(lines), area);
    if unseen > 0 && !follow {
        let msg = format!(" ↓ {} · G for latest ", plural(unseen, "new message"));
        let wd = (w(&msg) as u16).min(area.width);
        let r = Rect { x: area.x + area.width - wd, y: area.y + area.height.saturating_sub(1), width: wd, height: 1 };
        f.render_widget(Paragraph::new(Line::from(sp(msg, Style::new().fg(on_accent()).bg(t.accent)))), r);
    }
}

// ---------------------------------------------------------------------------
// Footer, overlays, empty state

fn footer(f: &mut Frame, area: Rect, app: &App) {
    let t = theme::get();
    // A filled bar (tmux) uses its own text color, like tmux's status line.
    let filled = t.bar_bg != Color::Reset && t.name == "tmux";
    let key_style = if filled { bold(t.bar_fg) } else { bold(t.text) };
    let lbl_style = if filled { fg(t.bar_fg) } else { faint() };
    let key = |k: &str| sp(format!(" {k}"), key_style);
    let lbl = |l: &str| sp(format!(" {l}  "), lbl_style);
    let mut left = vec![sp(" ", text())];
    match app.mode {
        Mode::Terminal => {
            let name = app.selected_pane().map(|p| p.name.clone()).unwrap_or_default();
            left.extend([sp(format!(" keys go to {name}   "), dim()), key("ctrl-q"), lbl("back")]);
        }
        Mode::Reading if app.searching => left.extend([
            sp(" search ", bold(t.accent)),
            sp(app.conv_query.clone(), text()),
            sp("▏  ", fg(t.accent)),
            key("⏎"),
            lbl("find"),
            key("esc"),
            lbl("cancel"),
        ]),
        Mode::Reading => left.extend([
            key("↑↓"),
            lbl("select"),
            key("⏎"),
            lbl("expand"),
            key("[ ]"),
            lbl("prompts"),
            key("/"),
            lbl("search"),
            key("y"),
            lbl("copy"),
            key("G"),
            lbl("latest"),
            key("esc"),
            lbl("back"),
        ]),
        Mode::Navigate if app.filtering => {
            left.extend([key("type"), lbl("to filter"), key("⏎"), lbl("keep"), key("esc"), lbl("clear")]);
        }
        Mode::Navigate => {
            let enter = match &app.selected {
                Some(RowRef::Pane(_)) if app.current_tab() == Tab::Terminal => "type",
                Some(RowRef::Pane(_) | RowRef::Session(_)) if app.current_tab() == Tab::Conversation => "read",
                Some(RowRef::Project(_)) => "open in editor",
                Some(_) => "read",
                None => "",
            };
            left.extend([key("↑↓"), lbl("move"), key("⇥"), lbl("tabs"), key("⏎"), lbl(enter)]);
            left.extend([key("c"), lbl("shell"), key("C"), lbl("claude"), key("n"), lbl("next alert")]);
            left.extend([key("t"), lbl("theme"), key("?"), lbl("help"), key("q"), lbl("quit")]);
        }
    }
    let right = vec![sp(format!("{} ", t.name), lbl_style)];
    f.render_widget(Paragraph::new(lr(left, right, area.width as usize)).style(Style::new().bg(t.bar_bg)), area);
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let w_ = width.min(area.width);
    let h_ = height.min(area.height);
    Rect { x: area.x + (area.width - w_) / 2, y: area.y + (area.height - h_) / 2, width: w_, height: h_ }
}

fn overlay(f: &mut Frame, r: Rect, title: &str) -> Rect {
    let t = theme::get();
    f.render_widget(Clear, r);
    let bg = if t.raised == Color::Reset { Color::Black } else { t.raised };
    let block = Block::bordered()
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(fg(t.border))
        .style(Style::new().bg(bg))
        .title(Line::from(sp(format!(" {title} "), bold(t.accent))));
    let inner = block.inner(r);
    f.render_widget(block, r);
    inner
}

fn help(f: &mut Frame, area: Rect) {
    let t = theme::get();
    let inner = overlay(f, centered(area, 76, 31), "keys");
    let row = |k: &str, d: &str| Line::from(vec![sp(format!("  {k:<14}"), bold(t.text)), sp(d.to_string(), dim())]);
    let head = |h: &str| Line::from(sp(format!(" {h}"), label()));
    let mut lines = vec![head("MOVING AROUND")];
    for (k, d) in [
        ("↑ ↓  j k", "move through projects, panes and sessions"),
        ("J K", "next / previous project"),
        ("⇥  1-4", "switch tabs"),
        ("n", "jump to whatever needs you"),
        ("/", "filter the sidebar"),
        ("e  h", "show ended sessions / past sessions"),
    ] {
        lines.push(row(k, d));
    }
    lines.push(head("PANES"));
    for (k, d) in [
        ("⏎", "type into the pane (ctrl-q comes back)"),
        ("c  C", "new shell / new Claude Code pane here"),
        ("z", "zoom the pane to the whole screen"),
        ("PgUp PgDn", "scroll back through its output"),
        ("r  x", "restart an exited pane / kill (twice) or close"),
    ] {
        lines.push(row(k, d));
    }
    lines.push(head("CONVERSATION"));
    for (k, d) in [
        ("⏎", "start reading, then expand a tool call"),
        ("[ ]", "previous / next prompt"),
        ("/  y", "search / copy the selected message"),
    ] {
        lines.push(row(k, d));
    }
    lines.push(head("ELSEWHERE"));
    for (k, d) in [
        ("o", "open the project in your editor"),
        ("t", "choose a theme"),
        ("x  X", "dismiss a session / every ended session"),
        ("q", "quit (panes and the daemon keep running)"),
    ] {
        lines.push(row(k, d));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(sp("  press any key to close", faint())));
    f.render_widget(Paragraph::new(lines), inner);
}

fn theme_picker(f: &mut Frame, area: Rect, app: &App) {
    let t = theme::get();
    let Some(sel) = app.theme_picker else { return };
    let h = theme::THEMES.len() as u16 + 4;
    let inner = overlay(f, centered(area, 48, h), "theme");
    let mut lines = Vec::new();
    for (i, th) in theme::THEMES.iter().enumerate() {
        let selected = i == sel;
        let mut l = vec![
            sp(if selected { " ▸ " } else { "   " }, bold(t.accent)),
            sp(format!("{:<18}", th.name), if selected { bold(t.text) } else { dim() }),
        ];
        for c in [th.accent, th.blue, th.green, th.yellow, th.red, th.magenta] {
            l.push(sp("■ ", fg(c)));
        }
        if th.light {
            l.push(sp("light", faint()));
        }
        lines.push(Line::from(l));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(vec![
        sp("   ⏎", bold(t.text)),
        sp(" keep   ", faint()),
        sp("esc", bold(t.text)),
        sp(" cancel", faint()),
    ]));
    f.render_widget(Paragraph::new(lines), inner);
}

fn toast(f: &mut Frame, area: Rect, app: &App) {
    let t = theme::get();
    let Some((msg, at)) = &app.toast else { return };
    if app.now - at > 2600 {
        return;
    }
    let width = (w(msg) + 4).min(area.width as usize) as u16;
    let r = Rect {
        x: area.x + area.width.saturating_sub(width + 1),
        y: area.y + area.height.saturating_sub(1),
        width,
        height: 1,
    };
    f.render_widget(Clear, r);
    let bg = if t.raised == Color::Reset { Color::DarkGray } else { t.raised };
    let line = Line::from(vec![sp(" ✦ ", bold(t.accent)), sp(format!("{msg} "), text())]);
    f.render_widget(Paragraph::new(line).style(Style::new().bg(bg)), r);
}

const LOGO: [&str; 3] = ["┌─┐┬ ┬┬  ┌─┐┬─┐┌─┐", "├─┤│ ││  │ │├┬┘├─┤", "┴ ┴└─┘┴─┘└─┘┴└─┴ ┴"];

fn empty_state(f: &mut Frame, area: Rect, app: &App) {
    let t = theme::get();
    let r = centered(area, 76, 14);
    let mut lines: Vec<Line> = LOGO.iter().map(|l| Line::from(sp(*l, bold(t.accent))).centered()).collect();
    lines.push(Line::raw(""));
    lines.push(Line::from(sp("Every Claude Code session across your projects, in one place.", dim())).centered());
    lines.push(Line::raw(""));
    match &app.conn {
        Conn::Offline(why) => {
            lines.push(Line::from(sp(format!("Can't reach the aulora daemon: {why}"), bold(t.red))).centered());
            lines.push(Line::from(sp("Retrying… or run `aulora daemon start`", faint())).centered());
        }
        Conn::Connecting => {
            lines.push(Line::from(sp(format!("{} starting the daemon…", spinner(app.now)), dim())).centered());
        }
        _ if !app.hooks_installed => {
            let l = vec![sp("aulora hooks install", bold(t.text)), sp("  connects Claude Code", faint())];
            lines.push(Line::from(l).centered());
        }
        _ => {
            let msg = "No sessions yet. Start `claude` in any project and it shows up here.";
            lines.push(Line::from(sp(msg, faint())).centered());
        }
    }
    lines.push(Line::raw(""));
    lines.push(
        Line::from(vec![
            sp("c", bold(t.accent)),
            sp(" shell here    ", faint()),
            sp("C", bold(t.accent)),
            sp(" Claude Code here    ", faint()),
            sp("h", bold(t.accent)),
            sp(" past sessions", faint()),
        ])
        .centered(),
    );
    f.render_widget(Paragraph::new(lines), r);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        assert_eq!(fit("abcdef", 4), "abc…");
        assert_eq!(spark(&[0, 4, 8], 3), "▁▅█");
        let l = lr(vec![sp("left side text", text())], vec![sp("R", text())], 10);
        assert_eq!(spans_width(&l.spans), 10);
    }
}
