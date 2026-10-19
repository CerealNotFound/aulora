# Contributing to aulora

Thanks for helping. This page covers setup, testing and how commits are written. Commits follow [Zulip's commit discipline](https://zulip.readthedocs.io/en/latest/contributing/commit-discipline.html), and CI checks every commit message.

## Setup

```sh
git clone https://github.com/CerealNotFound/aulora && cd aulora
git config core.hooksPath .githooks     # check commit messages locally
git config commit.template .gitmessage  # start each message from the template
cargo test
```

Useful while developing:

```sh
cargo run -- demo                               # dashboard with sample sessions
cargo run -- tui --demo --snapshot 180x50       # print one frame as text
AULORA_HOME=/tmp/aulora-dev cargo run -- daemon # isolated daemon
aulora hooks install --settings /tmp/s.json     # hooks in a throwaway settings file
claude --settings /tmp/s.json                   # a Claude Code session that uses them
```

Before pushing, run what CI runs:

```sh
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test --all
tools/lint-commits origin/main..HEAD
```

## Commits

### What goes in a commit

- **One minimal, coherent idea per commit.** Two features, or two refactors, go in two commits.
- **Every commit builds and passes tests**, so `git bisect` always works. If a commit breaks something, amend it. Don't add a "fix tests" commit on top.
- **Tests go in the same commit as the change that needs them.** So does error handling for new code.
- **Refactors, renames and moves get their own preparatory commits**, before the change that relies on them. Moving code between files is never mixed with changing it.
- A brand-new feature doesn't need one commit per sub-feature. Err toward smaller commits, since they're easy to squash later.

### Summary line

```
area: Imperative sentence that ends with a period.
```

- **area** is one or two lower-case words naming the part of the code:

  | Area | Covers |
  |---|---|
  | `core` | `crates/aulora-core`: model, protocol, reducer, transcript reader |
  | `daemon` | `crates/aulora-daemon` |
  | `tui` | `crates/aulora-tui` |
  | `cli` | `crates/aulora`: commands, hook entry point, hooks install |
  | `hooks` | how aulora talks to Claude Code hooks specifically |
  | `docs` | README, CONTRIBUTING, docs/ |
  | `ci` | `.github/`, `tools/` |
  | `build` | Cargo manifests, dependencies, release profile |

  A narrower name like `transcript` or `install` is fine when it's clearer. Never use a generic word such as `fix`, `bug`, `refactor`, `misc` or `update`. If a commit doesn't fit one area, it probably should be split.
- **The sentence** starts with a capital letter and an imperative verb ("Add", "Fix", "Rename", not "Added" or "Adds"), avoids abbreviations, and ends with a period.
- **72 characters at most**, area included.

Good:

```
daemon: Mark sessions ended when their Claude process exits.
tui: Keep the selected row visible after the list reorders.
transcript: Count output tokens once per message id.
```

Bad:

```
fixed liveness bug                       no area, not imperative
daemon: fixing stale sessions            not capitalized, not imperative
fix: Handle PID reuse.                   generic area
```

### Body

- Leave a blank line after the summary.
- Explain **why** the change is needed and why it's safe, in full sentences. Reviewers should be able to check your reasoning. Don't list the files or functions you changed; the diff shows that.
- Wrap at about 68 characters, and never past 70. Long links are fine.
- End with `Fixes #123.` when it closes an issue, or `Fixes part of #123.` when it doesn't fully close one.
- Credit co-authors with a `Co-authored-by:` trailer after a blank line. Don't @-mention people in commit messages.

`tools/lint-commits` checks the mechanical rules. The `.githooks/commit-msg` hook runs it locally, and CI runs it on every push and pull request.

## Pull requests

Keep pull requests focused. Rebase onto `main` instead of merging it in, and clean up history with `git rebase -i` before asking for review, so the commits that land are the ones described above.
