//! Shell integration for panes. zsh, bash and fish start with aulora's
//! own startup files, which load yours and then say when each command
//! starts and how it ended (OSC 133, as VS Code and iTerm2 use), and how
//! many jobs run in the background or are paused. That's how aulora
//! knows a dev server exited while its shell stays open.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use aulora_core::model::{Ms, ShellCommand};

const FILES: &[(&str, &str)] = &[
    ("zsh/.zshenv", include_str!("shell/zsh/zshenv")),
    ("zsh/.zprofile", include_str!("shell/zsh/zprofile")),
    ("zsh/.zshrc", include_str!("shell/zsh/zshrc")),
    ("bash/bashrc", include_str!("shell/bash/bashrc")),
    ("fish/aulora.fish", include_str!("shell/fish/aulora.fish")),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Zsh,
    Bash,
    Fish,
}

impl Kind {
    pub fn of(shell: &str) -> Option<Kind> {
        match Path::new(shell).file_name()?.to_str()? {
            "zsh" => Some(Kind::Zsh),
            "bash" => Some(Kind::Bash),
            "fish" => Some(Kind::Fish),
            _ => None,
        }
    }
}

/// aulora's startup files for a shell.
#[derive(Debug, Clone, Copy)]
pub struct Integration {
    pub kind: Kind,
    root: &'static Path,
}

impl Integration {
    /// For this shell, when it's one aulora knows and the files could be
    /// written; otherwise the shell runs as it is.
    pub fn of(shell: &str) -> Option<Integration> {
        Some(Integration { kind: Kind::of(shell)?, root: root()? })
    }

    /// ZDOTDIR for zsh.
    pub fn zdotdir(&self) -> PathBuf {
        self.root.join("zsh")
    }

    /// The file bash reads with --init-file.
    pub fn bashrc(&self) -> PathBuf {
        self.root.join("bash").join("bashrc")
    }

    /// The file fish sources at start.
    pub fn fish_script(&self) -> PathBuf {
        self.root.join("fish").join("aulora.fish")
    }
}

/// The folder with aulora's startup files, written once per daemon.
fn root() -> Option<&'static Path> {
    static ROOT: OnceLock<Option<PathBuf>> = OnceLock::new();
    ROOT.get_or_init(|| {
        let root = aulora_core::paths::home().join("shell");
        match write(&root) {
            Ok(()) => Some(root),
            Err(e) => {
                tracing::warn!("couldn't write the shell integration to {}: {e}", root.display());
                None
            }
        }
    })
    .as_deref()
}

fn write(root: &Path) -> std::io::Result<()> {
    for (name, text) in FILES {
        let path = root.join(name);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
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
    /// A prompt is drawn.
    Prompt,
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
            [b"133", b"A", ..] => Some(Mark::Prompt),
            [b"133", b"C", ..] => Some(Mark::Started),
            [b"133", b"D", code, ..] => num(code).map(Mark::Ended),
            [b"aulora", b"jobs", running, paused] => Some(Mark::Jobs { running: num(running)?, paused: num(paused)? }),
            [b"aulora", b"job-ended", code] => num(code).map(Mark::JobEnded),
            _ => None,
        }
    }
}

/// How long a command must have run for its end to be news: failing
/// after 10 s, or ending without a word after a minute (a dev server,
/// not a build).
const FAILED_RUN: Ms = 10_000;
const CLEAN_RUN: Ms = 60_000;

/// Ctrl-C, or a pause (Ctrl-Z and the like): you did it, or it isn't over.
fn quiet_end(code: u32) -> bool {
    [libc::SIGINT, libc::SIGTSTP, libc::SIGSTOP, libc::SIGTTIN, libc::SIGTTOU].iter().any(|s| code == 128 + *s as u32)
}

/// What a pane's shell has said so far, put together.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ShellState {
    heard: bool,
    /// Since when a command runs in front.
    running: Option<Ms>,
    /// How the last command or job ended.
    ended: Option<(Option<u32>, Ms)>,
    /// A job's end was reported since the jobs were last counted.
    job_ended: bool,
    /// When a job last ended.
    job_ended_at: Option<Ms>,
    jobs: u32,
    jobs_since: Ms,
    paused: u32,
    paused_since: Ms,
}

impl ShellState {
    /// Take in a mark. Returns an ending worth a notification (its code,
    /// None when unknown): a command that failed after running a while,
    /// or ended quietly after a long run, like a dev server; any job that
    /// ended. Not Ctrl-C, which is you, nor a pause.
    pub fn note(&mut self, mark: Mark, now: Ms) -> Option<Option<u32>> {
        self.heard = true;
        let news = |code: u32| (!quiet_end(code)).then_some(Some(code));
        match mark {
            Mark::Started => self.running = Some(now),
            Mark::Ended(code) => {
                // A job that died while this ran (say, the `kill` that
                // killed it) says more than how this went.
                let since = self.running.take();
                if !since.is_some_and(|since| self.job_ended_at.is_some_and(|at| at >= since)) {
                    self.ended = Some((Some(code), now));
                    let ran = since.map(|since| now - since).unwrap_or(0);
                    if ran >= if code == 0 { CLEAN_RUN } else { FAILED_RUN } {
                        return news(code);
                    }
                }
            }
            Mark::Jobs { running, paused } => {
                // A job went without the shell saying how (bash 5 forgets a
                // paused job's status once it's reported killed).
                let vanished = running + paused < self.jobs + self.paused && !self.job_ended;
                if vanished {
                    self.ended = Some((None, now));
                }
                self.job_ended = false;
                if running > 0 && self.jobs == 0 {
                    self.jobs_since = now;
                }
                if paused > 0 && self.paused == 0 {
                    self.paused_since = now;
                }
                (self.jobs, self.paused) = (running, paused);
                if vanished {
                    return Some(None);
                }
            }
            Mark::JobEnded(code) => {
                self.ended = Some((Some(code), now));
                self.job_ended = true;
                self.job_ended_at = Some(now);
                // One fewer, until the shell next counts.
                if self.jobs > 0 {
                    self.jobs -= 1;
                } else {
                    self.paused = self.paused.saturating_sub(1);
                }
                return news(code);
            }
            Mark::Prompt => {}
        }
        None
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
        assert_eq!(Mark::parse(&[b"133", b"A", b"click_events=1"]), Some(Mark::Prompt));
        assert_eq!(Mark::parse(&[b"133", b"C"]), Some(Mark::Started));
        assert_eq!(Mark::parse(&[b"133", b"D", b"3"]), Some(Mark::Ended(3)));
        assert_eq!(Mark::parse(&[b"133", b"D"]), None, "no code, nothing to say");
        assert_eq!(Mark::parse(&[b"aulora", b"jobs", b"1", b"0"]), Some(Mark::Jobs { running: 1, paused: 0 }));
        assert_eq!(Mark::parse(&[b"aulora", b"job-ended", b"137"]), Some(Mark::JobEnded(137)));
        assert_eq!(Mark::parse(&[b"2", b"title"]), None);
        assert_eq!(Kind::of("/bin/zsh"), Some(Kind::Zsh));
        assert_eq!(Kind::of("/opt/homebrew/bin/bash"), Some(Kind::Bash));
        assert_eq!(Kind::of("fish"), Some(Kind::Fish));
        assert_eq!(Kind::of("/bin/sh"), None);
    }

    #[test]
    fn says_which_endings_are_news() {
        let mut s = ShellState::default();
        let run = |s: &mut ShellState, ran: Ms, code: u32| {
            s.note(Mark::Started, 1_000);
            s.note(Mark::Ended(code), 1_000 + ran)
        };
        assert_eq!(run(&mut s, 2_000, 1), None, "a quick failure, in front of you");
        assert_eq!(run(&mut s, 15_000, 1), Some(Some(1)), "a server that died");
        assert_eq!(run(&mut s, 15_000, 0), None, "a build that finished");
        assert_eq!(run(&mut s, 120_000, 0), Some(Some(0)), "a server that stopped without a word");
        assert_eq!(run(&mut s, 120_000, 130), None, "Ctrl-C is you");
        assert_eq!(run(&mut s, 120_000, 128 + libc::SIGTSTP as u32), None, "paused, not over");
        s.note(Mark::Jobs { running: 1, paused: 0 }, 2_000);
        assert_eq!(s.note(Mark::JobEnded(4), 3_000), Some(Some(4)), "any job that ends");
        s.note(Mark::Jobs { running: 0, paused: 1 }, 4_000);
        assert_eq!(s.note(Mark::Jobs { running: 0, paused: 0 }, 5_000), Some(None), "gone without a word");
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
        assert_eq!(s.command(), Some(ShellCommand::Ended { code: Some(137), at: 6 }));

        // Killed by a command: the job's end is what counts.
        s.note(jobs(1, 0), 7);
        s.note(Mark::Started, 7);
        s.note(Mark::JobEnded(137), 7);
        s.note(Mark::Ended(0), 7);
        s.note(jobs(0, 0), 7);
        assert_eq!(s.command(), Some(ShellCommand::Ended { code: Some(137), at: 7 }));

        // Paused, then gone without a word: it ended, how unknown.
        s.note(jobs(0, 1), 7);
        s.note(Mark::Started, 8);
        s.note(Mark::Ended(0), 8);
        s.note(jobs(0, 0), 8);
        assert_eq!(s.command(), Some(ShellCommand::Ended { code: None, at: 8 }));

        for c in [s.command(), Some(ShellCommand::Paused { since: 9 }), None] {
            assert_eq!(ShellState::resume(c.clone()).command(), c);
        }
    }
}
