# Backlog

Every request made while building aulora, with where it stands. Nothing
leaves this list until it's done and checked in real use, or dropped on
purpose (with the reason). Newest requests are added at the bottom of
their section.

Status: ✅ done · 🟡 done but not yet checked in real use · ⬜ open · — dropped

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
| 🟡 | Cmd-V and Ctrl-V do exactly the same thing | Asked 2026-10-20 ("cmd+v and ctrl+v should be identical!!!"). Prompt box: both paste copied text, and both attach a copied image as `[Image #n]`. Claude panes: Ctrl-V with text copied pastes the text (Claude's own Ctrl-V only takes images); with an image copied it goes to Claude, which reads the image; Cmd-V with only an image copied does the same. Shells and editors keep Ctrl-V's own meaning. Checked in a pty with a fake clipboard, all four cases. |
| ✅ | Multi-line prompts must not break the screen | Line breaks and tabs are drawn as spaces everywhere; CRLF files too. |
| ✅ | After sending a prompt, the cursor stays in the input | The prompt box stays open and empty, ready for the next one. |
| 🟡 | Cmd-A selects everything in the input, then Delete clears it | In the prompt box: Cmd-A where the terminal sends it (Ghostty, kitty, WezTerm, iTerm2); Terminal.app keeps Cmd-A for its own Select All and never sends it, so there Ctrl-A twice does the same. The text highlights; Delete clears it, typing or pasting replaces it, an arrow lets it go. |
| 🟡 | Selecting in the input copies too | A mouse selection in the prompt box copies like anywhere else, and select-all copies the whole prompt. |
| 🟡 | Shift-Enter for a new line | Works in panes and the prompt box where the terminal sends Shift-Enter distinctly. Terminal.app sends it as plain Enter, so there: Option-Enter (Option as Meta, or Claude's /terminal-setup), Ctrl-J, or `\` then Enter, all of which work in panes and the prompt box. `aulora keys` says which case you're in. |
| ⬜ | Hold Space for voice from aulora | Reported 2026-10-20 that it isn't there (from aulora's prompt box for a VS Code session; Claude panes pass held Space through, microphone access under the daemon unchecked). |
| ⬜ | Change model and effort from aulora | Reported 2026-10-20 that it isn't there. |
| ⬜ | Claude Code's queue keys work from aulora: Ctrl-Enter sends a queued message at once, Up edits it, "stuff like that" | Asked 2026-10-20. Claude Code shows "ctrl+enter to send now" under a message queued while it works, and Up brings a queued message back to edit. Both must work from aulora: in a Claude pane, and in the prompt box for a VS Code session (where the queued message should show, too). Found: "send now" is Claude Code's `chat:sendNow`, on Ctrl-Enter and also Ctrl-X Ctrl-S; Terminal.app sends Ctrl-Enter as plain Enter, so there Ctrl-X Ctrl-S is the one that works (in a pane today). Wanted: "exactly as you see in VS Code." |

## Mouse and links

| | Request | Where it stands |
|---|---|---|
| ✅ | Scrolling and clicks that do something | Wheel under the pointer, click to focus or type, drag pane borders. |
| 🟡 | Close panes from their frame | Reported 2026-10-20 ("I am not able to close these files", two vim panes). Every pane's frame has a × now; it asks once, with Close highlighted, so ↵ closes, and stops and removes the pane in one step (it used to take two: stop, then close). The dialog's labels were misleading (↵ was shown on Close while ↵ cancelled); ↵ now marks whichever choice is highlighted. Checked in a pty: ×, ↵, the pane is gone. |
| 🟡 | Double-clicking a file opens it in VS Code | Asked 2026-10-20 ("double clicking should open the file straight up in vscode too"). Double-click in the Files panel now opens the file in your editor, as `o` does; `e` still opens it in your terminal editor if you want that. |
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
| 🟡 | A terminal follows its directory into the right workspace | Asked 2026-10-20 (screenshot: a shell opened in thnkrr-platform, `cd ..`, `cd projects`, still listed under thnkrr-platform). The daemon reads each pane's working directory every second; when it enters a git repository, the pane moves to that repository's workspace (opening it if needed), and when it enters a plain folder that is an open workspace (like projects), it moves there. Passing through a folder that is neither (`cd ..` to ~/Documents) leaves it where it is. If it's the pane you're on, the view and your typing go with it and a message says where. Checked against a daemon in a pty with the screenshot's steps. |
| ⬜ | Move a terminal to another workspace by hand | Asked 2026-10-20 ("move shells around"). A pane's menu (and a key) moves it to any workspace you pick. |
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
| 🟡 | A good ASCII design for aulora's branding | Studied Claude Code, Gemini CLI, Crush, opencode, lazygit, Herdr, Codex and others (most keep the working UI plain and put the big art on a welcome screen; several ship sizes by width; colour comes from the theme). Built: a lowercase half-block wordmark in a green→cyan→blue→magenta gradient from the theme on the welcome and empty screens, drawn so Terminal.app's row gaps don't show; a line-drawn version for narrow screens; the `▛▜` mark (an arch) with a gradient `aulora` in the sidebar header. Runners-up and research notes are in the scratchpad branding folder. A README image is still to do. |

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
| 🟡 | Answer Claude's questions and permission prompts from aulora, for VS Code sessions too | Reported 2026-10-20: "answering did not work. and it should show exactly as it does in cursor". Rebuilt as a copy of Claude Code's own picker, checked against a real Claude Code (2.1.304): the question with its header chip, `❯ 1. …` choices with descriptions under them, "Type something." and "Chat about this", and "Enter to select · ↑/↓ to navigate · Esc to cancel". Several questions get Claude's tab row (`←  ☐ Colour  ☐ Size  ✔ Submit  →`) and its review with Submit answers and Cancel. Numbers answer at once, ↑↓ and Enter choose, a click on a choice answers, typing your own answer types it into Claude's field; numbers also answer with the sidebar focused (before, only with the conversation focused, and clicks did nothing). Every key is pressed in the session's terminal by the extension, as tested against Claude Code. Multi-select questions are untested. |
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
| 🟡 | Files as a side panel by default, with changes visible in it | Every workspace gets a Files panel down the right (screens 110+ columns), without taking focus. Its top lists what changed (M/U, who wrote it); a click opens the file at its first change; the tree below marks changed files and folders. `f` hides or shows it everywhere (remembered), `d` switches it to Changes and back, the × in its title closes it. A new workspace starts empty instead of as a full-width Changes view. (The photo you mentioned didn't come through; this follows VS Code's explorer.) Checked in a pty: default, ×, `f`. |
| 🟡 | Changes showed nothing for aulora and adarshji (both git repos) | Cause: `d` with an agent selected showed only that agent's edits, so changes made by another agent or by hand were hidden ("Nothing this agent wrote is uncommitted"). Changes now always opens on every change; `a` narrows to one agent. (Both repos are clean right now, so there's nothing to show until something changes.) |
| 🟡 | The file tree shows git changes the way VS Code's Explorer does | Asked 2026-10-20 (screenshot of VS Code: changed files in amber with an `M`, folders holding changes with a dot, ignored folders dimmed). Cause of seeing nothing: aulora only looked for changes when the workspace itself was a git repository, and your "projects" folder isn't one; the repositories are inside it. Now a plain folder shows the changes of every repository up to two levels down, under its folder, in the panel's CHANGES list and the tree, with change marks in the file view; ignored files and folders (target, node_modules) are listed faded instead of hidden; .git and .DS_Store are never listed. Checked on your real projects workspace in a pty: CHANGES lists aulora's files. |
| 🟡 | Viewing files and their changes has to be friendly and obvious, or it's pointless | Said 2026-10-20: "unless the files viewing and updates are user friendly and intuitive, there's not much sense of having that." Done so far: the two rows above, and a file opened in your editor from the panel no longer lands in a sliver. Reading a file stays: click it, then `o` for VS Code. |
| — | Scroll a file sideways | Asked 2026-10-20, then dropped: "the fact that you can click on the file in files panel and then click o to open it in vscode is enough." Kept from it: a file opened in your editor from the panel (double-click, `e`) now opens beside your work instead of squeezed into the panel's strip, which is what made build-board.html unreadable. |
| 🟡 | Prompts with images sit queued ("ctrl+enter to send now") and never send | Reported 2026-10-20 (screenshot; again: "this message did not go through for some reason"). Reproduced against a real Claude Code (2.1.304): while Claude works, a queued message that *starts* with an image path is held until the whole turn ends; with the path anywhere later it arrives at the next step and Claude still reads the image (tested four ways). aulora put a pasted image's path first. Now a pasted image shows in the prompt box as `[Image #1]`, as in Claude Code, and the file paths are added at the end when it's sent. Checked: a busy Claude got such a prompt mid-turn and described the image. |
| 🟡 | Every changed file shows what kind of change and how big, in the Files tab | Asked 2026-10-20: "you should be able to see diffs for each file (modified, added, deleted, +xyz -xyz in the files tab) … that's imp." Each changed file now shows `+added −removed` (green, red; `bin` for binaries) and its letter (M modified, A added, D deleted, U new, R renamed) in the CHANGES list and on its row in the tree; staged and unstaged edits are added together. |
| 🟡 | No VS Code at all: one window where you open workspaces and move shells around | Asked 2026-10-20. Already possible: Ctrl-O or `aulora open` opens a workspace, `c` / `C` start a shell or Claude in it, panes live in the daemon and survive restarts. VS Code is only needed to reach sessions you started inside VS Code. Missing pieces: terminals following `cd` into their workspace, and moving a terminal by hand (both under Layout). |

## Reliability

| | Request | Where it stands |
|---|---|---|
| 🟡 | Resume keeps the session's permission mode | Reported 2026-10-20: resuming here "did not resume it as dangerously-skip-permissions". aulora resumed with `claude --resume <id>` and nothing else. Now a session that ever ran with bypassPermissions (from its hooks, or its transcript, where Claude records the mode on every prompt) resumes with `--dangerously-skip-permissions`, even if Shift-Tab left it in another mode; one left in acceptEdits or plan resumes in that mode. Forks too, and panes restored after a restart keep the flag. |
| 🟡 | Claude in an aulora pane is as quick as in VS Code's terminal | Reported 2026-10-20: after resuming in aulora "it became a bit more laggy and slow". Cause: the dashboard's loop woke for your keys but not for pane output, which waited for its timer: up to 100 ms while an agent works, 250 ms idle, on every echo and every streamed line. Terminal events now come in on the same channel as pane output, so either wakes it. Measured in a pty, key press to echo on screen: 109 ms before, 17 ms after (median of 20). |
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
