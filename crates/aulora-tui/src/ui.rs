use aulora_core::model::*;
use aulora_core::paths::tildify;
use aulora_core::reduce::tool_label;
use aulora_core::util::{ago, basename, first_line, fmt_tokens, human_duration, short_duration, short_model};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Sparkline};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::{App, Conn, RowRef};
use crate::theme::*;

// ---------------------------------------------------------------------------
// Text helpers

fn w(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// Cut to `max` display columns, with an ellipsis if cut.
fn fit(s: &str, max: usize) -> String {
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

/// Truncate a run of spans to `max` columns.
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

/// Word-wrap plain text to `width` columns.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(4);
    let mut out = Vec::new();
    for para in text.lines() {
        let para = para.trim_end();
        if para.is_empty() {
            if out.last().is_some_and(|l: &String| !l.is_empty()) {
                out.push(String::new());
            }
            continue;
        }
        let mut line = String::new();
        for word in para.split_whitespace() {
            let need = if line.is_empty() { w(word) } else { w(&line) + 1 + w(word) };
            if need > width && !line.is_empty() {
                out.push(std::mem::take(&mut line));
            }
            if w(word) > width {
                // Hard-break very long words (paths, URLs).
                let mut chunk = String::new();
                for c in word.chars() {
                    if w(&chunk) + c.width().unwrap_or(0) > width {
                        out.push(std::mem::take(&mut chunk));
                    }
                    chunk.push(c);
                }
                line = chunk;
            } else {
                if !line.is_empty() {
                    line.push(' ');
                }
                line.push_str(word);
            }
        }
        if !line.is_empty() {
            out.push(line);
        }
    }
    while out.last().is_some_and(|l| l.is_empty()) {
        out.pop();
    }
    out
}

/// Strip the most common markdown noise from replies.
fn plain(text: &str) -> String {
    text.lines()
        .map(|l| {
            let t = l.trim_start();
            let t = t.trim_start_matches('#').trim_start();
            t.replace("**", "").replace("__", "")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn clock(ms: Ms) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|d| d.with_timezone(&chrono::Local).format("%H:%M:%S").to_string())
        .unwrap_or_default()
}

fn hm(ms: Ms) -> String {
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

/// One-line sparkline from values, scaled to the max.
fn spark(values: &[u64], width: usize) -> String {
    const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    if values.is_empty() || width == 0 {
        return String::new();
    }
    let vals: Vec<u64> = if values.len() > width { values[values.len() - width..].to_vec() } else { values.to_vec() };
    let max = *vals.iter().max().unwrap_or(&1).max(&1);
    vals.iter().map(|v| BARS[((*v as f64 / max as f64) * 7.0).round() as usize]).collect()
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 { format!("1 {word}") } else { format!("{n} {word}s") }
}

fn panel(title: &str) -> Block<'static> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(BORDER))
        .style(Style::new().bg(PANEL))
        .title(Line::from(vec![Span::raw(" "), Span::styled(title.to_string(), label()), Span::raw(" ")]))
}

fn sp(s: impl Into<String>, style: Style) -> Span<'static> {
    Span::styled(s.into(), style)
}

fn gradient_text(text: &str, bold_: bool) -> Vec<Span<'static>> {
    let n = text.chars().count().max(2) - 1;
    text.chars()
        .enumerate()
        .map(|(i, c)| {
            let mut st = Style::new().fg(aurora(i as f32 / n as f32));
            if bold_ {
                st = st.add_modifier(Modifier::BOLD);
            }
            Span::styled(c.to_string(), st)
        })
        .collect()
}

fn state_pill(s: &Session, now: Ms) -> Vec<Span<'static>> {
    let c = live_state_color(&s.state, now);
    vec![sp(
        format!(" {} {} · {} ", state_glyph(&s.state, now), s.state.label(), short_duration(now - s.state_since)),
        Style::new().bg(c).fg(BG).add_modifier(Modifier::BOLD),
    )]
}

// ---------------------------------------------------------------------------
// Entry point

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    f.render_widget(Block::new().style(Style::new().bg(BG)), area);

    let has_alert = !app.alerts().is_empty();
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(if has_alert { 1 } else { 0 }),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .split(area);

    header(f, rows[0], app);
    if has_alert {
        alert_banner(f, rows[1], app);
    }
    let body = rows[2].inner(ratatui::layout::Margin { horizontal: 1, vertical: 0 });
    let no_sessions = app.snap.as_ref().is_none_or(|s| s.sessions.is_empty());
    if no_sessions {
        empty_state(f, body, app);
    } else {
        main_body(f, body, app);
    }
    footer(f, rows[3], app);
    if app.help {
        help(f, area, app);
    }
    toast(f, rows[2], app);
}

fn header(f: &mut Frame, area: Rect, app: &App) {
    let now = app.now;
    let c = app.counts();
    let mut left = vec![sp(" ✦ ", bold(TEAL))];
    left.extend(gradient_text("aulora", true));
    left.push(sp(format!("  v{}", aulora_core::VERSION), faint()));
    left.push(sp("   ", text()));

    let pill = |n: usize, glyph: &str, label_: &str, color: Color| -> Vec<Span<'static>> {
        let active = n > 0;
        vec![
            sp(format!("{glyph} "), Style::new().fg(if active { color } else { FAINT })),
            sp(n.to_string(), if active { bold(color) } else { faint() }),
            sp(format!(" {label_}   "), if active { dim() } else { faint() }),
        ]
    };
    let ny_color = if c.needs_you > 0 { lerp(AMBER, Color::Rgb(255, 236, 170), pulse(now, 1400)) } else { AMBER };
    left.extend(pill(c.needs_you, "◆", "needs you", ny_color));
    if c.errored > 0 {
        left.extend(pill(c.errored, "✗", "errored", RED));
    }
    left.extend(pill(c.working, spinner(now), "working", CYAN));
    left.extend(pill(c.your_turn, "✓", "your turn", GREEN));
    left.extend(pill(c.idle, "○", "idle", DIM));

    let mut right: Vec<Span<'static>> = Vec::new();
    match &app.conn {
        Conn::Live => {
            if let Some(s) = &app.snap {
                right.push(sp(format!("{} · {} live   ", plural(c.projects, "project"), c.live()), faint()));
                right.push(sp("● ", Style::new().fg(lerp(GREEN, TEAL, pulse(now, 3000)))));
                right.push(sp(
                    format!(
                        "daemon up {} · {} events   ",
                        short_duration(now - s.daemon.started_at),
                        s.daemon.hooks_seen
                    ),
                    dim(),
                ));
            }
        }
        Conn::Demo => right.push(sp("◇ demo data   ", bold(VIOLET))),
        Conn::Connecting => right.push(sp(format!("{} connecting to daemon…   ", spinner(now)), dim())),
        Conn::Offline(why) => right.push(sp(format!("● daemon offline: {why} · retrying   "), bold(RED))),
    }
    right.push(sp(format!("{} ", clock(now)), text()));
    let line = lr(left, right, area.width as usize);
    f.render_widget(Paragraph::new(line).style(Style::new().bg(PANEL)), area);
}

fn alert_banner(f: &mut Frame, area: Rect, app: &App) {
    let alerts = app.alerts();
    let Some(s) = alerts.first() else { return };
    let now = app.now;
    let pc = lerp(AMBER, Color::Rgb(255, 240, 190), pulse(now, 1400));
    let (reason, detail) = match &s.state {
        SessionState::NeedsYou { reason, detail } => (reason.label(), detail.clone()),
        _ => ("needs you", String::new()),
    };
    let left = vec![
        sp(" ◆ NEEDS YOU ", Style::new().bg(pc).fg(BG).add_modifier(Modifier::BOLD)),
        sp("  ", text()),
        sp(basename(&s.project_root), bold(project_color(&s.project_root))),
        sp(" › ", faint()),
        sp(s.display_title(), bold(TEXT)),
        sp(format!("  {reason}: "), Style::new().fg(AMBER)),
        sp(detail, text()),
        sp(format!("  · {}", short_duration(now - s.state_since)), dim()),
    ];
    let mut right = Vec::new();
    if alerts.len() > 1 {
        right.push(sp(format!("+{} more  ", alerts.len() - 1), bold(AMBER)));
    }
    right.push(sp("n", bold(TEXT)));
    right.push(sp(" next  ", dim()));
    right.push(sp("⏎", bold(TEXT)));
    right.push(sp(format!(" open in {} ", editor_name(app)), dim()));
    f.render_widget(Paragraph::new(lr(left, right, area.width as usize)).style(Style::new().bg(AMBER_BG)), area);
}

fn editor_name(app: &App) -> String {
    match app.editor.as_str() {
        "code" | "code-insiders" => "VS Code".into(),
        "cursor" => "Cursor".into(),
        "zed" => "Zed".into(),
        other => other.to_string(),
    }
}

fn main_body(f: &mut Frame, area: Rect, app: &mut App) {
    let show_feed = area.height >= 28;
    let feed_h = if area.height >= 44 { 10 } else { 7 };
    let rows =
        Layout::vertical([Constraint::Min(0), Constraint::Length(if show_feed { feed_h } else { 0 })]).split(area);
    let wide = area.width >= 118;
    if wide {
        let left_w = ((area.width as u32 * 40) / 100).clamp(52, 78) as u16;
        let cols =
            Layout::horizontal([Constraint::Length(left_w), Constraint::Length(1), Constraint::Min(0)]).split(rows[0]);
        session_list(f, cols[0], app);
        match app.selected.clone() {
            Some(RowRef::Session(id)) => {
                if let Some(s) = app.session(&id).cloned() {
                    session_detail(f, cols[2], app, &s);
                }
            }
            Some(RowRef::Project(root)) => project_detail(f, cols[2], app, &root),
            None => {}
        }
    } else {
        session_list(f, rows[0], app);
    }
    if show_feed {
        feed(f, rows[1], app);
    }
}

// ---------------------------------------------------------------------------
// Left: projects and sessions

fn git_spans(g: &GitInfo) -> Vec<Span<'static>> {
    let mut v = vec![sp(format!("⎇ {}", g.branch.clone().unwrap_or_else(|| "?".into())), dim())];
    if g.ahead > 0 {
        v.push(sp(format!(" ↑{}", g.ahead), Style::new().fg(GREEN)));
    }
    if g.behind > 0 {
        v.push(sp(format!(" ↓{}", g.behind), Style::new().fg(RED)));
    }
    if g.conflicts > 0 {
        v.push(sp(format!(" ✗{}", g.conflicts), bold(RED)));
    }
    if g.staged > 0 {
        v.push(sp(format!(" +{}", g.staged), Style::new().fg(GREEN)));
    }
    if g.modified > 0 {
        v.push(sp(format!(" ~{}", g.modified), Style::new().fg(BLUE)));
    }
    if g.untracked > 0 {
        v.push(sp(format!(" ?{}", g.untracked), faint()));
    }
    if !g.is_dirty() {
        v.push(sp(" ✓", faint()));
    }
    v
}

fn session_detail_line(s: &Session, now: Ms) -> Vec<Span<'static>> {
    match &s.state {
        SessionState::NeedsYou { reason, detail } => {
            vec![sp(format!("{}: ", reason.label()), bold(AMBER)), sp(detail.clone(), Style::new().fg(TEXT))]
        }
        SessionState::Errored { detail } => vec![sp(detail.clone(), Style::new().fg(RED))],
        SessionState::Working | SessionState::Compacting => {
            let mut v = match &s.current_tool {
                Some(t) => vec![
                    sp(tool_label(&t.name), bold(CYAN)),
                    sp(format!(" {}", t.target), dim()),
                    sp(format!(" {}", short_duration(now - t.started_at)), faint()),
                ],
                None if matches!(s.state, SessionState::Compacting) => {
                    vec![sp("compacting context…", Style::new().fg(VIOLET))]
                }
                None => vec![sp("thinking…", dim())],
            };
            if s.subagents_active > 0 {
                v.push(sp(format!(" · ⑂{}", s.subagents_active), Style::new().fg(PINK)));
            }
            v
        }
        SessionState::YourTurn => match &s.last_reply {
            Some(r) => {
                vec![sp(format!("“{}”", first_line(&plain(r))), Style::new().fg(DIM).add_modifier(Modifier::ITALIC))]
            }
            None => vec![sp("turn finished", dim())],
        },
        SessionState::Idle => vec![sp(
            s.last_prompt
                .as_deref()
                .map(|p| format!("› {}", first_line(p)))
                .unwrap_or_else(|| "waiting for a first prompt".into()),
            faint(),
        )],
        SessionState::Ended { reason } => {
            vec![sp(format!("ended · {reason} · {}", ago(now, s.state_since)), faint())]
        }
    }
}

struct ListLine {
    line: Line<'static>,
    row: usize,
}

fn session_list(f: &mut Frame, area: Rect, app: &mut App) {
    let now = app.now;
    let c = app.counts();
    let title_right = format!(" {} · {} ", plural(c.live() + c.ended, "session"), plural(c.projects, "project"));
    let mut block = panel("projects")
        .title(Line::from(sp(title_right, faint())).right_aligned())
        .border_style(Style::new().fg(BORDER_HI));
    if !app.filter.is_empty() || app.filtering {
        block = block.title_bottom(Line::from(vec![
            sp(" / ", bold(TEAL)),
            sp(app.filter.clone(), text()),
            sp(if app.filtering { "▏" } else { "" }, Style::new().fg(TEAL)),
            sp(" ", text()),
        ]));
    }
    let inner = block.inner(area);
    f.render_widget(block, area);
    let width = inner.width as usize;

    let rows = app.rows();
    let mut lines: Vec<ListLine> = Vec::new();
    let mut ri = 0usize;
    let projects: Vec<Project> = app.visible_projects().into_iter().cloned().collect();
    for (pi, p) in projects.iter().enumerate() {
        if pi > 0 {
            lines.push(ListLine { line: Line::raw(""), row: usize::MAX });
        }
        let pc = project_color(&p.root);
        let sessions: Vec<Session> = app.visible_sessions(&p.root).into_iter().cloned().collect();
        let selected = app.selected.as_ref() == Some(&RowRef::Project(p.root.clone()));
        let marker = if selected { sp("▌", Style::new().fg(pc)) } else { sp(" ", text()) };
        let needs = sessions.iter().filter(|s| matches!(s.state, SessionState::NeedsYou { .. })).count();
        let mut l1 = vec![marker.clone(), sp(p.name.clone(), bold(pc))];
        if needs > 0 {
            l1.push(sp(format!("  ◆{needs}"), bold(AMBER)));
        }
        let r1 = p.git.as_ref().map(git_spans).unwrap_or_else(|| vec![sp("no git", faint())]);
        let mut l2 = vec![marker, sp(tildify(&p.root), faint())];
        if let Some(g) = &p.git
            && let (Some(msg), Some(at)) = (&g.last_commit, g.last_commit_at)
        {
            l2.push(sp(format!("  ·  {} ", short_duration(now - at)), faint()));
            l2.push(sp(msg.clone(), dim()));
        }
        let bg = if selected { Style::new().bg(SELECT) } else { Style::new() };
        lines.push(ListLine { line: lr(l1, r1, width).style(bg), row: ri });
        lines.push(ListLine { line: lr(l2, vec![], width).style(bg), row: ri });
        ri += 1;

        let n = sessions.len();
        for (si, s) in sessions.iter().enumerate() {
            let last = si + 1 == n;
            let selected = app.selected.as_ref() == Some(&RowRef::Session(s.id.clone()));
            let marker = if selected { sp("▌", Style::new().fg(pc)) } else { sp(" ", text()) };
            let sc = live_state_color(&s.state, now);
            let title_style = match s.state {
                SessionState::NeedsYou { .. } => bold(TEXT),
                SessionState::Ended { .. } => faint(),
                _ => text(),
            };
            let l1 = vec![
                marker.clone(),
                sp(if last { " └─ " } else { " ├─ " }, faint()),
                sp(state_glyph(&s.state, now), bold(sc)),
                sp(" ", text()),
                sp(s.display_title(), title_style),
            ];
            let r1 =
                vec![sp(s.state.label(), bold(sc)), sp(format!(" {:>5}", short_duration(now - s.state_since)), dim())];
            let mut l2 = vec![marker, sp(if last { "      " } else { " │    " }, faint())];
            l2.extend(session_detail_line(s, now));
            let mut r2 = Vec::new();
            if let Some(app_name) = s.terminal.app_name() {
                r2.push(sp(app_name, faint()));
            }
            if let Some(m) = &s.model {
                if !r2.is_empty() {
                    r2.push(sp(" · ", faint()));
                }
                r2.push(sp(short_model(m), faint()));
            }
            let bg = if selected { Style::new().bg(SELECT) } else { Style::new() };
            lines.push(ListLine { line: lr(l1, r1, width).style(bg), row: ri });
            lines.push(ListLine { line: lr(l2, r2, width).style(bg), row: ri });
            ri += 1;
        }
    }

    // Keep the selected row in view.
    let sel = app.selected.as_ref().and_then(|s| rows.iter().position(|r| r == s));
    let h = inner.height as usize;
    if let Some(sel) = sel {
        let first = lines.iter().position(|l| l.row == sel).unwrap_or(0);
        let last = lines.iter().rposition(|l| l.row == sel).unwrap_or(first);
        if first < app.scroll {
            app.scroll = first.saturating_sub(if sel == 0 { 0 } else { 1 });
        } else if last >= app.scroll + h {
            app.scroll = last + 1 - h;
        }
    }
    app.scroll = app.scroll.min(lines.len().saturating_sub(h));
    let visible: Vec<Line> = lines.into_iter().skip(app.scroll).take(h).map(|l| l.line).collect();
    f.render_widget(Paragraph::new(visible), inner);
}

// ---------------------------------------------------------------------------
// Right: one session

fn meta(label_: &str, value: impl Into<String>, color: Color) -> Vec<Span<'static>> {
    vec![sp(format!("{label_} "), faint()), sp(value.into(), Style::new().fg(color)), sp("   ", text())]
}

fn session_detail(f: &mut Frame, area: Rect, app: &App, s: &Session) {
    let now = app.now;
    let tall = area.height >= 30;
    let rows = Layout::vertical([
        Constraint::Length(6),
        Constraint::Length(4),
        Constraint::Length(if tall { 6 } else { 5 }),
        Constraint::Min(6),
    ])
    .split(area);

    // Header card.
    let pc = project_color(&s.project_root);
    let block = panel("session")
        .border_style(Style::new().fg(if matches!(s.state, SessionState::NeedsYou { .. }) {
            lerp(AMBER_DIM, AMBER, pulse(now, 1400))
        } else {
            BORDER_HI
        }))
        .title(Line::from(sp(format!(" {} ", s.short_id()), faint())).right_aligned());
    let inner = block.inner(rows[0]);
    f.render_widget(block, rows[0]);
    let width = inner.width as usize;
    let mut lines = Vec::new();
    lines.push(lr(vec![sp(" ", text()), sp(s.display_title(), bold(TEXT))], state_pill(s, now), width));
    let branch =
        s.git_branch.clone().or_else(|| app.project(&s.project_root).and_then(|p| p.git.as_ref()?.branch.clone()));
    let mut l2 = vec![sp(" ", text()), sp(basename(&s.project_root), bold(pc))];
    if let Some(b) = branch {
        l2.push(sp(format!("  ⎇ {b}"), dim()));
    }
    l2.push(sp(format!("   {}", tildify(&s.cwd)), faint()));
    lines.push(lr(l2, vec![], width));
    let mut l3 = vec![sp(" ", text())];
    if let Some(m) = &s.model {
        l3.extend(meta("model", short_model(m), VIOLET));
    }
    if let Some(e) = &s.effort {
        l3.extend(meta("effort", e.clone(), TEXT));
    }
    if let Some(m) = &s.permission_mode {
        let color = if m == "bypassPermissions" {
            RED
        } else if m == "plan" {
            TEAL
        } else {
            TEXT
        };
        l3.extend(meta("mode", m.clone(), color));
    }
    if let Some(a) = s.terminal.app_name() {
        l3.extend(meta("in", a, TEXT));
    }
    if let Some(pid) = s.pid {
        l3.extend(meta("pid", pid.to_string(), DIM));
    }
    lines.push(lr(l3, vec![], width));
    let mut l4 = vec![
        sp(" started ", faint()),
        sp(hm(s.started_at), text()),
        sp(format!(" ({})", ago(now, s.started_at)), faint()),
        sp("   last event ", faint()),
        sp(ago(now, s.last_event_at), text()),
    ];
    if let Some(t) = s.turn_started_at {
        l4.push(sp("   turn running ", faint()));
        l4.push(sp(human_duration(now - t), bold(CYAN)));
    } else if let Some(t) = s.last_turn_ms {
        l4.push(sp("   last turn took ", faint()));
        l4.push(sp(human_duration(t as i64), text()));
    }
    lines.push(lr(l4, vec![], width));
    f.render_widget(Paragraph::new(lines), inner);

    // Stats tiles.
    let added: u32 = s.files_touched.iter().map(|f| f.added).sum();
    let removed: u32 = s.files_touched.iter().map(|f| f.removed).sum();
    let tiles: Vec<(&str, String, Color, String)> = vec![
        ("TURNS", s.turns.to_string(), TEXT, String::new()),
        ("TOOL CALLS", s.tool_calls.to_string(), CYAN, String::new()),
        ("FILES", s.files_touched.len().to_string(), VIOLET, format!(" +{added} −{removed}")),
        ("ERRORS", s.tool_errors.to_string(), if s.tool_errors > 0 { RED } else { FAINT }, String::new()),
        ("AGENTS", s.subagents_active.to_string(), if s.subagents_active > 0 { PINK } else { FAINT }, String::new()),
        ("CONTEXT", s.context_tokens.map(fmt_tokens).unwrap_or_else(|| "—".into()), TEAL, String::new()),
        ("OUTPUT", if s.output_tokens > 0 { fmt_tokens(s.output_tokens) } else { "—".into() }, BLUE, String::new()),
    ];
    stat_tiles(f, rows[1], &tiles);

    // Now + activity.
    let mid =
        Layout::horizontal([Constraint::Percentage(55), Constraint::Length(1), Constraint::Min(0)]).split(rows[2]);
    now_panel(f, mid[0], app, s);
    activity_panel(f, mid[2], now, &[s], s.context_history.clone(), s.context_tokens);

    // Conversation + tools/files.
    let bottom =
        Layout::horizontal([Constraint::Percentage(55), Constraint::Length(1), Constraint::Min(0)]).split(rows[3]);
    conversation(f, bottom[0], now, s);
    let right = Layout::vertical([Constraint::Percentage(58), Constraint::Min(4)]).split(bottom[2]);
    tools_panel(f, right[0], now, s);
    files_panel(f, right[1], &s.files_touched, s.files_touched.len());
}

fn stat_tiles(f: &mut Frame, area: Rect, tiles: &[(&str, String, Color, String)]) {
    let block = panel("stats");
    let inner = block.inner(area);
    f.render_widget(block, area);
    let n = tiles.len() as u32;
    let cols = Layout::horizontal((0..n).map(|_| Constraint::Ratio(1, n))).split(inner);
    for (i, (label_, value, color, sub)) in tiles.iter().enumerate() {
        let sep = if i == 0 { " " } else { "│" };
        let lines = vec![
            Line::from(vec![sp(sep, Style::new().fg(BORDER)), sp(format!(" {label_}"), label())]),
            Line::from(vec![
                sp(sep, Style::new().fg(BORDER)),
                sp(format!(" {value}"), bold(*color)),
                sp(if w(value) + w(sub) + 2 < cols[i].width as usize { sub.clone() } else { String::new() }, faint()),
            ]),
        ];
        f.render_widget(Paragraph::new(lines), cols[i]);
    }
}

fn now_panel(f: &mut Frame, area: Rect, app: &App, s: &Session) {
    let now = app.now;
    let (title_color, lines): (Color, Vec<Line<'static>>) = match &s.state {
        SessionState::NeedsYou { reason, detail } => {
            let headline = match reason {
                Attention::Permission => "Waiting for your permission",
                Attention::Question => "Claude asked you a question",
                Attention::PlanApproval => "A plan is ready for your review",
                Attention::Input => "Claude needs your input",
            };
            (
                lerp(AMBER_DIM, AMBER, pulse(now, 1400)),
                vec![
                    Line::from(vec![sp(" ◆ ", bold(AMBER)), sp(headline, bold(AMBER))]),
                    Line::from(vec![sp("   ", text()), sp(detail.clone(), text())]),
                    Line::from(vec![
                        sp(format!("   waiting {}  ·  ", human_duration(now - s.state_since)), faint()),
                        sp("⏎", bold(TEXT)),
                        sp(format!(" open in {}", editor_name(app)), faint()),
                    ]),
                ],
            )
        }
        SessionState::Working => {
            let mut v = match &s.current_tool {
                Some(t) => vec![
                    Line::from(vec![
                        sp(format!(" {} ", spinner(now)), bold(CYAN)),
                        sp(tool_label(&t.name), bold(CYAN)),
                        sp(format!("  {}", t.target), text()),
                    ]),
                    Line::from(vec![
                        sp(format!("   running {}", human_duration(now - t.started_at)), dim()),
                        sp(
                            t.agent.as_ref().map(|a| format!("  ·  in {a} agent")).unwrap_or_default(),
                            Style::new().fg(PINK),
                        ),
                    ]),
                ],
                None => vec![
                    Line::from(vec![sp(format!(" {} ", spinner(now)), bold(CYAN)), sp("Thinking…", bold(CYAN))]),
                    Line::raw(""),
                ],
            };
            let turn = s.turn_started_at.map(|t| human_duration(now - t)).unwrap_or_default();
            v.push(Line::from(vec![
                sp(format!("   turn {turn}"), faint()),
                sp(
                    if s.subagents_active > 0 {
                        format!("  ·  ⑂ {} agents running", s.subagents_active)
                    } else {
                        String::new()
                    },
                    Style::new().fg(PINK),
                ),
            ]));
            (CYAN, v)
        }
        SessionState::YourTurn => (
            GREEN,
            vec![
                Line::from(vec![sp(" ✓ ", bold(GREEN)), sp("Finished · your turn", bold(GREEN))]),
                Line::from(vec![sp(
                    format!(
                        "   took {}  ·  done {}",
                        s.last_turn_ms.map(|t| human_duration(t as i64)).unwrap_or_else(|| "—".into()),
                        ago(now, s.state_since)
                    ),
                    dim(),
                )]),
                Line::from(vec![sp("   ⏎", bold(TEXT)), sp(format!(" open in {}", editor_name(app)), faint())]),
            ],
        ),
        SessionState::Compacting => (
            VIOLET,
            vec![
                Line::from(vec![sp(" ◌ ", bold(VIOLET)), sp("Compacting context…", bold(VIOLET))]),
                Line::from(sp(format!("   for {}", human_duration(now - s.state_since)), dim())),
            ],
        ),
        SessionState::Errored { detail } => (
            RED,
            vec![
                Line::from(vec![sp(" ✗ ", bold(RED)), sp("The last turn failed", bold(RED))]),
                Line::from(vec![sp("   ", text()), sp(detail.clone(), text())]),
            ],
        ),
        SessionState::Idle => (
            DIM,
            vec![
                Line::from(vec![sp(" ○ ", dim()), sp("Idle", bold(DIM))]),
                Line::from(sp("   waiting for your first prompt", faint())),
            ],
        ),
        SessionState::Ended { reason } => (
            FAINT,
            vec![
                Line::from(vec![sp(" ■ ", faint()), sp(format!("Session ended ({reason})"), bold(DIM))]),
                Line::from(sp(format!("   {}  ·  x to dismiss", ago(now, s.state_since)), faint())),
            ],
        ),
    };
    let block = panel("now").border_style(Style::new().fg(title_color));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let width = inner.width as usize;
    let lines: Vec<Line> = lines.into_iter().map(|l| lr(l.spans, vec![], width)).collect();
    f.render_widget(Paragraph::new(lines), inner);
}

fn activity_panel(f: &mut Frame, area: Rect, now: Ms, sessions: &[&Session], ctx: Vec<u64>, ctx_now: Option<u64>) {
    let minute = now / 60_000;
    let mut buckets = vec![0u64; 60];
    let mut total = 0u64;
    for s in sessions {
        for (m, n) in &s.activity {
            let age = minute - m;
            if (0..60).contains(&age) {
                buckets[59 - age as usize] += *n as u64;
                total += *n as u64;
            }
        }
    }
    let block = panel("activity · 60 min").title(Line::from(sp(format!(" {total} events "), faint())).right_aligned());
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height == 0 {
        return;
    }
    let spark_h = inner.height.saturating_sub(1).max(1);
    let rows = Layout::vertical([Constraint::Length(spark_h), Constraint::Length(1)]).split(inner);
    let width = inner.width as usize;
    let data: Vec<u64> = if buckets.len() > width { buckets[buckets.len() - width..].to_vec() } else { buckets };
    let pad = width.saturating_sub(data.len());
    let spark_area = Rect { x: rows[0].x + pad as u16, width: rows[0].width.saturating_sub(pad as u16), ..rows[0] };
    f.render_widget(
        Sparkline::default().data(data).bar_set(symbols::bar::NINE_LEVELS).style(Style::new().fg(CYAN).bg(PANEL)),
        spark_area,
    );
    if inner.height >= 2 {
        let mut l = vec![sp(" context ", faint())];
        let ctx_spark = spark(&ctx, width.saturating_sub(20));
        l.push(sp(ctx_spark, Style::new().fg(TEAL)));
        let right = vec![sp(ctx_now.map(fmt_tokens).unwrap_or_else(|| "—".into()), bold(TEAL)), sp(" ", text())];
        f.render_widget(Paragraph::new(lr(l, right, width)), rows[1]);
    }
}

fn conversation(f: &mut Frame, area: Rect, now: Ms, s: &Session) {
    let block = panel("conversation").title(Line::from(sp(format!(" turn {} ", s.turns), faint())).right_aligned());
    let inner = block.inner(area);
    f.render_widget(block, area);
    let width = inner.width.saturating_sub(2) as usize;
    let h = inner.height as usize;
    let mut lines: Vec<Line> = Vec::new();

    let prompt = s.last_prompt.clone().unwrap_or_default();
    let mut p_lines = wrap(&prompt, width);
    let p_max = (h / 3).max(2);
    if p_lines.len() > p_max {
        p_lines.truncate(p_max);
        if let Some(l) = p_lines.last_mut() {
            *l = fit(&format!("{l} …"), width);
        }
    }
    lines.push(Line::from(vec![
        sp(" YOU", bold(TEAL)),
        sp(s.last_prompt_at.map(|t| format!("  {} · {}", hm(t), ago(now, t))).unwrap_or_default(), faint()),
    ]));
    if p_lines.is_empty() {
        lines.push(Line::from(sp("  no prompt yet", faint())));
    }
    for l in p_lines {
        lines.push(Line::from(vec![sp("  ", text()), sp(l, text())]));
    }
    lines.push(Line::raw(""));

    let working = matches!(s.state, SessionState::Working | SessionState::Compacting | SessionState::NeedsYou { .. });
    let live = if working { s.live_text.clone().filter(|t| !t.trim().is_empty()) } else { None };
    let (reply_label, reply_time) =
        if working { (" CLAUDE  · this turn", s.turn_started_at) } else { (" CLAUDE", s.last_reply_at) };
    lines.push(Line::from(vec![
        sp(reply_label, bold(VIOLET)),
        sp(reply_time.map(|t| format!("  {} · {}", hm(t), ago(now, t))).unwrap_or_default(), faint()),
    ]));
    let reply = match (&live, working) {
        (Some(t), _) => plain(t),
        (None, true) => String::from("working…"),
        (None, false) => s.last_reply.clone().map(|r| plain(&r)).unwrap_or_default(),
    };
    // Earlier prompts go at the bottom when there's room.
    let earlier: Vec<&PromptEntry> = s.prompts.iter().rev().skip(1).take(6).collect();
    let earlier_h = if earlier.is_empty() || h < 16 { 0 } else { (earlier.len() + 2).min(h / 3) };
    let remaining = h.saturating_sub(lines.len() + earlier_h);
    let mut r_lines = wrap(&reply, width);
    if r_lines.is_empty() {
        r_lines.push("—".into());
    }
    if r_lines.len() > remaining && remaining > 0 {
        r_lines.truncate(remaining);
        if let Some(l) = r_lines.last_mut() {
            *l = fit(&format!("{l} …"), width);
        }
    }
    let reply_style = if working && live.is_none() {
        faint()
    } else if working {
        dim()
    } else {
        text()
    };
    for l in r_lines {
        lines.push(Line::from(vec![sp("  ", text()), sp(l, reply_style)]));
    }
    if earlier_h > 0 {
        while lines.len() + earlier_h < h {
            lines.push(Line::raw(""));
        }
        lines.push(Line::raw(""));
        lines.push(Line::from(sp(" EARLIER IN THIS SESSION", label())));
        for p in earlier.into_iter().take(earlier_h - 2) {
            lines.push(lr(
                vec![sp(format!("  {}  ", hm(p.at)), faint()), sp(first_line(&p.text).to_string(), dim())],
                vec![],
                width + 2,
            ));
        }
    }
    f.render_widget(Paragraph::new(lines), inner);
}

fn tools_panel(f: &mut Frame, area: Rect, now: Ms, s: &Session) {
    let block =
        panel("recent tools").title(Line::from(sp(format!(" {} total ", s.tool_calls), faint())).right_aligned());
    let inner = block.inner(area);
    f.render_widget(block, area);
    let width = inner.width as usize;
    let mut lines = Vec::new();
    let mut calls: Vec<(&ToolCall, bool)> = Vec::new();
    if let Some(t) = &s.current_tool {
        calls.push((t, true));
    }
    calls.extend(s.recent_tools.iter().map(|t| (t, false)));
    if calls.is_empty() {
        lines.push(Line::from(sp(" no tool calls yet", faint())));
    }
    for (t, running) in calls.into_iter().take(inner.height as usize) {
        let (glyph, gc) = if running {
            (spinner(now).to_string(), CYAN)
        } else if t.ok == Some(false) {
            ("✗".into(), RED)
        } else {
            ("✓".into(), GREEN)
        };
        let dur =
            if running { short_duration(now - t.started_at) } else { t.duration_ms.map(dur_ms).unwrap_or_default() };
        let name = fit(&tool_label(&t.name), 10);
        lines.push(lr(
            vec![
                sp(format!(" {} ", hm(t.started_at)), faint()),
                sp(format!("{glyph} "), Style::new().fg(gc)),
                sp(format!("{name:<10} "), Style::new().fg(if running { CYAN } else { TEXT })),
                sp(t.target.clone(), dim()),
            ],
            vec![sp(format!("{dur} "), faint())],
            width,
        ));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

fn files_panel(f: &mut Frame, area: Rect, files: &[FileTouch], total: usize) {
    let block =
        panel("files touched").title(Line::from(sp(format!(" {} ", plural(total, "file")), faint())).right_aligned());
    let inner = block.inner(area);
    f.render_widget(block, area);
    let width = inner.width as usize;
    let mut lines = Vec::new();
    if files.is_empty() {
        lines.push(Line::from(sp(" no edits yet", faint())));
    }
    for ft in files.iter().take(inner.height as usize) {
        let dir = std::path::Path::new(&ft.path).parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
        let name = basename(&ft.path);
        let mut left = vec![sp(" ", text())];
        if !dir.is_empty() {
            left.push(sp(format!("{dir}/"), faint()));
        }
        left.push(sp(name, text()));
        let right = vec![
            sp(format!("+{}", ft.added), Style::new().fg(GREEN)),
            sp(format!(" −{}", ft.removed), Style::new().fg(RED)),
            sp(format!(" ×{} ", ft.edits), faint()),
        ];
        lines.push(lr(left, right, width));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

// ---------------------------------------------------------------------------
// Right: one project

fn project_detail(f: &mut Frame, area: Rect, app: &App, root: &str) {
    let now = app.now;
    let Some(p) = app.project(root).cloned() else { return };
    let sessions: Vec<Session> = app.visible_sessions(root).into_iter().cloned().collect();
    let pc = project_color(root);
    let rows =
        Layout::vertical([Constraint::Length(6), Constraint::Length(4), Constraint::Length(6), Constraint::Min(5)])
            .split(area);

    let block = panel("project").border_style(Style::new().fg(BORDER_HI));
    let inner = block.inner(rows[0]);
    f.render_widget(block, rows[0]);
    let width = inner.width as usize;
    let mut lines = vec![lr(
        vec![sp(" ", text()), sp(p.name.clone(), bold(pc)), sp(format!("   {}", tildify(&p.root)), faint())],
        vec![sp(format!(" {} ", ago(now, p.last_activity)), faint())],
        width,
    )];
    match &p.git {
        Some(g) => {
            let mut l = vec![sp(" ", text())];
            l.extend(git_spans(g));
            let mut parts = Vec::new();
            if g.staged > 0 {
                parts.push(format!("{} staged", g.staged));
            }
            if g.modified > 0 {
                parts.push(format!("{} modified", g.modified));
            }
            if g.untracked > 0 {
                parts.push(format!("{} untracked", g.untracked));
            }
            if g.conflicts > 0 {
                parts.push(format!("{} conflicted", g.conflicts));
            }
            l.push(sp(
                format!("   {}", if parts.is_empty() { "working tree clean".to_string() } else { parts.join(" · ") }),
                faint(),
            ));
            lines.push(lr(l, vec![], width));
            if let Some(msg) = &g.last_commit {
                lines.push(lr(
                    vec![
                        sp(" last commit ", faint()),
                        sp(msg.clone(), text()),
                        sp(g.last_commit_at.map(|t| format!("  · {}", ago(now, t))).unwrap_or_default(), faint()),
                    ],
                    vec![],
                    width,
                ));
            }
        }
        None => lines.push(Line::from(sp(" not a git repository", faint()))),
    }
    f.render_widget(Paragraph::new(lines), inner);

    let live = sessions.iter().filter(|s| s.state.is_live()).count();
    let needs = sessions.iter().filter(|s| matches!(s.state, SessionState::NeedsYou { .. })).count();
    let tools: u32 = sessions.iter().map(|s| s.tool_calls).sum();
    let turns: u32 = sessions.iter().map(|s| s.turns).sum();
    let mut files: Vec<FileTouch> = Vec::new();
    for s in &sessions {
        for ft in &s.files_touched {
            match files.iter_mut().find(|x| x.path == ft.path) {
                Some(x) => {
                    x.edits += ft.edits;
                    x.added += ft.added;
                    x.removed += ft.removed;
                    x.last_at = x.last_at.max(ft.last_at);
                }
                None => files.push(ft.clone()),
            }
        }
    }
    files.sort_by_key(|f| -f.last_at);
    let tiles: Vec<(&str, String, Color, String)> = vec![
        ("SESSIONS", sessions.len().to_string(), TEXT, String::new()),
        ("LIVE", live.to_string(), CYAN, String::new()),
        ("NEEDS YOU", needs.to_string(), if needs > 0 { AMBER } else { FAINT }, String::new()),
        ("TURNS", turns.to_string(), TEXT, String::new()),
        ("TOOL CALLS", tools.to_string(), CYAN, String::new()),
        ("FILES", files.len().to_string(), VIOLET, String::new()),
    ];
    stat_tiles(f, rows[1], &tiles);

    let refs: Vec<&Session> = sessions.iter().collect();
    activity_panel(f, rows[2], now, &refs, vec![], None);

    let bottom =
        Layout::horizontal([Constraint::Percentage(55), Constraint::Length(1), Constraint::Min(0)]).split(rows[3]);
    let block = panel("sessions");
    let inner = block.inner(bottom[0]);
    f.render_widget(block, bottom[0]);
    let width = inner.width as usize;
    let mut lines = Vec::new();
    for s in &sessions {
        let sc = live_state_color(&s.state, now);
        lines.push(lr(
            vec![sp(format!(" {} ", state_glyph(&s.state, now)), bold(sc)), sp(s.display_title(), text())],
            vec![sp(s.state.label(), bold(sc)), sp(format!(" {:>5} ", short_duration(now - s.state_since)), dim())],
            width,
        ));
        let mut detail = vec![sp("   ", text())];
        detail.extend(session_detail_line(s, now));
        lines.push(lr(
            detail,
            vec![sp(s.model.as_deref().map(short_model).unwrap_or_default(), faint()), sp(" ", text())],
            width,
        ));
    }
    f.render_widget(Paragraph::new(lines), inner);
    let n = files.len();
    files_panel(f, bottom[2], &files, n);
}

// ---------------------------------------------------------------------------
// Bottom: activity feed

fn feed(f: &mut Frame, area: Rect, app: &App) {
    let now = app.now;
    let Some(snap) = &app.snap else { return };
    let live_dot = match app.conn {
        Conn::Live | Conn::Demo => sp(" ● live ", Style::new().fg(lerp(GREEN, TEAL, pulse(now, 2000)))),
        _ => sp(" ○ paused ", faint()),
    };
    let block = panel("activity feed").title(Line::from(live_dot).right_aligned());
    let inner = block.inner(area);
    f.render_widget(block, area);
    let width = inner.width as usize;
    let titles: std::collections::HashMap<&str, String> =
        snap.sessions.iter().map(|s| (s.id.as_str(), s.display_title())).collect();
    let name_w = snap.projects.iter().map(|p| w(&p.name)).max().unwrap_or(8).clamp(6, 18);
    let title_w = if width > 140 { 30 } else { 20 };
    let mut lines = Vec::new();
    for e in snap.feed.iter().rev().take(inner.height as usize) {
        let (glyph, color) = feed_style(e.kind);
        let name = fit(&basename(&e.project_root), name_w);
        let title = titles.get(e.session_id.as_str()).map(|t| fit(t, title_w)).unwrap_or_default();
        let text_style = match e.kind {
            FeedKind::Permission | FeedKind::Question => Style::new().fg(AMBER),
            FeedKind::Error => Style::new().fg(RED),
            FeedKind::Done => text(),
            FeedKind::Start | FeedKind::End | FeedKind::Info => faint(),
            _ => dim(),
        };
        lines.push(lr(
            vec![
                sp(format!(" {} ", clock(e.at)), faint()),
                sp(format!(" {glyph} "), bold(color)),
                sp(format!("{name:<name_w$}  "), bold(project_color(&e.project_root))),
                sp(format!("{title:<title_w$}  "), faint()),
                sp(e.text.clone(), text_style),
            ],
            vec![sp(format!("{} ", short_duration(now - e.at)), faint())],
            width,
        ));
    }
    if lines.is_empty() {
        lines.push(Line::from(sp(" nothing yet", faint())));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

// ---------------------------------------------------------------------------
// Footer, help, toast, empty state

fn footer(f: &mut Frame, area: Rect, app: &App) {
    let key = |k: &str| sp(format!(" {k} "), Style::new().fg(TEXT).bg(RAISED).add_modifier(Modifier::BOLD));
    let lbl = |l: &str| sp(format!(" {l}  "), faint());
    let left = if app.filtering {
        vec![key("type"), lbl("to filter"), key("⏎"), lbl("keep"), key("esc"), lbl("clear")]
    } else {
        vec![
            key("↑↓"),
            lbl("move"),
            key("⇥"),
            lbl("project"),
            key("⏎"),
            lbl(&format!("open in {}", editor_name(app))),
            key("n"),
            lbl("next alert"),
            key("/"),
            lbl("filter"),
            key("e"),
            lbl(if app.show_ended { "hide ended" } else { "show ended" }),
            key("x"),
            lbl("dismiss"),
            key("?"),
            lbl("help"),
            key("q"),
            lbl("quit"),
        ]
    };
    let right = vec![sp("CerealNotFound/aulora ", faint())];
    f.render_widget(Paragraph::new(lr(left, right, area.width as usize)).style(Style::new().bg(PANEL)), area);
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let w_ = width.min(area.width);
    let h_ = height.min(area.height);
    Rect { x: area.x + (area.width - w_) / 2, y: area.y + (area.height - h_) / 2, width: w_, height: h_ }
}

const LOGO: [&str; 3] = ["┌─┐┬ ┬┬  ┌─┐┬─┐┌─┐", "├─┤│ ││  │ │├┬┘├─┤", "┴ ┴└─┘┴─┘└─┘┴└─┴ ┴"];

fn logo_lines(now: Ms) -> Vec<Line<'static>> {
    let shift = (now as f32 / 4000.0).sin() * 0.15;
    LOGO.iter()
        .map(|row| {
            let n = row.chars().count().max(2) - 1;
            Line::from(
                row.chars()
                    .enumerate()
                    .map(|(i, c)| {
                        let t = (i as f32 / n as f32 + shift).clamp(0.0, 1.0);
                        Span::styled(c.to_string(), Style::new().fg(aurora(t)).add_modifier(Modifier::BOLD))
                    })
                    .collect::<Vec<_>>(),
            )
            .centered()
        })
        .collect()
}

fn help(f: &mut Frame, area: Rect, app: &App) {
    let r = centered(area, 72, 30);
    f.render_widget(Clear, r);
    let block = panel("help").border_style(Style::new().fg(BORDER_HI)).style(Style::new().bg(RAISED));
    let inner = block.inner(r);
    f.render_widget(block, r);
    let mut lines = logo_lines(app.now);
    lines.push(Line::from(sp("every Claude Code session, one screen", dim())).centered());
    lines.push(Line::raw(""));
    let row = |k: &str, d: &str| Line::from(vec![sp(format!("  {k:<14}"), bold(TEXT)), sp(d.to_string(), dim())]);
    for (k, d) in [
        ("↑ ↓  j k", "move between projects and sessions"),
        ("⇥  ⇧⇥", "jump to next / previous project"),
        ("g  G", "first / last"),
        ("⏎  o", "open the project in your editor"),
        ("n", "cycle through sessions that need you"),
        ("/", "filter by project, title or prompt"),
        ("e", "show or hide ended sessions"),
        ("x  X", "dismiss this ended session / all ended"),
        ("q  ctrl-c", "quit (the daemon keeps running)"),
    ] {
        lines.push(row(k, d));
    }
    lines.push(Line::raw(""));
    let legend =
        |g: &str, c: Color, d: &str| Line::from(vec![sp(format!("  {g:<3}"), bold(c)), sp(d.to_string(), dim())]);
    lines.push(legend("◆", AMBER, "needs you: permission, question or plan review"));
    lines.push(legend(spinner(app.now), CYAN, "working"));
    lines.push(legend("✓", GREEN, "finished, your turn"));
    lines.push(legend("○", DIM, "idle"));
    lines.push(legend("✗", RED, "turn failed"));
    lines.push(legend("·", FAINT, "ended"));
    lines.push(Line::raw(""));
    lines.push(Line::from(sp("press any key to close", faint())).centered());
    f.render_widget(Paragraph::new(lines), inner);
}

fn toast(f: &mut Frame, area: Rect, app: &App) {
    let Some((msg, at)) = &app.toast else { return };
    if app.now - at > 2600 {
        return;
    }
    let width = (w(msg) + 4).min(area.width as usize) as u16;
    let r = Rect {
        x: area.x + area.width.saturating_sub(width + 2),
        y: area.y + area.height.saturating_sub(2),
        width,
        height: 1,
    };
    f.render_widget(Clear, r);
    f.render_widget(
        Paragraph::new(Line::from(vec![sp(" ✦ ", bold(TEAL)), sp(format!("{msg} "), text())]))
            .style(Style::new().bg(SELECT)),
        r,
    );
}

fn empty_state(f: &mut Frame, area: Rect, app: &App) {
    let r = centered(area, 76, 20);
    let mut lines = logo_lines(app.now);
    lines.push(Line::raw(""));
    lines.push(Line::from(sp("Every Claude Code session across your projects, in one place.", dim())).centered());
    lines.push(Line::raw(""));
    match &app.conn {
        Conn::Offline(why) => {
            lines.push(Line::from(sp(format!("Can't reach the aulora daemon: {why}"), bold(RED))).centered());
            lines.push(Line::from(sp("Retrying… or run `aulora daemon start`", faint())).centered());
        }
        Conn::Connecting => {
            lines.push(Line::from(sp(format!("{} starting the daemon…", spinner(app.now)), dim())).centered());
        }
        _ if !app.hooks_installed => {
            lines.push(Line::from(sp("Two steps to get started:", bold(TEXT))).centered());
            lines.push(Line::raw(""));
            lines.push(
                Line::from(vec![
                    sp("1  ", bold(TEAL)),
                    sp("aulora hooks install", bold(TEXT)),
                    sp("   connect Claude Code", faint()),
                ])
                .centered(),
            );
            lines.push(
                Line::from(vec![
                    sp("2  ", bold(VIOLET)),
                    sp("claude", bold(TEXT)),
                    sp("                 in any project", faint()),
                ])
                .centered(),
            );
        }
        _ => {
            lines.push(
                Line::from(vec![
                    sp(format!("{} ", spinner(app.now)), Style::new().fg(TEAL)),
                    sp("Waiting for Claude Code sessions", bold(TEXT)),
                ])
                .centered(),
            );
            lines.push(Line::raw(""));
            lines.push(
                Line::from(sp("Start `claude` in any project, or send a prompt in one that's", faint())).centered(),
            );
            lines.push(Line::from(sp("already open. It will show up here within a second.", faint())).centered());
        }
    }
    lines.push(Line::raw(""));
    lines.push(
        Line::from(vec![
            sp("try ", faint()),
            sp("aulora demo", bold(TEXT)),
            sp(" to see it with sample data", faint()),
        ])
        .centered(),
    );
    f.render_widget(Paragraph::new(lines), r);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_and_fits() {
        assert_eq!(wrap("hello world foo", 11), vec!["hello world", "foo"]);
        assert_eq!(fit("abcdef", 4), "abc…");
        assert_eq!(spark(&[0, 4, 8], 3), "▁▅█");
        let l = lr(vec![sp("left side text", text())], vec![sp("R", text())], 10);
        assert_eq!(spans_width(&l.spans), 10);
    }

    #[test]
    fn rel() {
        assert_eq!(aulora_core::util::rel_path("/a/b", "/a"), "b");
    }
}
