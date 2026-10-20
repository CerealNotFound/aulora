//! Keys and mouse: everything the user does goes through here.

use std::time::{Duration, Instant};

use aulora_core::model::PaneKind;
use aulora_core::proto::{self, ClientMsg, NewPane, ServerMsg};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use crate::app::{App, Confirm, Conn, Focus, HelpState, Hit, Menu, MenuItem, RowRef, Status, Tab};
use crate::keymap::{self, Action};
use crate::{Ctx, Options, client, keys, theme};

/// Something that needs a yes before it happens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmAction {
    KillPane(String),
    /// Resume a session that is still running somewhere else, as a fork.
    Fork(String),
    /// Stage everything in a project and commit it with this message.
    CommitAll(String, String),
}

impl ConfirmAction {
    pub fn verb(&self) -> &'static str {
        match self {
            ConfirmAction::KillPane(_) => "kill",
            ConfirmAction::Fork(_) => "fork",
            ConfirmAction::CommitAll(..) => "commit all",
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
        confirm_key(app, key, opts, ctx);
        return false;
    }
    if app.theme_picker.is_some() {
        picker_key(app, key);
        return false;
    }
    if app.menu.is_some() {
        menu_key(app, key, opts, ctx);
        return false;
    }
    if app.finder.is_some() {
        if let Some(out) = app.finder_key(key) {
            return finder_outcome(app, out, opts, ctx);
        }
        return false;
    }
    if let Some(root) = app.selected_root()
        && app.current_tab() == Tab::Files
        && app.focus == Focus::Main
        && app.files_view(&root).search.as_ref().is_some_and(|s| s.editing)
    {
        files_search_key(app, &root, key);
        return false;
    }
    if let Some(c) = app.commit_edit.as_mut() {
        match c.key(key) {
            Some(crate::compose::Action::Send) => app.commit(),
            Some(crate::compose::Action::Close) => app.close_commit_message(),
            None => {}
        }
        return false;
    }
    if app.compose.is_some() && key.code == KeyCode::Char('v') && key.modifiers.contains(KeyModifiers::CONTROL) {
        paste_image_into_prompt(app);
        return false;
    }
    if app.compose.is_some() && app.slash_key(&key, &opts.socket) {
        return false;
    }
    if let Some(c) = app.compose.as_mut() {
        match c.key(key) {
            Some(crate::compose::Action::Send) => app.send_compose(&opts.socket),
            Some(crate::compose::Action::Close) => app.close_compose(),
            None => {}
        }
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
            _ => {
                crate::compose::edit_line(&mut app.filter, &key);
            }
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
        && let Some(id) = app.focused_pane()
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
    if scope == keymap::Scope::Changes && changes_action(app, action) {
        return false;
    }
    if scope == keymap::Scope::Files && files_action(app, action, opts, ctx) {
        return false;
    }
    let h = conv_height(app);
    match action {
        Action::Up | Action::Down => {
            let d = if action == Action::Up { -1 } else { 1 };
            match scope {
                // The view itself scrolls, a line a press (less, Claude Code).
                keymap::Scope::Conversation => with_conv(app, |c, h| c.scroll_by(d as isize, h)),
                keymap::Scope::Pane => scroll_pane(app, d as isize),
                _ => app.move_selection(d),
            }
        }
        Action::HalfUp | Action::HalfDown => {
            let d: isize = if action == Action::HalfUp { -1 } else { 1 };
            match scope {
                keymap::Scope::Conversation => with_conv(app, |c, h| c.scroll_by(d * (h as isize / 2).max(1), h)),
                keymap::Scope::Pane => {
                    let h = app
                        .focused_pane()
                        .and_then(|id| app.terms.get(&id))
                        .map_or(20, |t| t.parser.screen().size().0.get()) as isize;
                    scroll_pane(app, d * (h / 2).max(1));
                }
                _ => app.move_selection(d as i32 * 5),
            }
        }
        Action::CursorUp | Action::CursorDown => {
            let d: isize = if action == Action::CursorUp { -1 } else { 1 };
            with_conv(app, |c, h| c.move_cursor(d, h));
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
                    if let Some(t) = app.focused_pane().and_then(|id| app.terms.get_mut(&id)) {
                        t.reset_scroll();
                    }
                }
                _ => app.select_edge(!top),
            }
        }
        Action::FocusNext => app.cycle_focus(1),
        Action::FocusPrev => app.cycle_focus(-1),
        Action::FocusLeft => {
            app.move_focus(-1, 0);
        }
        Action::FocusRight => {
            app.move_focus(1, 0);
        }
        Action::FocusUp => {
            app.move_focus(0, -1);
        }
        Action::FocusDown => {
            app.move_focus(0, 1);
        }
        Action::SplitRight | Action::SplitDown | Action::NewTab => {
            app.place_next = Some(match action {
                Action::SplitRight => Some(crate::layout::Dir::Right),
                Action::SplitDown => Some(crate::layout::Dir::Down),
                _ => None,
            });
            new_pane(app, opts, ctx, PaneKind::Shell);
        }
        Action::OpenChanges => {
            if app.selected_root().and_then(|r| app.project(&r)).is_some_and(|p| p.git.is_some()) {
                app.set_tab(Tab::Changes);
            } else {
                app.toast("Not a git repository");
            }
        }
        Action::OpenFiles => app.set_tab(Tab::Files),
        Action::Grow | Action::Shrink => {
            let d = if action == Action::Grow { 0.05 } else { -0.05 };
            if let Some(tab) = app.tab_mut()
                && !tab.nudge(crate::layout::Dir::Right, d)
            {
                tab.nudge(crate::layout::Dir::Down, d);
            }
        }
        Action::Equalize => {
            if let Some(tab) = app.tab_mut() {
                tab.equalize();
            }
        }
        Action::CloseLeaf => close_leaf(app, opts),
        Action::Tab(n) => {
            app.switch_tab(n as usize);
        }
        Action::NextTab => app.cycle_tab(1),
        Action::PrevTab => app.cycle_tab(-1),
        Action::Open => open(app, opts, ctx),
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
        Action::Zoom => {
            if let Some(tab) = app.tab_mut() {
                tab.zoom = !tab.zoom && tab.leaves().len() > 1;
            }
        }
        Action::OpenEditor => {
            if let Some(root) = app.selected_root() {
                open_in_editor(app, &root);
            }
        }
        Action::Close => {
            if let Some(r) = app.selected.clone() {
                close(app, opts, r);
            }
        }
        Action::DismissEnded => {
            if app.conn == Conn::Demo {
                app.toast("Cleared ended sessions (demo)");
            } else {
                client::command(&opts.socket, &ClientMsg::ClearEnded);
                app.toast("Cleared ended sessions");
            }
        }
        Action::Restart | Action::Resume => match app.selected.clone() {
            Some(RowRef::Pane(id)) if action == Action::Restart => restart(app, opts, &id),
            Some(RowRef::Session(id)) => resume(app, opts, ctx, &id),
            _ => {}
        },
        Action::Menu => menu_at_selection(app),
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
        // Only meaningful in the Changes tab, handled above.
        Action::Compose => match app.selected_session_id() {
            Some(id) if app.can_compose(&id) => {
                app.focus = Focus::Main;
                app.start_compose(&id);
            }
            Some(id) if app.session(&id).is_some_and(|s| s.state.is_live()) => {
                let place =
                    app.session(&id).and_then(|s| s.terminal.app_name()).unwrap_or_else(|| "its terminal".into());
                app.toast(format!("Can't type into {place} from here. In VS Code or Cursor: aulora vscode install"));
            }
            _ => {}
        },
        Action::Finder => app.open_finder(),
        Action::OpenProject => app.open_project_picker(),
        Action::Reveal => {
            let path = match (app.current_tab(), app.selected_root()) {
                (Tab::Files, Some(root)) if app.focus == Focus::Main => {
                    let sel = app.files_view(&root).selected.clone();
                    sel.map(|s| format!("{root}/{s}")).or(Some(root))
                }
                (_, root) => match app.selected.clone() {
                    Some(RowRef::Pane(id)) => app.pane(&id).map(|p| p.cwd.clone()),
                    _ => root,
                },
            };
            if let Some(p) = path {
                reveal_in_finder(app, &p);
            }
        }
        Action::Edit => {}
        Action::Commit
        | Action::Push
        | Action::Pull
        | Action::Stage
        | Action::ToggleAll
        | Action::PrevHunk
        | Action::NextHunk
        | Action::Left => {}
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
    if let Some(id) = app.focused_pane()
        && let Some(t) = app.terms.get_mut(&id)
    {
        t.scroll_by(-delta as i32);
    }
}

fn open(app: &mut App, opts: &Options, ctx: &Ctx) {
    let Some(sel) = app.selected.clone() else { return };
    match app.focus {
        // An ended session opens as a terminal running `claude --resume`.
        Focus::Sidebar if matches!(&sel, RowRef::Session(id) if !app.session(id).is_some_and(|s| s.state.is_live())) => {
            if let RowRef::Session(id) = &sel {
                resume(app, opts, ctx, id);
            }
        }
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
            Tab::Overview => {
                if let Some(root) = app.selected_root() {
                    open_in_editor(app, &root);
                }
            }
            Tab::Changes => {
                changes_action(app, Action::Open);
            }
            Tab::Files => {
                files_action(app, Action::Open, opts, ctx);
            }
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
    let Some(id) = app.focused_pane() else {
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
    // Cmd-V with only an image on the clipboard pastes nothing: terminals
    // can't paste images as text. Hand it to whatever can take the image.
    if text.is_empty() {
        if app.compose.is_some() {
            paste_image_into_prompt(app);
        } else if app.typing
            && let Some(id) = app.focused_pane()
        {
            // Claude Code reads the clipboard's image itself on Ctrl-V.
            ctx.send(ClientMsg::Input { pane_id: id, data: proto::encode(&[0x16]) });
        }
        return;
    }
    if let Some(c) = app.commit_edit.as_mut().or(app.compose.as_mut()) {
        c.insert(text);
        return;
    }
    if app.typing {
        let Some(id) = app.focused_pane() else { return };
        let bracketed = app.terms.get(&id).is_some_and(|t| t.parser.screen().bracketed_paste());
        ctx.send(ClientMsg::Input { pane_id: id, data: proto::encode(&keys::paste(text, bracketed)) });
    } else if app.filtering {
        app.filter.push_str(text.lines().next().unwrap_or(""));
    } else if app.searching {
        app.query.push_str(text.lines().next().unwrap_or(""));
    }
}

/// Save the clipboard's image and put its path in the prompt being
/// written; Claude Code attaches an image given by its path.
fn paste_image_into_prompt(app: &mut App) {
    match crate::clipboard::save_image() {
        Ok(Some(path)) => {
            if let Some(c) = app.compose.as_mut() {
                let sep = if c.text.is_empty() || c.text.ends_with(' ') { "" } else { " " };
                c.insert(&format!("{sep}{} ", path.display()));
            }
            app.toast("Image attached");
        }
        Ok(None) => app.toast("No image on the clipboard"),
        Err(e) => app.error(format!("Couldn't read the clipboard: {e}")),
    }
}

fn search_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => app.searching = false,
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
        _ => {
            crate::compose::edit_line(&mut app.query, &key);
        }
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
        KeyCode::Backspace | KeyCode::Char(_) if crate::compose::edit_line(&mut h.query, &key) => h.selected = 0,
        KeyCode::Enter => {
            let action = rows.get(h.selected).map(|r| r.3);
            app.help = None;
            if let Some(a) = action
                && a != Action::Help
            {
                return perform(app, a, opts, ctx);
            }
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

fn confirm_key(app: &mut App, key: KeyEvent, opts: &Options, ctx: &Ctx) {
    let Some(c) = app.confirm.as_mut() else { return };
    match key.code {
        KeyCode::Left | KeyCode::Right | KeyCode::Tab | KeyCode::BackTab | KeyCode::Char('h') | KeyCode::Char('l') => {
            c.on_action = !c.on_action;
        }
        KeyCode::Char('y') => run_confirmed(app, opts, ctx),
        KeyCode::Enter if c.on_action => run_confirmed(app, opts, ctx),
        KeyCode::Enter | KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('q') => app.confirm = None,
        _ => {}
    }
}

fn run_confirmed(app: &mut App, opts: &Options, ctx: &Ctx) {
    let Some(c) = app.confirm.take() else { return };
    match c.action {
        ConfirmAction::KillPane(id) => pane_action(app, opts, ClientMsg::KillPane { pane_id: id }, "Stopped"),
        ConfirmAction::CommitAll(root, message) => app.start_op(&root, crate::git::Op::Commit { message, all: true }),
        ConfirmAction::Fork(id) => {
            let Some(cwd) = app.session(&id).map(session_dir) else { return };
            let args = vec!["--resume".into(), id, "--fork-session".into()];
            spawn(app, opts, ctx, PaneKind::Claude, cwd, args, "a fork of the session");
        }
    }
}

fn close(app: &mut App, opts: &Options, row: RowRef) {
    let demo = app.conn == Conn::Demo;
    match Some(row) {
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
                app.toast("This session runs outside aulora; end it where it runs, or r to fork it here");
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

fn restart(app: &mut App, opts: &Options, id: &str) {
    let Some(p) = app.pane(id).cloned() else { return };
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

/// A new pane in the selected project's folder (or ours, with nothing selected).
pub fn new_pane(app: &mut App, opts: &Options, ctx: &Ctx, kind: PaneKind) {
    let cwd = app.selected_root().or_else(|| std::env::current_dir().ok().map(|d| d.to_string_lossy().to_string()));
    let Some(cwd) = cwd else { return };
    let label = if kind == PaneKind::Claude { "Claude Code" } else { "a shell" };
    spawn(app, opts, ctx, kind, cwd, vec![], label);
}

/// Where to resume a session: the folder it ran in, which is where Claude
/// Code looks for its transcript.
fn session_dir(s: &aulora_core::model::Session) -> String {
    if std::path::Path::new(&s.cwd).is_dir() { s.cwd.clone() } else { s.project_root.clone() }
}

/// Open a session as a terminal: `claude --resume <id>` in a new pane.
pub fn resume(app: &mut App, opts: &Options, ctx: &Ctx, id: &str) {
    if let Some(p) = app.resumed_in(id).map(|p| p.id.clone()) {
        app.select(RowRef::Pane(p.clone()));
        start_typing(app, opts, &p);
        return;
    }
    let Some(s) = app.session(id).cloned() else { return };
    if s.state.is_live() {
        // Two processes writing one transcript would interleave it, so a
        // session that's still running elsewhere resumes as a fork.
        let place = s.terminal.app_name().unwrap_or_else(|| "another terminal".into());
        app.confirm = Some(Confirm {
            title: "fork this session?".into(),
            body: format!(
                "It's still running in {place}. Forking starts a new session here with the same history; the original keeps running."
            ),
            action: ConfirmAction::Fork(id.to_string()),
            on_action: false,
        });
        return;
    }
    let args = vec!["--resume".into(), id.to_string()];
    spawn(app, opts, ctx, PaneKind::Claude, session_dir(&s), args, "the session");
}

fn spawn(app: &mut App, opts: &Options, ctx: &Ctx, kind: PaneKind, cwd: String, args: Vec<String>, label: &str) {
    spawn_named(app, opts, ctx, kind, cwd, args, label, None);
}

#[allow(clippy::too_many_arguments)]
fn spawn_named(
    app: &mut App,
    opts: &Options,
    ctx: &Ctx,
    kind: PaneKind,
    cwd: String,
    args: Vec<String>,
    label: &str,
    name: Option<String>,
) {
    if app.conn == Conn::Demo {
        app.toast(format!("Would start {label} in {} (demo)", aulora_core::util::basename(&cwd)));
        return;
    }
    let _ = ctx;
    // Beside the focused leaf unless asked otherwise (None: a new tab).
    let dir = app.place_next.take().unwrap_or(Some(crate::layout::Dir::Right));
    let (cols, rows) = app.new_pane_size(dir).unwrap_or_else(|| {
        crate::terminal_size().map(|(w, h)| (w.saturating_sub(40), h.saturating_sub(4))).unwrap_or((100, 30))
    });
    let root = app.place_root.take().or_else(|| app.space_root()).unwrap_or_else(|| cwd.clone());
    let req = NewPane { cwd, kind, args, name, cols: cols.max(20), rows: rows.max(5) };
    match client::command(&opts.socket, &ClientMsg::NewPane(req)) {
        Some(ServerMsg::PaneCreated { pane_id }) => {
            app.place_pane(&root, &pane_id, dir);
            app.focus = Focus::Main;
            app.pending_select = Some((pane_id, true));
            app.toast(if label == "the session" { "Resumed".to_string() } else { format!("Started {label}") });
        }
        Some(ServerMsg::Error { message }) => app.error(format!("Couldn't start {label}: {message}")),
        _ => app.error("The daemon didn't answer"),
    }
}

pub fn open_in_editor(app: &mut App, root: &str) {
    let editor = app.editor.clone();
    let name = aulora_core::util::basename(root);
    if app.conn == Conn::Demo {
        app.toast(format!("Would open {name} in {editor} (demo)"));
        return;
    }
    match std::process::Command::new(&editor)
        .arg(root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(_) => app.toast(format!("Opened {name} in {editor}")),
        Err(e) => app.error(format!("Couldn't run `{editor}`: {e}. Set AULORA_EDITOR.")),
    }
}

// ---------------------------------------------------------------------------
// Changes

/// Handle an action in the Changes tab. Returns false to fall through to
/// the general handling (focus, tabs, help and so on).
fn changes_action(app: &mut App, action: Action) -> bool {
    use crate::changes::Node;
    let Some(root) = app.selected_root() else { return false };
    let rows = app.change_rows(&root);
    let sel = app.change_selected(&root, &rows);
    let diff_focused = app.view_mut(&root).focus_diff;
    let row = sel.map(|i| rows[i].clone());
    let select = |app: &mut App, i: usize| {
        if let Some(r) = rows.get(i) {
            let v = app.view_mut(&root);
            if v.selected.as_ref() != Some(&r.id) {
                v.selected = Some(r.id.clone());
                v.diff_scroll = 0;
            }
        }
    };
    let scroll_diff = |app: &mut App, d: isize| {
        let v = app.view_mut(&root);
        v.diff_scroll = (v.diff_scroll as isize + d).max(0) as usize;
    };
    match action {
        Action::Up | Action::Down => {
            let d = if action == Action::Up { -1 } else { 1 };
            if diff_focused {
                scroll_diff(app, d);
            } else if let Some(i) = sel {
                select(app, (i as isize + d).clamp(0, rows.len() as isize - 1) as usize);
            }
        }
        Action::PageUp | Action::PageDown => {
            let page = app.view_mut(&root).diff_height.saturating_sub(2).max(1) as isize;
            scroll_diff(app, if action == Action::PageUp { -page } else { page });
        }
        Action::Top | Action::Bottom => {
            let top = action == Action::Top;
            if diff_focused {
                app.view_mut(&root).diff_scroll = if top { 0 } else { usize::MAX / 2 };
            } else if !rows.is_empty() {
                select(app, if top { 0 } else { rows.len() - 1 });
            }
        }
        Action::Open => match row.as_ref().map(|r| &r.node) {
            Some(Node::File(_)) => app.view_mut(&root).focus_diff = true,
            Some(_) => toggle_folder(app, &root, row.as_ref().map(|r| r.id.clone())),
            None => {}
        },
        Action::Left => {
            if diff_focused {
                app.view_mut(&root).focus_diff = false;
            } else if let Some(r) = &row {
                if !matches!(r.node, Node::File(_)) && r.expanded {
                    toggle_folder(app, &root, Some(r.id.clone()));
                } else if r.depth == 0 {
                    app.focus = Focus::Sidebar;
                } else if let Some(i) = sel {
                    let parent = rows[..i].iter().rposition(|p| p.depth < r.depth);
                    if let Some(p) = parent {
                        select(app, p);
                    }
                }
            }
        }
        Action::Back => {
            if let Some(p) = app.changes.get_mut(&root).filter(|p| p.op_error.is_some()) {
                p.op_error = None;
            } else if diff_focused {
                app.view_mut(&root).focus_diff = false;
            } else {
                app.focus = Focus::Sidebar;
            }
        }
        Action::Stage => app.stage_selected(&root),
        Action::Commit => app.start_commit_message(&root),
        Action::Push => app.start_op(&root, crate::git::Op::Push),
        Action::Pull => app.start_op(&root, crate::git::Op::Pull),
        Action::ToggleAll => {
            // Every change → each agent's in turn → every change again.
            let authors = app.change_authors(&root);
            let next = match app.focused_filter() {
                None => authors.first().cloned(),
                Some(cur) => authors.iter().position(|a| *a == cur).and_then(|i| authors.get(i + 1)).cloned(),
            };
            if let Some(tab) = app.tab_mut() {
                let f = tab.focus;
                tab.replace(f, crate::layout::View::Changes { filter: next });
            }
            let v = app.view_mut(&root);
            v.selected = None;
            v.scroll = 0;
        }
        Action::PrevHunk | Action::NextHunk => {
            let file = row.as_ref().and_then(|r| app.change_file(&root, r));
            let Some(file) = file else { return true };
            let Some(crate::changes::DiffState::Ready(d)) = app.diff_for(&root, &file) else { return true };
            let starts = crate::view_changes::hunk_starts(d);
            let v = app.view_mut(&root);
            let cur = v.diff_scroll;
            let next = if action == Action::NextHunk {
                starts.iter().find(|&&s| s > cur).copied()
            } else {
                starts.iter().rev().find(|&&s| s < cur).copied()
            };
            match next {
                Some(s) => v.diff_scroll = s,
                None => app.toast(if action == Action::NextHunk {
                    "Last change in this file"
                } else {
                    "First change in this file"
                }),
            }
        }
        Action::OpenEditor => {
            let file = row.as_ref().and_then(|r| app.change_file(&root, r));
            match file {
                Some(file) => {
                    let at = app.view_mut(&root).diff_scroll;
                    let line = match app.diff_for(&root, &file) {
                        Some(crate::changes::DiffState::Ready(d)) => crate::view_changes::line_at(d, at),
                        _ => None,
                    };
                    open_file_in_editor(app, &root, &file.entry.path, line);
                }
                None => open_in_editor(app, &root),
            }
        }
        Action::Copy => {
            if let Some(file) = row.as_ref().and_then(|r| app.change_file(&root, r)) {
                copy_to_clipboard(&file.entry.path);
                app.toast(format!("Copied {}", file.entry.path));
            }
        }
        _ => return false,
    }
    true
}

fn toggle_folder(app: &mut App, root: &str, id: Option<String>) {
    let Some(id) = id else { return };
    let v = app.view_mut(root);
    if !v.collapsed.remove(&id) {
        v.collapsed.insert(id);
    }
}

fn click_change(app: &mut App, i: usize, double: bool) {
    use crate::changes::Node;
    let Some(root) = app.selected_root() else { return };
    let rows = app.change_rows(&root);
    let Some(r) = rows.get(i) else { return };
    let v = app.view_mut(&root);
    let was = v.selected.as_ref() == Some(&r.id);
    if !was {
        v.selected = Some(r.id.clone());
        v.diff_scroll = 0;
    }
    v.focus_diff = false;
    match r.node {
        // A click opens or closes a folder (VS Code).
        Node::Group(_) | Node::Dir(..) => toggle_folder(app, &root, Some(r.id.clone())),
        Node::File(_) if double => app.view_mut(&root).focus_diff = true,
        Node::File(_) => {}
    }
}

/// Open a file at a line: `code -g file:line` for VS Code and its forks,
/// `+line file` for terminal editors, just the file otherwise.
fn open_file_in_editor(app: &mut App, root: &str, rel: &str, line: Option<u32>) {
    let editor = app.editor.clone();
    let path = format!("{root}/{rel}");
    if app.conn == Conn::Demo {
        app.toast(format!("Would open {rel}{} in {editor} (demo)", line.map(|l| format!(":{l}")).unwrap_or_default()));
        return;
    }
    let exe = editor.rsplit('/').next().unwrap_or(&editor).to_string();
    let mut cmd = std::process::Command::new(&editor);
    match (exe.as_str(), line) {
        ("code" | "code-insiders" | "cursor" | "windsurf" | "codium" | "zed", Some(l)) => {
            if exe == "zed" {
                cmd.arg(format!("{path}:{l}"));
            } else {
                cmd.args(["-g", &format!("{path}:{l}")]);
            }
        }
        _ => {
            cmd.arg(&path);
        }
    }
    let spawned = cmd
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    match spawned {
        Ok(_) => app.toast(format!("Opened {rel} in {editor}")),
        Err(e) => app.error(format!("Couldn't run `{editor}`: {e}. Set AULORA_EDITOR.")),
    }
}

/// Do what a Ctrl-P choice asks. Returns true to quit.
fn finder_outcome(app: &mut App, out: crate::finder::Outcome, opts: &Options, ctx: &Ctx) -> bool {
    match out {
        crate::finder::Outcome::Run(a) => perform(app, a, opts, ctx),
        crate::finder::Outcome::Open(path, claude) => {
            open_project(app, opts, ctx, &path, claude);
            false
        }
    }
}

/// Open a folder as a project: select it if aulora shows it already,
/// otherwise start a shell (or Claude) there in a new tab, which makes it
/// a project in the sidebar (VS Code's Open Folder).
pub fn open_project(app: &mut App, opts: &Options, ctx: &Ctx, path: &str, claude: bool) {
    let root = crate::git::run(path, &["rev-parse", "--show-toplevel"])
        .ok()
        .map(|o| String::from_utf8_lossy(&o).trim().to_string())
        .filter(|r| !r.is_empty())
        .unwrap_or_else(|| path.trim_end_matches('/').to_string());
    let shown = app.rows().contains(&RowRef::Project(root.clone()));
    if shown && !claude {
        app.select(RowRef::Project(root));
        return;
    }
    app.place_root = Some(root.clone());
    app.place_next = Some(None);
    let (kind, label) = if claude { (PaneKind::Claude, "Claude Code") } else { (PaneKind::Shell, "a shell") };
    spawn(app, opts, ctx, kind, path.to_string(), vec![], label);
}

/// Show a file or folder in Finder (`open -R`), or the system's file
/// manager elsewhere.
pub fn reveal_in_finder(app: &mut App, path: &str) {
    let name = aulora_core::util::basename(path);
    if app.conn == Conn::Demo {
        app.toast(format!("Would show {name} in Finder (demo)"));
        return;
    }
    let mut cmd = if cfg!(target_os = "macos") {
        let mut c = std::process::Command::new("open");
        c.arg("-R").arg(path);
        c
    } else {
        let dir = if std::path::Path::new(path).is_dir() {
            path.to_string()
        } else {
            std::path::Path::new(path).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()
        };
        let mut c = std::process::Command::new("xdg-open");
        c.arg(dir);
        c
    };
    let spawned = cmd
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    match spawned {
        Ok(_) => app.toast(format!("Showing {name} in Finder")),
        Err(e) => app.error(format!("Couldn't open Finder: {e}")),
    }
}

// ---------------------------------------------------------------------------
// Leaves

/// Close the focused leaf: a view just goes; a terminal is killed (asking
/// first if it's running) and its leaf goes when the pane does.
fn close_leaf(app: &mut App, opts: &Options) {
    let Some(view) = app.focused_view() else { return };
    match view {
        crate::layout::View::Pane { id } => close(app, opts, RowRef::Pane(id)),
        _ => {
            let Some(root) = app.space_root() else { return };
            let space = app.layouts.space(&root);
            let Some(tab) = space.tab_mut() else { return };
            let leaf = tab.focus;
            if !tab.close(leaf) {
                let active = space.active;
                space.tabs.remove(active);
                space.active = active.saturating_sub(1).min(space.tabs.len().saturating_sub(1));
            }
            app.layouts_dirty = true;
            app.sync_selection();
        }
    }
}

// ---------------------------------------------------------------------------
// Files

/// Handle an action in the Files tab. Returns false to fall through.
fn files_action(app: &mut App, action: Action, opts: &Options, ctx: &Ctx) -> bool {
    let Some(root) = app.selected_root() else { return false };
    let in_results = app.files_view(&root).search.as_ref().is_some_and(|s| s.results.is_some() && !s.editing);
    let focus_file = app.files_view(&root).focus_file;
    let rows = if in_results { vec![] } else { app.file_rows(&root) };
    let sel = app.files_view(&root).selected.clone();
    let idx = sel.as_ref().and_then(|s| rows.iter().position(|r| &r.rel == s));
    let row = idx.map(|i| rows[i].clone());
    let select = |app: &mut App, i: usize| {
        if let Some(r) = rows.get(i) {
            let v = app.files_view(&root);
            if v.selected.as_ref() != Some(&r.rel) {
                v.selected = Some(r.rel.clone());
                v.file_scroll = 0;
                v.mark_line = None;
            }
        }
    };
    let scroll_file = |app: &mut App, d: isize| {
        let v = app.files_view(&root);
        v.file_scroll = (v.file_scroll as isize + d).max(0) as usize;
    };
    match action {
        Action::Up | Action::Down => {
            let d: isize = if action == Action::Up { -1 } else { 1 };
            if focus_file {
                scroll_file(app, d);
            } else if in_results {
                let n = match app.files_view(&root).search.as_ref().and_then(|s| s.results.as_ref()) {
                    Some(Ok((m, _))) => m.len(),
                    _ => 0,
                };
                if let Some(s) = app.files_view(&root).search.as_mut() {
                    s.selected = (s.selected as isize + d).clamp(0, n.saturating_sub(1) as isize) as usize;
                }
            } else if let Some(i) = idx {
                select(app, (i as isize + d).clamp(0, rows.len() as isize - 1) as usize);
            } else if !rows.is_empty() {
                select(app, 0);
            }
        }
        Action::PageUp | Action::PageDown => {
            let page = app.files_view(&root).file_height.saturating_sub(2).max(1) as isize;
            scroll_file(app, if action == Action::PageUp { -page } else { page });
        }
        Action::Top | Action::Bottom => {
            let top = action == Action::Top;
            if focus_file {
                app.files_view(&root).file_scroll = if top { 0 } else { usize::MAX / 2 };
            } else if !rows.is_empty() {
                select(app, if top { 0 } else { rows.len() - 1 });
            }
        }
        Action::Open => {
            if in_results {
                open_result(app, &root);
            } else if let Some(r) = &row {
                if r.is_dir {
                    toggle_dir(app, &root, &r.rel);
                } else {
                    app.files_view(&root).focus_file = true;
                }
            }
        }
        Action::Left => {
            if focus_file {
                app.files_view(&root).focus_file = false;
            } else if let Some(r) = &row {
                if r.is_dir && r.expanded {
                    toggle_dir(app, &root, &r.rel);
                } else if r.depth == 0 {
                    app.focus = Focus::Sidebar;
                } else if let Some(i) = idx
                    && let Some(p) = rows[..i].iter().rposition(|p| p.depth < r.depth)
                {
                    select(app, p);
                }
            }
        }
        Action::Back => {
            let v = app.files_view(&root);
            if v.focus_file {
                v.focus_file = false;
            } else if v.search.is_some() {
                v.search = None;
            } else {
                app.focus = Focus::Sidebar;
            }
        }
        Action::Search => {
            let v = app.files_view(&root);
            let query = v.search.as_ref().map(|s| s.searched.clone()).unwrap_or_default();
            v.focus_file = false;
            v.search = Some(crate::files::Search {
                query,
                editing: true,
                searched: String::new(),
                results: None,
                selected: 0,
                scroll: 0,
            });
        }
        Action::PrevHunk | Action::NextHunk => {
            let Some(rel) = sel.filter(|_| row.as_ref().is_none_or(|r| !r.is_dir)) else { return true };
            let marks: Vec<u32> = match app.file_text(&root, &rel) {
                Some(crate::files::Loaded::Ready(t, _)) => {
                    let mut m: Vec<u32> = t.marks.keys().copied().collect();
                    m.sort_unstable();
                    // The first line of each run of marks.
                    m.iter().enumerate().filter(|(i, n)| *i == 0 || m[i - 1] + 1 != **n).map(|(_, n)| *n).collect()
                }
                _ => vec![],
            };
            let v = app.files_view(&root);
            let cur = v.mark_line.unwrap_or(v.file_scroll as u32 + v.file_height as u32 / 3);
            let next = if action == Action::NextHunk {
                marks.iter().find(|&&n| n > cur).copied()
            } else {
                marks.iter().rev().find(|&&n| n < cur).copied()
            };
            match next {
                Some(n) => {
                    v.mark_line = Some(n);
                    v.file_scroll = (n as usize).saturating_sub(v.file_height / 3);
                }
                None if marks.is_empty() => app.toast("No changes in this file"),
                None => app.toast(if action == Action::NextHunk { "Last change" } else { "First change" }),
            }
        }
        Action::Edit | Action::OpenEditor | Action::Copy => {
            let Some(rel) = sel.filter(|_| row.as_ref().is_none_or(|r| !r.is_dir)) else {
                app.toast("Pick a file first");
                return true;
            };
            let v = app.files_view(&root);
            let line = v.mark_line.or(Some(v.file_scroll as u32 + 1));
            match action {
                Action::Edit => edit_in_pane(app, opts, ctx, &root, &rel, line),
                Action::OpenEditor => open_file_in_editor(app, &root, &rel, line),
                _ => {
                    copy_to_clipboard(&rel);
                    app.toast(format!("Copied {rel}"));
                }
            }
        }
        _ => return false,
    }
    true
}

fn toggle_dir(app: &mut App, root: &str, rel: &str) {
    let v = app.files_view(root);
    if !v.expanded.remove(rel) {
        v.expanded.insert(rel.to_string());
    }
}

fn click_file(app: &mut App, i: usize, double: bool, opts: &Options, ctx: &Ctx) {
    let Some(root) = app.selected_root() else { return };
    let rows = app.file_rows(&root);
    let Some(r) = rows.get(i) else { return };
    let v = app.files_view(&root);
    if v.selected.as_ref() != Some(&r.rel) {
        v.selected = Some(r.rel.clone());
        v.file_scroll = 0;
        v.mark_line = None;
    }
    v.focus_file = false;
    if r.is_dir {
        toggle_dir(app, &root, &r.rel);
    } else if double {
        edit_in_pane(app, opts, ctx, &root, &r.rel, Some(1));
    }
}

fn open_result(app: &mut App, root: &str) {
    let pick = app.files_view(root).search.as_ref().and_then(|s| match &s.results {
        Some(Ok((m, _))) => m.get(s.selected).cloned(),
        _ => None,
    });
    if let Some(m) = pick {
        // The results stay on the left; the file opens beside them.
        app.reveal_file(root, &m.path, Some(m.line));
    }
}

fn files_search_key(app: &mut App, root: &str, key: KeyEvent) {
    let Some(s) = app.files_view(root).search.as_mut() else { return };
    match key.code {
        KeyCode::Esc => {
            if s.results.is_some() {
                s.editing = false;
            } else {
                app.files_view(root).search = None;
            }
        }
        KeyCode::Enter => {
            let q = s.query.clone();
            if q.trim().is_empty() {
                return;
            }
            s.editing = false;
            app.start_search(root, q);
        }
        _ => {
            crate::compose::edit_line(&mut s.query, &key);
        }
    }
}

/// The user's terminal editor: $VISUAL, $EDITOR, else the first of nvim,
/// vim and nano that exists, else vi.
fn terminal_editor() -> String {
    for var in ["VISUAL", "EDITOR"] {
        if let Ok(v) = std::env::var(var)
            && !v.trim().is_empty()
        {
            return v;
        }
    }
    for e in ["nvim", "vim", "nano"] {
        if crate::which(e) {
            return e.into();
        }
    }
    "vi".into()
}

/// Edit a file in a new pane with the user's terminal editor, at a line.
/// aulora has no editor of its own; yours is better.
fn edit_in_pane(app: &mut App, opts: &Options, ctx: &Ctx, root: &str, rel: &str, line: Option<u32>) {
    let editor = terminal_editor();
    let exe = editor.split_whitespace().next().unwrap_or(&editor).rsplit('/').next().unwrap_or("").to_string();
    if matches!(exe.as_str(), "code" | "cursor" | "zed" | "subl" | "windsurf") {
        // A GUI editor in $EDITOR: open the file there instead.
        open_file_in_editor(app, root, rel, line);
        return;
    }
    let mut args: Vec<String> = editor.split_whitespace().map(String::from).collect();
    if let Some(l) = line {
        args.push(format!("+{l}"));
    }
    args.push(rel.to_string());
    let name = rel.rsplit('/').next().unwrap_or(rel).to_string();
    spawn_named(app, opts, ctx, PaneKind::Command, root.to_string(), args, &format!("{exe} {name}"), Some(name));
}

// ---------------------------------------------------------------------------
// Menu

fn open_menu(app: &mut App, x: u16, y: u16, items: Vec<MenuItem>) {
    if !items.is_empty() {
        app.menu = Some(Menu { x, y, items, selected: 0 });
    }
}

/// `m`: the menu for the selection, under its row.
fn menu_at_selection(app: &mut App) {
    let Some(sel) = app.selected.clone() else { return };
    let at = app
        .hits
        .iter()
        .find(|(_, h)| *h == Hit::Row(sel.clone()))
        .or_else(|| app.hits.iter().find(|(_, h)| *h == Hit::Main))
        .map(|(r, _)| (r.x + 3, r.y + 1));
    let (x, y) = at.unwrap_or((2, 2));
    let items = app.menu_for(&sel);
    open_menu(app, x, y, items);
}

fn menu_key(app: &mut App, key: KeyEvent, opts: &Options, ctx: &Ctx) {
    let Some(m) = app.menu.as_mut() else { return };
    let n = m.items.len();
    match key.code {
        KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => m.selected = (m.selected + 1) % n,
        KeyCode::Up | KeyCode::Char('k') | KeyCode::BackTab => m.selected = (m.selected + n - 1) % n,
        KeyCode::Enter | KeyCode::Char(' ') | KeyCode::Right | KeyCode::Char('l') => {
            let item = m.items[m.selected].clone();
            app.menu = None;
            run_menu_item(app, opts, ctx, item);
        }
        _ => app.menu = None,
    }
}

fn run_menu_item(app: &mut App, opts: &Options, ctx: &Ctx, item: MenuItem) {
    match item {
        MenuItem::Type(id) => {
            app.select(RowRef::Pane(id.clone()));
            start_typing(app, opts, &id);
        }
        MenuItem::Resume(id) => resume(app, opts, ctx, &id),
        MenuItem::Restart(id) => restart(app, opts, &id),
        MenuItem::Close(r) => {
            app.select(r.clone());
            close(app, opts, r);
        }
        MenuItem::OpenEditor(root) => open_in_editor(app, &root),
        MenuItem::NewClaude(root) => spawn(app, opts, ctx, PaneKind::Claude, root, vec![], "Claude Code"),
        MenuItem::NewShell(root) => spawn(app, opts, ctx, PaneKind::Shell, root, vec![], "a shell"),
        MenuItem::NewTab(root) => {
            app.place_next = Some(None);
            spawn(app, opts, ctx, PaneKind::Shell, root, vec![], "a shell");
        }
        MenuItem::Reveal(path) => reveal_in_finder(app, &path),
        MenuItem::Run(action, _) => {
            perform(app, action, opts, ctx);
        }
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

/// Track the row under the mouse. Returns true when it changed.
pub fn on_hover(app: &mut App, ev: MouseEvent) -> bool {
    let before = app.hover_url.clone();
    app.mouse_pos = Some((ev.column, ev.row));
    // Redraw when entering or leaving a link, or moving along text where
    // one might be.
    let over_link = before.as_ref().is_some_and(|(r, x0, x1, _)| ev.row == *r && ev.column >= *x0 && ev.column < *x1);
    let link_changed = before.is_some() != over_link || before.is_none();
    let hit = app.hit(ev.column, ev.row).map(|(_, h)| h);
    if let Some(m) = app.menu.as_mut() {
        if let Some(Hit::MenuItem(i)) = hit
            && m.selected != i
        {
            m.selected = i;
            return true;
        }
        return false;
    }
    let hover = match hit {
        Some(Hit::Row(r) | Hit::Close(r)) => Some(r),
        Some(Hit::Add(root)) => Some(RowRef::Project(root)),
        _ => None,
    };
    let changed = hover != app.hover;
    app.hover = hover;
    changed || link_changed
}

pub fn on_mouse(app: &mut App, ev: MouseEvent, opts: &Options, ctx: &Ctx, input: &mut Input) {
    let Some((rect, hit)) = app.hit(ev.column, ev.row) else { return };
    let (col, row) = (ev.column, ev.row);
    // Ctrl-P is open: a click outside it closes it.
    if app.finder.is_some() && matches!(ev.kind, MouseEventKind::Down(_)) && !matches!(hit, Hit::FinderItem(_)) {
        app.finder = None;
        return;
    }
    // An open menu takes the next click: on an item runs it, anywhere else closes it.
    if app.menu.is_some() && !matches!(ev.kind, MouseEventKind::Up(_) | MouseEventKind::Moved) {
        let item = match (&hit, ev.kind) {
            (Hit::MenuItem(i), MouseEventKind::Down(MouseButton::Left)) => {
                app.menu.as_ref().and_then(|m| m.items.get(*i).cloned())
            }
            _ => None,
        };
        app.menu = None;
        if let Some(item) = item {
            run_menu_item(app, opts, ctx, item);
        }
        if !matches!(ev.kind, MouseEventKind::Down(MouseButton::Right)) {
            return;
        }
    }
    if ev.kind == MouseEventKind::Down(MouseButton::Right) {
        // A program you're typing into that reads the mouse gets the click.
        if let Hit::Terminal(id) = &hit
            && app.typing
        {
            forward_mouse(app, id, 2, col - rect.x, row - rect.y, false, ctx);
            return;
        }
        let target = match &hit {
            Hit::Row(r) | Hit::Close(r) => Some(r.clone()),
            Hit::Add(root) => Some(RowRef::Project(root.clone())),
            Hit::Terminal(_) | Hit::Conversation(_) | Hit::Main | Hit::Leaf(_) => app.selected.clone(),
            _ => None,
        };
        if let Some(r) = target {
            app.select(r.clone());
            let items = app.menu_for(&r);
            open_menu(app, col, row.saturating_add(1), items);
        }
        return;
    }
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
                Hit::FileTree | Hit::FileRow(_) | Hit::FileResult(_) => {
                    if let Some(root) = app.selected_root() {
                        let v = app.files_view(&root);
                        if let Some(s) = v.search.as_mut().filter(|s| s.results.is_some() && !s.editing) {
                            s.scroll = (s.scroll as isize + lines).max(0) as usize;
                        } else {
                            v.scroll = (v.scroll as isize + lines).max(0) as usize;
                        }
                    }
                }
                Hit::FileView => {
                    if let Some(root) = app.selected_root() {
                        let v = app.files_view(&root);
                        v.file_scroll = (v.file_scroll as isize + lines).max(0) as usize;
                    }
                }
                Hit::ChangeTree | Hit::ChangeRow(_) => {
                    if let Some(root) = app.selected_root() {
                        let v = app.view_mut(&root);
                        v.scroll = (v.scroll as isize + lines).max(0) as usize;
                    }
                }
                Hit::Diff => {
                    if let Some(root) = app.selected_root() {
                        let v = app.view_mut(&root);
                        v.diff_scroll = (v.diff_scroll as isize + lines).max(0) as usize;
                    }
                }
                Hit::Row(_) | Hit::Sidebar | Hit::Close(_) | Hit::Add(_) => {
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
            // A click on an underlined link opens it, unless a program in
            // the pane asked for the mouse and you're typing into it.
            let program_wants_mouse = matches!(&hit, Hit::Terminal(id) if app.typing
                && app.terms.get(id).is_some_and(|t| t.parser.screen().mouse_protocol_mode() != vt100::MouseProtocolMode::None));
            if let Some((r, x0, x1, url)) = app.hover_url.clone()
                && row == r
                && col >= x0
                && col < x1
                && !program_wants_mouse
            {
                if app.conn == Conn::Demo {
                    app.toast(format!("Would open {url} (demo)"));
                } else {
                    match crate::links::open(&url) {
                        Ok(()) => app.toast(format!("Opened {}", crate::ui::fit(&url, 60))),
                        Err(e) => app.error(format!("Couldn't open the link: {e}")),
                    }
                }
                return;
            }
            // A click in a pane focuses it first (Herdr, zellij).
            let inside = |r: &ratatui::layout::Rect| col >= r.x && col < r.right() && row >= r.y && row < r.bottom();
            if !matches!(hit, Hit::Split(_))
                && let Some(leaf) = app.leaf_rects.iter().find(|(_, r)| inside(r)).map(|(id, _)| *id)
                && (app.focus != Focus::Main || app.tab().is_some_and(|t| t.focus != leaf))
            {
                app.focus_leaf(leaf);
            }
            match hit {
                Hit::Row(r) => {
                    let was = app.selected.as_ref() == Some(&r);
                    app.select(r);
                    app.focus = Focus::Sidebar;
                    if double || was {
                        open(app, opts, ctx);
                    }
                }
                Hit::Close(r) => {
                    app.select(r.clone());
                    close(app, opts, r);
                }
                Hit::Add(root) => {
                    app.select(RowRef::Project(root.clone()));
                    let items = vec![MenuItem::NewClaude(root.clone()), MenuItem::NewShell(root)];
                    open_menu(app, col.saturating_sub(16), row + 1, items);
                }
                Hit::Resume(id) => resume(app, opts, ctx, &id),
                Hit::FinderItem(i) => {
                    if let Some(f) = app.finder.as_mut() {
                        f.selected = i;
                    }
                    if let Some(out) = app.finder_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)) {
                        finder_outcome(app, out, opts, ctx);
                    }
                }
                Hit::FileRow(i) => {
                    app.focus = Focus::Main;
                    app.typing = false;
                    click_file(app, i, double, opts, ctx);
                }
                Hit::FileResult(i) => {
                    app.focus = Focus::Main;
                    if let Some(root) = app.selected_root() {
                        if let Some(s) = app.files_view(&root).search.as_mut() {
                            s.selected = i;
                        }
                        open_result(app, &root);
                    }
                }
                Hit::FileTree => {
                    app.focus = Focus::Main;
                    if let Some(root) = app.selected_root() {
                        app.files_view(&root).focus_file = false;
                    }
                }
                Hit::FileView => {
                    app.focus = Focus::Main;
                    app.typing = false;
                    if let Some(root) = app.selected_root() {
                        app.files_view(&root).focus_file = true;
                    }
                }
                Hit::Action(a) => {
                    app.focus = Focus::Main;
                    app.typing = false;
                    perform(app, a, opts, ctx);
                }
                Hit::Compose(id) => {
                    app.focus = Focus::Main;
                    if app.compose.as_ref().is_none_or(|c| c.session != id) {
                        app.close_compose();
                        app.start_compose(&id);
                    }
                }
                Hit::ChangeRow(i) => {
                    app.focus = Focus::Main;
                    app.typing = false;
                    click_change(app, i, double);
                }
                Hit::ChangeTree => {
                    app.focus = Focus::Main;
                    if let Some(root) = app.selected_root() {
                        app.view_mut(&root).focus_diff = false;
                    }
                }
                Hit::Diff => {
                    app.focus = Focus::Main;
                    app.typing = false;
                    if let Some(root) = app.selected_root() {
                        app.view_mut(&root).focus_diff = true;
                    }
                }
                Hit::MenuItem(_) | Hit::Dismiss => {}
                Hit::NewMenu => {
                    if let Some(root) = app.selected_root() {
                        let items = vec![
                            MenuItem::NewClaude(root.clone()),
                            MenuItem::NewShell(root.clone()),
                            MenuItem::NewTab(root),
                            MenuItem::Run(Action::OpenProject, "Open a project…  ctrl-o"),
                        ];
                        open_menu(app, col, row.saturating_sub(5), items);
                    }
                }
                Hit::AppMenu => {
                    let items = vec![
                        MenuItem::Run(Action::Finder, "Find a file…  ctrl-p"),
                        MenuItem::Run(Action::Themes, "Theme…  t"),
                        MenuItem::Run(Action::ToggleHistory, "Past sessions  h"),
                        MenuItem::Run(Action::ToggleMouse, "Mouse capture  F12"),
                        MenuItem::Run(Action::Help, "All keys  ?"),
                        MenuItem::Run(Action::Quit, "Quit  q"),
                    ];
                    open_menu(app, col.saturating_sub(24), row.saturating_sub(7), items);
                }
                Hit::SpaceTab(i) => {
                    app.switch_tab(i);
                    app.focus = Focus::Main;
                }
                Hit::NewTab => {
                    app.place_next = Some(None);
                    new_pane(app, opts, ctx, PaneKind::Shell);
                }
                Hit::Leaf(_) => {}
                Hit::Split(id) => app.dragging = Some(id),
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
        MouseEventKind::Drag(MouseButton::Left) if app.dragging.is_some() => {
            // Move the split border under the pointer.
            let split = app.dragging.unwrap_or_default();
            if let Some((_, dir, area, _)) = app.split_rects.iter().find(|(id, ..)| *id == split).cloned() {
                let ratio = match dir {
                    crate::layout::Dir::Right => (col.saturating_sub(area.x) as f32 + 0.5) / area.width.max(1) as f32,
                    crate::layout::Dir::Down => (row.saturating_sub(area.y) as f32 + 0.5) / area.height.max(1) as f32,
                };
                if let Some(tab) = app.tab_mut() {
                    tab.set_ratio(split, ratio);
                }
            }
        }
        MouseEventKind::Up(MouseButton::Left) if app.dragging.is_some() => app.dragging = None,
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
