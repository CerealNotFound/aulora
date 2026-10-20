//! Writing a prompt for a Claude session that runs in an editor's terminal.
//! The daemon has the editor's aulora extension type it there, so it shows
//! up in the editor as if typed, in the same session.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::App;
use crate::client;

pub struct Compose {
    pub session: String,
    pub text: String,
    /// The highlighted entry of the slash-command menu.
    pub menu: usize,
    /// Byte offset into `text`, always on a char boundary.
    pub cursor: usize,
}

/// What came back from a send running in the background.
pub enum Sent {
    Ok { title: String, editor: String },
    Failed { session: String, text: String, error: String },
}

impl Compose {
    pub fn new(session: String, draft: String) -> Compose {
        let cursor = draft.len();
        Compose { session, text: draft, cursor, menu: 0 }
    }

    pub fn insert(&mut self, s: &str) {
        let s = s.replace("\r\n", "\n").replace('\r', "\n");
        self.text.insert_str(self.cursor, &s);
        self.cursor += s.len();
    }

    fn prev(&self) -> usize {
        self.text[..self.cursor].char_indices().next_back().map_or(0, |(i, _)| i)
    }

    fn next(&self) -> usize {
        self.text[self.cursor..].chars().next().map_or(self.cursor, |c| self.cursor + c.len_utf8())
    }

    fn line_start(&self) -> usize {
        self.text[..self.cursor].rfind('\n').map_or(0, |i| i + 1)
    }

    fn line_end(&self) -> usize {
        self.text[self.cursor..].find('\n').map_or(self.text.len(), |i| self.cursor + i)
    }

    /// Edit for a key; returns what it asks for beyond editing.
    pub fn key(&mut self, key: KeyEvent) -> Option<Action> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Esc => return Some(Action::Close),
            // Shift-Enter, Alt-Enter or Ctrl-J for a new line (Claude Code).
            KeyCode::Enter if shift || alt => self.insert("\n"),
            KeyCode::Char('j') if ctrl => self.insert("\n"),
            KeyCode::Enter => return Some(Action::Send),
            KeyCode::Backspace if alt || ctrl => self.delete_word(),
            KeyCode::Backspace => {
                let p = self.prev();
                self.text.replace_range(p..self.cursor, "");
                self.cursor = p;
            }
            KeyCode::Delete => {
                let n = self.next();
                self.text.replace_range(self.cursor..n, "");
            }
            KeyCode::Left => self.cursor = self.prev(),
            KeyCode::Right => self.cursor = self.next(),
            KeyCode::Home => self.cursor = self.line_start(),
            KeyCode::End => self.cursor = self.line_end(),
            KeyCode::Char('a') if ctrl => self.cursor = self.line_start(),
            KeyCode::Char('e') if ctrl => self.cursor = self.line_end(),
            KeyCode::Char('w') if ctrl => self.delete_word(),
            KeyCode::Char('u') if ctrl => {
                let s = self.line_start();
                self.text.replace_range(s..self.cursor, "");
                self.cursor = s;
            }
            KeyCode::Char(c) if !ctrl => {
                let mut b = [0u8; 4];
                self.insert(c.encode_utf8(&mut b));
            }
            _ => {}
        }
        None
    }

    fn delete_word(&mut self) {
        let before = &self.text[..self.cursor];
        let trimmed = before.trim_end();
        let start = trimmed.rfind(|c: char| c.is_whitespace()).map_or(0, |i| i + 1);
        self.text.replace_range(start..self.cursor, "");
        self.cursor = start;
    }
}

/// Edit a one-line text field (filter, search, Ctrl-P, help) as a shell
/// would: Option- or Ctrl-Delete and Ctrl-W delete a word, Ctrl-U the line.
/// Returns true if the key changed the text.
pub fn edit_line(s: &mut String, key: &KeyEvent) -> bool {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    match key.code {
        KeyCode::Backspace if alt || ctrl => delete_word(s),
        KeyCode::Char('w') if ctrl => delete_word(s),
        KeyCode::Char('h') if ctrl => s.pop().is_some(),
        KeyCode::Backspace => s.pop().is_some(),
        KeyCode::Char('u') if ctrl => {
            let had = !s.is_empty();
            s.clear();
            had
        }
        KeyCode::Char(c) if !ctrl && !alt => {
            s.push(c);
            true
        }
        _ => false,
    }
}

fn delete_word(s: &mut String) -> bool {
    let trimmed = s.trim_end_matches(char::is_whitespace);
    let start = trimmed.rfind(|c: char| c.is_whitespace() || c == '/').map_or(0, |i| i + 1);
    let changed = start < s.len();
    s.truncate(start);
    changed
}

pub enum Action {
    Send,
    Close,
}

impl App {
    /// Start writing a prompt for a session, picking up any draft.
    pub fn start_compose(&mut self, session: &str) {
        let draft = self.drafts.remove(session).unwrap_or_default();
        self.compose = Some(Compose::new(session.to_string(), draft));
    }

    /// Close the box, keeping what was written as a draft.
    pub fn close_compose(&mut self) {
        if let Some(c) = self.compose.take()
            && !c.text.trim().is_empty()
        {
            self.drafts.insert(c.session, c.text);
        }
    }

    /// Send the prompt in the background; the answer arrives in `poll_sent`.
    pub fn send_compose(&mut self, socket: &std::path::Path) {
        let Some(c) = self.compose.take() else { return };
        let text = c.text.trim_end().to_string();
        if text.trim().is_empty() {
            self.compose = Some(c);
            return;
        }
        // The box stays open and empty for the next prompt (Claude Code).
        self.compose = Some(Compose::new(c.session.clone(), String::new()));
        let Some(s) = self.session(&c.session) else { return };
        let title = s.display_title();
        let editor = s.terminal.editor.clone().unwrap_or_else(|| "the editor".into());
        if self.conn == crate::app::Conn::Demo {
            self.toast(format!("Would type it into {editor} (demo)"));
            return;
        }
        let tx = self.sent_tx.clone();
        let socket = socket.to_path_buf();
        let session = c.session.clone();
        self.toast(format!("Typing into {editor}…"));
        std::thread::spawn(move || {
            let msg = aulora_core::proto::ClientMsg::SendPrompt { session_id: session.clone(), text: text.clone() };
            let result = match client::command(&socket, &msg) {
                Some(aulora_core::proto::ServerMsg::Error { message }) => {
                    Sent::Failed { session, text, error: message }
                }
                Some(_) => Sent::Ok { title, editor },
                None => Sent::Failed { session, text, error: "the daemon didn't answer".into() },
            };
            let _ = tx.send(result);
        });
    }

    /// Take in finished sends. Returns true if any did.
    pub fn poll_sent(&mut self) -> bool {
        let mut any = false;
        while let Ok(r) = self.sent_rx.try_recv() {
            any = true;
            match r {
                Sent::Ok { title, editor } => self.toast(format!("Sent to {title} in {editor}")),
                Sent::Failed { session, text, error } => {
                    // Keep the words; nothing was lost. Back in the box if
                    // it's still open and empty, else as a draft.
                    match self.compose.as_mut().filter(|c| c.session == session && c.text.is_empty()) {
                        Some(c) => c.insert(&text),
                        None => {
                            self.drafts.insert(session, text);
                        }
                    }
                    self.error(format!("Not sent: {error}"));
                }
            }
        }
        any
    }

    /// The slash commands matching what's being typed, when the prompt is a
    /// command being typed.
    pub fn slash_menu(&mut self) -> Vec<(String, String)> {
        let Some(c) = &self.compose else { return vec![] };
        let Some(typed) = crate::slash::typing(&c.text).map(str::to_string) else { return vec![] };
        let Some(root) = self.session(&c.session).map(|s| s.cwd.clone()) else { return vec![] };
        let all = self.slash.entry(root.clone()).or_insert_with(|| crate::slash::commands(&root));
        crate::slash::matching(all, &typed).into_iter().cloned().collect()
    }

    /// A key while the slash menu is open: ↑↓ choose, Tab completes, Enter
    /// completes and sends. Returns true if the menu took the key.
    pub fn slash_key(&mut self, key: &KeyEvent, socket: &std::path::Path) -> bool {
        let menu = self.slash_menu();
        if menu.is_empty() {
            return false;
        }
        let Some(c) = self.compose.as_mut() else { return false };
        match key.code {
            KeyCode::Up => c.menu = c.menu.saturating_sub(1),
            KeyCode::Down => c.menu = (c.menu + 1).min(menu.len() - 1),
            KeyCode::Tab | KeyCode::Enter => {
                let name = &menu[c.menu.min(menu.len() - 1)].0;
                c.text = format!("/{name}");
                c.cursor = c.text.len();
                c.menu = 0;
                if key.code == KeyCode::Enter {
                    self.send_compose(socket);
                } else if let Some(c) = self.compose.as_mut() {
                    c.insert(" ");
                }
            }
            _ => {
                c.menu = 0;
                return false;
            }
        }
        true
    }

    /// Whether a session can take a prompt from here.
    pub fn can_compose(&self, session: &str) -> bool {
        self.session(session).is_some_and(|s| s.state.is_live() && s.terminal.editor.is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn one_line_fields_delete_words() {
        let mut s = String::from("web/app/billing page");
        assert!(edit_line(&mut s, &KeyEvent::new(KeyCode::Backspace, KeyModifiers::ALT)));
        assert_eq!(s, "web/app/billing ");
        assert!(edit_line(&mut s, &KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL)));
        assert_eq!(s, "web/app/");
        edit_line(&mut s, &k(KeyCode::Char('x')));
        assert_eq!(s, "web/app/x");
        edit_line(&mut s, &KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert!(s.is_empty());
    }

    #[test]
    fn edits_like_a_prompt_box() {
        let mut c = Compose::new("s".into(), String::new());
        for ch in "fix tst".chars() {
            c.key(k(KeyCode::Char(ch)));
        }
        c.key(k(KeyCode::Left));
        c.key(k(KeyCode::Left));
        c.key(k(KeyCode::Char('e')));
        assert_eq!(c.text, "fix test");
        c.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
        assert_eq!(c.text, "fix te\nst");
        c.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert_eq!(c.text, "fix te\nst", "nothing before the cursor on this line");
        c.key(k(KeyCode::End));
        c.key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL));
        assert_eq!(c.text, "fix te\n");
        assert!(matches!(c.key(k(KeyCode::Enter)), Some(Action::Send)));
        c.insert("é✓");
        c.key(k(KeyCode::Backspace));
        assert_eq!(c.text, "fix te\né");
    }
}
