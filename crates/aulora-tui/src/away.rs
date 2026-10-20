//! "Where you left off": coming back to a project after a while, a card
//! says what happened since you last looked: what each agent finished or
//! is asking, the files edited, new commits.

use std::collections::HashMap;

use aulora_core::model::{Ms, SessionState};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};

use crate::app::App;
use crate::theme::{self, bold, dim, faint, fg, text};
use crate::ui::{fit, sp, status_color, status_glyph};

/// Away this long before the card is worth showing.
const AWAY: Ms = 30 * 60_000;

fn visits_path() -> std::path::PathBuf {
    aulora_core::paths::home().join("visits.json")
}

pub fn load() -> HashMap<String, Ms> {
    std::fs::read(visits_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// One thing that happened in a project while you were away.
pub struct Event {
    pub glyph: &'static str,
    pub color: ratatui::style::Color,
    pub title: String,
    pub detail: String,
}

pub struct Card {
    pub root: String,
    pub since: Ms,
    pub events: Vec<Event>,
}

impl App {
    /// Note a visit to a project, and say whether to show the card.
    pub fn visit(&mut self, root: &str) {
        let now = self.now;
        let last = self.visits.insert(root.to_string(), now);
        if let Some(since) = last.filter(|t| now - t >= AWAY)
            && let Some(card) = self.away_card(root, since)
        {
            self.away = Some(card);
        }
        if self.conn != crate::app::Conn::Demo {
            let _ = std::fs::write(visits_path(), serde_json::to_vec(&self.visits).unwrap_or_default());
        }
    }

    /// Leaving aulora: the project on screen was seen until now.
    pub fn leave(&mut self) {
        if let Some(root) = self.space_root() {
            self.visits.insert(root, self.now);
        }
        if self.conn != crate::app::Conn::Demo {
            let _ = std::fs::write(visits_path(), serde_json::to_vec(&self.visits).unwrap_or_default());
        }
    }

    /// What happened in a project since `since`, or None if nothing did.
    pub fn away_card(&self, root: &str, since: Ms) -> Option<Card> {
        let snap = self.snap.as_ref()?;
        let mut events = Vec::new();
        for s in snap.sessions.iter().filter(|s| s.project_root == root && s.last_event_at > since) {
            let status = self.session_status(s);
            let detail = match &s.state {
                SessionState::NeedsYou { reason, detail } => format!("{}: {detail}", reason.label()),
                SessionState::Working | SessionState::Compacting => "still working".to_string(),
                _ => s.last_reply.clone().unwrap_or_default(),
            };
            let files: Vec<_> = s.files_touched.iter().filter(|f| f.last_at > since).collect();
            let edits = if files.is_empty() {
                String::new()
            } else {
                let (a, r) = files.iter().fold((0, 0), |acc, f| (acc.0 + f.added, acc.1 + f.removed));
                format!(" · edited {} file{} +{a} −{r}", files.len(), if files.len() == 1 { "" } else { "s" })
            };
            events.push(Event {
                glyph: status_glyph(status),
                color: status_color(status),
                title: format!("{}{edits}", s.display_title()),
                detail: aulora_core::util::squash(&detail),
            });
        }
        if let Some(g) = snap.projects.iter().find(|p| p.root == root).and_then(|p| p.git.as_ref())
            && let (Some(msg), Some(at)) = (&g.last_commit, g.last_commit_at)
            && at > since
        {
            let t = theme::get();
            events.push(Event { glyph: "◆", color: t.magenta, title: "committed".into(), detail: msg.clone() });
        }
        (!events.is_empty()).then(|| Card { root: root.to_string(), since, events })
    }
}

pub fn draw(f: &mut Frame, area: Rect, app: &App) {
    let t = theme::get();
    let Some(card) = &app.away else { return };
    let width = area.width.saturating_sub(6).min(90);
    let iw = width.saturating_sub(2) as usize;
    let mut lines = vec![Line::raw("")];
    for e in card.events.iter().take(8) {
        lines.push(Line::from(vec![
            sp(format!(" {} ", e.glyph), fg(e.color)),
            sp(fit(&e.title, iw.saturating_sub(4)), bold(t.text)),
        ]));
        if !e.detail.is_empty() {
            let wrapped = crate::conv::wrap(&e.detail, iw.saturating_sub(7));
            let n = wrapped.len();
            for (k, l) in wrapped.into_iter().take(4).enumerate() {
                let l = if k == 3 && n > 4 { format!("{l} …") } else { l };
                lines.push(Line::from(vec![sp("   ", text()), sp(l, dim())]));
            }
        }
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(sp(" any key to carry on", faint())));
    let height = (lines.len() as u16 + 2).min(area.height.saturating_sub(2));
    let r = Rect { x: area.x + (area.width - width) / 2, y: area.y + 2, width, height };
    f.render_widget(Clear, r);
    let name = aulora_core::util::basename(&card.root);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(fg(t.accent))
        .style(Style::new().bg(t.panel))
        .title(sp(
            format!(" {name} · since you were here {} ", aulora_core::util::ago(app.now, card.since)),
            bold(t.accent),
        ));
    let inner = block.inner(r);
    f.render_widget(block, r);
    f.render_widget(Paragraph::new(lines), inner);
}
