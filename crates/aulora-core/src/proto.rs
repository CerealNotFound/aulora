//! Newline-delimited JSON over a Unix socket.

use serde::{Deserialize, Serialize};

use crate::model::{Ms, Snapshot};

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
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMsg {
    Snapshot(Box<Snapshot>),
    Pong { version: String, protocol: u32 },
    Ok,
    Error { message: String },
}
