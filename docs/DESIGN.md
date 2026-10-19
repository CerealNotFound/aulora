# aulora design

aulora should feel like **home**: familiar from the first minute because it behaves like the tools people already love (Herdr, tmux, lazygit, VS Code), and trustworthy enough that they stay. Every decision below names the product it comes from. Anything without a precedent needs a stated reason.

## Principles

1. **Familiar over clever.** Keys, mouse and layout follow Herdr first, then tmux, lazygit, helix and VS Code. Nothing new to learn before it's useful.
2. **Calm.** No boxes around regions; spacing, one `│` separator and row backgrounds do the work (Herdr, helix, yazi). Color only where it means something.
3. **Mouse and keyboard are equals.** Anything you can click has a key, and anything with a key can be clicked (Herdr: "mouse-native, keyboard optional").
4. **Never lose work, never surprise.** Terminals survive closing the dashboard. Destructive actions ask, with Cancel focused (k9s). The daemon refuses to stop while panes run.
5. **Feels instant.** Scrolling and typing redraw within one frame. Nothing re-renders what didn't change. A 10 MB transcript opens at once (codex, gitui).

## Layout

```
 aulora                  23:41 │ 1 Terminal   2 Changes 7   3 Files      ● working 4m
 projects                    3 │ claude · Credit billing → FastAPI   ~/…/multibrain
                               │
 ● multibrain                + │  (the agent's terminal, live)
   feat/credit-billing ↑3 ●7   │
 ├ ● claude · billing    32m × │
 │   Edit page.tsx             │
 └ ○ dev server         1h36   │
     pnpm dev                  │
                               │
 ● adarshji                  1 │
   main ↑1 ●4                  │
 └ · Remove SEO keywords       │
     past session · 2h ago     │
```

- **Sidebar, one tree** (VS Code's explorer, Herdr's spaces). Each project shows its rolled-up state and a second line for branch, ahead/behind and changes. Under it, `├`/`└` branches list everything that runs there, most urgent first: Claude panes, shells, commands, and Claude sessions running elsewhere. Projects sort the same way, so whatever needs you is near the top. A flat priority list was tried and dropped: it hid which project an agent belongs to, and that's the first thing you need when switching.
- **The project is where new things start.** With a project or any of its agents selected, `C`, `c`, the `+` on the project row and the menu's *New Claude here* / *New shell here* start in that project's folder.
- **Every agent is a terminal.** An agent aulora runs shows its live terminal. A session that has ended (or a past one, `h`) opens as a terminal running `claude --resume <id>` in the folder it ran in, so you continue it rather than read it. A session still running somewhere else (VS Code, Ghostty) shows its conversation read-only, with `r` to fork it here (`claude --resume <id> --fork-session`): resuming it outright would put two processes on one transcript.
- **Closing** is a `×` on the hovered or selected row (VS Code's editor tabs), `x`, or the menu. Killing a running pane asks first; closing an exited pane or dismissing an ended session doesn't.
- **Main area** shows the selected agent or project: a tab bar, a title and meta line, and the tab's content. Tabs: **Terminal · Changes · Files** for agents, **Overview** for projects, **Conversation** only for sessions running outside aulora. There is no Activity tab: events appear in the conversation where they happened (Claude Code's own transcript does this).
- **Rows** are two lines: `● name · detail` then an indented second line, tokens joined by ` · `. When space runs out, whole tokens drop before text is cut with `…` (Herdr).
- **Narrow terminals** (< 90 columns): the sidebar collapses to glyphs and counts, and focus shows one region at a time (Herdr's compact sidebar, lazygit's portrait mode).

## States

Five states, as in Herdr. Color and glyph both carry the meaning, so it reads under `NO_COLOR` too.

| State | Meaning | Glyph | Color |
|---|---|---|---|
| blocked | needs you: permission, question, plan approval | `●` | red |
| working | Claude is running a turn | `●` (spinner in the main title) | yellow |
| done | finished a turn **you haven't looked at yet** | `●` | teal |
| idle | finished and seen, or waiting for a first prompt | `○` | green |
| ended / unknown | not running, or no information | `·` | overlay |

"done" clears to "idle" once you view the session (Herdr; cones' unread marker). Projects show the most urgent state of their agents.

## Focus and modes

- Two focus regions: the **sidebar** and the **main area**. The focused region keeps full color; the other dims slightly (gitui).
- **Navigate** is the resting mode: single keys drive the dashboard.
- **Terminal**: when a Terminal tab has focus, every key goes to the pane. **Ctrl-Q** returns to Navigate; pressing it twice sends Ctrl-Q to the pane (tmux's double prefix). Ctrl-Q because Claude Code doesn't use it and claude-squad uses it for the same job; Ctrl-B (tmux, and Claude's "background"), Ctrl-G (Claude's editor) and Esc are taken.
- A small mode pill shows only outside the resting state, e.g. ` TERMINAL ` with `ctrl-q navigate` (Herdr's mode bar).
- Clicking anywhere focuses what you clicked, from any mode (Herdr).

## Keys

One keymap table drives key handling, the footer hints and the `?` help, so they can never disagree (zellij, helix, lazygit).

| Key | Action | Precedent |
|---|---|---|
| `↑` `↓` / `j` `k` | move in the focused list, or scroll by line | everywhere |
| `⇥` / `⇧⇥` | next / previous region | lazygit panels |
| `1`–`4` | switch tabs | Herdr `prefix+1..9`, lazygit panel jumps |
| `[` `]` | previous / next tab | vim-unimpaired, Herdr `n`/`p` |
| `⏎` | open: type into a pane, focus a view, expand an item | |
| `Ctrl-Q` | leave a terminal | claude-squad |
| `Ctrl-P` / `:` | fuzzy jump to any project, agent, file or action | VS Code, k9s, posting |
| `/` | search the focused view | less, vim, lazygit |
| `n` | next thing that needs you (outside search) | Herdr `prefix+o` |
| `c` / `C` | new shell / new Claude pane in this project | tmux `c` |
| `z` | zoom the main area to the whole screen | tmux, Herdr `prefix+z` |
| `e` | edit the selected file in a terminal editor pane | lazygit `e` |
| `o` | open in VS Code at the line | lazygit `o` |
| `x` | close / kill (asks first when it would interrupt work) | k9s |
| `r` | restart an exited pane; resume a session in a new terminal | k9s `r` |
| `m` | the selection's menu (same as right-click) | lazygit `?`-menus, VS Code context menu |
| `t` | themes | Herdr settings |
| `?` | help, searchable, runs the binding on Enter | lazygit |
| `q` | quit; panes keep running | |

The footer shows 5–7 hints for the focused region and `? more`, shortening labels before dropping hints as width shrinks (zellij, gitui).

## Mouse

- Mouse capture is on (Herdr, zellij). Click focuses and selects; clicking a selected row (or double-clicking) opens it; clicking a tab switches to it.
- Hovering a row shows its button: `×` on agents, `+` on projects (VS Code's tabs and explorer). Only the hovered or selected row shows one, so the list stays calm.
- Right-click opens a menu for what's under the pointer: Type here, Resume in a terminal, Restart, Close, New Claude here, New shell here, Open in VS Code (VS Code's context menu). In a program that reads the mouse, right-click goes to the program instead.
- **The wheel scrolls whatever is under the pointer**, not whatever has focus (VS Code, Herdr).
- In a terminal pane the wheel goes to the program if it asked for mouse events, becomes arrow keys if it runs full-screen, and otherwise scrolls aulora's copy of the scrollback (xterm.js, zellij, Herdr).
- 3 lines per wheel notch. Fast trackpad streams scroll about 1 line per event and speed up gently; fractions carry over and reset on direction change (helix, codex).
- Typing in a pane snaps it back to the bottom (Herdr).
- A thin scrollbar (`▕` track, `▐` thumb) shows position in long views (Herdr).
- `F12` toggles mouse capture so the terminal's own text selection works; Shift-drag does the same in most terminals.

## Smoothness

- Scroll by lines. A view's position is a content anchor, `Latest` or `(entry, row)`, so new output never moves what you're reading (codex). Scrolling to the bottom follows again.
- Wrap each entry once per width and keep row prefix sums; draw only visible rows with `Buffer::set_line` (codex, gitui). Never clone the whole view per frame.
- Draw only when something changed, at most about 120 times a second, and immediately on the first input after idle (codex's frame requester).
- Each frame is one synchronized update (DEC 2026), so nothing tears (codex).
- A panic restores the terminal fully: mouse, paste and keyboard modes included.

## Conversation

Claude Code's own transcript, readable and scrollable. It shows for sessions running outside aulora; everything else is a terminal.

- Your prompts, Claude's replies with light markdown, and every tool call in place, with its result: commands with exit status, duration and the first lines of output; edits with `+n −m` and a short colored diff; reads and searches as one-line summaries. `⏎` expands any of them.
- Permission requests, questions and your answers, subagents, compaction, interruptions and session start and end appear inline as quiet notes.
- At the bottom it follows new messages; scrolled up it stays put and shows `↓ 3 new · G latest`.

## Changes

VS Code's Source Control panel, plus who did it.

- A tree of changed files built from `git status`, in VS Code's groups: Merge conflicts, Staged, Changes. Folders with a single folder inside are compressed (`app/billing`), sorted folders first, and colored by the most important change inside (VS Code, lazygit). File names take their status color, with VS Code's letters (M, A, D, R, U, !).
- Each file shows a dot per Claude session that wrote it (a steady color per session), then `+n −m`.
- Selecting a file shows its diff beside the tree: old and new line numbers (GitHub), syntax highlighting, changed words marked within lines (delta). Colors come from aulora's theme, not a separate syntax theme, so every theme gets a matching diff. When more than one session wrote a file, each hunk is labeled with the one that wrote it. `[` `]` jump between hunks, `o` opens the editor at the change, `space` stages or unstages a file or folder (VS Code's + and −).
- With an agent selected, the tab shows that agent's changes; `a` switches to every change in the project. The tab you chose stays as you move between agents, so you can walk the sidebar reviewing each one.
- Attribution reads the session transcripts Claude Code already keeps: every Edit and Write records its exact patch (`structuredPatch`) or the new file's content. A change belongs to the session whose recorded lines it contains. Transcripts are read incrementally, so this costs nothing after the first look, and it covers sessions from before aulora was installed.
- git runs on a background thread: one `git status` and two `git diff -U0` calls give the whole tree, counts and attribution; the selected file's diff is read with context when you select it. The tab refreshes when the daemon's git poll sees something new, and every few seconds while it's shown.
- Not yet: side-by-side diffs, discarding changes, committing.

## Files

- A gitignore-aware project tree, loaded a folder at a time, with git and session decorations (VS Code explorer, yazi).
- A file viewer with syntax highlighting, line numbers and change markers in the gutter (VS Code's quick diff).
- `Ctrl-P` finds files by fuzzy name (nucleo, as helix). `/` in the tree searches contents (ripgrep's engine).
- `e` opens the file at the current line in the user's terminal editor (`$VISUAL` / `$EDITOR`: nvim, helix, micro, nano) in a pane; aulora does not include its own editor.

## Visual system

Theme tokens follow Herdr's roles: `panel_bg`, `sidebar_bg`, `active_row_bg`, `selection_bg`, `surface0`, `surface1`, `surface_dim`, `overlay0`, `overlay1`, `text`, `subtext0`, `accent`, and the named colors. The default is Catppuccin Mocha on the terminal's own background; Catppuccin Latte on light terminals.

- Focused row: `active_row_bg` across the full width. Keyboard cursor in a list: `selection_bg`.
- Section headers: `overlay0`, bold, lower case with a leading space (Herdr).
- Separators: `surface_dim`.
- Modals: dim everything behind, accent border on `panel_bg`, buttons ` ↵ confirm ` and ` esc cancel `, red border for destructive ones (Herdr).
- Toasts: bottom-right, info for 2 s, errors until the next key (lazygit, cones).

## First run

One card explaining the three things that matter: the sidebar, `⏎` to type into a pane and `Ctrl-Q` to leave, and `?` for everything else (Herdr, lazygit). Contextual hints show once each, such as how to leave a terminal the first time you enter one (claude-squad).
