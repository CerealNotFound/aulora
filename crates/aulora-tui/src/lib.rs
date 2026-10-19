//! The aulora dashboard.

mod app;
mod attach;
mod client;
mod demo;
mod keys;
mod term;
mod theme;
mod ui;

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use anyhow::Result;
use aulora_core::model::{PaneKind, Snapshot, now_ms};
use aulora_core::proto::{self, ClientMsg, NewPane, ServerMsg};
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::{execute, terminal};

use crate::app::{App, Conn, Mode, RowRef};
use crate::client::Incoming;
use crate::term::Term;

pub use attach::attach;
pub use client::SpawnFn;

/// Current terminal size in columns and rows.
pub fn terminal_size() -> Option<(u16, u16)> {
    ratatui::crossterm::terminal::size().ok()
}

pub struct Options {
    pub demo: bool,
    pub socket: PathBuf,
    pub hooks_installed: bool,
    pub editor: String,
    pub spawn_daemon: Option<SpawnFn>,
    /// Pane to select on start.
    pub select: Option<String>,
}

/// Connection state the main loop needs besides `App`.
struct Ctx {
    stream: Option<client::Stream>,
    sent_size: Option<(String, u16, u16)>,
}

impl Ctx {
    fn send(&self, msg: ClientMsg) {
        if let Some(s) = &self.stream {
            s.send(msg);
        }
    }
}

pub fn run(mut opts: Options) -> Result<()> {
    let (tx, rx) = mpsc::channel();
    let mut demo = None;
    let mut app =
        App::new(if opts.demo { Conn::Demo } else { Conn::Connecting }, opts.hooks_installed, opts.editor.clone());
    let mut ctx = Ctx { stream: None, sent_size: None };
    if opts.demo {
        let d = demo::Demo::new();
        app.set_snapshot(d.snap.clone());
        load_demo_screens(&mut app);
        demo = Some(d);
    } else {
        client::spawn(opts.socket.clone(), opts.spawn_daemon.take(), tx.clone());
        ctx.stream = Some(client::Stream::spawn(opts.socket.clone(), tx));
    }
    if let Some(id) = opts.select.clone() {
        app.pending_select = Some((id, false));
    }

    let mut terminal = ratatui::init();
    let mut out = std::io::stdout();
    let _ = execute!(out, EnableBracketedPaste);
    // Lets us tell Shift+Enter from Enter, among others, where supported.
    let enhanced = matches!(terminal::supports_keyboard_enhancement(), Ok(true));
    if enhanced {
        let _ = execute!(out, PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES));
    }

    let result = (|| -> Result<()> {
        loop {
            app.now = now_ms();
            while let Ok(msg) = rx.try_recv() {
                on_incoming(&mut app, msg);
            }
            if let Some(d) = demo.as_mut()
                && d.tick(app.now)
            {
                app.set_snapshot(d.snap.clone());
            }
            sync_attach(&mut app, &mut ctx);
            terminal.draw(|f| ui::draw(f, &mut app))?;
            sync_size(&mut app, &mut ctx);

            let wait = if app.mode == Mode::Terminal { 16 } else { 60 };
            if !event::poll(Duration::from_millis(wait))? {
                continue;
            }
            // Handle everything already queued before drawing again, so fast
            // typing and pastes don't lag behind.
            loop {
                match event::read()? {
                    Event::Key(key) if key.kind != KeyEventKind::Release => {
                        if app.mode == Mode::Terminal {
                            terminal_key(&mut app, key, &ctx);
                        } else if key.kind == KeyEventKind::Press && handle_key(&mut app, key, &opts, &ctx) {
                            return Ok(());
                        }
                    }
                    Event::Paste(text) if app.mode == Mode::Terminal => paste(&mut app, &text, &ctx),
                    _ => {}
                }
                if !event::poll(Duration::ZERO)? {
                    break;
                }
            }
        }
    })();
    if enhanced {
        let _ = execute!(out, PopKeyboardEnhancementFlags);
    }
    let _ = execute!(out, DisableBracketedPaste);
    ratatui::restore();
    result
}

fn load_demo_screens(app: &mut App) {
    for (id, cols, rows, data) in demo::screens() {
        let mut t = Term::new(cols, rows, 0);
        t.reset(cols, rows, &data);
        app.terms.insert(id, t);
    }
}

fn on_incoming(app: &mut App, msg: Incoming) {
    match msg {
        Incoming::Snapshot(s) => {
            app.conn = Conn::Live;
            app.set_snapshot(*s);
        }
        Incoming::Connecting => app.conn = Conn::Connecting,
        Incoming::Offline(why) => app.conn = Conn::Offline(why),
        Incoming::PaneScreen { pane_id, cols, rows, data } => {
            let generation = app.pane(&pane_id).map(|p| p.generation).unwrap_or(0);
            let t = app.terms.entry(pane_id).or_insert_with(|| Term::new(cols, rows, generation));
            t.reset(cols, rows, &data);
            t.generation = generation;
        }
        Incoming::PaneOutput { pane_id, data } => {
            if let Some(t) = app.terms.get_mut(&pane_id) {
                t.feed(&data);
            }
        }
        Incoming::PaneExited { pane_id } => {
            if let Some(t) = app.terms.get_mut(&pane_id) {
                t.exited = true;
            }
        }
    }
}

/// Stream exactly the selected pane, re-attaching after a restart.
fn sync_attach(app: &mut App, ctx: &mut Ctx) {
    if ctx.stream.is_none() {
        return;
    }
    let want = app.selected_pane().map(|p| (p.id.clone(), p.generation));
    if want == app.attached {
        return;
    }
    if let Some((old, _)) = app.attached.take() {
        ctx.send(ClientMsg::Detach { pane_id: old });
    }
    if let Some((id, generation)) = &want {
        if app.terms.get(id).is_some_and(|t| t.generation != *generation) {
            app.terms.remove(id);
        }
        ctx.sent_size = None;
        ctx.send(ClientMsg::Attach { pane_id: id.clone() });
    }
    app.attached = want;
}

/// Make the pane's terminal match the space it's drawn in.
fn sync_size(app: &mut App, ctx: &mut Ctx) {
    let Some((id, cols, rows)) = app.view_size.take() else { return };
    if cols < 2 || rows < 1 || app.attached.as_ref().map(|a| &a.0) != Some(&id) {
        return;
    }
    if ctx.sent_size.as_ref() == Some(&(id.clone(), cols, rows)) {
        return;
    }
    if let Some(t) = app.terms.get_mut(&id) {
        t.resize(cols, rows);
    }
    ctx.send(ClientMsg::Resize { pane_id: id.clone(), cols, rows });
    ctx.sent_size = Some((id, cols, rows));
}

fn terminal_key(app: &mut App, key: KeyEvent, ctx: &Ctx) {
    let Some((id, _)) = app.attached.clone() else {
        app.mode = Mode::Navigate;
        return;
    };
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    if ctrl && matches!(key.code, KeyCode::Char('q') | KeyCode::Char('Q')) {
        app.mode = Mode::Navigate;
        return;
    }
    let Some(t) = app.terms.get_mut(&id) else { return };
    // Shift+PgUp/PgDn scroll back locally, like most terminals.
    if shift && matches!(key.code, KeyCode::PageUp | KeyCode::PageDown) {
        let page = t.parser.screen().size().0.get() as i32 / 2;
        t.scroll_by(if key.code == KeyCode::PageUp { page } else { -page });
        return;
    }
    if t.scrolled() > 0 {
        t.reset_scroll();
    }
    let app_cursor = t.parser.screen().application_cursor();
    if let Some(bytes) = keys::encode(key, app_cursor) {
        ctx.send(ClientMsg::Input { pane_id: id, data: proto::encode(&bytes) });
    }
}

fn paste(app: &mut App, text: &str, ctx: &Ctx) {
    let Some((id, _)) = app.attached.clone() else { return };
    let bracketed = app.terms.get(&id).is_some_and(|t| t.parser.screen().bracketed_paste());
    ctx.send(ClientMsg::Input { pane_id: id, data: proto::encode(&keys::paste(text, bracketed)) });
}

fn new_pane(app: &mut App, opts: &Options, ctx: &Ctx, kind: PaneKind) {
    let label = if kind == PaneKind::Claude { "Claude Code" } else { "a shell" };
    if matches!(app.conn, Conn::Demo) {
        app.toast(format!("Would start {label} (demo)"));
        return;
    }
    let cwd = app.selected_root().or_else(|| std::env::current_dir().ok().map(|d| d.to_string_lossy().to_string()));
    let Some(cwd) = cwd else { return };
    // Start at the size it will be drawn at, so the first screen fits.
    let (cols, rows) = match &ctx.sent_size {
        Some((_, c, r)) => (*c, *r),
        None => terminal_size()
            .map(|(w, h)| (w.saturating_sub(if w >= 118 { 70 } else { 4 }), h.saturating_sub(6)))
            .unwrap_or((100, 30)),
    };
    let req = NewPane { cwd, kind, args: vec![], name: None, cols: cols.max(20), rows: rows.max(5) };
    match client::command(&opts.socket, &ClientMsg::NewPane(req)) {
        Some(ServerMsg::PaneCreated { pane_id }) => {
            app.pending_select = Some((pane_id, true));
            app.toast(format!("Started {label}"));
        }
        Some(ServerMsg::Error { message }) => app.toast(format!("Couldn't start {label}: {message}")),
        _ => app.toast("The daemon didn't answer"),
    }
}

fn pane_action(app: &mut App, opts: &Options, msg: ClientMsg, done: &str) {
    match client::command(&opts.socket, &msg) {
        Some(ServerMsg::Error { message }) => app.toast(message),
        Some(_) => app.toast(done.to_string()),
        None => app.toast("The daemon didn't answer"),
    }
}

/// Navigate-mode keys for the selected pane. Returns true if handled.
fn pane_key(app: &mut App, key: KeyEvent, opts: &Options) -> bool {
    let Some(p) = app.selected_pane().cloned() else { return false };
    let demo = matches!(app.conn, Conn::Demo);
    match key.code {
        KeyCode::Enter if demo => app.toast("Demo panes are pictures; run aulora for real ones"),
        KeyCode::Enter => {
            if p.is_running() {
                app.mode = Mode::Terminal;
            } else {
                app.toast("The process has exited. Press r to restart it.");
            }
        }
        KeyCode::Char('z') => app.zoom = !app.zoom,
        KeyCode::Esc if app.zoom => app.zoom = false,
        KeyCode::PageUp | KeyCode::PageDown => {
            if let Some(t) = app.terms.get_mut(&p.id) {
                let page = t.parser.screen().size().0.get() as i32 / 2;
                t.scroll_by(if key.code == KeyCode::PageUp { page } else { -page });
            }
        }
        KeyCode::End if app.terms.get(&p.id).is_some_and(|t| t.scrolled() > 0) => {
            if let Some(t) = app.terms.get_mut(&p.id) {
                t.reset_scroll();
            }
        }
        KeyCode::Char('x') if !demo => {
            if p.is_running() {
                if app.confirmed('x', &p.id, &format!("Press x again to kill {}", p.name)) {
                    pane_action(app, opts, ClientMsg::KillPane { pane_id: p.id.clone() }, "Stopped");
                }
            } else {
                pane_action(app, opts, ClientMsg::ClosePane { pane_id: p.id.clone() }, "Closed");
            }
        }
        KeyCode::Char('r') if !demo => {
            if p.is_running() {
                app.toast("Still running. Kill it first with x.");
            } else {
                pane_action(app, opts, ClientMsg::RestartPane { pane_id: p.id.clone() }, "Restarted");
            }
        }
        _ => return false,
    }
    true
}

/// Returns true to quit.
fn handle_key(app: &mut App, key: KeyEvent, opts: &Options, ctx: &Ctx) -> bool {
    if app.help {
        app.help = false;
        return false;
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return true;
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
    if pane_key(app, key, opts) {
        return false;
    }
    match key.code {
        KeyCode::Char('q') => return true,
        KeyCode::Esc if !app.filter.is_empty() => app.filter.clear(),
        KeyCode::Down | KeyCode::Char('j') => app.move_selection(1),
        KeyCode::Up | KeyCode::Char('k') => app.move_selection(-1),
        KeyCode::PageDown => app.move_selection(8),
        KeyCode::PageUp => app.move_selection(-8),
        KeyCode::Char('g') | KeyCode::Home => app.select_edge(false),
        KeyCode::Char('G') | KeyCode::End => app.select_edge(true),
        KeyCode::Tab => app.move_project(1),
        KeyCode::BackTab => app.move_project(-1),
        KeyCode::Char('n') => app.next_attention(),
        KeyCode::Char('/') => app.filtering = true,
        KeyCode::Char('?') => app.help = true,
        KeyCode::Char('c') => new_pane(app, opts, ctx, PaneKind::Shell),
        KeyCode::Char('C') => new_pane(app, opts, ctx, PaneKind::Claude),
        KeyCode::Char('z') => app.toast("Select a pane to zoom it"),
        KeyCode::Char('e') => {
            app.show_ended = !app.show_ended;
            app.move_selection(0);
            app.toast(if app.show_ended { "Showing ended sessions" } else { "Hiding ended sessions" });
        }
        KeyCode::Enter | KeyCode::Char('o') => open_in_editor(app),
        KeyCode::Char('x') => dismiss(app, opts, false),
        KeyCode::Char('X') => dismiss(app, opts, true),
        _ => {}
    }
    false
}

fn open_in_editor(app: &mut App) {
    let Some(root) = app.selected_root() else { return };
    let editor = app.editor.clone();
    let name = aulora_core::util::basename(&root);
    if matches!(app.conn, Conn::Demo) {
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
        Err(e) => app.toast(format!("Couldn't run `{editor}`: {e}. Set AULORA_EDITOR.")),
    }
}

fn dismiss(app: &mut App, opts: &Options, all: bool) {
    if all {
        if matches!(app.conn, Conn::Demo) {
            app.toast("Cleared ended sessions (demo)");
        } else {
            client::command(&opts.socket, &ClientMsg::ClearEnded);
            app.toast("Cleared ended sessions");
        }
        return;
    }
    let Some(RowRef::Session(id)) = app.selected.clone() else { return };
    let ended = app.session(&id).is_some_and(|s| !s.state.is_live());
    if !ended {
        app.toast("Only ended sessions can be dismissed");
        return;
    }
    if matches!(app.conn, Conn::Demo) {
        app.toast("Dismissed (demo)");
    } else {
        client::command(&opts.socket, &ClientMsg::Dismiss { session_id: id });
        app.toast("Dismissed");
    }
}

/// Render a single frame as plain text (for docs, tests and debugging).
/// `screens` holds (pane id, cols, rows, screen bytes) for panes to show.
pub fn render_text(
    opts: &Options,
    live: Option<Snapshot>,
    screens: Vec<(String, u16, u16, Vec<u8>)>,
    width: u16,
    height: u16,
) -> String {
    let mut app = App::new(if opts.demo { Conn::Demo } else { Conn::Live }, opts.hooks_installed, opts.editor.clone());
    if let Some(id) = &opts.select {
        app.pending_select = Some((id.clone(), false));
    }
    match (opts.demo, live) {
        (true, _) => {
            app.set_snapshot(demo::Demo::new().snap);
            load_demo_screens(&mut app);
        }
        (false, Some(s)) => app.set_snapshot(s),
        (false, None) => app.conn = Conn::Offline("not running".into()),
    }
    for (id, cols, rows, data) in screens {
        let mut t = Term::new(cols, rows, 0);
        t.reset(cols, rows, &data);
        app.terms.insert(id, t);
    }
    let mut terminal = ratatui::Terminal::new(TestBackend::new(width, height)).expect("test backend");
    terminal.draw(|f| ui::draw(f, &mut app)).expect("draw");
    let buf = terminal.backend().buffer();
    let mut out = String::new();
    for y in 0..height {
        let mut skip = 0;
        for x in 0..width {
            if skip > 0 {
                skip -= 1;
                continue;
            }
            let sym = buf[(x, y)].symbol();
            out.push_str(sym);
            skip = unicode_width::UnicodeWidthStr::width(sym).saturating_sub(1);
        }
        out.push('\n');
    }
    out
}
