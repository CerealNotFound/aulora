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

## Dashboard

- Gets snapshots on a background thread and redraws at about 16 fps, so spinners and timers move without daemon traffic.
- Layout: header with counters, alert banner, project/session tree, then for the selected session a detail card, stats tiles, now panel, activity sparkline, conversation, recent tools and files. The activity feed runs along the bottom. Narrow terminals get the list only.
- `--snapshot WxH` renders one frame to text with Ratatui's `TestBackend`. It's useful for docs and checking the layout.
