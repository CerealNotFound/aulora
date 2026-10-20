# Backlog

Every request made while building aulora, with where it stands. Nothing
leaves this list until it's done and checked in real use, or dropped on
purpose (with the reason). Newest requests are added at the bottom of
their section.

Status: ✅ done · 🟡 done but not yet checked in real use · ⬜ open

## Keys and input

| | Request | Where it stands |
|---|---|---|
| 🟡 | Option-Delete deletes a whole word | Done in every aulora text field (with Ctrl-W, Ctrl-U). In panes it passes ESC DEL through, which works only if the terminal sends it: Terminal.app needs "Use Option as Meta key". `aulora keys` shows what arrives; waiting on that check. |
| ✅ | Option-arrows keep jumping words while typing in a pane | They go to the program; pane focus moves with Tab, clicks, or Option-arrows only when not typing. |
| ✅ | Ctrl-Delete deletes a word in panes | Sends Ctrl-W. |
| ✅ | Scroll a conversation reliably with the keyboard | ↑↓ / j k a line, Ctrl-U / Ctrl-D half a page, PgUp / PgDn, g / G; J / K move the cursor. |
| ✅ | Scroll a pane's history with the keyboard | Ctrl-Q, then ↑↓ / j k, Ctrl-U / Ctrl-D, G back to live; Shift-PgUp / PgDn while typing. |
| ✅ | Split a specific Claude terminal with a key | Ctrl-Q, then `\|` right, `-` down, `C` Claude beside it; the frame says so while typing; the pane's menu has it too. |
| 🟡 | Voice mode: hold Space for Claude | Repeated spaces reach Claude in a pane as in Terminal.app. Microphone permission for a pane under the daemon is untested. |
| ✅ | Claude's / commands in aulora | Native in panes. In the prompt box for VS Code sessions: a `/` menu with built-in, your, the project's commands and skills; Tab completes. |
| ✅ | Paste images | Ctrl-V in a Claude pane (Claude reads the clipboard); an image-only Cmd-V is passed on as Ctrl-V; Ctrl-V in the prompt box attaches the image by path. |
| ✅ | Multi-line prompts must not break the screen | Line breaks and tabs are drawn as spaces everywhere; CRLF files too. |
| ✅ | After sending a prompt, the cursor stays in the input | The prompt box stays open and empty, ready for the next one. |

## Mouse and links

| | Request | Where it stands |
|---|---|---|
| ✅ | Scrolling and clicks that do something | Wheel under the pointer, click to focus or type, drag pane borders. |
| 🟡 | Cmd-click a URL to open it | A URL under the pointer underlines; a click (Cmd-click arrives as a click) opens it. Untested in Terminal.app, which may also open it itself (twice). |
| ✅ | Open in Finder | `O`, and "Open in Finder" in row menus. |

## Layout and look

| | Request | Where it stands |
|---|---|---|
| ✅ | Look like Herdr (the screenshot) | Tiled panes in tabs per project, accent frame on the focused pane, `new` / `menu` at the bottom of the sidebar. |
| ✅ | Projects and agents in one tree, not two sections | One tree; kept when the Herdr screenshot showed two sections. |
| ✅ | Every agent is a terminal | Ended sessions open as `claude --resume`; sessions in VS Code show their conversation read-only. |
| ✅ | A way to close terminals and agents | `×` on hover, `x`, the row menu; a running pane asks first. |
| ✅ | New terminals start in the selected project | `c`, `C`, `\|`, `-`, the menus. |
| ✅ | Changes and Files visible on screen | Buttons in the tab strip (`d changes N`, `f files`). |
| ✅ | The line under an agent must be true | Its last reply, your last prompt since, or what it's doing; "waiting for a first prompt" only with no history. |
| ✅ | No truncation that hides what you came to read | Agent rows wrap to 2+2 lines; notes wrap to 6 lines. |
| ✅ | Notes such as "agent done" readable in full | Whole summary kept (1,200 chars), wrapped. |
| ⬜ | Compact sidebar on narrow terminals | Designed (glyphs and counts), not built. |
| ⬜ | First-run onboarding | Not built. |

## Conversations

| | Request | Where it stands |
|---|---|---|
| ✅ | No Activity tab; events inline like Claude Code | Notes in the conversation. |
| ✅ | Claude Code's working line ("Doing…") with tips | Spinner, verb, time, target, rotating aulora tips. |
| 🟡 | Recap at the end of a response | aulora's recap: "✻ Worked for 3m 12s · edited N files · ran N commands". Claude Code's own AI-written "※ recap" isn't in transcripts, so it can't be shown yet. |
| ✅ | Prompts sent mid-turn show up | Read from queued commands. |
| ✅ | Queued messages visible, as in Claude Code | Under the working line until absorbed. |
| ✅ | Prompts with images show up | Fixed in both readers. |
| ✅ | Background task notifications aren't prompts | Shown as notes. |

## VS Code becomes optional

| | Request | Where it stands |
|---|---|---|
| 🟡 | Send a prompt from aulora into a session running in VS Code, same session | Extension built and installed; prompt box in aulora. Needs the daemon restarted once to work live. |
| ✅ | Read code and changes inside aulora | Changes tab (diffs, attribution), Files tab (tree, viewer, gutter marks). |
| ✅ | Commit and push from aulora | `c`, `P`, `p` in Changes. |
| ✅ | Discard changes | `X` (asks; new files go to the Trash). |
| ✅ | Edit in a terminal editor | `e` opens $EDITOR in a pane at the line. |
| ✅ | Open a new project without VS Code | Ctrl-O (repos on disk, recent, any path), `aulora open [dir]`. |
| ✅ | Find anything | Ctrl-P: files, `#` projects, `@` agents, `>` actions. |
| ⬜ | Jump from aulora to a session's VS Code terminal | Extension can do it (terminal.show); not built. |
| ⬜ | aulora's panes inside VS Code | Roadmap step 3; not built. |
| ⬜ | Side-by-side diffs | Not built. |

## Reliability

| | Request | Where it stands |
|---|---|---|
| ✅ | Restarts and updates must not kill panes | `aulora daemon restart`; the daemon upgrades itself in place when the program is replaced. |
| ✅ | Panes come back after a reboot | Saved and restored with the same ids; Claude resumes its session. |
| ✅ | Notifications when an agent needs you | Desktop notification from the daemon. |
| ⬜ | "Where you left off" cards | Roadmap step 5; not built. |
| ⬜ | Global hotkey switcher | Roadmap step 6; not built. |

## Process

- Commits follow Zulip's discipline; every commit passes fmt, clippy and tests; push only when asked.
- Never restart the real daemon without checking its panes and asking.
- No artifacts; everything stays in the terminal or local files.
- The bar is market-tested products (Herdr, lazygit, VS Code), checked by rendering and using it, not by assuming.
