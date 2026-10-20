//! Every key binding in one table. Key handling, the footer hints and the
//! help overlay all read it, so they can't disagree (zellij, helix, lazygit).

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Where a binding applies. Lookups try the focused scope, then Global.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scope {
    Global,
    /// The sidebar has focus.
    Sidebar,
    /// The main area has focus, showing a conversation.
    Conversation,
    /// The main area has focus, showing a pane you're not typing into.
    Pane,
    /// The main area has focus, showing a project overview.
    Overview,
    /// The main area has focus, showing changes.
    Changes,
    /// The main area has focus, showing files.
    Files,
}

impl Scope {
    pub fn title(self) -> &'static str {
        match self {
            Scope::Global => "everywhere",
            Scope::Sidebar => "sidebar",
            Scope::Conversation => "conversation",
            Scope::Pane => "terminal pane",
            Scope::Overview => "workspace",
            Scope::Changes => "changes",
            Scope::Files => "files",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    Up,
    Down,
    PageUp,
    PageDown,
    Top,
    Bottom,
    FocusNext,
    FocusPrev,
    Tab(u8),
    NextTab,
    PrevTab,
    Open,
    Back,
    NewShell,
    NewClaude,
    NextAlert,
    Filter,
    Zoom,
    OpenEditor,
    Close,
    DismissEnded,
    Restart,
    Themes,
    Help,
    Quit,
    ToggleHistory,
    ToggleEnded,
    Resume,
    Menu,
    Stage,
    MoveUp,
    MoveDown,
    SideBySide,
    Welcome,
    /// Hide the Files and Changes panel (its ×).
    ClosePanel,
    ToggleSidebar,
    Discard,
    HalfUp,
    HalfDown,
    CursorUp,
    CursorDown,
    OpenProject,
    Reveal,
    FocusLeft,
    FocusRight,
    FocusUp,
    FocusDown,
    SplitRight,
    SplitDown,
    NewTab,
    OpenChanges,
    OpenFiles,
    Grow,
    Shrink,
    Equalize,
    CloseLeaf,
    Edit,
    Finder,
    Commit,
    Push,
    Pull,
    Compose,
    ToggleAll,
    PrevHunk,
    NextHunk,
    Left,
    PrevPrompt,
    NextPrompt,
    Copy,
    Search,
    NextMatch,
    ToggleMouse,
}

#[derive(Debug, Clone, Copy)]
pub struct Key {
    pub code: KeyCode,
    pub mods: KeyModifiers,
}

const fn k(c: char) -> Key {
    Key { code: KeyCode::Char(c), mods: KeyModifiers::NONE }
}
const fn code(code: KeyCode) -> Key {
    Key { code, mods: KeyModifiers::NONE }
}
const fn ctrl(c: char) -> Key {
    Key { code: KeyCode::Char(c), mods: KeyModifiers::CONTROL }
}
const fn alt(code: KeyCode) -> Key {
    Key { code, mods: KeyModifiers::ALT }
}

impl Key {
    fn matches(&self, e: &KeyEvent) -> bool {
        // Shift is implied by the character for printable keys.
        let mods = if matches!(e.code, KeyCode::Char(_)) { e.modifiers - KeyModifiers::SHIFT } else { e.modifiers };
        let code = e.code;
        code == self.code && mods == self.mods
    }
}

pub struct Binding {
    pub keys: &'static [Key],
    /// How the keys are shown: "j k", "⏎", "ctrl-p".
    pub label: &'static str,
    pub action: Action,
    pub scope: Scope,
    pub desc: &'static str,
    /// Short label for the footer; None keeps it out of the footer.
    pub hint: Option<&'static str>,
}

macro_rules! b {
    ($scope:ident, $label:expr, [$($key:expr),+], $action:expr, $desc:expr) => {
        Binding { keys: &[$($key),+], label: $label, action: $action, scope: Scope::$scope, desc: $desc, hint: None }
    };
    ($scope:ident, $label:expr, [$($key:expr),+], $action:expr, $desc:expr, $hint:expr) => {
        Binding { keys: &[$($key),+], label: $label, action: $action, scope: Scope::$scope, desc: $desc, hint: Some($hint) }
    };
}

use Action::*;

/// The order here is the order hints appear in the footer.
pub static BINDINGS: &[Binding] = &[
    // Sidebar.
    b!(
        Sidebar,
        "↑↓ j k",
        [code(KeyCode::Up), code(KeyCode::Down), k('k'), k('j')],
        Up,
        "move through workspaces and agents",
        "move"
    ),
    b!(Sidebar, "⏎", [code(KeyCode::Enter), code(KeyCode::Right), k('l')], Open, "open the selection", "open"),
    b!(Sidebar, "c", [k('c')], NewShell, "new shell in this workspace", "shell"),
    b!(Sidebar, "C", [k('C')], NewClaude, "new Claude Code pane in this workspace", "claude"),
    b!(
        Sidebar,
        "x",
        [k('x')],
        Close,
        "close a pane, end a session (VS Code closes its terminal), dismiss an ended one"
    ),
    b!(Sidebar, "r", [k('r')], Restart, "restart a pane, or resume a session in a new terminal"),
    b!(Sidebar, "m", [k('m')], Menu, "actions for the selection (or right-click)"),
    b!(Sidebar, "K J", [k('K'), k('J')], MoveUp, "move this workspace up / down (only you move workspaces)"),
    b!(Sidebar, "h", [k('h')], ToggleHistory, "show past sessions from the last 7 days"),
    b!(Sidebar, "e", [k('e')], ToggleEnded, "show or hide ended sessions"),
    b!(Sidebar, "X", [k('X')], DismissEnded, "dismiss every ended session"),
    b!(Sidebar, "/", [k('/')], Filter, "filter the sidebar"),
    b!(Sidebar, "g G", [k('g'), code(KeyCode::Home)], Top, "first item"),
    b!(Sidebar, "G", [k('G'), code(KeyCode::End)], Bottom, "last item"),
    // Conversation.
    b!(Conversation, "↑↓ j k", [code(KeyCode::Up), code(KeyCode::Down), k('k'), k('j')], Up, "scroll a line", "scroll"),
    b!(Conversation, "ctrl-u ctrl-d", [ctrl('u'), ctrl('d')], HalfUp, "scroll half a page"),
    b!(Conversation, "K J", [k('K'), k('J')], CursorUp, "move the cursor (⏎ expands what it's on)"),
    b!(
        Conversation,
        "⏎",
        [code(KeyCode::Enter), k(' ')],
        Open,
        "expand or collapse the tool call at the cursor",
        "expand"
    ),
    b!(Conversation, "[ ]", [k('['), k(']')], PrevPrompt, "previous / next prompt you sent", "prompts"),
    b!(Conversation, "/", [k('/')], Search, "search the conversation", "search"),
    b!(Conversation, "n", [k('n')], NextMatch, "next match"),
    b!(Conversation, "y", [k('y')], Copy, "copy the message at the cursor", "copy"),
    b!(Conversation, "i", [k('i')], Compose, "write a prompt; it's typed into the session's editor terminal", "type"),
    b!(Conversation, "r", [k('r')], Resume, "resume this session in a terminal here", "resume"),
    b!(Conversation, "PgUp PgDn", [code(KeyCode::PageUp), code(KeyCode::PageDown)], PageUp, "scroll a page"),
    b!(Conversation, "g", [k('g'), code(KeyCode::Home)], Top, "start of the conversation"),
    b!(Conversation, "G", [k('G'), code(KeyCode::End)], Bottom, "latest message, and follow new ones", "latest"),
    b!(Conversation, "esc", [code(KeyCode::Esc), code(KeyCode::Left), k('h')], Back, "back to the sidebar", "back"),
    b!(Conversation, "x", [k('x')], CloseLeaf, "close this pane"),
    // Pane (focused, not typing).
    b!(Pane, "⏎", [code(KeyCode::Enter), k('i')], Open, "type into the pane (ctrl-q comes back)", "type"),
    b!(
        Pane,
        "PgUp PgDn",
        [code(KeyCode::PageUp), code(KeyCode::PageDown)],
        PageUp,
        "scroll back through output",
        "scroll"
    ),
    b!(
        Pane,
        "↑↓ j k",
        [code(KeyCode::Up), code(KeyCode::Down), k('k'), k('j')],
        Up,
        "scroll back through output a line"
    ),
    b!(Pane, "ctrl-u ctrl-d", [ctrl('u'), ctrl('d')], HalfUp, "scroll half a page"),
    b!(Pane, "G", [k('G'), code(KeyCode::End)], Bottom, "back to the live bottom"),
    b!(Pane, "x", [k('x')], Close, "kill or close the pane"),
    b!(Pane, "r", [k('r')], Restart, "restart an exited pane"),
    b!(Pane, "m", [k('m')], Menu, "actions for this pane (or right-click)"),
    b!(Pane, "esc", [code(KeyCode::Esc), code(KeyCode::Left), k('h')], Back, "back to the sidebar", "back"),
    // Changes.
    b!(
        Changes,
        "↑↓ j k",
        [code(KeyCode::Up), code(KeyCode::Down), k('k'), k('j')],
        Up,
        "move through files, or scroll the diff",
        "move"
    ),
    b!(
        Changes,
        "⏎ l",
        [code(KeyCode::Enter), code(KeyCode::Right), k('l')],
        Open,
        "open a folder, or read the file's diff",
        "diff"
    ),
    b!(Changes, "h", [code(KeyCode::Left), k('h')], Left, "back to the files, close a folder, or up a level"),
    b!(Changes, "space", [k(' ')], Stage, "stage or unstage the file or folder", "stage"),
    b!(Changes, "c", [k('c')], Commit, "write a commit message; ⏎ commits what's staged", "commit"),
    b!(Changes, "P", [k('P')], Push, "push (publishes a new branch)", "push"),
    b!(Changes, "p", [k('p')], Pull, "pull, fast-forward only"),
    b!(Changes, "[ ]", [k('['), k(']')], PrevHunk, "previous / next change in the diff"),
    b!(Changes, "a", [k('a')], ToggleAll, "this agent's changes, or every change in the workspace", "all"),
    b!(Changes, "o", [k('o')], OpenEditor, "open the file in your editor at the change"),
    b!(Changes, "e", [k('e')], Edit, "edit the file at the change in your terminal editor"),
    b!(Changes, "s", [k('s')], SideBySide, "diffs side by side, or unified"),
    b!(Changes, "X", [k('X')], Discard, "discard the file's changes (asks first; new files go to the Trash)"),
    b!(Changes, "y", [k('y')], Copy, "copy the file's path"),
    b!(Changes, "PgUp PgDn", [code(KeyCode::PageUp), code(KeyCode::PageDown)], PageUp, "scroll the diff a page"),
    b!(Changes, "g", [k('g'), code(KeyCode::Home)], Top, "first file"),
    b!(Changes, "G", [k('G'), code(KeyCode::End)], Bottom, "last file"),
    b!(Changes, "esc", [code(KeyCode::Esc)], Back, "back to the files, then the sidebar", "back"),
    b!(Changes, "x", [k('x')], CloseLeaf, "close this pane"),
    // Files.
    b!(
        Files,
        "↑↓ j k",
        [code(KeyCode::Up), code(KeyCode::Down), k('k'), k('j')],
        Up,
        "move through files, or scroll the file",
        "move"
    ),
    b!(
        Files,
        "⏎ l",
        [code(KeyCode::Enter), code(KeyCode::Right), k('l')],
        Open,
        "open a folder, or read the file",
        "open"
    ),
    b!(Files, "h", [code(KeyCode::Left), k('h')], Left, "back to the tree, close a folder, or up a level"),
    b!(Files, "/", [k('/')], Search, "search every file's contents", "search"),
    b!(Files, "e", [k('e')], Edit, "edit in your terminal editor ($EDITOR), at this line", "edit"),
    b!(Files, "o", [k('o')], OpenEditor, "open in your editor (VS Code) at this line"),
    b!(Files, "[ ]", [k('['), k(']')], PrevHunk, "previous / next change in the file"),
    b!(Files, "y", [k('y')], Copy, "copy the file's path"),
    b!(Files, "PgUp PgDn", [code(KeyCode::PageUp), code(KeyCode::PageDown)], PageUp, "scroll the file a page"),
    b!(Files, "g", [k('g'), code(KeyCode::Home)], Top, "top"),
    b!(Files, "G", [k('G'), code(KeyCode::End)], Bottom, "bottom"),
    b!(Files, "esc", [code(KeyCode::Esc)], Back, "back to the tree, then the sidebar", "back"),
    b!(Files, "x", [k('x')], CloseLeaf, "close this pane"),
    // Project overview.
    b!(Overview, "esc", [code(KeyCode::Esc), code(KeyCode::Left), k('h')], Back, "back to the sidebar", "back"),
    b!(Overview, "x", [k('x')], CloseLeaf, "close this pane"),
    // Everywhere.
    b!(Global, "⇥", [code(KeyCode::Tab)], FocusNext, "next pane (the sidebar, then each pane)", "focus"),
    b!(Global, "⇧⇥", [code(KeyCode::BackTab)], FocusPrev, "previous pane"),
    b!(
        Global,
        "⌥←↑↓→",
        [alt(KeyCode::Left), alt(KeyCode::Right), alt(KeyCode::Up), alt(KeyCode::Down)],
        FocusLeft,
        "move to the pane in that direction (zellij)"
    ),
    b!(Global, "|", [k('|')], SplitRight, "split: a new shell to the right", "split"),
    b!(Global, "-", [k('-')], SplitDown, "split: a new shell below"),
    b!(Global, "C", [k('C')], NewClaude, "Claude Code in a new pane beside this one"),
    b!(Global, "T", [k('T')], NewTab, "a new tab with a shell"),
    b!(Global, "d", [k('d')], OpenChanges, "Changes in the panel on the right (again: back to Files)", "changes"),
    b!(Global, "f", [k('f')], OpenFiles, "show or hide the Files panel on the right", "files"),
    b!(Global, "< >", [k('<'), k('>')], Shrink, "make this pane smaller / bigger"),
    b!(Global, "=", [k('=')], Equalize, "make every pane even"),
    b!(
        Global,
        "1-9",
        [k('1'), k('2'), k('3'), k('4'), k('5'), k('6'), k('7'), k('8'), k('9')],
        Tab(0),
        "go to tab n",
        "tabs"
    ),
    b!(Global, "{ }", [k('{'), k('}')], PrevTab, "previous / next tab"),
    b!(Global, "ctrl-p", [ctrl('p')], Finder, "find anything: files, # workspaces, @ agents, > actions", "find"),
    b!(Global, "ctrl-o", [ctrl('o')], OpenProject, "open a workspace (any folder) in a new tab", "open"),
    b!(Global, "n", [k('n')], NextAlert, "jump to the next agent that needs you", "next alert"),
    b!(Global, "z", [k('z')], Zoom, "zoom this pane (again to unzoom)"),
    b!(Global, "\\", [k('\\')], ToggleSidebar, "shrink the sidebar to glyphs, or widen it"),
    b!(Global, "o", [k('o')], OpenEditor, "open in your editor; a VS Code session: go to its terminal"),
    b!(Global, "O", [k('O')], Reveal, "show it in Finder"),
    b!(Global, "t", [k('t')], Themes, "choose a theme", "theme"),
    b!(Global, "F12", [code(KeyCode::F(12))], ToggleMouse, "turn mouse capture off so the terminal can select text"),
    b!(Global, "?", [k('?')], Help, "all keys", "help"),
    b!(Global, "q", [k('q'), ctrl('c')], Quit, "quit (panes keep running)", "quit"),
];

/// The action bound to a key in a scope (falling back to Global).
pub fn lookup(scope: Scope, e: &KeyEvent) -> Option<Action> {
    for s in [scope, Scope::Global] {
        for b in BINDINGS.iter().filter(|b| b.scope == s) {
            if let Some(i) = b.keys.iter().position(|key| key.matches(e)) {
                return Some(resolve(b, i));
            }
        }
    }
    None
}

/// Paired keys on one row map to related actions.
fn resolve(b: &Binding, i: usize) -> Action {
    match (b.action, i) {
        (Up, 1 | 3) => Down,
        (PageUp, 1) => PageDown,
        (PrevPrompt, 1) => NextPrompt,
        (PrevHunk, 1) => NextHunk,
        (FocusLeft, 1) => FocusRight,
        (FocusLeft, 2) => FocusUp,
        (FocusLeft, 3) => FocusDown,
        (Shrink, 1) => Grow,
        (MoveUp, 1) => MoveDown,
        (HalfUp, 1) => HalfDown,
        (CursorUp, 1) => CursorDown,
        (PrevTab, 1) => NextTab,
        (Tab(_), n) => Tab(n as u8),
        (a, _) => a,
    }
}

/// Footer hints for a scope: its own first, then global ones.
pub fn hints(scope: Scope) -> Vec<(&'static str, &'static str)> {
    BINDINGS
        .iter()
        .filter(|b| b.scope == scope || b.scope == Scope::Global)
        .filter_map(|b| b.hint.map(|h| (b.label, h)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn ev(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn lookups_and_fallback() {
        assert_eq!(lookup(Scope::Sidebar, &ev(KeyCode::Char('j'))), Some(Down));
        assert_eq!(lookup(Scope::Conversation, &ev(KeyCode::Char(']'))), Some(NextPrompt));
        assert_eq!(lookup(Scope::Sidebar, &ev(KeyCode::Char('3'))), Some(Tab(2)));
        assert_eq!(lookup(Scope::Pane, &ev(KeyCode::Char('?'))), Some(Help));
        assert_eq!(lookup(Scope::Sidebar, &KeyEvent::new(KeyCode::Char('C'), KeyModifiers::SHIFT)), Some(NewClaude));
        assert_eq!(lookup(Scope::Sidebar, &KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)), Some(Quit));
    }

    #[test]
    fn no_key_is_bound_twice_in_one_scope() {
        for scope in [
            Scope::Global,
            Scope::Sidebar,
            Scope::Conversation,
            Scope::Pane,
            Scope::Overview,
            Scope::Changes,
            Scope::Files,
        ] {
            let mut seen = HashSet::new();
            for b in BINDINGS.iter().filter(|b| b.scope == scope) {
                for key in b.keys {
                    assert!(
                        seen.insert(format!("{:?}{:?}", key.code, key.mods)),
                        "{:?} bound twice in {scope:?}",
                        key.code
                    );
                }
            }
        }
    }

    #[test]
    fn every_binding_is_described() {
        assert!(BINDINGS.iter().all(|b| !b.desc.is_empty() && !b.label.is_empty()));
    }
}
