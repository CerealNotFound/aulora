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
 aulora   multibrain › claude                       ● 1 blocked  ● 2 working   23:41
 PROJECTS                      │ claude · Credit billing → FastAPI       ● working 4m
 ● adarshji                    │ multibrain  ~/…/multibrain  opus 5.5  ctx 142k
   main ↑1                     │ Terminal   Conversation   Changes 7   Files
 ● multibrain                  │ ─────────────────────────────────────────────────
   feat/credit-billing ↑3      │
 ──────────────────────────────│   (one view at a time)
 AGENTS               priority │
 ● claude · billing  multibrain│
   Edit page.tsx 9s            │
 ● README + demo GIF    aulora │
 ○ dev server       multibrain │
```

- **Sidebar, two sections** (Herdr: spaces above agents). `PROJECTS` lists projects with their rolled-up state and a second line for branch and ahead/behind. `AGENTS` lists everything that runs: Claude sessions, Claude panes, shells and commands. Agents sort by priority (blocked, done, working, idle) or group by project, toggled from the header (Herdr's `priority` / `grouped`).
- **Selecting a project** filters the agents list to it and shows the project in the main area. Selecting it again clears the filter.
- **Main area** shows the selected agent or project: a title line, a meta line, a tab bar, and the tab's content. Tabs: **Terminal · Conversation · Changes · Files**. There is no Activity tab: events appear in the conversation where they happened (Claude Code's own transcript does this).
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
| `1`–`4` | Terminal, Conversation, Changes, Files | Herdr `prefix+1..9`, lazygit panel jumps |
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
| `t` | themes | Herdr settings |
| `?` | help, searchable, runs the binding on Enter | lazygit |
| `q` | quit; panes keep running | |

The footer shows 5–7 hints for the focused region and `? more`, shortening labels before dropping hints as width shrinks (zellij, gitui).

## Mouse

- Mouse capture is on (Herdr, zellij). Click focuses and selects; clicking a tab switches to it; right-click opens a context menu.
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

Claude Code's own transcript, readable and scrollable.

- Your prompts, Claude's replies with light markdown, and every tool call in place, with its result: commands with exit status, duration and the first lines of output; edits with `+n −m` and a short colored diff; reads and searches as one-line summaries. `⏎` expands any of them.
- Permission requests, questions and your answers, subagents, compaction, interruptions and session start and end appear inline as quiet notes.
- At the bottom it follows new messages; scrolled up it stays put and shows `↓ 3 new · G latest`.

## Changes

VS Code's Source Control panel, plus who did it.

- A tree of changed files built from `git status`, folders compressed when they have one child and showing the most important status of their contents (lazygit, yazi). Groups: Staged, Changes, Conflicts.
- Each file shows `+n −m` and a colored dot for the Claude session that changed it.
- Selecting a file shows its diff: unified, syntax-highlighted (syntect with Catppuccin), changed words highlighted within lines (delta), hunk headers labeled with the session that wrote them. `n`/`N` jump between hunks, `s` toggles side by side on wide screens.
- Attribution comes from the exact patch Claude Code passes to the PostToolUse hook for every edit. The daemon keeps a log of edits per session, so "what did this session change" is answerable even after later edits.

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
