//! Newline-delimited JSON over a Unix socket.

use serde::{Deserialize, Serialize};

use base64::Engine as _;

use crate::model::{Ms, PaneKind, Snapshot};

pub const PROTOCOL_VERSION: u32 = 1;

/// What the `aulora hook` command sends for every Claude Code hook event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HookEnvelope {
    pub v: u32,
    /// When the hook process started.
    pub at: Ms,
    /// The hook's stdin JSON, with large fields trimmed.
    pub payload: serde_json::Value,
    pub env: HookEnv,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookEnv {
    pub term_program: Option<String>,
    pub bundle_id: Option<String>,
    pub pane_id: Option<String>,
    pub project_dir: Option<String>,
    pub tmux: bool,
    /// PID of the Claude Code process that ran the hook.
    pub claude_pid: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMsg {
    Hook(HookEnvelope),
    /// Receive a snapshot now and after every change.
    Subscribe,
    /// One snapshot, then close.
    Snapshot,
    Ping,
    /// Remove a session from the dashboard.
    Dismiss {
        session_id: String,
    },
    /// Remove every ended session.
    ClearEnded,
    Shutdown,
    /// Start a new terminal. Replies with `PaneCreated`.
    NewPane(NewPane),
    /// Stop a pane's process (SIGHUP), keeping its last screen.
    KillPane {
        pane_id: String,
    },
    /// Remove a pane, killing it first if it still runs.
    ClosePane {
        pane_id: String,
    },
    /// Start an exited pane's program again in the same pane.
    RestartPane {
        pane_id: String,
    },
    /// Stream a pane: `PaneScreen` now, then `PaneOutput` as it arrives.
    Attach {
        pane_id: String,
    },
    Detach {
        pane_id: String,
    },
    /// Bytes to write to the pane, base64-encoded.
    Input {
        pane_id: String,
        data: String,
    },
    Resize {
        pane_id: String,
        cols: u16,
        rows: u16,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewPane {
    pub cwd: String,
    pub kind: PaneKind,
    #[serde(default)]
    pub args: Vec<String>,
    pub name: Option<String>,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMsg {
    Snapshot(Box<Snapshot>),
    Pong {
        version: String,
        protocol: u32,
    },
    Ok,
    Error {
        message: String,
    },
    PaneCreated {
        pane_id: String,
    },
    /// Escape sequences that redraw the pane's current screen from blank.
    PaneScreen {
        pane_id: String,
        cols: u16,
        rows: u16,
        data: String,
    },
    /// Raw output, base64-encoded.
    PaneOutput {
        pane_id: String,
        data: String,
    },
    PaneExited {
        pane_id: String,
        code: u32,
    },
}

pub fn encode(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

pub fn decode(s: &str) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD.decode(s).unwrap_or_default()
}
