# aulora for VS Code

Lets the [aulora](https://github.com/CerealNotFound/aulora) dashboard send prompts to the Claude Code sessions running in this window's terminals. A prompt sent from aulora is typed into the terminal, so it shows up exactly as if you had typed it, in the same session.

Install it with `aulora vscode install` (it also works in Cursor: `aulora vscode install --editor cursor`), then reload the window.

The extension connects to the aulora daemon's socket (`~/.aulora/aulora.sock`), tells it which terminals the window has, and types what aulora sends into the matching terminal. It doesn't start the daemon, makes no network requests, and does nothing while aulora isn't running. Logs are in the "aulora" output channel.
