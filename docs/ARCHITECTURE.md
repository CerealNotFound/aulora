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
- `--snapshot WxH` renders one frame to text with Ratatui's `TestBackend`. It's useful for docs and checking the layout.
