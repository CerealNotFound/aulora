//! Claude Code's question picker, drawn and driven as Claude Code draws it
//! in its own terminal (checked against 2.1.304), for sessions running in
//! an editor: what you press here is pressed there.
//!
//! One question: a number answers at once. "Type something." opens a field
//! in place; letters replace its label and Enter sends. Several questions:
//! a tab per question and a Submit tab; each answer moves to the next, and
//! the last shows a review with Submit and Cancel.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use aulora_core::model::{Ask, AskQuestion};

use crate::theme::{self, bold, faint, fg, text};
use crate::ui::sp;

/// Where you are in a picker. Rebuilt whenever the question changes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Picker {
    pub session: String,
    /// Which ask this is for, so a new question starts fresh.
    pub sig: u64,
    /// The question shown; `questions.len()` is the review.
    pub tab: usize,
    pub cursor: usize,
    pub answers: Vec<Option<String>>,
    /// Typing an answer of your own ("Type something.").
    pub typing: Option<String>,
}

/// A row under a question.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    Choice(usize),
    /// "Type something."
    Other,
    /// "Chat about this"
    Chat,
    Submit,
    Cancel,
}

/// What a key or click did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    /// Keys to press in Claude's terminal.
    pub keys: String,
    /// Whether the picker took the key at all.
    pub took: bool,
}

impl Outcome {
    fn press(keys: impl Into<String>) -> Outcome {
        Outcome { keys: keys.into(), took: true }
    }

    fn quiet() -> Outcome {
        Outcome { keys: String::new(), took: true }
    }
}

pub fn sig(ask: &Ask) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for q in &ask.questions {
        q.question.hash(&mut h);
        for o in &q.options {
            o.label.hash(&mut h);
        }
    }
    h.finish()
}

impl Picker {
    pub fn new(session: &str, ask: &Ask) -> Picker {
        Picker {
            session: session.to_string(),
            sig: sig(ask),
            answers: vec![None; ask.questions.len()],
            ..Default::default()
        }
    }

    fn many(ask: &Ask) -> bool {
        ask.questions.len() > 1
    }

    /// The rows on the current screen. `own` is whether Claude adds its
    /// "Type something." and "Chat about this" (questions, not permission
    /// or plan prompts).
    pub fn rows(&self, ask: &Ask, own: bool) -> Vec<Row> {
        if self.tab >= ask.questions.len() {
            return vec![Row::Submit, Row::Cancel];
        }
        let q = &ask.questions[self.tab];
        let mut rows: Vec<Row> = (0..q.options.len()).map(Row::Choice).collect();
        if own {
            rows.extend([Row::Other, Row::Chat]);
        }
        rows.truncate(9);
        rows
    }

    /// Choose row `i`: what Claude's picker does when you press its number.
    pub fn pick(&mut self, ask: &Ask, own: bool, i: usize) -> Outcome {
        let rows = self.rows(ask, own);
        let Some(row) = rows.get(i).copied() else { return Outcome::quiet() };
        self.cursor = i;
        let digit = (i + 1).to_string();
        match row {
            Row::Choice(k) => {
                let label = ask.questions[self.tab].options[k].label.clone();
                self.answered(ask, label);
                Outcome::press(digit)
            }
            Row::Other => {
                self.typing = Some(String::new());
                Outcome::press(digit)
            }
            Row::Chat | Row::Submit | Row::Cancel => Outcome::press(digit),
        }
    }

    fn answered(&mut self, ask: &Ask, answer: String) {
        if let Some(a) = self.answers.get_mut(self.tab) {
            *a = Some(answer);
        }
        self.typing = None;
        if Self::many(ask) {
            self.tab += 1;
            self.cursor = 0;
        }
    }

    /// A key, as Claude's picker takes it.
    pub fn key(&mut self, ask: &Ask, own: bool, key: &ratatui::crossterm::event::KeyEvent) -> Outcome {
        use ratatui::crossterm::event::{KeyCode, KeyModifiers};
        let n = self.rows(ask, own).len();
        if let Some(t) = self.typing.as_mut() {
            return match key.code {
                KeyCode::Enter => {
                    let answer = t.clone();
                    self.answered(ask, answer);
                    Outcome::press("\r")
                }
                KeyCode::Backspace => {
                    t.pop();
                    Outcome::press("\x7f")
                }
                KeyCode::Esc => Outcome::press("\x1b"),
                KeyCode::Up | KeyCode::Down => {
                    self.typing = None;
                    let up = key.code == KeyCode::Up;
                    self.cursor = if up { self.cursor.saturating_sub(1) } else { (self.cursor + 1).min(n - 1) };
                    Outcome::press(if up { "\x1b[A" } else { "\x1b[B" })
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    t.push(c);
                    Outcome::press(c.to_string())
                }
                _ => Outcome::quiet(),
            };
        }
        match key.code {
            KeyCode::Char(c @ '1'..='9') if key.modifiers.is_empty() => self.pick(ask, own, c as usize - '1' as usize),
            KeyCode::Enter => self.pick(ask, own, self.cursor),
            KeyCode::Up | KeyCode::Char('k') => {
                self.cursor = self.cursor.saturating_sub(1);
                Outcome::quiet()
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.cursor = (self.cursor + 1).min(n.saturating_sub(1));
                Outcome::quiet()
            }
            // Between questions, as Claude's Tab and arrow keys move.
            KeyCode::Tab | KeyCode::Right if Self::many(ask) => {
                self.tab = (self.tab + 1).min(ask.questions.len());
                self.cursor = 0;
                Outcome::press(if key.code == KeyCode::Tab { "\t" } else { "\x1b[C" })
            }
            KeyCode::BackTab | KeyCode::Left if Self::many(ask) => {
                self.tab = self.tab.saturating_sub(1);
                self.cursor = 0;
                Outcome::press(if key.code == KeyCode::BackTab { "\x1b[Z" } else { "\x1b[D" })
            }
            KeyCode::Esc => Outcome::press("\x1b"),
            _ => Outcome::default(),
        }
    }
}

/// The picker's lines, as Claude Code lays them out, and which line each
/// row starts on (for clicks).
pub fn lines(p: &Picker, ask: &Ask, own: bool, width: usize) -> (Vec<Line<'static>>, Vec<(usize, usize)>) {
    let t = theme::get();
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut hits = Vec::new();
    let many = ask.questions.len() > 1;
    let chip = Style::new().bg(t.active).fg(t.text).add_modifier(Modifier::BOLD);
    if many {
        let mut spans = vec![sp("←  ", faint())];
        for (i, q) in ask.questions.iter().enumerate() {
            let mark = if p.answers.get(i).is_some_and(Option::is_some) { "☒" } else { "☐" };
            let name = q.header.clone().unwrap_or_else(|| format!("Question {}", i + 1));
            let style = if i == p.tab { chip } else { text() };
            spans.push(Span::styled(format!(" {mark} {name} "), style));
            spans.push(sp(" ", text()));
        }
        let style = if p.tab >= ask.questions.len() { chip } else { text() };
        spans.push(Span::styled(" ✔ Submit ", style));
        spans.push(sp("  →", faint()));
        out.push(Line::from(spans));
    }
    let rows = p.rows(ask, own);
    if p.tab >= ask.questions.len() {
        out.push(Line::from(sp("Review your answers", bold(t.text))));
        for (q, a) in ask.questions.iter().zip(&p.answers) {
            out.push(Line::from(vec![sp(" ● ", fg(t.accent)), sp(q.question.clone(), text())]));
            out.push(Line::from(sp(format!("   → {}", a.clone().unwrap_or_default()), fg(t.green))));
        }
        out.push(Line::from(sp("Ready to submit your answers?", text())));
        for (i, row) in rows.iter().enumerate() {
            let label = if *row == Row::Submit { "Submit answers" } else { "Cancel" };
            hits.push((out.len(), i));
            out.push(choice_line(i, label, i == p.cursor));
        }
        return (out, hits);
    }
    let q: &AskQuestion = &ask.questions[p.tab];
    if !many && let Some(h) = &q.header {
        out.push(Line::from(Span::styled(format!(" ☐ {h} "), chip)));
    }
    for l in crate::conv::wrap(&q.question, width.max(10)) {
        out.push(Line::from(sp(l, bold(t.text))));
    }
    for (i, row) in rows.iter().enumerate() {
        let here = i == p.cursor;
        hits.push((out.len(), i));
        match row {
            Row::Choice(k) => {
                let o = &q.options[*k];
                let mark = if q.multi { "[ ] " } else { "" };
                out.push(choice_line(i, &format!("{mark}{}", o.label), here));
                if let Some(d) = o.description.as_deref().filter(|d| !d.trim().is_empty()) {
                    for l in crate::conv::wrap(d.trim(), width.saturating_sub(5).max(10)) {
                        out.push(Line::from(sp(format!("     {l}"), faint())));
                    }
                }
            }
            Row::Other => match &p.typing {
                Some(typed) => {
                    let mut line = choice_line(i, typed, true);
                    line.spans.push(sp("▏", fg(t.accent)));
                    out.push(line);
                }
                None => out.push(choice_line(i, "Type something.", here)),
            },
            Row::Chat => out.push(choice_line(i, "Chat about this", here)),
            Row::Submit | Row::Cancel => {}
        }
    }
    let nav = if many { "Tab/Arrow keys to navigate" } else { "↑/↓ to navigate" };
    out.push(Line::from(sp(format!("Enter to select · {nav} · Esc to cancel"), faint())));
    (out, hits)
}

fn choice_line(i: usize, label: &str, here: bool) -> Line<'static> {
    let t = theme::get();
    if here {
        Line::from(vec![sp("❯ ", fg(t.accent)), sp(format!("{}. {label}", i + 1), fg(t.accent))])
    } else {
        Line::from(vec![sp("  ", text()), sp(format!("{}. {label}", i + 1), text())])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aulora_core::model::AskOption;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn q(question: &str, header: &str, options: &[&str]) -> AskQuestion {
        AskQuestion {
            question: question.into(),
            header: Some(header.into()),
            options: options.iter().map(|o| AskOption { label: o.to_string(), description: None }).collect(),
            multi: false,
        }
    }

    fn key(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    #[test]
    fn one_question_answers_with_its_number() {
        let ask = Ask { questions: vec![q("Pick a colour", "Colour", &["Red", "Blue"])] };
        let mut p = Picker::new("s", &ask);
        assert_eq!(p.rows(&ask, true), [Row::Choice(0), Row::Choice(1), Row::Other, Row::Chat]);
        assert_eq!(p.key(&ask, true, &key(KeyCode::Char('1'))).keys, "1");
        // Arrow keys move here only; Enter presses the number of the row.
        let mut p = Picker::new("s", &ask);
        p.key(&ask, true, &key(KeyCode::Down));
        assert_eq!(p.key(&ask, true, &key(KeyCode::Enter)).keys, "2");
    }

    #[test]
    fn type_something_sends_what_you_type() {
        let ask = Ask { questions: vec![q("Pick a colour", "Colour", &["Red", "Blue"])] };
        let mut p = Picker::new("s", &ask);
        let mut keys = String::new();
        for k in [KeyCode::Char('3'), KeyCode::Char('p'), KeyCode::Char('u'), KeyCode::Backspace, KeyCode::Enter] {
            keys += &p.key(&ask, true, &key(k)).keys;
        }
        // As checked against Claude Code: 3, the letters, Enter.
        assert_eq!(keys, "3pu\x7f\r");
        assert_eq!(p.answers[0].as_deref(), Some("p"));
    }

    #[test]
    fn several_questions_step_through_to_the_review() {
        let ask = Ask {
            questions: vec![q("Pick a colour", "Colour", &["Red", "Blue"]), q("Pick a size", "Size", &["S", "L"])],
        };
        let mut p = Picker::new("s", &ask);
        assert_eq!(p.key(&ask, true, &key(KeyCode::Char('2'))).keys, "2");
        assert_eq!(p.tab, 1);
        assert_eq!(p.key(&ask, true, &key(KeyCode::Char('1'))).keys, "1");
        assert_eq!(p.rows(&ask, true), [Row::Submit, Row::Cancel]);
        let (lines, _) = lines(&p, &ask, true, 60);
        let text: Vec<String> = lines.iter().map(|l| l.spans.iter().map(|s| s.content.to_string()).collect()).collect();
        assert!(text.iter().any(|l| l.contains("☒ Colour") && l.contains("☒ Size") && l.contains("✔ Submit")));
        assert!(text.iter().any(|l| l == "   → Blue") && text.iter().any(|l| l == "   → S"));
        assert_eq!(p.key(&ask, true, &key(KeyCode::Enter)).keys, "1", "Submit answers");
    }

    #[test]
    fn permission_prompts_have_no_extra_rows() {
        let ask = Ask { questions: vec![q("Do you want to proceed?", "Bash", &["Yes", "No"])] };
        assert_eq!(Picker::new("s", &ask).rows(&ask, false), [Row::Choice(0), Row::Choice(1)]);
    }
}
