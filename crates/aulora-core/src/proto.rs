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
    /// From an editor extension: the terminals in its window. Sent on
    /// connect and whenever they change; the connection stays open.
    Editor {
        /// "Visual Studio Code", "Cursor", …
        app: String,
        terminals: Vec<EditorTerminal>,
    },
    /// From an editor extension: a `Type` request was carried out (or not).
    Typed {
        request_id: u64,
        error: Option<String>,
    },
    /// Replace the daemon with the program at `exe` (this one's, if None),
    /// keeping every pane running: the new daemon adopts their terminals.
    Upgrade {
        exe: Option<String>,
    },
    /// Bring a session's editor terminal into view.
    ShowSession {
        session_id: String,
    },
    /// Send a prompt to a Claude session running in an editor's terminal,
    /// as if typed there. Replies once the editor has typed it.
    SendPrompt {
        session_id: String,
        text: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditorTerminal {
    /// The extension's id for the terminal.
    pub id: u32,
    /// The terminal's shell process.
    pub pid: u32,
    pub name: String,
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
    /// To an editor extension: show this terminal and give it focus.
    Show {
        terminal_id: u32,
    },
    /// To an editor extension: type this into a terminal and press Enter.
    /// Answer with `Typed`.
    Type {
        request_id: u64,
        terminal_id: u32,
        text: String,
    },
}

pub fn encode(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

pub fn decode(s: &str) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD.decode(s).unwrap_or_default()
}
