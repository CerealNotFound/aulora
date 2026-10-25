//! A desktop notification when an agent starts needing you, or a dev
//! server in a terminal stops, so you hear about it with the dashboard
//! closed. Off with `"notify": false` in ~/.aulora/config.json.

use std::collections::HashMap;
use std::sync::Mutex;

use aulora_core::model::{Ms, now_ms};

/// The same session or terminal won't notify more often than this.
const QUIET: Ms = 30_000;

#[derive(Default)]
pub struct Notifier {
    last: Mutex<HashMap<String, Ms>>,
}

fn enabled() -> bool {
    let path = aulora_core::paths::home().join("config.json");
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .and_then(|v| v.get("notify").and_then(serde_json::Value::as_bool))
        .unwrap_or(true)
}

/// Escape for an AppleScript string.
fn quote(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

impl Notifier {
    pub fn needs_you(&self, session: &str, title: &str, detail: &str) {
        let now = now_ms();
        {
            let mut last = self.last.lock().unwrap();
            if last.get(session).is_some_and(|t| now - t < QUIET) {
                return;
            }
            last.insert(session.to_string(), now);
        }
        if !enabled() {
            return;
        }
        let (title, detail) = (quote(title), quote(detail));
        let spawned = if cfg!(target_os = "macos") {
            let script = format!(
                "display notification \"{detail}\" with title \"aulora\" subtitle \"{title}\" sound name \"Glass\""
            );
            std::process::Command::new("osascript").args(["-e", &script]).spawn()
        } else {
            std::process::Command::new("notify-send").args(["aulora", &format!("{title}: {detail}")]).spawn()
        };
        if let Err(e) = spawned {
            tracing::debug!("notification failed: {e}");
        }
    }
}
