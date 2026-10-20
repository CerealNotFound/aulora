//! The aulora dashboard.

mod app;
mod attach;
mod attrib;
mod changes;
mod client;
mod clipboard;
mod compose;
mod conv;
mod demo;
mod files;
mod finder;
mod git;
mod highlight;
mod input;
mod keymap;
mod keys;
mod layout;
mod links;
mod slash;
mod term;
mod theme;
mod tiles;
mod ui;
mod view_changes;
mod view_files;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::Result;
use aulora_core::model::{SessionState, Snapshot, now_ms};
use aulora_core::proto::ClientMsg;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event, KeyEventKind,
    KeyboardEnhancementFlags, MouseEventKind, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::{SynchronizedUpdate, execute, terminal};

use crate::app::{App, Conn, Focus, RowRef, Status, Tab};
use crate::client::Incoming;
use crate::term::Term;

pub use attach::attach;
pub use client::SpawnFn;
pub use input::ConfirmAction;

pub fn theme_names() -> Vec<&'static str> {
    theme::THEMES.iter().map(|t| t.name).collect()
}

pub fn theme_exists(name: &str) -> bool {
    theme::index_of(name).is_some()
}

/// Current terminal size in columns and rows.
pub fn terminal_size() -> Option<(u16, u16)> {
    terminal::size().ok()
}

pub struct Options {
    pub demo: bool,
    pub socket: PathBuf,
    pub hooks_installed: bool,
    pub editor: String,
    pub spawn_daemon: Option<SpawnFn>,
    /// Pane or session to select on start.
    pub select: Option<String>,
    /// Theme name; otherwise the saved choice, otherwise matched to the terminal.
    pub theme: Option<String>,
    /// Tab to show on start: terminal, conversation or overview.
    pub tab: Option<String>,
}

fn parse_tab(name: &str) -> Option<Tab> {
    match name.to_lowercase().as_str() {
        "terminal" | "term" => Some(Tab::Terminal),
        "conversation" | "conv" | "chat" => Some(Tab::Conversation),
        "overview" => Some(Tab::Overview),
        "changes" | "git" | "diff" => Some(Tab::Changes),
        "files" | "file" => Some(Tab::Files),
        _ => None,
    }
}

/// Select a pane id, a session by id prefix, or a project by name or
/// path, then switch to `tab`.
fn preselect(app: &mut App, select: Option<&str>, tab: Option<&str>) {
    if let Some(q) = select {
        if app.pane(q).is_some() {
            app.selected = Some(RowRef::Pane(q.to_string()));
        } else if let Some(s) = app.snap.as_ref().and_then(|s| s.sessions.iter().find(|s| s.id.starts_with(q))) {
            let r = match s.terminal.pane_id.clone().filter(|p| app.pane(p).is_some()) {
                Some(p) => RowRef::Pane(p),
                None => RowRef::Session(s.id.clone()),
            };
            app.selected = Some(r);
        } else if let Some(p) = app.snap.as_ref().and_then(|s| s.projects.iter().find(|p| p.name == q || p.root == q)) {
            app.selected = Some(RowRef::Project(p.root.clone()));
        }
    }
    if let Some(tab) = tab.and_then(parse_tab) {
        app.set_tab(tab);
        app.focus = Focus::Main;
    }
}

/// `aulora keys`: show what each key press sends.
pub fn keys_probe() -> std::io::Result<()> {
    keys::probe()
}

/// Whether a program is on PATH.
pub(crate) fn which(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(name).is_file()))
}

fn config_path() -> PathBuf {
    aulora_core::paths::home().join("config.json")
}

fn saved_theme() -> Option<String> {
    let text = std::fs::read_to_string(config_path()).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    v.get("theme")?.as_str().map(str::to_string)
}

pub(crate) fn save_theme(name: &str) {
    let path = config_path();
    let mut v: serde_json::Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .filter(serde_json::Value::is_object)
        .unwrap_or_else(|| serde_json::json!({}));
    v["theme"] = serde_json::Value::from(name);
    let _ = aulora_core::paths::ensure_home();
    let _ = std::fs::write(path, serde_json::to_string_pretty(&v).unwrap_or_default());
}

/// Pick the theme: an explicit name, the saved one, or Catppuccin matched
/// to the terminal's background. Call in raw mode, before reading events.
fn choose_theme(explicit: Option<&str>, detect: bool) {
    let named = explicit.map(str::to_string).or_else(saved_theme).and_then(|n| theme::index_of(&n));
    let index = match named {
        Some(i) => i,
        None if detect => theme::default_for(theme::terminal_is_light().unwrap_or(false)),
        None => theme::default_for(false),
    };
    theme::set(index);
}

/// Connection state the input code needs besides `App`.
pub(crate) struct Ctx {
    stream: Option<client::Stream>,
    /// The size last sent for each attached pane.
    pub(crate) sent_size: HashMap<String, (u16, u16)>,
}

impl Ctx {
    pub(crate) fn send(&self, msg: ClientMsg) {
        if let Some(s) = &self.stream {
            s.send(msg);
        }
    }
}

/// Put the terminal back the way we found it.
fn restore_terminal(enhanced: bool) {
    let mut out = std::io::stdout();
    if enhanced {
        let _ = execute!(out, PopKeyboardEnhancementFlags);
    }
    let _ = execute!(out, DisableMouseCapture, DisableBracketedPaste);
    ratatui::restore();
}

pub fn run(mut opts: Options) -> Result<()> {
    let (tx, rx) = mpsc::channel();
    let mut demo = None;
    let mut app =
        App::new(if opts.demo { Conn::Demo } else { Conn::Connecting }, opts.hooks_installed, opts.editor.clone());
    let mut ctx = Ctx { stream: None, sent_size: HashMap::new() };
    if opts.demo {
        let d = demo::Demo::new();
        app.set_snapshot(d.snap.clone());
        load_demo_screens(&mut app);
        app.layouts = demo::layouts();
        demo = Some(d);
    } else {
        app.layouts = tiles::load();
        client::spawn(opts.socket.clone(), opts.spawn_daemon.take(), tx.clone());
        ctx.stream = Some(client::Stream::spawn(opts.socket.clone(), tx));
    }
    if let Some(id) = opts.select.clone() {
        app.pending_select = Some((id, false));
    }
    // Live data arrives later; select once the first snapshot is in.
    if opts.demo {
        preselect(&mut app, opts.select.as_deref(), opts.tab.as_deref());
    } else {
        app.preselect = Some((opts.select.clone(), opts.tab.clone()));
    }

    let mut term = ratatui::init();
    choose_theme(opts.theme.as_deref(), true);
    let mut out = std::io::stdout();
    let _ = execute!(out, EnableBracketedPaste, EnableMouseCapture);
    let enhanced = matches!(terminal::supports_keyboard_enhancement(), Ok(true));
    if enhanced {
        let _ = execute!(out, PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES));
    }
    // A panic must not leave the shell with mouse reporting on.
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal(enhanced);
        previous(info);
    }));

    let mut input = input::Input::default();
    let mut dirty = true;
    let mut last_draw = Instant::now() - Duration::from_secs(1);
    let frame = Duration::from_millis(8);

    let result = (|| -> Result<()> {
        loop {
            app.now = now_ms();
            while let Ok(msg) = rx.try_recv() {
                on_incoming(&mut app, msg);
                dirty = true;
            }
            dirty |= app.poll_changes();
            dirty |= app.poll_sent();
            if let Some(d) = demo.as_mut()
                && d.tick(app.now)
            {
                app.set_snapshot(d.snap.clone());
                dirty = true;
            }
            // Spinners and timers move a few times a second; nothing else redraws idly.
            let since = last_draw.elapsed();
            let animating = app.counts().get(&Status::Working).copied().unwrap_or(0) > 0
                || app.toast.as_ref().is_some_and(|t| app.now.saturating_sub(t.at) < t.ttl.min(10_000));
            if (animating && since >= Duration::from_millis(100)) || since >= Duration::from_secs(1) {
                dirty = true;
            }
            if dirty && since >= frame {
                app.mark_seen();
                std::io::stdout().sync_update(|_| term.draw(|f| ui::draw(f, &mut app)).map(|_| ()))??;
                sync_attach(&mut app, &mut ctx);
                sync_size(&mut app, &mut ctx);
                app.save_layouts();
                app.save_seen();
                dirty = false;
                last_draw = Instant::now();
            }
            let wait = if dirty {
                frame.saturating_sub(last_draw.elapsed())
            } else if app.changes_busy() {
                // Git is running in the background; show its answer promptly.
                Duration::from_millis(15)
            } else if animating {
                Duration::from_millis(100)
            } else {
                Duration::from_millis(250)
            };
            if !event::poll(wait)? {
                continue;
            }
            // Handle everything already queued before drawing again, so fast
            // typing, pastes and wheel bursts don't lag behind.
            loop {
                match event::read()? {
                    Event::Key(key) if key.kind != KeyEventKind::Release => {
                        if (app.typing || key.kind == KeyEventKind::Press)
                            && input::on_key(&mut app, key, &opts, &ctx, &mut input)
                        {
                            return Ok(());
                        }
                        dirty = true;
                    }
                    Event::Mouse(m) if m.kind == MouseEventKind::Moved => {
                        // Only redraw when the hovered row changes.
                        dirty |= input::on_hover(&mut app, m);
                    }
                    Event::Mouse(m) => {
                        input::on_mouse(&mut app, m, &opts, &ctx, &mut input);
                        dirty = true;
                    }
                    Event::Paste(text) => {
                        input::paste(&mut app, &text, &ctx);
                        dirty = true;
                    }
                    Event::Resize(..) => dirty = true,
                    _ => {}
                }
                if !event::poll(Duration::ZERO)? {
                    break;
                }
            }
        }
    })();
    app.seen_dirty = true;
    app.save_seen();
    let _ = std::panic::take_hook();
    restore_terminal(enhanced);
    result
}

fn load_demo_screens(app: &mut App) {
    for (id, cols, rows, data) in demo::screens() {
        let mut t = Term::new(cols, rows, 0);
        t.reset(cols, rows, &data);
        app.terms.insert(id, t);
    }
}

fn blocked_ids(app: &App) -> HashSet<String> {
    app.snap
        .as_ref()
        .map(|s| {
            s.sessions
                .iter()
                .filter(|s| matches!(s.state, SessionState::NeedsYou { .. }))
                .map(|s| s.id.clone())
                .collect()
        })
        .unwrap_or_default()
}

fn on_incoming(app: &mut App, msg: Incoming) {
    match msg {
        Incoming::Snapshot(s) => {
            let first = app.snap.is_none();
            let before = blocked_ids(app);
            app.conn = Conn::Live;
            app.set_snapshot(*s);
            if let Some((select, tab)) = app.preselect.take() {
                preselect(app, select.as_deref(), tab.as_deref());
            }
            // Tell the user when something starts needing them, unless
            // they're looking at it already (Herdr's attention toasts).
            if !first {
                let watching = app.selected_session_id();
                for id in blocked_ids(app).difference(&before) {
                    if watching.as_deref() == Some(id.as_str()) {
                        continue;
                    }
                    if let Some(s) = app.session(id) {
                        let msg = format!(
                            "{} › {} needs you · n",
                            aulora_core::util::basename(&s.project_root),
                            s.display_title()
                        );
                        app.alert(msg);
                    }
                }
            }
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

/// Stream every pane on screen, re-attaching after a restart, and stop
/// streaming the ones that left the screen.
fn sync_attach(app: &mut App, ctx: &mut Ctx) {
    if ctx.stream.is_none() {
        return;
    }
    let want: HashMap<String, u32> =
        app.view_sizes.iter().filter_map(|(id, _, _)| app.pane(id).map(|p| (id.clone(), p.generation))).collect();
    let gone: Vec<String> = app.attached.keys().filter(|id| !want.contains_key(*id)).cloned().collect();
    for id in gone {
        app.attached.remove(&id);
        ctx.sent_size.remove(&id);
        ctx.send(ClientMsg::Detach { pane_id: id });
    }
    for (id, generation) in want {
        if app.attached.get(&id) == Some(&generation) {
            continue;
        }
        if app.terms.get(&id).is_some_and(|t| t.generation != generation) {
            app.terms.remove(&id);
        }
        ctx.sent_size.remove(&id);
        ctx.send(ClientMsg::Attach { pane_id: id.clone() });
        app.attached.insert(id, generation);
    }
}

/// Make each pane's terminal match the space it's drawn in.
fn sync_size(app: &mut App, ctx: &mut Ctx) {
    for (id, cols, rows) in std::mem::take(&mut app.view_sizes) {
        if cols < 2 || rows < 1 || !app.attached.contains_key(&id) {
            continue;
        }
        if ctx.sent_size.get(&id) == Some(&(cols, rows)) {
            continue;
        }
        if let Some(t) = app.terms.get_mut(&id) {
            t.resize(cols, rows);
        }
        ctx.send(ClientMsg::Resize { pane_id: id.clone(), cols, rows });
        ctx.sent_size.insert(id, (cols, rows));
    }
}

pub fn render_text(
    opts: &Options,
    live: Option<Snapshot>,
    screens: Vec<(String, u16, u16, Vec<u8>)>,
    width: u16,
    height: u16,
) -> String {
    render(opts, live, screens, width, height, false, &|_| {})
}

/// Like `render_text`, but every cell with its colors as JSON lines, for
/// turning frames into images.
pub fn render_json(
    opts: &Options,
    live: Option<Snapshot>,
    screens: Vec<(String, u16, u16, Vec<u8>)>,
    width: u16,
    height: u16,
) -> String {
    render(opts, live, screens, width, height, true, &|_| {})
}

fn color_json(c: ratatui::style::Color) -> serde_json::Value {
    use ratatui::style::Color as C;
    match c {
        C::Reset => serde_json::Value::Null,
        C::Rgb(r, g, b) => serde_json::json!([r, g, b]),
        C::Indexed(i) => serde_json::json!(i),
        other => {
            let named = [
                C::Black,
                C::Red,
                C::Green,
                C::Yellow,
                C::Blue,
                C::Magenta,
                C::Cyan,
                C::Gray,
                C::DarkGray,
                C::LightRed,
                C::LightGreen,
                C::LightYellow,
                C::LightBlue,
                C::LightMagenta,
                C::LightCyan,
                C::White,
            ];
            serde_json::json!(named.iter().position(|x| *x == other).unwrap_or(7))
        }
    }
}

fn render(
    opts: &Options,
    live: Option<Snapshot>,
    screens: Vec<(String, u16, u16, Vec<u8>)>,
    width: u16,
    height: u16,
    json: bool,
    prep: &dyn Fn(&mut App),
) -> String {
    choose_theme(opts.theme.as_deref(), false);
    let mut app = App::new(if opts.demo { Conn::Demo } else { Conn::Live }, opts.hooks_installed, opts.editor.clone());
    match (opts.demo, live) {
        (true, _) => {
            app.set_snapshot(demo::Demo::new().snap);
            load_demo_screens(&mut app);
            app.layouts = demo::layouts();
        }
        (false, Some(s)) => app.set_snapshot(s),
        (false, None) => app.conn = Conn::Offline("not running".into()),
    }
    for (id, cols, rows, data) in screens {
        let mut t = Term::new(cols, rows, 0);
        t.reset(cols, rows, &data);
        app.terms.insert(id, t);
    }
    preselect(&mut app, opts.select.as_deref(), opts.tab.as_deref());
    prep(&mut app);
    let mut terminal = ratatui::Terminal::new(TestBackend::new(width, height)).expect("test backend");
    // Twice: the first frame measures views (conversation height) the second uses.
    terminal.draw(|f| ui::draw(f, &mut app)).expect("draw");
    // Wait for background git work the frame asked for (each answer can
    // ask for more: status, then the selected file's diff).
    let deadline = Instant::now() + Duration::from_secs(10);
    while app.changes_busy() && Instant::now() < deadline {
        if app.poll_changes() {
            terminal.draw(|f| ui::draw(f, &mut app)).expect("draw");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    terminal.draw(|f| ui::draw(f, &mut app)).expect("draw");
    let buf = terminal.backend().buffer();
    let mut out = String::new();
    for y in 0..height {
        if json {
            let row: Vec<serde_json::Value> = (0..width)
                .map(|x| {
                    let c = &buf[(x, y)];
                    let bold = c.modifier.contains(ratatui::style::Modifier::BOLD);
                    serde_json::json!([c.symbol(), color_json(c.fg), color_json(c.bg), bold])
                })
                .collect();
            out.push_str(&serde_json::to_string(&row).unwrap_or_default());
        } else {
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
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Menu, RowRef};

    fn demo_opts(select: Option<&str>) -> Options {
        Options {
            demo: true,
            socket: PathBuf::from("/nonexistent"),
            hooks_installed: true,
            editor: "code".into(),
            spawn_daemon: None,
            select: select.map(String::from),
            theme: Some("catppuccin".into()),
            tab: None,
        }
    }

    fn frame(select: Option<&str>, prep: &dyn Fn(&mut App)) -> String {
        render(&demo_opts(select), None, vec![], 150, 44, false, prep)
    }

    #[test]
    fn agents_sit_under_their_project() {
        let out = frame(None, &|_| {});
        let lines: Vec<&str> = out.lines().collect();
        let project = lines.iter().position(|l| l.contains("● multibrain")).expect("project row");
        assert!(lines[project + 2].contains("├ ● Fix streaming"), "{}", lines[project + 2]);
        assert!(lines.iter().any(|l| l.contains("└ ● dev server")), "the last agent closes the branch");
    }

    #[test]
    fn the_menu_lists_actions_for_the_row() {
        let out = frame(Some("a41c07"), &|app| {
            let items = app.menu_for(&RowRef::Pane("a41c07".into()));
            app.menu = Some(Menu { x: 10, y: 10, items, selected: 0 });
        });
        for label in ["Type here", "Close", "New Claude here", "New shell here", "Open in VS Code"] {
            assert!(out.contains(label), "missing {label}:\n{out}");
        }
    }

    #[test]
    fn ended_sessions_offer_resume() {
        let out = frame(None, &|app| {
            let id =
                app.snap.as_ref().and_then(|s| s.sessions.iter().find(|s| !s.state.is_live())).map(|s| s.id.clone());
            app.selected = id.map(RowRef::Session);
            app.show_ended = true;
        });
        assert!(out.contains("resume in a terminal"), "{out}");
    }
}
