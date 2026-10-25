use serde::{Deserialize, Serialize};

/// Unix epoch milliseconds.
pub type Ms = i64;

pub fn now_ms() -> Ms {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as Ms).unwrap_or(0)
}

/// Why a session is blocked on you.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Attention {
    /// A tool call is waiting for permission.
    Permission,
    /// Claude asked you a question (AskUserQuestion).
    Question,
    /// A plan is waiting for approval (ExitPlanMode).
    PlanApproval,
    /// Some other dialog needs input (MCP elicitation, agent input…).
    Input,
}

impl Attention {
    pub fn label(self) -> &'static str {
        match self {
            Attention::Permission => "needs permission",
            Attention::Question => "asked you a question",
            Attention::PlanApproval => "plan ready for review",
            Attention::Input => "needs input",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionState {
    /// Session started, nothing asked yet.
    Idle,
    /// Claude is working on a turn.
    Working,
    /// Claude is blocked on you.
    NeedsYou { reason: Attention, detail: String },
    /// Claude finished its turn; the ball is in your court.
    YourTurn,
    /// Context is being compacted.
    Compacting,
    /// The turn failed (API error, etc.).
    Errored { detail: String },
    /// The session is over.
    Ended { reason: String },
}

impl SessionState {
    /// Lower is more urgent. Used for sorting.
    pub fn rank(&self) -> u8 {
        match self {
            SessionState::NeedsYou { .. } => 0,
            SessionState::Errored { .. } => 1,
            SessionState::YourTurn => 2,
            SessionState::Working | SessionState::Compacting => 3,
            SessionState::Idle => 4,
            SessionState::Ended { .. } => 9,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            SessionState::Idle => "IDLE",
            SessionState::Working => "WORKING",
            SessionState::NeedsYou { .. } => "NEEDS YOU",
            SessionState::YourTurn => "YOUR TURN",
            SessionState::Compacting => "COMPACTING",
            SessionState::Errored { .. } => "ERROR",
            SessionState::Ended { .. } => "ENDED",
        }
    }

    pub fn is_live(&self) -> bool {
        !matches!(self, SessionState::Ended { .. })
    }

    pub fn same_kind(&self, other: &SessionState) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalInfo {
    /// $TERM_PROGRAM, e.g. "vscode", "iTerm.app", "ghostty".
    pub program: Option<String>,
    /// macOS $__CFBundleIdentifier of the hosting app.
    pub bundle_id: Option<String>,
    /// Set when the session runs in an aulora-managed pane (step 2+).
    pub pane_id: Option<String>,
    /// Running inside tmux.
    pub tmux: bool,
    /// The editor whose aulora extension can type into this session's
    /// terminal, e.g. "Visual Studio Code". Set by the daemon.
    #[serde(default)]
    pub editor: Option<String>,
}

impl TerminalInfo {
    /// Human name of the app hosting the terminal.
    pub fn app_name(&self) -> Option<String> {
        let b = self.bundle_id.as_deref().unwrap_or("").to_ascii_lowercase();
        let p = self.program.as_deref().unwrap_or("").to_ascii_lowercase();
        let name = if b.contains("cursor") || b.contains("todesktop") {
            "Cursor"
        } else if b.contains("vscodium") {
            "VSCodium"
        } else if b.contains("com.microsoft.vscode") || p == "vscode" {
            "VS Code"
        } else if b.contains("windsurf") {
            "Windsurf"
        } else if b.contains("ghostty") || p == "ghostty" {
            "Ghostty"
        } else if b.contains("iterm") || p == "iterm.app" {
            "iTerm"
        } else if b.contains("com.apple.terminal") || p == "apple_terminal" {
            "Terminal"
        } else if b.contains("warp") || p == "warpterminal" {
            "Warp"
        } else if b.contains("wezterm") || p == "wezterm" {
            "WezTerm"
        } else if b.contains("zed") || p == "zed" {
            "Zed"
        } else if b.contains("kitty") || p == "kitty" {
            "kitty"
        } else if b.contains("alacritty") {
            "Alacritty"
        } else if p == "aulora" {
            "aulora"
        } else if !p.is_empty() {
            return self.program.clone();
        } else {
            return None;
        };
        Some(if self.tmux { format!("{name} · tmux") } else { name.to_string() })
    }

    pub fn is_vscode_like(&self) -> bool {
        matches!(
            self.app_name().as_deref().map(|s| s.split(" · ").next().unwrap_or(s)),
            Some("VS Code" | "Cursor" | "VSCodium" | "Windsurf")
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: Option<String>,
    pub name: String,
    /// Short human description: a file path, a command, a URL…
    pub target: String,
    pub started_at: Ms,
    pub duration_ms: Option<u64>,
    pub ok: Option<bool>,
    /// Subagent type when the call came from a subagent.
    pub agent: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptEntry {
    pub at: Ms,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileTouch {
    pub path: String,
    pub edits: u32,
    pub added: u32,
    pub removed: u32,
    pub last_at: Ms,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub project_root: String,
    pub cwd: String,
    pub transcript_path: Option<String>,

    pub state: SessionState,
    pub state_since: Ms,
    pub started_at: Ms,
    pub last_event_at: Ms,

    /// AI-generated or user-set title.
    pub title: Option<String>,
    pub last_prompt: Option<String>,
    pub last_prompt_at: Option<Ms>,
    pub last_reply: Option<String>,
    pub last_reply_at: Option<Ms>,
    /// What Claude has written so far in the current turn (from the transcript).
    #[serde(default)]
    pub live_text: Option<String>,
    /// Earlier prompts in this session, oldest first.
    #[serde(default)]
    pub prompts: Vec<PromptEntry>,

    pub model: Option<String>,
    pub permission_mode: Option<String>,
    pub effort: Option<String>,
    pub source: Option<String>,
    pub git_branch: Option<String>,

    pub terminal: TerminalInfo,
    pub pid: Option<u32>,

    pub turns: u32,
    pub tool_calls: u32,
    pub tool_errors: u32,
    pub subagents_active: u32,
    pub background_tasks: u32,

    pub current_tool: Option<ToolCall>,
    /// Newest first.
    pub recent_tools: Vec<ToolCall>,
    /// Most recently touched first.
    pub files_touched: Vec<FileTouch>,

    /// Tokens in the context window as of the last assistant message.
    pub context_tokens: Option<u64>,
    /// Context size at the end of each turn, oldest first.
    pub context_history: Vec<u64>,
    pub output_tokens: u64,

    pub turn_started_at: Option<Ms>,
    pub last_turn_ms: Option<u64>,

    /// (minute since epoch, events in that minute), oldest first, last hour.
    pub activity: Vec<(i64, u32)>,

    #[serde(default)]
    pub origin: Origin,
    /// What Claude is asking, while it waits on you: the question and its
    /// options, or a permission prompt's choices.
    #[serde(default)]
    pub ask: Option<Ask>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ask {
    pub questions: Vec<AskQuestion>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskQuestion {
    pub question: String,
    pub header: Option<String>,
    /// In the order Claude Code numbers them, from 1.
    pub options: Vec<AskOption>,
    pub multi: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskOption {
    pub label: String,
    pub description: Option<String>,
}

/// How aulora learned about a session.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    /// Claude Code hooks: the full picture.
    #[default]
    Hooks,
    /// Claude Code's list of running sessions, before any hook arrived.
    Running,
    /// A transcript on disk from a session that isn't running.
    History,
}

impl Session {
    pub fn new(id: &str, project_root: &str, cwd: &str, at: Ms) -> Self {
        Session {
            id: id.to_string(),
            project_root: project_root.to_string(),
            cwd: cwd.to_string(),
            transcript_path: None,
            state: SessionState::Idle,
            state_since: at,
            started_at: at,
            last_event_at: at,
            title: None,
            last_prompt: None,
            last_prompt_at: None,
            last_reply: None,
            last_reply_at: None,
            live_text: None,
            prompts: Vec::new(),
            model: None,
            permission_mode: None,
            effort: None,
            source: None,
            git_branch: None,
            terminal: TerminalInfo::default(),
            pid: None,
            turns: 0,
            tool_calls: 0,
            tool_errors: 0,
            subagents_active: 0,
            background_tasks: 0,
            current_tool: None,
            recent_tools: Vec::new(),
            files_touched: Vec::new(),
            context_tokens: None,
            context_history: Vec::new(),
            output_tokens: 0,
            turn_started_at: None,
            last_turn_ms: None,
            activity: Vec::new(),
            origin: Origin::Hooks,
            ask: None,
        }
    }

    /// Best label for lists: title, else last prompt, else short id.
    pub fn display_title(&self) -> String {
        if let Some(t) = self.title.as_deref().filter(|t| !t.trim().is_empty()) {
            return crate::util::squash(t);
        }
        if let Some(p) = self.last_prompt.as_deref().filter(|p| !p.trim().is_empty()) {
            return crate::util::first_line(p).to_string();
        }
        format!("session {}", self.short_id())
    }

    pub fn short_id(&self) -> &str {
        &self.id[..self.id.len().min(8)]
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitInfo {
    pub branch: Option<String>,
    pub detached: bool,
    pub ahead: u32,
    pub behind: u32,
    pub staged: u32,
    pub modified: u32,
    pub untracked: u32,
    pub conflicts: u32,
    pub last_commit: Option<String>,
    pub last_commit_at: Option<Ms>,
}

impl GitInfo {
    pub fn is_dirty(&self) -> bool {
        self.staged + self.modified + self.untracked + self.conflicts > 0
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub root: String,
    pub name: String,
    pub git: Option<GitInfo>,
    pub last_activity: Ms,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedKind {
    Start,
    Prompt,
    Tool,
    Edit,
    Permission,
    Question,
    Done,
    Error,
    Compact,
    Agent,
    End,
    Info,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeedEvent {
    pub at: Ms,
    pub session_id: String,
    pub project_root: String,
    pub kind: FeedKind,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DaemonInfo {
    pub version: String,
    pub pid: u32,
    pub started_at: Ms,
    pub hooks_seen: u64,
}

/// Everything a client needs to draw the dashboard.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub daemon: DaemonInfo,
    pub projects: Vec<Project>,
    pub sessions: Vec<Session>,
    /// Newest last.
    pub feed: Vec<FeedEvent>,
    /// Terminals run by the daemon.
    #[serde(default)]
    pub panes: Vec<Pane>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaneKind {
    /// An interactive login shell.
    Shell,
    /// Claude Code, dropping back to a shell when it exits.
    Claude,
    /// Any other command.
    Command,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneExit {
    pub code: u32,
    pub signal: Option<String>,
    pub at: Ms,
}

/// How the last command run in a pane's shell ended, as the shell
/// reported it (aulora's zsh integration, OSC 133).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandExit {
    pub code: u32,
    pub at: Ms,
}

/// A terminal whose process is owned by the daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pane {
    pub id: String,
    pub project_root: String,
    pub cwd: String,
    pub name: String,
    pub kind: PaneKind,
    /// Extra arguments (Claude) or the command line (Command).
    pub args: Vec<String>,
    pub pid: Option<u32>,
    pub cols: u16,
    pub rows: u16,
    pub created_at: Ms,
    pub last_output_at: Ms,
    pub bytes_out: u64,
    /// Title set by the program with an OSC escape sequence.
    pub title: Option<String>,
    pub bell_at: Option<Ms>,
    pub exit: Option<PaneExit>,
    /// The last command run in the pane's shell, once it has ended; None
    /// while one runs or when the shell doesn't say.
    #[serde(default)]
    pub command_exit: Option<CommandExit>,
    /// Bumped on every restart so clients know to re-attach.
    pub generation: u32,
}

impl Pane {
    pub fn is_running(&self) -> bool {
        self.exit.is_none()
    }
}
