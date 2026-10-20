//! Turn key presses into the bytes a terminal program expects.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// xterm modifier parameter: 1 + shift(1) + alt(2) + ctrl(4).
fn modifier_param(m: KeyModifiers) -> u8 {
    1 + m.contains(KeyModifiers::SHIFT) as u8
        + 2 * m.contains(KeyModifiers::ALT) as u8
        + 4 * m.contains(KeyModifiers::CONTROL) as u8
}

/// Encode a key for a program running in a pane. `app_cursor` is the
/// DECCKM mode, under which plain arrow keys use SS3 instead of CSI.
pub fn encode(key: KeyEvent, app_cursor: bool) -> Option<Vec<u8>> {
    let m = key.modifiers;
    let alt = m.contains(KeyModifiers::ALT);
    let ctrl = m.contains(KeyModifiers::CONTROL);
    let shift = m.contains(KeyModifiers::SHIFT);
    let plain = !alt && !ctrl && !shift;
    let modp = modifier_param(m);

    let csi_letter = |letter: char| -> Vec<u8> {
        if modp > 1 {
            format!("\x1b[1;{modp}{letter}").into_bytes()
        } else if app_cursor {
            format!("\x1bO{letter}").into_bytes()
        } else {
            format!("\x1b[{letter}").into_bytes()
        }
    };
    let csi_tilde = |n: u8| -> Vec<u8> {
        if modp > 1 { format!("\x1b[{n};{modp}~").into_bytes() } else { format!("\x1b[{n}~").into_bytes() }
    };
    let with_alt = |mut bytes: Vec<u8>| -> Vec<u8> {
        if alt {
            bytes.insert(0, 0x1b);
        }
        bytes
    };

    let bytes = match key.code {
        KeyCode::Char(c) if ctrl => {
            let b = match c.to_ascii_lowercase() {
                c @ 'a'..='z' => c as u8 - b'a' + 1,
                '@' | ' ' | '2' => 0,
                '[' | '3' => 0x1b,
                '\\' | '4' => 0x1c,
                ']' | '5' => 0x1d,
                '^' | '6' => 0x1e,
                '_' | '-' | '7' => 0x1f,
                '?' | '8' => 0x7f,
                _ => return None,
            };
            with_alt(vec![b])
        }
        KeyCode::Char(c) => {
            let mut s = [0u8; 4];
            with_alt(c.encode_utf8(&mut s).as_bytes().to_vec())
        }
        // Shift+Enter: what Claude Code's terminal setup sends for a newline.
        KeyCode::Enter if shift && !ctrl => b"\x1b\r".to_vec(),
        KeyCode::Enter => with_alt(b"\r".to_vec()),
        KeyCode::Tab if plain => b"\t".to_vec(),
        KeyCode::Tab | KeyCode::BackTab if shift => b"\x1b[Z".to_vec(),
        KeyCode::Tab => with_alt(b"\t".to_vec()),
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        // Ctrl-Delete deletes a word, as Ctrl-W does in shells and Claude Code.
        KeyCode::Backspace if ctrl => with_alt(vec![0x17]),
        KeyCode::Backspace => with_alt(vec![0x7f]),
        KeyCode::Esc => with_alt(vec![0x1b]),
        KeyCode::Up => csi_letter('A'),
        KeyCode::Down => csi_letter('B'),
        KeyCode::Right => csi_letter('C'),
        KeyCode::Left => csi_letter('D'),
        KeyCode::Home => csi_letter('H'),
        KeyCode::End => csi_letter('F'),
        KeyCode::Insert => csi_tilde(2),
        KeyCode::Delete => csi_tilde(3),
        KeyCode::PageUp => csi_tilde(5),
        KeyCode::PageDown => csi_tilde(6),
        KeyCode::F(n @ 1..=4) => {
            let letter = (b'P' + n - 1) as char;
            if modp > 1 { format!("\x1b[1;{modp}{letter}").into_bytes() } else { format!("\x1bO{letter}").into_bytes() }
        }
        KeyCode::F(n @ 5..=12) => {
            let code = [15, 17, 18, 19, 20, 21, 23, 24][(n - 5) as usize];
            csi_tilde(code)
        }
        _ => return None,
    };
    Some(bytes)
}

/// Wrap pasted text the way the program asked for.
pub fn paste(text: &str, bracketed: bool) -> Vec<u8> {
    // Terminals send CR for newlines in pastes.
    let body = text.replace("\r\n", "\r").replace('\n', "\r");
    if bracketed { format!("\x1b[200~{body}\x1b[201~").into_bytes() } else { body.into_bytes() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(code: KeyCode, m: KeyModifiers) -> Vec<u8> {
        encode(KeyEvent::new(code, m), false).unwrap()
    }

    #[test]
    fn printable_and_control() {
        assert_eq!(k(KeyCode::Char('a'), KeyModifiers::NONE), b"a");
        assert_eq!(k(KeyCode::Char('é'), KeyModifiers::NONE), "é".as_bytes());
        assert_eq!(k(KeyCode::Char('c'), KeyModifiers::CONTROL), [3]);
        assert_eq!(k(KeyCode::Char('x'), KeyModifiers::ALT), b"\x1bx");
        assert_eq!(k(KeyCode::Char(' '), KeyModifiers::CONTROL), [0]);
        assert_eq!(k(KeyCode::Enter, KeyModifiers::NONE), b"\r");
        assert_eq!(k(KeyCode::Enter, KeyModifiers::SHIFT), b"\x1b\r");
        assert_eq!(k(KeyCode::Backspace, KeyModifiers::NONE), [0x7f]);
        assert_eq!(k(KeyCode::BackTab, KeyModifiers::SHIFT), b"\x1b[Z");
    }

    #[test]
    fn cursor_keys_follow_application_mode() {
        assert_eq!(k(KeyCode::Up, KeyModifiers::NONE), b"\x1b[A");
        assert_eq!(encode(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE), true).unwrap(), b"\x1bOA");
        assert_eq!(k(KeyCode::Right, KeyModifiers::CONTROL), b"\x1b[1;5C");
        assert_eq!(k(KeyCode::Left, KeyModifiers::ALT | KeyModifiers::SHIFT), b"\x1b[1;4D");
    }

    #[test]
    fn editing_and_function_keys() {
        assert_eq!(k(KeyCode::Delete, KeyModifiers::NONE), b"\x1b[3~");
        assert_eq!(k(KeyCode::PageUp, KeyModifiers::SHIFT), b"\x1b[5;2~");
        assert_eq!(k(KeyCode::F(1), KeyModifiers::NONE), b"\x1bOP");
        assert_eq!(k(KeyCode::F(5), KeyModifiers::NONE), b"\x1b[15~");
        assert_eq!(k(KeyCode::F(12), KeyModifiers::NONE), b"\x1b[24~");
    }

    #[test]
    fn pastes() {
        assert_eq!(paste("a\nb", true), b"\x1b[200~a\rb\x1b[201~");
        assert_eq!(paste("a\r\nb", false), b"a\rb");
    }
}
