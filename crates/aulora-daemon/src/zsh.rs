//! zsh integration for panes: zsh starts with ZDOTDIR pointing at
//! aulora's own startup files, which load yours and then add two hooks
//! that say when each command starts and how it ended (OSC 133, as VS
//! Code and iTerm2 use). That's how aulora knows a dev server exited
//! while its shell stays open.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const FILES: &[(&str, &str)] = &[
    (".zshenv", include_str!("zsh/zshenv")),
    (".zprofile", include_str!("zsh/zprofile")),
    (".zshrc", include_str!("zsh/zshrc")),
];

pub fn is_zsh(shell: &str) -> bool {
    Path::new(shell).file_name().is_some_and(|n| n == "zsh")
}

/// The folder with aulora's zsh startup files, written once per daemon.
/// None when it can't be written; panes then run zsh as it is.
pub fn dir() -> Option<&'static Path> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = aulora_core::paths::home().join("shell").join("zsh");
        match write(&dir) {
            Ok(()) => Some(dir),
            Err(e) => {
                tracing::warn!("couldn't write zsh integration to {}: {e}", dir.display());
                None
            }
        }
    })
    .as_deref()
}

fn write(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    for (name, text) in FILES {
        let path = dir.join(name);
        if std::fs::read_to_string(&path).ok().as_deref() != Some(text) {
            std::fs::write(&path, text)?;
        }
    }
    Ok(())
}

/// A shell's report about a command, from an OSC 133 sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    /// A command started.
    Started,
    /// The command ended with this code.
    Ended(u32),
}

impl Mark {
    /// The mark in an OSC's parameters, split at `;` as vt100 does.
    pub fn parse(params: &[&[u8]]) -> Option<Mark> {
        match params {
            [b"133", b"C", ..] => Some(Mark::Started),
            [b"133", b"D", code, ..] => std::str::from_utf8(code).ok()?.trim().parse().ok().map(Mark::Ended),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_marks() {
        assert_eq!(Mark::parse(&[b"133", b"A"]), None, "prompts don't matter here");
        assert_eq!(Mark::parse(&[b"133", b"C"]), Some(Mark::Started));
        assert_eq!(Mark::parse(&[b"133", b"D", b"3"]), Some(Mark::Ended(3)));
        assert_eq!(Mark::parse(&[b"133", b"D"]), None, "no code, nothing to say");
        assert_eq!(Mark::parse(&[b"2", b"title"]), None);
        assert!(is_zsh("/bin/zsh") && is_zsh("zsh") && !is_zsh("/bin/bash"));
    }
}
