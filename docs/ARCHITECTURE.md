# Architecture

aulora has three parts in one binary:

| Part | Crate | Job |
|---|---|---|
| `aulora hook` | `crates/aulora` | Run by Claude Code for every hook event. Trims the event and forwards it. |
| daemon | `crates/aulora-daemon` | Holds the state of every session and serves it over a Unix socket. |
| dashboard | `crates/aulora-tui` | Terminal UI built with Ratatui. |

Shared types, the protocol and all state logic live in `crates/aulora-core`.

## The hook path

Speed and silence matter here: hooks run synchronously inside Claude Code, and on some events (`UserPromptSubmit`, `SessionStart`) anything printed to stdout is added to Claude's context.

- `main()` checks for `hook` before parsing arguments.
- The hook command is `exec '<path>/aulora' hook`. `exec` replaces the shell, so the hook's parent process is Claude Code. That PID is sent along so the daemon can tell when a session's process exits.
- `reduce::trim_payload` drops `tool_response`, file contents and edit strings. Before dropping them it counts lines added and removed. Prompts and messages are clipped.
- Environment variables identify where the session runs: `TERM_PROGRAM`, `__CFBundleIdentifier` (macOS), `TMUX`, `CLAUDE_PROJECT_DIR`, and `AULORA_PANE_ID` for terminals aulora will own in step 2.
- Send to the socket with a 300 ms write timeout. If that fails, append to `~/.aulora/spool.jsonl` and start the daemon detached (`setsid`). The daemon replays the spool on startup.

A hook takes about 4 ms (release build, Apple Silicon).

## State

`reduce::apply_hook(&mut Session, &HookEnvelope) -> Vec<FeedEvent>` is a pure function, unit-tested and also used by the demo.

| Event | State |
|---|---|
| `SessionStart` | idle |
| `UserPromptSubmit` | working, turn starts |
| `PreToolUse` | working, current tool. `AskUserQuestion` → needs you (question). `ExitPlanMode` → needs you (plan) |
| `PermissionRequest`, `Notification(permission_prompt)` | needs you (permission) |
| `Notification(elicitation_*, agent_needs_input)` | needs you (input) |
| `PostToolUse(Failure)` | back to working, tool recorded with duration, files touched |
| `Stop` | your turn, last reply recorded, turn duration |
| `StopFailure` | errored |
| `PreCompact` / `PostCompact` | compacting / back |
| `SessionEnd`, process exited | ended |

The transcript (`transcript_path`) is read incrementally from a saved byte offset. It provides the AI title, model, context size (input + cache tokens of the latest assistant message), output tokens (counted once per message id) and the text Claude has written so far in the current turn. Large `api-request*` lines are skipped without parsing.

## Daemon

- Tokio, one Unix socket at `~/.aulora/aulora.sock` (`$AULORA_HOME` overrides; long paths fall back to `/tmp/aulora-<hash>.sock` because macOS limits socket paths to 104 bytes).
- `flock` on `daemon.lock` keeps a single instance.
- Newline-delimited JSON. Clients send `hook`, `subscribe`, `snapshot`, `ping`, `dismiss`, `clear_ended` or `shutdown`.
- Subscribers get a full snapshot after each change, coalesced over 40 ms. Snapshots are small (tens of KB), which keeps clients stateless.
- Background loops: git status every 3 s for active projects (`--no-optional-locks`, so it never fights your own git commands), process liveness every 3 s (start time guards against PID reuse), transcript refresh every 2 s for busy sessions.
- SQLite in WAL mode on a writer thread, batching writes per transaction. Sessions and the feed survive daemon restarts. Ended sessions expire after 12 hours.

## Discovery

Hooks only describe sessions that fire an event after the hooks are installed. `crates/aulora-daemon/src/discover.rs` fills the gaps from two files Claude Code keeps. Neither is a documented interface, so both are read best-effort and hooks always win.

- `~/.claude/sessions/<pid>.json` lists running sessions with their id, folder and a busy/idle status. It's polled every 2 s. A listed session whose process is alive and that no hook has described becomes a session with origin `running`. Its state follows the listed status, and its counts come from its transcript.
- `~/.claude/projects/<dir>/<id>.jsonl` transcripts from the last 7 days are scanned every minute, skipping files whose mtime hasn't changed. Ones with at least one prompt become sessions with origin `history`: ended, but with their title, prompts, last reply, tool calls and edited files rebuilt from the transcript. The dashboard hides them until you press `h`.
- The first hook event for a session switches its origin to `hooks`. Dismissing a session keeps it from being rediscovered, while sessions that simply expire can come back as history.

## Panes

The daemon runs terminals itself (`crates/aulora-daemon/src/pty.rs`), so they outlive any client.

- Each pane is a pseudo-terminal from `portable-pty` running the user's login shell: `$SHELL -l` for a shell, `$SHELL -l -i -c 'claude …; exec $SHELL -l'` for Claude, so a pane drops back to a shell when Claude exits, and `$SHELL -l -c …` for a command.
- The environment is cleaned of variables a pane must not inherit from whoever started the daemon, which is often a Claude Code hook inside another terminal (`CLAUDECODE`, `TERM_PROGRAM`, `VSCODE_*` and similar). Then `TERM_PROGRAM=aulora` and `AULORA_PANE_ID` are set. Hooks pass `AULORA_PANE_ID` back, which is how a Claude session is linked to the exact pane it runs in.
- A reader thread feeds all output through a `vt100` parser that mirrors the screen, with 5,000 lines of scrollback, and publishes it on a broadcast channel. Both happen under one lock, so `attach()` can take the current screen and subscribe atomically: a client never misses or double-applies a byte.
- Attaching sends the screen redrawn from blank (`state_formatted`, which includes input modes such as application cursor keys and bracketed paste), then raw output. Output already queued is merged into one message. A client that falls behind the channel gets a fresh screen instead of a gap.
- Writes go through their own thread, because writing to a terminal blocks when the program isn't reading.
- The PTY size follows the last client to resize it.
- Panes don't survive the daemon stopping. Bringing them back is step 4.

Clients use two kinds of connection. The dashboard keeps one for snapshots and one for pane streams (attach, input, resize), which reconnects and re-attaches on its own. `aulora attach` passes bytes straight through in both directions, so mouse modes, bracketed paste and every key combination work as if the program ran in that terminal.

## Dashboard

- Gets snapshots on a background thread and redraws at about 16 fps, so spinners and timers move without daemon traffic.
- Layout: header with counters, alert banner, project/session tree, then for the selected session a detail card, stats tiles, now panel, activity sparkline, conversation, recent tools and files. The activity feed runs along the bottom. Narrow terminals get the list only.
- A selected pane is drawn from the dashboard's own `vt100` copy of its screen. In terminal mode, keys are encoded to the bytes a terminal sends (xterm modifier codes, application cursor mode, Shift+Enter as `ESC CR` for Claude Code) and pastes are wrapped when the program asked for bracketed paste. The pane is resized to exactly the space it's drawn in.
- The main area shows one tab at a time. The Conversation tab reads the session's transcript directly (the dashboard runs on the same machine), keeps a byte offset so a multi-megabyte transcript is parsed once, pairs each tool call with its result, and caches the wrapped lines per width.
- Every color comes from the active theme (`crates/aulora-tui/src/theme.rs`). Themes whose background is `Reset` draw on the terminal's own background. Without a saved choice, the dashboard asks the terminal for its background with OSC 11 and picks Catppuccin or Catppuccin Latte.
- `--snapshot WxH` renders one frame to text with Ratatui's `TestBackend`. It's useful for docs and checking the layout.

## Editor extension

`editors/vscode` is a VS Code (and Cursor) extension in plain JavaScript, with no dependencies and no build step. `aulora vscode install` packages it as a .vsix from files built into the binary and installs it with the editor's CLI.

The extension connects to the daemon's socket and sends `editor` with its window's terminals and their shell PIDs, again whenever terminals open or close. The connection stays open. Every few seconds, and when a session starts, the daemon walks each live Claude process's parents (up to 8 levels: `claude` → `zsh` is typical) and matches them against those shell PIDs. A matched session gets `terminal.editor` in the snapshot, which is what makes aulora show a prompt box for it.

`send_prompt` refuses sessions that have ended or are waiting on a permission prompt or question, where typed keys would answer the dialog. Otherwise it sends `type` to the extension, which writes the text with `terminal.sendText` (a bracketed paste when it has several lines), waits briefly, sends Enter, and answers `typed`. The daemon waits up to 5 seconds for that answer, so the client hears whether the prompt really arrived.

## Changes and Files

The Changes and Files tabs read git and the file system on one worker thread in the dashboard (`changes.rs`), never on the UI thread. Jobs go in over a channel and answers come back to the event loop, which polls faster while any are pending. The worker drops stale requests: only the newest status per project, the newest diff, file read and search survive a burst.

- Changes: one `git status --porcelain=v2 -z` and two `git diff -U0` calls give the tree, counts and per-file attribution; the selected file's diff is read with context on demand. Attribution (`attrib.rs`) reads each session's transcript incrementally for Edit and Write patches. Commit, push and pull run git in a new session with no controlling terminal and `GIT_TERMINAL_PROMPT=0`, so nothing can prompt over the dashboard.
- Files (`files.rs`): folders listed one level at a time with the `ignore` crate, the whole tree indexed for Ctrl-P (up to 200,000 files) and ranked with `nucleo-matcher`, contents searched with `grep-searcher`, and gutter marks from `git diff -U0 HEAD`.
- Highlighting (`highlight.rs`): syntect with two-face's grammars and a theme whose colors encode roles, mapped to aulora's palette when drawn.
