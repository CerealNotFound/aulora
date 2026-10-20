//! Claude Code's question picker, drawn and driven as Claude Code draws it
//! in its own terminal (checked against 2.1.304), for sessions running in
//! an editor: what you press here is pressed there.
//!
//! One question: a number answers at once. "Type something." opens a field
//! in place; letters replace its label and Enter sends. Several questions:
//! a tab per question and a Submit tab; each answer moves to the next, and
//! the last shows a review with Submit and Cancel.
//!
//! A multi-select question always gets the tabs. Its numbers tick and
//! untick choices without moving Claude's cursor, letters typed on its
//! "Type something" row tick that too, and an unnumbered "Next" (or
//! "Submit", on the last question) row moves on. Only arrow keys move
//! Claude's cursor there, so they are pressed there too, keeping both
//! cursors on the same row.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use aulora_core::model::{Ask, AskQuestion};

use crate::theme::{self, bold, faint, fg, text};
use crate::ui::sp;

const UP: &str = "\x1b[A";
const DOWN: &str = "\x1b[B";

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
    /// Multi-select questions: which choices are ticked.
    pub ticked: Vec<Vec<bool>>,
    /// Multi-select questions: what was typed for "Type something".
    pub own: Vec<String>,
}

/// A row under a question.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    Choice(usize),
    /// "Type something."
    Other,
    /// A multi-select question's unnumbered "Next" or "Submit".
    Next,
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
        q.multi.hash(&mut h);
        for o in &q.options {
            o.label.hash(&mut h);
        }
    }
    h.finish()
}

/// Whether Claude shows its tab row: several questions, or a
/// multi-select one.
fn tabbed(ask: &Ask) -> bool {
    ask.questions.len() > 1 || ask.questions.iter().any(|q| q.multi)
}

/// The number Claude shows for row `i` (the "Next" row has none).
fn number(rows: &[Row], i: usize) -> Option<usize> {
    (rows.get(i) != Some(&Row::Next)).then(|| rows[..i].iter().filter(|r| **r != Row::Next).count() + 1)
}

impl Picker {
    pub fn new(session: &str, ask: &Ask) -> Picker {
        Picker {
            session: session.to_string(),
            sig: sig(ask),
            answers: vec![None; ask.questions.len()],
            ticked: ask.questions.iter().map(|q| vec![false; q.options.len()]).collect(),
            own: vec![String::new(); ask.questions.len()],
            ..Default::default()
        }
    }

    /// Whether the question on screen is multi-select.
    fn multi(&self, ask: &Ask) -> bool {
        ask.questions.get(self.tab).is_some_and(|q| q.multi)
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
            rows.push(Row::Other);
        }
        if q.multi {
            rows.push(Row::Next);
        }
        if own {
            rows.push(Row::Chat);
        }
        rows
    }

    /// Choose row `i`: what Claude's picker does when you press its number
    /// (or, for "Next", go there and press Enter).
    pub fn pick(&mut self, ask: &Ask, own: bool, i: usize) -> Outcome {
        let rows = self.rows(ask, own);
        let Some(row) = rows.get(i).copied() else { return Outcome::quiet() };
        let digit = number(&rows, i).map(|n| n.to_string()).unwrap_or_default();
        if self.multi(ask) {
            return match row {
                Row::Choice(k) => {
                    if let Some(t) = self.ticked.get_mut(self.tab).and_then(|t| t.get_mut(k)) {
                        *t = !*t;
                    }
                    self.update_multi(ask);
                    Outcome::press(digit)
                }
                // The field takes letters while Claude's cursor is on it.
                Row::Other => {
                    let keys = self.move_to(i);
                    self.typing = Some(self.own[self.tab].clone());
                    Outcome::press(keys)
                }
                Row::Next => {
                    let mut keys = self.move_to(i);
                    keys.push('\r');
                    self.next_tab(ask);
                    Outcome::press(keys)
                }
                _ => {
                    self.cursor = i;
                    Outcome::press(digit)
                }
            };
        }
        self.cursor = i;
        match row {
            Row::Choice(k) => {
                let options = &ask.questions[self.tab].options;
                let label = options[k].label.clone();
                // A permission prompt's "No" may have another number in
                // Claude; Esc says no there whatever it is.
                let no = !own && k + 1 == options.len() && label == aulora_core::reduce::PERMISSION_NO;
                self.answered(ask, label);
                Outcome::press(if no { "\x1b".to_string() } else { digit })
            }
            Row::Other => {
                self.typing = Some(String::new());
                Outcome::press(digit)
            }
            Row::Next | Row::Chat | Row::Submit | Row::Cancel => Outcome::press(digit),
        }
    }

    /// Arrow keys that take Claude's cursor from ours to row `i`.
    fn move_to(&mut self, i: usize) -> String {
        let keys = if i > self.cursor { DOWN.repeat(i - self.cursor) } else { UP.repeat(self.cursor - i) };
        self.cursor = i;
        keys
    }

    fn next_tab(&mut self, ask: &Ask) {
        self.tab = (self.tab + 1).min(ask.questions.len());
        self.cursor = 0;
        self.typing = None;
    }

    /// A multi-select answer as Claude reviews it: the ticked choices,
    /// then your own.
    fn update_multi(&mut self, ask: &Ask) {
        let Some(q) = ask.questions.get(self.tab) else { return };
        let mut picked: Vec<String> =
            q.options.iter().zip(&self.ticked[self.tab]).filter(|(_, t)| **t).map(|(o, _)| o.label.clone()).collect();
        if !self.own[self.tab].is_empty() {
            picked.push(self.own[self.tab].clone());
        }
        self.answers[self.tab] = (!picked.is_empty()).then(|| picked.join(", "));
    }

    fn answered(&mut self, ask: &Ask, answer: String) {
        if let Some(a) = self.answers.get_mut(self.tab) {
            *a = Some(answer);
        }
        self.typing = None;
        if tabbed(ask) {
            self.next_tab(ask);
        }
    }

    /// Up or down one row; in a multi-select question Claude's cursor goes
    /// too, and landing on "Type something" types there.
    fn step(&mut self, ask: &Ask, own: bool, up: bool) -> Outcome {
        let rows = self.rows(ask, own);
        let to = if up { self.cursor.saturating_sub(1) } else { (self.cursor + 1).min(rows.len().saturating_sub(1)) };
        if !self.multi(ask) {
            self.cursor = to;
            return Outcome::quiet();
        }
        let keys = self.move_to(to);
        self.typing = (rows.get(to) == Some(&Row::Other)).then(|| self.own[self.tab].clone());
        Outcome::press(keys)
    }

    /// A key, as Claude's picker takes it.
    pub fn key(&mut self, ask: &Ask, own: bool, key: &ratatui::crossterm::event::KeyEvent) -> Outcome {
        use ratatui::crossterm::event::{KeyCode, KeyModifiers};
        let multi = self.multi(ask);
        if let Some(t) = self.typing.as_mut() {
            return match key.code {
                // Enter on the field would untick it in a multi-select
                // question; go down to "Next" instead, keeping it.
                KeyCode::Enter if multi => {
                    self.typing = None;
                    self.update_multi(ask);
                    let next = self.cursor + 1;
                    Outcome::press(self.move_to(next))
                }
                KeyCode::Enter => {
                    let answer = t.clone();
                    self.answered(ask, answer);
                    Outcome::press("\r")
                }
                KeyCode::Backspace => {
                    t.pop();
                    if multi {
                        self.own[self.tab] = t.clone();
                        self.update_multi(ask);
                    }
                    Outcome::press("\x7f")
                }
                KeyCode::Esc => Outcome::press("\x1b"),
                KeyCode::Up | KeyCode::Down => {
                    self.typing = None;
                    let up = key.code == KeyCode::Up;
                    if multi {
                        return self.step(ask, own, up);
                    }
                    let n = self.rows(ask, own).len();
                    self.cursor = if up { self.cursor.saturating_sub(1) } else { (self.cursor + 1).min(n - 1) };
                    Outcome::press(if up { UP } else { DOWN })
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    t.push(c);
                    if multi {
                        self.own[self.tab] = t.clone();
                        self.update_multi(ask);
                    }
                    Outcome::press(c.to_string())
                }
                _ => Outcome::quiet(),
            };
        }
        match key.code {
            KeyCode::Char(c @ '1'..='9') if key.modifiers.is_empty() => {
                let n = c as usize - '0' as usize;
                let rows = self.rows(ask, own);
                match (0..rows.len()).find(|&i| number(&rows, i) == Some(n)) {
                    Some(i) => self.pick(ask, own, i),
                    None => Outcome::quiet(),
                }
            }
            KeyCode::Enter => self.pick(ask, own, self.cursor),
            KeyCode::Up | KeyCode::Char('k') => self.step(ask, own, true),
            KeyCode::Down | KeyCode::Char('j') => self.step(ask, own, false),
            // Between questions, as Claude's Tab and arrow keys move.
            KeyCode::Tab | KeyCode::Right if tabbed(ask) => {
                self.next_tab(ask);
                Outcome::press(if key.code == KeyCode::Tab { "\t" } else { "\x1b[C" })
            }
            KeyCode::BackTab | KeyCode::Left if tabbed(ask) => {
                self.tab = self.tab.saturating_sub(1);
                self.cursor = 0;
                self.typing = None;
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
    let blank = || Line::from("");
    let chip = Style::new().bg(t.active).fg(t.text).add_modifier(Modifier::BOLD);
    if tabbed(ask) {
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
        out.push(blank());
    } else if let Some(h) = ask.questions.first().and_then(|q| q.header.as_ref()) {
        out.push(Line::from(Span::styled(format!(" ☐ {h} "), chip)));
        out.push(blank());
    }
    let rows = p.rows(ask, own);
    if p.tab >= ask.questions.len() {
        out.push(Line::from(sp("Review your answers", bold(t.text))));
        out.push(blank());
        for (q, a) in ask.questions.iter().zip(&p.answers) {
            out.push(Line::from(vec![sp(" ● ", fg(t.accent)), sp(q.question.clone(), text())]));
            out.push(Line::from(sp(format!("   → {}", a.clone().unwrap_or_default()), fg(t.green))));
        }
        out.push(blank());
        out.push(Line::from(sp("Ready to submit your answers?", text())));
        out.push(blank());
        for (i, row) in rows.iter().enumerate() {
            let label = if *row == Row::Submit { "Submit answers" } else { "Cancel" };
            hits.push((out.len(), i));
            out.push(choice_line(i + 1, label, i == p.cursor, false));
        }
        return (out, hits);
    }
    let q: &AskQuestion = &ask.questions[p.tab];
    for l in crate::conv::wrap(&q.question, width.max(10)) {
        out.push(Line::from(sp(l, bold(t.text))));
    }
    out.push(blank());
    let tick = |on: bool| if on { "[✔] " } else { "[ ] " };
    for (i, row) in rows.iter().enumerate() {
        let here = i == p.cursor;
        let n = number(&rows, i).unwrap_or(0);
        if *row == Row::Chat {
            out.push(Line::from(sp("─".repeat(width), faint())));
        }
        hits.push((out.len(), i));
        match row {
            Row::Choice(k) => {
                let o = &q.options[*k];
                let mark = if q.multi { tick(p.ticked[p.tab].get(*k).copied().unwrap_or(false)) } else { "" };
                out.push(choice_line(n, &format!("{mark}{}", o.label), here, false));
                if let Some(d) = o.description.as_deref().filter(|d| !d.trim().is_empty()) {
                    let indent = " ".repeat(5 + mark.chars().count());
                    for l in crate::conv::wrap(d.trim(), width.saturating_sub(indent.len()).max(10)) {
                        out.push(Line::from(sp(format!("{indent}{l}"), faint())));
                    }
                }
            }
            Row::Other => {
                let typed = p.typing.as_ref().or(Some(&p.own[p.tab]).filter(|s| !s.is_empty()));
                let mark = if q.multi { tick(typed.is_some_and(|s| !s.is_empty())) } else { "" };
                match (&p.typing, typed) {
                    (Some(_), Some(typed)) => {
                        let mut line = choice_line(n, &format!("{mark}{typed}"), true, false);
                        line.spans.push(sp("▏", fg(t.accent)));
                        out.push(line);
                    }
                    (None, Some(typed)) => out.push(choice_line(n, &format!("{mark}{typed}"), here, false)),
                    _ if q.multi => out.push(choice_line(n, &format!("{mark}Type something"), here, true)),
                    _ => out.push(choice_line(n, "Type something.", here, true)),
                }
            }
            Row::Next => {
                let label = if p.tab + 1 >= ask.questions.len() { "Submit" } else { "Next" };
                out.push(if here {
                    Line::from(vec![sp("❯ ", fg(t.accent)), sp(format!("   {label}"), fg(t.accent))])
                } else {
                    Line::from(sp(format!("     {label}"), text()))
                });
            }
            Row::Chat => out.push(choice_line(n, "Chat about this", here, false)),
            Row::Submit | Row::Cancel => {}
        }
    }
    out.push(blank());
    let nav = if ask.questions.len() > 1 && !q.multi { "Tab/Arrow keys to navigate" } else { "↑/↓ to navigate" };
    out.push(Line::from(sp(format!("Enter to select · {nav} · Esc to cancel"), faint())));
    (out, hits)
}

/// `❯ 1. Red` with the cursor on it, as Claude colours it; `dim` greys a
/// row Claude draws grey ("Type something.").
fn choice_line(n: usize, label: &str, here: bool, dim: bool) -> Line<'static> {
    let t = theme::get();
    if here {
        Line::from(vec![sp("❯ ", fg(t.accent)), sp(format!("{n}. "), faint()), sp(label.to_string(), fg(t.accent))])
    } else {
        Line::from(vec![sp("  ", text()), sp(format!("{n}. {label}"), if dim { faint() } else { text() })])
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

    fn multi(question: &str, header: &str, options: &[&str]) -> AskQuestion {
        AskQuestion { multi: true, ..q(question, header, options) }
    }

    fn key(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    fn press(p: &mut Picker, ask: &Ask, keys: &[KeyCode]) -> Vec<String> {
        keys.iter().map(|k| p.key(ask, true, &key(*k)).keys).collect()
    }

    fn texts(p: &Picker, ask: &Ask) -> Vec<String> {
        let (lines, _) = lines(p, ask, true, 40);
        lines.iter().map(|l| l.spans.iter().map(|s| s.content.to_string()).collect()).collect()
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
    fn one_question_is_laid_out_as_claude_does() {
        let ask = Ask { questions: vec![q("Pick a colour", "Colour", &["Red", "Blue"])] };
        let rule = "─".repeat(40);
        let want = [
            " ☐ Colour ",
            "",
            "Pick a colour",
            "",
            "❯ 1. Red",
            "  2. Blue",
            "  3. Type something.",
            rule.as_str(),
            "  4. Chat about this",
            "",
            "Enter to select · ↑/↓ to navigate · Esc to cancel",
        ];
        assert_eq!(texts(&Picker::new("s", &ask), &ask), want);
    }

    #[test]
    fn type_something_sends_what_you_type() {
        let ask = Ask { questions: vec![q("Pick a colour", "Colour", &["Red", "Blue"])] };
        let mut p = Picker::new("s", &ask);
        let keys = [KeyCode::Char('3'), KeyCode::Char('p'), KeyCode::Char('u'), KeyCode::Backspace, KeyCode::Enter];
        // As checked against Claude Code: 3, the letters, Enter.
        assert_eq!(press(&mut p, &ask, &keys).concat(), "3pu\x7f\r");
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
        let text = texts(&p, &ask);
        assert!(text.iter().any(|l| l.contains("☒ Colour") && l.contains("☒ Size") && l.contains("✔ Submit")));
        assert!(text.iter().any(|l| l == "   → Blue") && text.iter().any(|l| l == "   → S"));
        assert_eq!(p.key(&ask, true, &key(KeyCode::Enter)).keys, "1", "Submit answers");
    }

    #[test]
    fn multi_select_ticks_with_numbers_and_moves_on_with_next() {
        let ask = Ask { questions: vec![multi("Which toppings?", "Toppings", &["Cheese", "Olives", "Basil"])] };
        let mut p = Picker::new("s", &ask);
        assert_eq!(
            p.rows(&ask, true),
            [Row::Choice(0), Row::Choice(1), Row::Choice(2), Row::Other, Row::Next, Row::Chat]
        );
        // Numbers tick without moving Claude's cursor.
        assert_eq!(press(&mut p, &ask, &[KeyCode::Char('1'), KeyCode::Char('3')]), ["1", "3"]);
        assert_eq!((p.tab, p.cursor), (0, 0));
        assert_eq!(p.answers[0].as_deref(), Some("Cheese, Basil"));
        let text = texts(&p, &ask);
        assert!(text[0].contains("☒ Toppings") && text[0].contains("✔ Submit"), "{text:?}");
        assert!(text.contains(&"❯ 1. [✔] Cheese".to_string()) && text.contains(&"  4. [ ] Type something".to_string()));
        assert!(text.contains(&"     Submit".to_string()) && text.contains(&"  5. Chat about this".to_string()));
        // "Submit" has no number: Claude's cursor goes there, then Enter.
        let submit = p.rows(&ask, true).iter().position(|r| *r == Row::Next).unwrap();
        assert_eq!(p.pick(&ask, true, submit).keys, format!("{}\r", DOWN.repeat(4)));
        assert_eq!(p.rows(&ask, true), [Row::Submit, Row::Cancel], "the review");
        assert_eq!(p.key(&ask, true, &key(KeyCode::Char('1'))).keys, "1");
    }

    #[test]
    fn multi_select_types_your_own_on_its_row() {
        let ask = Ask { questions: vec![multi("Which toppings?", "Toppings", &["Cheese", "Olives", "Basil"])] };
        let mut p = Picker::new("s", &ask);
        // Its number would only tick an empty field: go there instead.
        let keys = [KeyCode::Char('4'), KeyCode::Char('h'), KeyCode::Char('a'), KeyCode::Char('m'), KeyCode::Enter];
        assert_eq!(press(&mut p, &ask, &keys), [DOWN.repeat(3).as_str(), "h", "a", "m", DOWN]);
        assert_eq!(p.answers[0].as_deref(), Some("ham"));
        assert_eq!(p.rows(&ask, true)[p.cursor], Row::Next);
        assert_eq!(p.key(&ask, true, &key(KeyCode::Enter)).keys, "\r");
        // Arrows move both cursors; landing on the field types there.
        let mut p = Picker::new("s", &ask);
        assert_eq!(press(&mut p, &ask, &[KeyCode::Down, KeyCode::Down, KeyCode::Down]).concat(), DOWN.repeat(3));
        assert_eq!(p.typing.as_deref(), Some(""));
        assert_eq!(p.key(&ask, true, &key(KeyCode::Char('x'))).keys, "x");
    }

    #[test]
    fn multi_select_says_next_before_the_last_question() {
        let ask = Ask {
            questions: vec![multi("Which toppings?", "Toppings", &["Cheese"]), q("Pick a size", "Size", &["S", "L"])],
        };
        let p = Picker::new("s", &ask);
        assert!(texts(&p, &ask).contains(&"     Next".to_string()));
    }

    #[test]
    fn permission_prompts_have_no_extra_rows() {
        let ask = Ask { questions: vec![q("Do you want to proceed?", "Bash", &["Yes", "No"])] };
        assert_eq!(Picker::new("s", &ask).rows(&ask, false), [Row::Choice(0), Row::Choice(1)]);
        assert_eq!(Picker::new("s", &ask).key(&ask, false, &key(KeyCode::Char('1'))).keys, "1");
        // Claude may number "No" 3 or 4; Esc is no either way.
        assert_eq!(Picker::new("s", &ask).key(&ask, false, &key(KeyCode::Char('2'))).keys, "\x1b");
    }
}
