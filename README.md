<div align="center">

```
┌─┐┬ ┬┬  ┌─┐┬─┐┌─┐
├─┤│ ││  │ │├┬┘├─┤
┴ ┴└─┘┴─┘└─┘┴└─┴ ┴
```

**Every Claude Code session across all your projects, on one screen.**

See which ones are working, which ones finished, and which ones are waiting on you.

</div>

![aulora dashboard](docs/screenshots/overview.png)

## Why

If you run Claude Code in several projects at once, you end up with a VS Code window per project and a few terminals in each. Claude finishes or stops to ask for permission, and you don't notice until you go looking. Then you have to remember what each session was doing.

aulora watches every session through Claude Code's hooks and shows them all in one terminal dashboard:

- **What needs you.** Permission prompts, questions (`AskUserQuestion`) and plans waiting for review show in amber at the top, with what Claude is asking for.
- **What's happening where.** Each project shows its branch and git status. For each session, tabs show the whole conversation (scrollable, searchable, with every tool call), an overview (current tool call, your last prompt, Claude's latest reply, recent tool calls with timings, files edited with `+/−` lines, context size) and its activity.
- **Run Claude and your shells inside it.** <kbd>C</kbd> starts Claude Code in a pane and <kbd>c</kbd> starts a shell. The daemon owns these terminals, so they keep running when you close the dashboard, and `aulora attach` puts any of them in another terminal. A Claude session in a pane shows its live status right above its screen.
- **Jump straight there.** <kbd>⏎</kbd> opens the project in your editor, and VS Code brings that project's window to the front. <kbd>n</kbd> cycles through the sessions that need you.

It works with Claude Code running anywhere: VS Code, Cursor, Ghostty, iTerm, tmux. aulora shows which app each session runs in.

## Screenshots

| A Claude Code pane, live | The whole conversation |
|---|---|
| ![pane](docs/screenshots/pane.png) | ![conversation](docs/screenshots/conversation.png) |
| **GitHub Light** | **tmux** |
| ![github light](docs/screenshots/github-light.png) | ![tmux](docs/screenshots/tmux.png) |

All screenshots use `aulora demo`'s sample data.

## Install

Requires Rust 1.85+ and Claude Code with hooks support.

```sh
cargo install --git https://github.com/CerealNotFound/aulora aulora
aulora hooks install     # adds aulora's hooks to ~/.claude/settings.json (backed up first)
aulora                   # open the dashboard
```

Sessions show up as soon as they start or you send a prompt. To try the dashboard without touching your settings, run `aulora demo`.

## Commands

| Command | What it does |
|---|---|
| `aulora` | Open the dashboard. Starts the daemon if it isn't running. |
| `aulora demo` | Dashboard with sample sessions. |
| `aulora status [--json] [--all]` | Print every live session and its state. `--all` adds past sessions. |
| `aulora hooks install [--dry-run]` | Add the hooks. Leaves your other hooks and settings untouched. |
| `aulora hooks uninstall` | Remove only aulora's hooks. |
| `aulora hooks status` | Show whether the hooks are installed. |
| `aulora daemon start\|stop\|status` | Control the background daemon. `stop` refuses while panes run, unless `--force`. |
| `aulora doctor` | Check hooks, daemon, editor and `claude` on PATH. |
| `aulora new [--claude] [-n NAME] [--cwd DIR] [-a] [-- CMD…]` | Start a pane: a shell, Claude Code, or any command. `-a` attaches right away. |
| `aulora ls [--json]` | List panes. |
| `aulora attach PANE` | Put a pane in this terminal, like `tmux attach`. <kbd>Ctrl</kbd>+<kbd>Q</kbd> detaches. |
| `aulora kill\|restart\|close PANE` | Stop a pane, start its program again, or remove it. |

`PANE` can be a pane id, a unique id prefix, a name, or `project/name`.

### Keys

aulora follows Herdr: a sidebar of **projects** above **agents** (Claude sessions, Claude panes, shells), and a main area with tabs. The mouse works everywhere: click to select or focus, wheel to scroll whatever is under the pointer, click a pane to type into it.

| Key | Action |
|---|---|
| <kbd>↑</kbd> <kbd>↓</kbd> / <kbd>j</kbd> <kbd>k</kbd> | Move in the sidebar; scroll a line in the main area |
| <kbd>⏎</kbd> | Open: focus the main area, type into a pane, expand a tool call |
| <kbd>⇥</kbd> | Switch focus between the sidebar and the main area |
| <kbd>1</kbd>–<kbd>4</kbd> | Switch tabs (Terminal, Conversation) |
| <kbd>Ctrl</kbd>+<kbd>Q</kbd> | Leave a terminal you're typing into (press it again to send Ctrl-Q to the program) |
| <kbd>c</kbd> / <kbd>C</kbd> | New shell / new Claude Code pane in the selected project |
| <kbd>n</kbd> | Jump to the next agent that needs you |
| <kbd>s</kbd> | Sort agents by priority, or group them by project |
| <kbd>x</kbd> | Kill a pane (asks first), close an exited one, or dismiss an ended session |
| <kbd>z</kbd> | Zoom the main area to the whole screen |
| <kbd>/</kbd> | Filter the sidebar; search a conversation |
| <kbd>h</kbd> / <kbd>e</kbd> | Show past sessions / ended sessions |
| <kbd>t</kbd> | Choose a theme |
| <kbd>F12</kbd> | Turn mouse capture off so your terminal can select text |
| <kbd>?</kbd> | All keys: type to filter, <kbd>⏎</kbd> to run one |

In a conversation: <kbd>[</kbd> <kbd>]</kbd> jump between your prompts, <kbd>y</kbd> copies the message at the cursor, <kbd>G</kbd> goes to the latest and follows new messages, <kbd>Esc</kbd> goes back to the sidebar.

States, as in Herdr: <span>●</span> red **blocked** (needs you) · yellow **working** · teal **done** (finished, and you haven't looked yet) · <span>○</span> green **idle** · <span>·</span> **ended**.

### Themes

The default follows Herdr: Catppuccin on your terminal's own background, switching to Catppuccin Latte when your terminal is light. Also built in: `tmux` (your terminal's colors and the classic green status bar), `terminal` (only your terminal's 16 colors), `github-dark`, `github-light`, `tokyo-night`, `gruvbox`, `nord`, `dracula`, `one-dark` and `aurora`. Press <kbd>t</kbd> to browse them, or pass `--theme NAME`. The choice is saved in `~/.aulora/config.json`.

## How it works

```
 claude (VS Code)  ─┐                                          ┌─ panes: shells, claude, dev servers
 claude (Ghostty)  ─┼─ hooks ─▶ aulora hook ─▶ unix socket ─▶ aulora daemon ─▶ dashboard(s), aulora attach
 claude (in a pane)─┘                                          │  sessions, git, liveness
                                                               └─ SQLite (~/.aulora/state.db)
```

1. Claude Code runs `aulora hook` on each event: session start, prompt, tool call, permission request, notification, stop, compaction, subagent start and stop, and session end. The hook reads the event, drops large fields like file contents and tool output, and sends it to the daemon in about 4 ms. It never prints anything. If the daemon is down, the event is saved and the daemon is started.
2. The daemon turns events into session state, reads new lines from each session's transcript (for the AI title, model and token usage), polls `git status` for active projects, and checks whether each Claude process is still alive.
3. The dashboard subscribes over the socket and gets a fresh snapshot after every change.
4. It also reads Claude Code's list of running sessions and recent transcripts, so sessions that were already open when you installed the hooks show up, and so do past sessions from the last 7 days (<kbd>h</kbd>).
5. Panes are terminals the daemon runs itself. It keeps a copy of each screen, so the dashboard or `aulora attach` can show a pane instantly, and keystrokes go back the same way.

Everything stays on your machine. aulora makes no network requests.

More detail is in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Roadmap

- [x] **1. Status dashboard.** Hooks, daemon and the terminal dashboard.
- [x] **2. Daemon-owned terminals.** Live panes in the dashboard that survive closing it, and `aulora attach` from any terminal.
- [ ] **3. VS Code extension.** The same terminals inside VS Code, plus jump-to-terminal.
- [ ] **4. Restore.** Bring every project's terminals back after a reboot, with `claude --resume`.
- [ ] **5. "Where you left off" cards.** A summary per project when you switch back to it.
- [ ] **6. Notifications and a global hotkey switcher.**

## Development

```sh
cargo test
cargo run -- demo                              # interactive demo
cargo run -- tui --demo --snapshot 180x50      # print one frame as text
AULORA_HOME=/tmp/aulora-dev cargo run -- daemon # isolated daemon for testing
```

To test against real sessions without touching your settings:

```sh
aulora hooks install --settings /tmp/aulora-settings.json
claude --settings /tmp/aulora-settings.json
```

## Contributing

Issues and pull requests are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md) for setup and checks, and for the commit guidelines, which follow Zulip's commit discipline.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
