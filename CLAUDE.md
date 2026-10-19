# aulora

Rust workspace: `crates/aulora-core` (model, protocol, reducer, transcript reader), `crates/aulora-daemon`, `crates/aulora-tui`, `crates/aulora` (the binary). Design notes are in `docs/ARCHITECTURE.md`.

## Checks

Run all of these before committing:

```sh
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test --all
```

`cargo run -- tui --demo --snapshot 180x50` prints a dashboard frame as text, so you can check layout changes without a terminal.

Never edit `~/.claude/settings.json` to test hooks. Use `aulora hooks install --settings <tmp file>` with an isolated `AULORA_HOME` instead.

## Commits

Follow `CONTRIBUTING.md` exactly. It's Zulip's commit discipline:

- one coherent idea per commit, and every commit passes the checks above
- summary `area: Imperative sentence.`, at most 72 characters, area from the list in CONTRIBUTING.md
- body explains why, wrapped at 70 characters

Check with `tools/lint-commits origin/main..HEAD` before pushing.
