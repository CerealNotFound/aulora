//! Keys and mouse: everything the user does goes through here.

use std::time::{Duration, Instant};

use aulora_core::model::PaneKind;
use aulora_core::proto::{self, ClientMsg, NewPane, ServerMsg};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use crate::app::{App, Confirm, Conn, Focus, HelpState, Hit, RowRef, Sort, Status, Tab};
use crate::keymap::{self, Action};
use crate::{Ctx, Options, client, keys, theme};

/// Something that needs a yes before it happens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmAction {
    KillPane(String),
}

impl ConfirmAction {
    pub fn verb(&self) -> &'static str {
        match self {
            ConfirmAction::KillPane(_) => "kill",
        }
    }
}

/// Turns wheel events into lines: 3 per lone notch, about 1 per event in a
/// fast trackpad stream, speeding up gently, fractions carried (helix, codex).
#[derive(Default)]
pub struct Wheel {
    last: Option<Instant>,
    dir: i8,
    acc: f32,
    streak: u32,
}

impl Wheel {
    pub fn feed(&mut self, dir: i8) -> isize {
        let now = Instant::now();
        let gap = self.last.map_or(Duration::MAX, |t| now - t);
        self.last = Some(now);
        if dir != self.dir {
            self.dir = dir;
            self.acc = 0.0;
            self.streak = 0;
        }
        self.streak = if gap < Duration::from_millis(35) { self.streak + 1 } else { 0 };
        let per = if self.streak == 0 { 3.0 } else { (1.0 + self.streak as f32 * 0.1).min(6.0) };
        self.acc += dir as f32 * per;
        let whole = self.acc.trunc();
        self.acc -= whole;
        whole as isize
    }
}

/// Everything the main loop needs to handle input.
#[derive(Default)]
pub struct Input {
    pub wheel: Wheel,
    last_click: Option<(Instant, u16, u16)>,
    last_ctrl_q: Option<Instant>,
}

fn conv_height(app: &App) -> usize {
    app.conv_height.max(3)
}

/// Returns true to quit.
pub fn on_key(app: &mut App, key: KeyEvent, opts: &Options, ctx: &Ctx, input: &mut Input) -> bool {
    if app.toast.as_ref().is_some_and(|t| t.error) {
        app.toast = None;
    }
    if app.confirm.is_some() {
        confirm_key(app, key, opts);
        return false;
    }
    if app.theme_picker.is_some() {
        picker_key(app, key);
        return false;
    }
    if app.help.is_some() {
        return help_key(app, key, opts, ctx, input);
    }
    if app.typing {
        terminal_key(app, key, ctx, input);
        return false;
    }
    if app.filtering {
        match key.code {
            KeyCode::Esc => {
                app.filter.clear();
                app.filtering = false;
            }
            KeyCode::Enter => app.filtering = false,
            KeyCode::Backspace => {
                app.filter.pop();
            }
            KeyCode::Char(c) => app.filter.push(c),
            _ => {}
        }
        app.move_selection(0);
        return false;
    }
    if app.searching {
        search_key(app, key);
        return false;
    }
    // Ctrl-Q right after leaving a terminal sends Ctrl-Q through (tmux's double prefix).
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    if ctrl
        && matches!(key.code, KeyCode::Char('q'))
        && input.last_ctrl_q.is_some_and(|t| t.elapsed() < Duration::from_millis(600))
        && let Some((id, _)) = app.attached.clone()
    {
        ctx.send(ClientMsg::Input { pane_id: id, data: proto::encode(&[0x11]) });
        app.typing = true;
        return false;
    }
    match keymap::lookup(app.scope(), &key) {
        Some(action) => perform(app, action, opts, ctx),
        None => false,
    }
}

/// Run an action. Returns true to quit.
pub fn perform(app: &mut App, action: Action, opts: &Options, ctx: &Ctx) -> bool {
    let scope = app.scope();
    let h = conv_height(app);
    match action {
        Action::Up | Action::Down => {
            let d = if action == Action::Up { -1 } else { 1 };
            match app.focus {
                Focus::Sidebar => app.move_selection(d),
                Focus::Main => with_conv(app, |c, h| c.move_cursor(d as isize, h)),
            }
        }
        Action::PageUp | Action::PageDown => {
            let d: isize = if action == Action::PageUp { -1 } else { 1 };
            match scope {
                keymap::Scope::Conversation => with_conv(app, |c, h| c.move_cursor(d * (h as isize - 2).max(1), h)),
                keymap::Scope::Pane => scroll_pane(app, d * (h as isize / 2).max(1)),
                _ => app.move_selection(d as i32 * 5),
            }
        }
        Action::Top | Action::Bottom => {
            let top = action == Action::Top;
            match scope {
                keymap::Scope::Conversation => {
                    with_conv(app, |c, h| if top { c.go_top(h) } else { c.go_bottom() });
                }
                keymap::Scope::Pane => {
                    if let Some(t) = app.attached.clone().and_then(|(id, _)| app.terms.get_mut(&id)) {
                        t.reset_scroll();
                    }
                }
                _ => app.select_edge(!top),
            }
        }
        Action::FocusNext | Action::FocusPrev => {
            app.typing = false;
            app.focus = match app.focus {
                Focus::Sidebar if app.selected.is_some() => Focus::Main,
                _ => Focus::Sidebar,
            };
        }
        Action::Tab(n) => {
            if let Some(tab) = app.tabs().get(n as usize).copied() {
                app.set_tab(tab);
            }
        }
        Action::NextTab => app.cycle_tab(1),
        Action::PrevTab => app.cycle_tab(-1),
        Action::Open => open(app, opts),
        Action::Back => {
            app.focus = Focus::Sidebar;
            app.typing = false;
        }
        Action::NewShell => new_pane(app, opts, ctx, PaneKind::Shell),
        Action::NewClaude => new_pane(app, opts, ctx, PaneKind::Claude),
        Action::NextAlert => {
            if !app.next_attention() {
                app.toast("Nothing needs you right now");
            }
        }
        Action::Filter => app.filtering = true,
        Action::Zoom => app.zoom = !app.zoom,
        Action::OpenEditor => open_in_editor(app),
        Action::Close => close(app, opts),
        Action::DismissEnded => {
            if app.conn == Conn::Demo {
                app.toast("Cleared ended sessions (demo)");
            } else {
                client::command(&opts.socket, &ClientMsg::ClearEnded);
                app.toast("Cleared ended sessions");
            }
        }
        Action::Restart => restart(app, opts),
        Action::Themes => {
            app.theme_before = theme::current_index();
            app.theme_picker = Some(theme::current_index());
        }
        Action::Help => app.help = Some(HelpState { query: String::new(), selected: 0 }),
        Action::Quit => return true,
        Action::ToggleHistory => {
            app.show_history = !app.show_history;
            app.move_selection(0);
            let n = app.history_count();
            app.toast(if app.show_history {
                format!("Showing {n} past sessions")
            } else {
                "Hiding past sessions".into()
            });
        }
        Action::ToggleEnded => {
            app.show_ended = !app.show_ended;
            app.move_selection(0);
            app.toast(if app.show_ended { "Showing ended sessions" } else { "Hiding ended sessions" });
        }
        Action::ToggleSort => {
            app.sort = match app.sort {
                Sort::Priority => Sort::Grouped,
                Sort::Grouped => Sort::Priority,
            };
        }
        Action::PrevPrompt | Action::NextPrompt => {
            let fwd = action == Action::NextPrompt;
            with_conv(app, |c, h| c.jump_prompt(fwd, h));
        }
        Action::Copy => {
            let text = app.selected_session_id().and_then(|id| {
                let c = app.conversation(&id)?;
                c.entries.get(c.cursor.entry).map(|e| e.plain())
            });
            if let Some(text) = text {
                copy_to_clipboard(&text);
                app.toast("Copied");
            }
        }
        Action::Search => {
            app.searching = true;
            app.query.clear();
        }
        Action::NextMatch => {
            let mut found = None;
            with_conv(app, |c, h| {
                if !c.search.is_empty() {
                    found = Some(c.find_next(h));
                }
            });
            match found {
                Some(false) => app.toast("No more matches"),
                Some(true) => {}
                None => return perform(app, Action::NextAlert, opts, ctx),
            }
        }
        Action::ToggleMouse => {
            app.mouse = !app.mouse;
            set_mouse(app.mouse);
            app.toast(if app.mouse { "Mouse on" } else { "Mouse off: the terminal can select text" });
        }
    }
    let _ = h;
    false
}

pub fn set_mouse(on: bool) {
    use ratatui::crossterm::event::{DisableMouseCapture, EnableMouseCapture};
    let mut out = std::io::stdout();
    let _ = if on {
        ratatui::crossterm::execute!(out, EnableMouseCapture)
    } else {
        ratatui::crossterm::execute!(out, DisableMouseCapture)
    };
}

fn with_conv(app: &mut App, f: impl FnOnce(&mut crate::conv::Conversation, usize)) {
    let h = conv_height(app);
    if let Some(id) = app.selected_session_id()
        && let Some(c) = app.conversation(&id)
    {
        f(c, h);
    }
}

fn scroll_pane(app: &mut App, delta: isize) {
    if let Some((id, _)) = app.attached.clone()
        && let Some(t) = app.terms.get_mut(&id)
    {
        t.scroll_by(-delta as i32);
    }
}

fn open(app: &mut App, opts: &Options) {
    let Some(sel) = app.selected.clone() else { return };
    match app.focus {
        Focus::Sidebar => {
            app.focus = Focus::Main;
            if let RowRef::Pane(id) = &sel
                && app.current_tab() == Tab::Terminal
            {
                start_typing(app, opts, id);
            }
        }
        Focus::Main => match app.current_tab() {
            Tab::Terminal => {
                if let RowRef::Pane(id) = &sel {
                    start_typing(app, opts, id);
                }
            }
            Tab::Conversation => with_conv(app, |c, _| {
                c.toggle_at_cursor();
            }),
            Tab::Overview => open_in_editor(app),
        },
    }
}

fn start_typing(app: &mut App, _opts: &Options, id: &str) {
    if app.conn == Conn::Demo {
        app.toast("Demo panes are pictures; run aulora for real ones");
        return;
    }
    match app.pane(id) {
        Some(p) if p.is_running() => {
            app.focus = Focus::Main;
            app.typing = true;
        }
        Some(_) => app.toast("The process has exited. Press r to restart it."),
        None => {}
    }
}

fn terminal_key(app: &mut App, key: KeyEvent, ctx: &Ctx, input: &mut Input) {
    let Some((id, _)) = app.attached.clone() else {
        app.typing = false;
        return;
    };
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    if ctrl && matches!(key.code, KeyCode::Char('q') | KeyCode::Char('Q')) {
        app.typing = false;
        input.last_ctrl_q = Some(Instant::now());
        return;
    }
    let Some(t) = app.terms.get_mut(&id) else { return };
    // Shift+PgUp/PgDn scroll back locally, like most terminals.
    if shift && matches!(key.code, KeyCode::PageUp | KeyCode::PageDown) {
        let page = t.parser.screen().size().0.get() as i32 / 2;
        t.scroll_by(if key.code == KeyCode::PageUp { page } else { -page });
        return;
    }
    // Typing snaps back to the live bottom (Herdr).
    if t.scrolled() > 0 {
        t.reset_scroll();
    }
    let app_cursor = t.parser.screen().application_cursor();
    if let Some(bytes) = keys::encode(key, app_cursor) {
        ctx.send(ClientMsg::Input { pane_id: id, data: proto::encode(&bytes) });
    }
}

pub fn paste(app: &mut App, text: &str, ctx: &Ctx) {
    if app.typing {
        let Some((id, _)) = app.attached.clone() else { return };
        let bracketed = app.terms.get(&id).is_some_and(|t| t.parser.screen().bracketed_paste());
        ctx.send(ClientMsg::Input { pane_id: id, data: proto::encode(&keys::paste(text, bracketed)) });
    } else if app.filtering {
        app.filter.push_str(text.lines().next().unwrap_or(""));
    } else if app.searching {
        app.query.push_str(text.lines().next().unwrap_or(""));
    }
}

fn search_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => app.searching = false,
        KeyCode::Backspace => {
            app.query.pop();
        }
        KeyCode::Enter => {
            app.searching = false;
            let q = app.query.clone();
            let mut found = false;
            with_conv(app, |c, h| {
                c.search = q;
                found = c.find_next(h);
            });
            if !found {
                app.toast("No matches");
            }
        }
        KeyCode::Char(c) => app.query.push(c),
        _ => {}
    }
}

fn help_key(app: &mut App, key: KeyEvent, opts: &Options, ctx: &Ctx, _input: &mut Input) -> bool {
    let Some(h) = app.help.as_mut() else { return false };
    let rows = crate::ui::help_rows(&h.query);
    match key.code {
        KeyCode::Esc => {
            if h.query.is_empty() {
                app.help = None;
            } else {
                h.query.clear();
                h.selected = 0;
            }
        }
        KeyCode::Up => h.selected = h.selected.saturating_sub(1),
        KeyCode::Down => h.selected = (h.selected + 1).min(rows.len().saturating_sub(1)),
        KeyCode::Backspace => {
            h.query.pop();
            h.selected = 0;
        }
        KeyCode::Enter => {
            let action = rows.get(h.selected).map(|r| r.3);
            app.help = None;
            if let Some(a) = action
                && a != Action::Help
            {
                return perform(app, a, opts, ctx);
            }
        }
        KeyCode::Char(c) => {
            h.query.push(c);
            h.selected = 0;
        }
        _ => {}
    }
    false
}

fn picker_key(app: &mut App, key: KeyEvent) {
    let Some(sel) = app.theme_picker else { return };
    let n = theme::THEMES.len();
    match key.code {
        KeyCode::Down | KeyCode::Char('j') => {
            app.theme_picker = Some((sel + 1) % n);
            theme::set((sel + 1) % n);
        }
        KeyCode::Up | KeyCode::Char('k') => {
            app.theme_picker = Some((sel + n - 1) % n);
            theme::set((sel + n - 1) % n);
        }
        KeyCode::Enter => {
            let name = theme::get().name;
            crate::save_theme(name);
            app.theme_picker = None;
            app.toast(format!("Theme: {name}"));
        }
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('t') => {
            theme::set(app.theme_before);
            app.theme_picker = None;
        }
        _ => {}
    }
}

fn confirm_key(app: &mut App, key: KeyEvent, opts: &Options) {
    let Some(c) = app.confirm.as_mut() else { return };
    match key.code {
        KeyCode::Left | KeyCode::Right | KeyCode::Tab | KeyCode::BackTab | KeyCode::Char('h') | KeyCode::Char('l') => {
            c.on_action = !c.on_action;
        }
        KeyCode::Char('y') => run_confirmed(app, opts),
        KeyCode::Enter if c.on_action => run_confirmed(app, opts),
        KeyCode::Enter | KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('q') => app.confirm = None,
        _ => {}
    }
}

fn run_confirmed(app: &mut App, opts: &Options) {
    let Some(c) = app.confirm.take() else { return };
    match c.action {
        ConfirmAction::KillPane(id) => pane_action(app, opts, ClientMsg::KillPane { pane_id: id }, "Stopped"),
    }
}

fn close(app: &mut App, opts: &Options) {
    let demo = app.conn == Conn::Demo;
    match app.selected.clone() {
        Some(RowRef::Pane(id)) => {
            let Some(p) = app.pane(&id).cloned() else { return };
            if demo {
                app.toast("Demo panes can't be closed");
            } else if p.is_running() {
                let what = match app.status(&RowRef::Pane(id.clone())) {
                    Status::Working => "It is working right now; whatever it's doing stops.",
                    Status::Blocked => "It is waiting for you; whatever it was doing stops.",
                    _ => "Its process gets SIGHUP. The last screen stays until you close it.",
                };
                app.confirm = Some(Confirm {
                    title: format!("kill {}?", p.name),
                    body: what.into(),
                    action: ConfirmAction::KillPane(id),
                    on_action: false,
                });
            } else {
                pane_action(app, opts, ClientMsg::ClosePane { pane_id: id }, "Closed");
            }
        }
        Some(RowRef::Session(id)) => {
            let ended = app.session(&id).is_some_and(|s| !s.state.is_live());
            if !ended {
                app.toast("This session runs outside aulora; end it where it runs");
            } else if demo {
                app.toast("Dismissed (demo)");
            } else {
                client::command(&opts.socket, &ClientMsg::Dismiss { session_id: id });
                app.toast("Dismissed");
            }
        }
        _ => {}
    }
}

fn restart(app: &mut App, opts: &Options) {
    let Some(p) = app.selected_pane().cloned() else { return };
    if app.conn == Conn::Demo {
        return;
    }
    if p.is_running() {
        app.toast("Still running. Kill it first with x.");
    } else {
        pane_action(app, opts, ClientMsg::RestartPane { pane_id: p.id }, "Restarted");
    }
}

fn pane_action(app: &mut App, opts: &Options, msg: ClientMsg, done: &str) {
    match client::command(&opts.socket, &msg) {
        Some(ServerMsg::Error { message }) => app.error(message),
        Some(_) => app.toast(done.to_string()),
        None => app.error("The daemon didn't answer"),
    }
}

pub fn new_pane(app: &mut App, opts: &Options, ctx: &Ctx, kind: PaneKind) {
    let label = if kind == PaneKind::Claude { "Claude Code" } else { "a shell" };
    if app.conn == Conn::Demo {
        app.toast(format!("Would start {label} (demo)"));
        return;
    }
    let cwd = app.selected_root().or_else(|| std::env::current_dir().ok().map(|d| d.to_string_lossy().to_string()));
    let Some(cwd) = cwd else { return };
    let (cols, rows) = match &ctx.sent_size {
        Some((_, c, r)) => (*c, *r),
        None => crate::terminal_size()
            .map(|(w, h)| (w.saturating_sub(if w >= 90 { 40 } else { 2 }), h.saturating_sub(4)))
            .unwrap_or((100, 30)),
    };
    let req = NewPane { cwd, kind, args: vec![], name: None, cols: cols.max(20), rows: rows.max(5) };
    match client::command(&opts.socket, &ClientMsg::NewPane(req)) {
        Some(ServerMsg::PaneCreated { pane_id }) => {
            app.pending_select = Some((pane_id, true));
            app.toast(format!("Started {label}"));
        }
        Some(ServerMsg::Error { message }) => app.error(format!("Couldn't start {label}: {message}")),
        _ => app.error("The daemon didn't answer"),
    }
}

pub fn open_in_editor(app: &mut App) {
    let Some(root) = app.selected_root() else { return };
    let editor = app.editor.clone();
    let name = aulora_core::util::basename(&root);
    if app.conn == Conn::Demo {
        app.toast(format!("Would open {name} in {editor} (demo)"));
        return;
    }
    match std::process::Command::new(&editor)
        .arg(&root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(_) => app.toast(format!("Opened {name} in {editor}")),
        Err(e) => app.error(format!("Couldn't run `{editor}`: {e}. Set AULORA_EDITOR.")),
    }
}

/// Copy through the terminal (OSC 52), which works over SSH too.
fn copy_to_clipboard(text: &str) {
    use std::io::Write;
    let mut out = std::io::stdout();
    let _ = write!(out, "\x1b]52;c;{}\x07", proto::encode(text.as_bytes()));
    let _ = out.flush();
}

// ---------------------------------------------------------------------------
// Mouse

/// Encode a mouse event for a program that asked for mouse reporting.
fn mouse_bytes(screen: &vt100::Screen, button: u8, col: u16, row: u16, release: bool) -> Vec<u8> {
    let (x, y) = (col + 1, row + 1);
    match screen.mouse_protocol_encoding() {
        vt100::MouseProtocolEncoding::Sgr => {
            format!("\x1b[<{button};{x};{y}{}", if release { 'm' } else { 'M' }).into_bytes()
        }
        _ => {
            let b = if release { 3 } else { button };
            vec![0x1b, b'[', b'M', 32 + b, (32 + x.min(223)) as u8, (32 + y.min(223)) as u8]
        }
    }
}

pub fn on_mouse(app: &mut App, ev: MouseEvent, opts: &Options, ctx: &Ctx, input: &mut Input) {
    let Some((rect, hit)) = app.hit(ev.column, ev.row) else { return };
    let (col, row) = (ev.column, ev.row);
    match ev.kind {
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
            let dir: i8 = if ev.kind == MouseEventKind::ScrollUp { -1 } else { 1 };
            let lines = input.wheel.feed(dir);
            if lines == 0 {
                return;
            }
            match hit {
                Hit::Conversation(id) => {
                    let h = conv_height(app);
                    if let Some(c) = app.conversation(&id) {
                        c.scroll_by(lines, h);
                    }
                }
                Hit::Terminal(id) => wheel_pane(app, &id, lines, col - rect.x, row - rect.y, ctx),
                Hit::Row(_) | Hit::Sidebar | Hit::SortToggle => {
                    app.sidebar_scroll = (app.sidebar_scroll as isize + lines).max(0) as usize;
                }
                _ => {}
            }
        }
        MouseEventKind::Down(MouseButton::Left) => {
            let double = input
                .last_click
                .is_some_and(|(t, c, r)| t.elapsed() < Duration::from_millis(350) && c == col && r == row);
            input.last_click = Some((Instant::now(), col, row));
            match hit {
                Hit::Row(r) => {
                    let was = app.selected.as_ref() == Some(&r);
                    app.select(r);
                    app.focus = Focus::Sidebar;
                    if double || was {
                        open(app, opts);
                    }
                }
                Hit::SortToggle => {
                    perform(app, Action::ToggleSort, opts, ctx);
                }
                Hit::NewClaude => new_pane(app, opts, ctx, PaneKind::Claude),
                Hit::NewShell => new_pane(app, opts, ctx, PaneKind::Shell),
                Hit::Tab(t) => {
                    app.set_tab(t);
                    app.focus = Focus::Main;
                }
                Hit::Terminal(id) => {
                    let wants_mouse = app
                        .terms
                        .get(&id)
                        .is_some_and(|t| t.parser.screen().mouse_protocol_mode() != vt100::MouseProtocolMode::None);
                    if app.typing && wants_mouse {
                        forward_mouse(app, &id, 0, col - rect.x, row - rect.y, false, ctx);
                    } else {
                        start_typing(app, opts, &id);
                    }
                }
                Hit::Conversation(id) => {
                    app.focus = Focus::Main;
                    let h = conv_height(app);
                    if let Some(c) = app.conversation(&id) {
                        let before = c.cursor_row();
                        c.click((row - rect.y) as usize, h);
                        if c.cursor_row() == before || double {
                            c.toggle_at_cursor();
                        }
                    }
                }
                Hit::Main => {
                    app.focus = Focus::Main;
                }
                Hit::Sidebar => {
                    app.focus = Focus::Sidebar;
                    app.typing = false;
                }
            }
        }
        MouseEventKind::Up(MouseButton::Left) | MouseEventKind::Drag(MouseButton::Left) => {
            if let Hit::Terminal(id) = hit
                && app.typing
            {
                let release = matches!(ev.kind, MouseEventKind::Up(_));
                let button = if release { 0 } else { 32 };
                forward_mouse(app, &id, button, col - rect.x, row - rect.y, release, ctx);
            }
        }
        _ => {}
    }
}

fn forward_mouse(app: &App, id: &str, button: u8, col: u16, row: u16, release: bool, ctx: &Ctx) {
    let Some(t) = app.terms.get(id) else { return };
    let screen = t.parser.screen();
    if screen.mouse_protocol_mode() == vt100::MouseProtocolMode::None {
        return;
    }
    let bytes = mouse_bytes(screen, button, col, row, release);
    ctx.send(ClientMsg::Input { pane_id: id.to_string(), data: proto::encode(&bytes) });
}

/// The wheel over a pane: to the program if it wants the mouse, arrow keys
/// for a full-screen program, our own scrollback otherwise (xterm.js, Herdr).
fn wheel_pane(app: &mut App, id: &str, lines: isize, col: u16, row: u16, ctx: &Ctx) {
    let Some(t) = app.terms.get_mut(id) else { return };
    let screen = t.parser.screen();
    let n = lines.unsigned_abs();
    if screen.mouse_protocol_mode() != vt100::MouseProtocolMode::None {
        let button = if lines < 0 { 64 } else { 65 };
        let mut bytes = Vec::new();
        for _ in 0..n {
            bytes.extend(mouse_bytes(screen, button, col, row, false));
        }
        ctx.send(ClientMsg::Input { pane_id: id.to_string(), data: proto::encode(&bytes) });
    } else if screen.alternate_screen() {
        let app_cursor = screen.application_cursor();
        let arrow = match (lines < 0, app_cursor) {
            (true, true) => "\x1bOA",
            (true, false) => "\x1b[A",
            (false, true) => "\x1bOB",
            (false, false) => "\x1b[B",
        };
        ctx.send(ClientMsg::Input { pane_id: id.to_string(), data: proto::encode(arrow.repeat(n).as_bytes()) });
    } else {
        t.scroll_by(-lines as i32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lone_notch_scrolls_three_lines() {
        let mut w = Wheel::default();
        assert_eq!(w.feed(1), 3);
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(w.feed(-1), -3, "direction change resets");
    }

    #[test]
    fn fast_streams_scroll_finely_then_speed_up() {
        let mut w = Wheel::default();
        w.feed(1);
        let fast: Vec<isize> = (0..20).map(|_| w.feed(1)).collect();
        assert_eq!(fast[0], 1, "streams start at about a line per event");
        assert!(fast.iter().sum::<isize>() > 20, "and speed up a little");
    }
}
