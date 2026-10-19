//! The aulora dashboard.

mod app;
mod attach;
mod client;
mod demo;
mod theme;
mod ui;

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use anyhow::Result;
use aulora_core::model::{Snapshot, now_ms};
use aulora_core::proto::ClientMsg;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::app::{App, Conn, RowRef};
use crate::client::Incoming;

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
}

pub fn run(mut opts: Options) -> Result<()> {
    let (tx, rx) = mpsc::channel();
    let mut demo = None;
    let mut app =
        App::new(if opts.demo { Conn::Demo } else { Conn::Connecting }, opts.hooks_installed, opts.editor.clone());
    if opts.demo {
        let d = demo::Demo::new();
        app.set_snapshot(d.snap.clone());
        demo = Some(d);
    } else {
        client::spawn(opts.socket.clone(), opts.spawn_daemon.take(), tx);
    }

    let mut terminal = ratatui::init();
    let result = (|| -> Result<()> {
        loop {
            app.now = now_ms();
            while let Ok(msg) = rx.try_recv() {
                match msg {
                    Incoming::Snapshot(s) => {
                        app.conn = Conn::Live;
                        app.set_snapshot(*s);
                    }
                    Incoming::Connecting => app.conn = Conn::Connecting,
                    Incoming::Offline(why) => app.conn = Conn::Offline(why),
                }
            }
            if let Some(d) = demo.as_mut()
                && d.tick(app.now)
            {
                app.set_snapshot(d.snap.clone());
            }
            terminal.draw(|f| ui::draw(f, &mut app))?;
            if event::poll(Duration::from_millis(60))?
                && let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
                && handle_key(&mut app, key, &opts)
            {
                return Ok(());
            }
        }
    })();
    ratatui::restore();
    result
}

/// Returns true to quit.
fn handle_key(app: &mut App, key: KeyEvent, opts: &Options) -> bool {
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
pub fn render_text(opts: &Options, live: Option<Snapshot>, width: u16, height: u16) -> String {
    let mut app = App::new(if opts.demo { Conn::Demo } else { Conn::Live }, opts.hooks_installed, opts.editor.clone());
    match (opts.demo, live) {
        (true, _) => app.set_snapshot(demo::Demo::new().snap),
        (false, Some(s)) => app.set_snapshot(s),
        (false, None) => app.conn = Conn::Offline("not running".into()),
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
