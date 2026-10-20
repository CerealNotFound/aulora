# Backlog

Every request made while building aulora, with where it stands. Nothing
leaves this list until it's done and checked in real use, or dropped on
purpose (with the reason). Newest requests are added at the bottom of
their section.

Status: ✅ done · 🟡 done but not yet checked in real use · ⬜ open

## Keys and input

| | Request | Where it stands |
|---|---|---|
| 🟡 | Option-Delete deletes a whole word | Reported 2026-10-20 that it doesn't. Cause: none of your Terminal.app profiles has "Use Option as Meta key" on, so Option-Delete arrives as a plain Delete and no program can tell them apart. aulora handles ESC DEL (and Ctrl-W, Ctrl-U) in its text fields and passes it to panes. Fix: tick Terminal → Settings → Profiles → Keyboard → "Use Option as Meta key" (takes effect at once). The welcome checklist (menu → Getting started) now checks this live in Terminal.app. |
| ✅ | Option-arrows keep jumping words while typing in a pane | They go to the program; pane focus moves with Tab, clicks, or Option-arrows only when not typing. |
| ✅ | Ctrl-Delete deletes a word in panes | Sends Ctrl-W. |
| ✅ | Scroll a conversation reliably with the keyboard | ↑↓ / j k a line, Ctrl-U / Ctrl-D half a page, PgUp / PgDn, g / G; J / K move the cursor. |
| ✅ | Scroll a pane's history with the keyboard | Ctrl-Q, then ↑↓ / j k, Ctrl-U / Ctrl-D, G back to live; Shift-PgUp / PgDn while typing. |
| ✅ | Split a specific Claude terminal with a key | Ctrl-Q, then `\|` right, `-` down, `C` Claude beside it; the frame says so while typing; the pane's menu has it too. |
| 🟡 | Voice mode: hold Space for Claude | Repeated spaces reach Claude in a pane as in Terminal.app. Microphone permission for a pane under the daemon is untested. |
| ✅ | Claude's / commands in aulora | Native in panes. In the prompt box for VS Code sessions: a `/` menu with built-in, your, the workspace's commands and skills; Tab completes. |
| ✅ | Paste images | Ctrl-V in a Claude pane (Claude reads the clipboard); an image-only Cmd-V is passed on as Ctrl-V; Ctrl-V in the prompt box attaches the image by path. |
| 🟡 | Cmd-V pastes text and images | Reported again 2026-10-20. Cause for text: with the sidebar focused (where you are after launching or clicking a row) a paste was dropped silently. Now it goes to the selected pane (typing starts) or session (prompt box opens), into Ctrl-P, help and filter boxes, and when it can't go anywhere a message says why. Checked by driving aulora in a pty with the exact bracketed-paste bytes Cmd-V sends. Images: if Terminal.app sends nothing at all for an image-only Cmd-V, no terminal program can see it; Ctrl-V is then the image paste, as in Claude Code. `aulora keys` with a screenshot copied settles which. |
| ✅ | Multi-line prompts must not break the screen | Line breaks and tabs are drawn as spaces everywhere; CRLF files too. |
| ✅ | After sending a prompt, the cursor stays in the input | The prompt box stays open and empty, ready for the next one. |
| 🟡 | Cmd-A selects everything in the input, then Delete clears it | In the prompt box: Cmd-A where the terminal sends it (Ghostty, kitty, WezTerm, iTerm2); Terminal.app keeps Cmd-A for its own Select All and never sends it, so there Ctrl-A twice does the same. The text highlights; Delete clears it, typing or pasting replaces it, an arrow lets it go. |
| 🟡 | Selecting in the input copies too | A mouse selection in the prompt box copies like anywhere else, and select-all copies the whole prompt. |
| 🟡 | Shift-Enter for a new line | Works in panes and the prompt box where the terminal sends Shift-Enter distinctly. Terminal.app sends it as plain Enter, so there: Option-Enter (Option as Meta, or Claude's /terminal-setup), Ctrl-J, or `\` then Enter, all of which work in panes and the prompt box. `aulora keys` says which case you're in. |

## Mouse and links

| | Request | Where it stands |
|---|---|---|
| ✅ | Scrolling and clicks that do something | Wheel under the pointer, click to focus or type, drag pane borders. |
| 🟡 | Cmd-click a URL to open it | A URL under the pointer underlines; a click (Cmd-click arrives as a click) opens it. Untested in Terminal.app, which may also open it itself (twice). |
| 🟡 | Select text with the mouse and copy it, as in Claude Code | Drag over text anywhere (panes, conversations, files, diffs, the sidebar, the prompt box): it highlights, and letting go copies it and says what. Double-click copies a word in a pane, file or diff. A pane gives wrapped lines back whole. Copying now goes through pbcopy, because Terminal.app ignores the OSC 52 that `y` used, so `y` never copied there either. Checked by driving aulora in a pty with real mouse events. A program in a pane that takes the mouse (vim, htop) still gets the drag while you type into it. |
| ✅ | Open in Finder | `O`, and "Open in Finder" in row menus. |

## Layout and look

| | Request | Where it stands |
|---|---|---|
| ✅ | Look like Herdr (the screenshot) | Tiled panes in tabs per workspace, accent frame on the focused pane, `new` / `menu` at the bottom of the sidebar. |
| ✅ | Workspaces and agents in one tree, not two sections | One tree; kept when the Herdr screenshot showed two sections. |
| ✅ | Every agent is a terminal | Ended sessions open as `claude --resume`; sessions in VS Code show their conversation read-only. |
| 🟡 | A way to close terminals and agents | Panes: `×`, `x`, the menu. Live sessions outside aulora had no close at all ("why am I not able to close a specific terminal here"); now `×`, `x` and "End session" ask, then close a VS Code session's terminal through the extension (0.2.2), or end Claude (SIGTERM, after checking the PID is still Claude) in other terminals. Needs the restarted daemon and reloaded VS Code. |
| ✅ | New terminals start in the selected workspace | `c`, `C`, `\|`, `-`, the menus. |
| ✅ | Changes and Files visible on screen | Buttons in the tab strip (`d changes N`, `f files`). |
| ✅ | The line under an agent must be true | Its last reply, your last prompt since, or what it's doing; "waiting for a first prompt" only with no history. |
| ✅ | No truncation that hides what you came to read | Agent rows wrap to 2+2 lines; notes wrap to 6 lines. |
| ✅ | Notes such as "agent done" readable in full | Whole summary kept (1,200 chars), wrapped. |
| ✅ | The workspace list mustn't shuffle | Workspaces keep the order they first appeared in, saved across restarts; `J` / `K` move a workspace. Dots and `n` show what needs you. |
| ✅ | Move agents up and down in the sidebar | Replaced by the next row at your request: agents order themselves by activity, so a hand-set order would be undone; `J` / `K` move workspaces (on an agent row, its workspace). |
| 🟡 | Recently active agent on top; workspaces only move by hand | Within a workspace, the agent that most recently started, got a prompt, finished or started needing you is on top (not every tool call, so two busy agents don't keep swapping). Workspaces never move by themselves; `J` / `K` or the workspace menu move them. |
| ✅ | A better name than "projects" | "workspaces", as VS Code calls the folder you open: the sidebar, palette (`#`), Ctrl-O, menus, help, welcome and docs. |
| ✅ | Compact sidebar on narrow terminals | Below 100 columns, or `\` any time: a strip of status dots and workspace initials; click to select. |
| ✅ | First-run onboarding | A welcome checklist on first launch: connect Claude Code (`H`), the VS Code extension (`V`), open a workspace (Ctrl-O), the keys that matter, the Terminal.app Option tip. "Getting started" in the menu brings it back. |
| ⬜ | A good ASCII design for aulora's branding | Asked 2026-10-20: study how other products built theirs (Claude Code, Gemini CLI, Crush, opencode, lazygit…), then design aulora's. Research under way. |

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
| 🟡 | Answer Claude's questions and permission prompts from aulora, for VS Code sessions too | A red "Claude is waiting on you" card replaces the prompt box with the question and its numbered choices (AskUserQuestion, plan approval, permission requests); 1-9 picks, Enter submits a multi-question form, Esc says no, pressed in the VS Code terminal by the extension (0.2.1). aulora panes already show Claude's own picker. Needs the restarted daemon and reloaded VS Code; unverified how Claude Code's multi-question form takes keys. |
| 🟡 | Send a prompt from aulora into a session running in VS Code, same session | Extension built and installed; prompt box in aulora. Needs the daemon restarted once to work live. |
| ✅ | Read code and changes inside aulora | Changes tab (diffs, attribution), Files tab (tree, viewer, gutter marks). |
| ✅ | Commit and push from aulora | `c`, `P`, `p` in Changes. |
| ✅ | Discard changes | `X` (asks; new files go to the Trash). |
| ✅ | Edit in a terminal editor | `e` opens $EDITOR in a pane at the line. |
| ✅ | Open a new workspace without VS Code | Ctrl-O (repos on disk, recent, any path), `aulora open [dir]`. |
| ✅ | Find anything | Ctrl-P: files, `#` workspaces, `@` agents, `>` actions. |
| 🟡 | Jump from aulora to a session's VS Code terminal | `o` on a VS Code session, or its menu: `code <workspace>` raises the window, the extension shows the terminal. Needs the restarted daemon and reloaded VS Code to check live. |
| 🟡 | aulora's panes inside VS Code | Extension 0.2.0: "aulora: Open a pane…" and "aulora: Open this workspace's panes" open daemon panes as VS Code terminals (screen, typing, resizing; closing leaves the pane running). Tested against a real daemon; check in your VS Code after reloading. |
| ✅ | Side-by-side diffs | `s` in Changes: old left, new right, removals beside their additions, words marked (from 100 columns). |
| 🟡 | Files and Changes in a panel | `d` / `f` open one panel down the right edge of the tab, full height, beside every pane and conversation; it never replaces them. `d` or `f` again closes it; the other key switches it. Layouts saved earlier keep their Changes leaf where it was. |
| ⬜ | Files as a side panel by default, with changes visible in it | Asked 2026-10-20 ("similar to how you have in this photo"): a Files panel on one side, open by default, showing which files changed; easy to toggle and close. |
| 🟡 | Changes showed nothing for aulora and adarshji (both git repos) | Cause: `d` with an agent selected showed only that agent's edits, so changes made by another agent or by hand were hidden ("Nothing this agent wrote is uncommitted"). Changes now always opens on every change; `a` narrows to one agent. (Both repos are clean right now, so there's nothing to show until something changes.) |

## Reliability

| | Request | Where it stands |
|---|---|---|
| ✅ | Restarts and updates must not kill panes | `aulora daemon restart`; the daemon upgrades itself in place when the program is replaced. |
| ✅ | Panes come back after a reboot | Saved and restored with the same ids; Claude resumes its session. |
| ✅ | Notifications when an agent needs you | Desktop notification from the daemon. |
| ✅ | "Where you left off" cards | Back on a workspace after 30 minutes or more: a card with what each agent finished or is asking, files edited, new commits. Any key carries on. |
| 🟡 | Global hotkey switcher | `aulora focus` brings the dashboard's Terminal.app or iTerm2 window to the front (or opens one). A terminal program can't own a system-wide key, so bind `aulora focus` to one in macOS Shortcuts or Raycast. |
| ✅ | Exact steps to move onto the new daemon | Given in chat: `aulora daemon stop --force && aulora daemon start`, reload VS Code windows; later upgrades keep panes. |

## Process

- Commits follow Zulip's discipline; every commit passes fmt, clippy and tests; push only when asked.
- Never restart the real daemon without checking its panes and asking.
- No artifacts; everything stays in the terminal or local files.
- The bar is market-tested products (Herdr, lazygit, VS Code), checked by rendering and using it, not by assuming.
