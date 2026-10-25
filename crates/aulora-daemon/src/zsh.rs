//! zsh integration for panes: zsh starts with ZDOTDIR pointing at
//! aulora's own startup files, which load yours and then add two hooks
//! that say when each command starts and how it ended (OSC 133, as VS
//! Code and iTerm2 use). That's how aulora knows a dev server exited
//! while its shell stays open.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use aulora_core::model::{Ms, ShellCommand};

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

/// A shell's report about a command, from an OSC 133 sequence or one
/// of aulora's own (`OSC aulora;…`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    /// A command started.
    Started,
    /// The command ended with this code.
    Ended(u32),
    /// At a prompt: jobs running in the background, and paused.
    Jobs { running: u32, paused: u32 },
    /// A background or paused job ended with this code.
    JobEnded(u32),
}

impl Mark {
    /// The mark in an OSC's parameters, split at `;` as vt100 does.
    pub fn parse(params: &[&[u8]]) -> Option<Mark> {
        let num = |b: &[u8]| std::str::from_utf8(b).ok()?.trim().parse::<u32>().ok();
        match params {
            [b"133", b"C", ..] => Some(Mark::Started),
            [b"133", b"D", code, ..] => num(code).map(Mark::Ended),
            [b"aulora", b"jobs", running, paused] => Some(Mark::Jobs { running: num(running)?, paused: num(paused)? }),
            [b"aulora", b"job-ended", code] => num(code).map(Mark::JobEnded),
            _ => None,
        }
    }
}

/// What a pane's shell has said so far, put together.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ShellState {
    heard: bool,
    /// Since when a command runs in front.
    running: Option<Ms>,
    /// How the last command or job ended.
    ended: Option<(u32, Ms)>,
    jobs: u32,
    jobs_since: Ms,
    paused: u32,
    paused_since: Ms,
}

impl ShellState {
    pub fn note(&mut self, mark: Mark, now: Ms) {
        self.heard = true;
        match mark {
            Mark::Started => self.running = Some(now),
            Mark::Ended(code) => {
                self.running = None;
                self.ended = Some((code, now));
            }
            Mark::Jobs { running, paused } => {
                if running > 0 && self.jobs == 0 {
                    self.jobs_since = now;
                }
                if paused > 0 && self.paused == 0 {
                    self.paused_since = now;
                }
                (self.jobs, self.paused) = (running, paused);
            }
            Mark::JobEnded(code) => self.ended = Some((code, now)),
        }
    }

    /// The state to show: what runs in front, else a paused job, else
    /// jobs in the background, else how the last thing ended.
    pub fn command(&self) -> Option<ShellCommand> {
        if !self.heard {
            return None;
        }
        if let Some(since) = self.running {
            return Some(ShellCommand::Running { since, background: false });
        }
        if self.paused > 0 {
            return Some(ShellCommand::Paused { since: self.paused_since });
        }
        if self.jobs > 0 {
            return Some(ShellCommand::Running { since: self.jobs_since, background: true });
        }
        self.ended.map(|(code, at)| ShellCommand::Ended { code, at })
    }

    /// Pick up from what a replaced daemon showed.
    pub fn resume(command: Option<ShellCommand>) -> ShellState {
        let mut s = ShellState { heard: command.is_some(), ..ShellState::default() };
        match command {
            Some(ShellCommand::Running { since, background: false }) => s.running = Some(since),
            Some(ShellCommand::Running { since, background: true }) => (s.jobs, s.jobs_since) = (1, since),
            Some(ShellCommand::Paused { since }) => (s.paused, s.paused_since) = (1, since),
            Some(ShellCommand::Ended { code, at }) => s.ended = Some((code, at)),
            None => {}
        }
        s
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
        assert_eq!(Mark::parse(&[b"aulora", b"jobs", b"1", b"0"]), Some(Mark::Jobs { running: 1, paused: 0 }));
        assert_eq!(Mark::parse(&[b"aulora", b"job-ended", b"137"]), Some(Mark::JobEnded(137)));
        assert_eq!(Mark::parse(&[b"2", b"title"]), None);
        assert!(is_zsh("/bin/zsh") && is_zsh("zsh") && !is_zsh("/bin/bash"));
    }

    #[test]
    fn puts_the_marks_together() {
        let mut s = ShellState::default();
        assert_eq!(s.command(), None, "nothing said yet");
        let jobs = |running, paused| Mark::Jobs { running, paused };

        s.note(jobs(0, 0), 1);
        s.note(Mark::Started, 2);
        assert_eq!(s.command(), Some(ShellCommand::Running { since: 2, background: false }));

        // Ctrl-Z: zsh says 146, and that a job is paused.
        s.note(Mark::Ended(146), 3);
        s.note(jobs(0, 1), 3);
        assert_eq!(s.command(), Some(ShellCommand::Paused { since: 3 }));

        // `bg`: it runs on behind the prompt.
        s.note(Mark::Started, 4);
        s.note(Mark::Ended(0), 4);
        s.note(jobs(1, 0), 4);
        assert_eq!(s.command(), Some(ShellCommand::Running { since: 4, background: true }));

        // Something typed meanwhile doesn't hide it.
        s.note(Mark::Started, 5);
        s.note(Mark::Ended(0), 5);
        s.note(jobs(1, 0), 5);
        assert_eq!(s.command(), Some(ShellCommand::Running { since: 4, background: true }));

        // It's killed.
        s.note(Mark::JobEnded(137), 6);
        s.note(jobs(0, 0), 6);
        assert_eq!(s.command(), Some(ShellCommand::Ended { code: 137, at: 6 }));

        for c in [s.command(), Some(ShellCommand::Paused { since: 9 }), None] {
            assert_eq!(ShellState::resume(c.clone()).command(), c);
        }
    }
}
