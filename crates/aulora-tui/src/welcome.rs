//! The first-run checklist: connect Claude Code, add the VS Code
//! extension, open a project, and the few keys that matter. Shown once;
//! "Getting started" in the menu brings it back.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};

use crate::app::{App, Conn};
use crate::theme::{self, bold, dim, faint, fg, text};
use crate::ui::sp;

pub struct Welcome {
    /// Whether the editor extension is installed; None while checking.
    pub extension: std::sync::Arc<std::sync::Mutex<Option<bool>>>,
    /// In Terminal.app, whether Option is sent as Meta; checked again
    /// every two seconds, so ticking the box shows at once.
    pub meta: std::sync::Arc<std::sync::Mutex<Option<bool>>>,
}

/// In Terminal.app, whether the default profile sends Option as Meta,
/// which Option-Delete (delete a word) and Option-Enter (new line) need.
/// Without it Terminal.app sends Option-Delete as a plain Delete, and no
/// program can tell the two apart. None in other terminals.
pub fn option_as_meta() -> Option<bool> {
    if std::env::var("TERM_PROGRAM").ok()? != "Apple_Terminal" {
        return None;
    }
    let out = std::process::Command::new("defaults")
        .args(["read", "com.apple.Terminal", "Default Window Settings"])
        .output()
        .ok()?;
    let profile = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let script =
        r#"defaults export com.apple.Terminal - | plutil -extract "Window Settings.$1.useOptionAsMetaKey" raw -o - -"#;
    let out = std::process::Command::new("sh").args(["-c", script, "sh", &profile]).output().ok()?;
    Some(matches!(String::from_utf8_lossy(&out.stdout).trim(), "true" | "1"))
}

fn config() -> serde_json::Value {
    std::fs::read_to_string(crate::config_path_pub())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .filter(serde_json::Value::is_object)
        .unwrap_or_else(|| serde_json::json!({}))
}

pub fn seen() -> bool {
    config().get("welcomed").and_then(serde_json::Value::as_bool).unwrap_or(false)
}

fn mark_seen() {
    let mut v = config();
    v["welcomed"] = serde_json::json!(true);
    let _ = aulora_core::paths::ensure_home();
    let _ = std::fs::write(crate::config_path_pub(), serde_json::to_string_pretty(&v).unwrap_or_default());
}

/// Run an aulora subcommand in the background and report through `done`.
fn run_self(args: &[&str]) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let out = std::process::Command::new(exe).args(args).output().map_err(|e| e.to_string())?;
    if out.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&out.stderr).trim().to_string()) }
}

impl App {
    pub fn open_welcome(&mut self) {
        let extension = std::sync::Arc::new(std::sync::Mutex::new(None));
        if self.conn != Conn::Demo {
            let e = extension.clone();
            let editor = self.editor.clone();
            std::thread::spawn(move || {
                let cli = if editor.contains("cursor") { "cursor" } else { "code" };
                let installed = std::process::Command::new(cli)
                    .args(["--list-extensions"])
                    .output()
                    .map(|o| String::from_utf8_lossy(&o.stdout).lines().any(|l| l == "aulora.aulora"))
                    .unwrap_or(false);
                *e.lock().unwrap() = Some(installed);
            });
        } else {
            *extension.lock().unwrap() = Some(true);
        }
        let meta = std::sync::Arc::new(std::sync::Mutex::new(None));
        if self.conn != Conn::Demo {
            let weak = std::sync::Arc::downgrade(&meta);
            std::thread::spawn(move || {
                // Until the checklist closes.
                while let Some(m) = weak.upgrade() {
                    let now = option_as_meta();
                    *m.lock().unwrap() = now;
                    drop(m);
                    if now.is_none() {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_secs(2));
                }
            });
        }
        self.welcome = Some(Welcome { extension, meta });
    }

    /// A key while the checklist is open. Returns an action to run.
    pub fn welcome_key(&mut self, key: KeyEvent) -> Option<crate::keymap::Action> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('H') | KeyCode::Char('h') if !self.hooks_installed => match run_self(&["hooks", "install"]) {
                Ok(()) => {
                    self.hooks_installed = true;
                    self.toast("Claude Code is connected");
                }
                Err(e) => self.error(format!("Couldn't install the hooks: {e}")),
            },
            KeyCode::Char('V') | KeyCode::Char('v') if !ctrl => {
                let editor = if self.editor.contains("cursor") { "cursor" } else { "code" };
                match run_self(&["vscode", "install", "--editor", editor]) {
                    Ok(()) => {
                        if let Some(w) = &self.welcome {
                            *w.extension.lock().unwrap() = Some(true);
                        }
                        self.toast("Extension installed; reload VS Code windows to start it");
                    }
                    Err(e) => self.error(format!("Couldn't install the extension: {e}")),
                }
            }
            KeyCode::Char('o') if ctrl => {
                self.welcome = None;
                mark_seen();
                return Some(crate::keymap::Action::OpenProject);
            }
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => {
                self.welcome = None;
                mark_seen();
            }
            _ => {}
        }
        None
    }
}

pub fn draw(f: &mut Frame, area: Rect, app: &App) {
    let t = theme::get();
    let Some(w) = &app.welcome else { return };
    let ext = *w.extension.lock().unwrap();
    let meta = *w.meta.lock().unwrap();
    let width = area.width.saturating_sub(4).min(76);
    // The wordmark when there's room for it and the checklist.
    let art = area.height >= 35;
    let height = (if art { 31 } else { 25 }).min(area.height.saturating_sub(2));
    let r = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    };
    f.render_widget(Clear, r);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(fg(t.accent))
        .style(Style::new().bg(t.panel))
        .title(sp(" welcome to aulora ", bold(t.accent)));
    let inner = block.inner(r);
    f.render_widget(block, r);
    let check = |ok: Option<bool>| match ok {
        Some(true) => sp(" ✓ ", fg(t.green)),
        Some(false) => sp(" ○ ", fg(t.yellow)),
        None => sp(" … ", faint()),
    };
    let key = |k: &str| sp(format!(" {k} "), bold(t.accent));
    let mut lines = vec![Line::raw("")];
    if art {
        lines.extend(crate::brand::banner(t, inner.width.saturating_sub(4), t.panel).into_iter().map(|l| {
            let mut spans = vec![sp("  ", text())];
            spans.extend(l.spans);
            Line::from(spans)
        }));
    } else {
        // The banner's tagline says it when the banner is there.
        lines.push(Line::from(sp(" Every Claude session and terminal, for every workspace, in one place.", dim())));
    }
    lines.extend([
        Line::raw(""),
        Line::from(vec![
            check(Some(app.hooks_installed)),
            sp("Connect Claude Code", text()),
            sp("  so sessions show up live", faint()),
        ]),
    ]);
    if !app.hooks_installed {
        lines.push(Line::from(vec![
            sp("     ", text()),
            key("H"),
            sp("install the hooks (backs up ~/.claude/settings.json)", faint()),
        ]));
    }
    lines.push(Line::from(vec![
        check(ext),
        sp("VS Code extension", text()),
        sp("  to send prompts into VS Code terminals", faint()),
    ]));
    if ext == Some(false) {
        lines.push(Line::from(vec![
            sp("     ", text()),
            key("V"),
            sp("install it, then reload VS Code windows", faint()),
        ]));
    }
    lines.push(Line::from(vec![
        check(Some(!app.rows().is_empty())),
        sp("Open a workspace", text()),
        sp("  any folder becomes one", faint()),
    ]));
    lines.push(Line::from(vec![
        sp("     ", text()),
        key("ctrl-o"),
        sp("pick a repo on disk, a recent one, or type a path", faint()),
    ]));
    lines.push(Line::raw(""));
    lines.push(Line::from(sp(" The keys that matter", bold(t.text))));
    for (k, what) in [
        ("⇥", "move between the sidebar, the main view and the panel"),
        ("⏎", "type into a pane; ctrl-q gives the keys back to aulora"),
        ("c  C", "a new shell, or Claude, in the selected workspace"),
        ("d f", "Changes or Files in the panel on the right"),
        ("ctrl-p", "find anything: files, # workspaces, @ agents, > actions"),
        ("?", "every key, searchable"),
    ] {
        lines.push(Line::from(vec![sp(format!("   {k:<8}"), bold(t.accent)), sp(what, dim())]));
    }
    if let Some(on) = meta {
        lines.push(Line::raw(""));
        lines.push(Line::from(vec![
            check(Some(on)),
            sp("Option as Meta in Terminal.app", text()),
            sp("  for Option-Delete and Option-Enter", faint()),
        ]));
        if !on {
            lines
                .push(Line::from(sp("     Settings → Profiles → Keyboard → tick \"Use Option as Meta key\"", faint())));
        }
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(vec![key("⏎"), sp("start", faint())]));
    f.render_widget(Paragraph::new(lines).wrap(ratatui::widgets::Wrap { trim: false }), inner);
}
