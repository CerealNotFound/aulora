use std::collections::HashMap;

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

/// Tracks whether the Claude Code processes behind sessions are still alive.
/// Start times guard against PID reuse.
pub struct Liveness {
    sys: System,
    first_seen_start: HashMap<u32, u64>,
}

impl Default for Liveness {
    fn default() -> Self {
        Liveness { sys: System::new(), first_seen_start: HashMap::new() }
    }
}

impl Liveness {
    /// Returns the subset of `pids` whose process is gone (or was replaced).
    pub fn dead(&mut self, pids: &[u32]) -> Vec<u32> {
        if pids.is_empty() {
            return Vec::new();
        }
        let list: Vec<Pid> = pids.iter().map(|p| Pid::from_u32(*p)).collect();
        self.sys.refresh_processes_specifics(ProcessesToUpdate::Some(&list), true, ProcessRefreshKind::nothing());
        let mut dead = Vec::new();
        for pid in pids {
            match self.sys.process(Pid::from_u32(*pid)) {
                None => dead.push(*pid),
                Some(p) => {
                    let start = p.start_time();
                    let first = *self.first_seen_start.entry(*pid).or_insert(start);
                    if first != start || matches!(p.status(), sysinfo::ProcessStatus::Zombie) {
                        dead.push(*pid);
                    }
                }
            }
        }
        for pid in &dead {
            self.first_seen_start.remove(pid);
        }
        dead
    }
}

/// Reads the working directory of processes, to see a shell `cd`.
#[derive(Default)]
pub struct Cwds {
    sys: System,
}

impl Cwds {
    pub fn of(&mut self, pids: &[u32]) -> HashMap<u32, String> {
        if pids.is_empty() {
            return HashMap::new();
        }
        let list: Vec<Pid> = pids.iter().map(|p| Pid::from_u32(*p)).collect();
        let kind = ProcessRefreshKind::nothing().with_cwd(sysinfo::UpdateKind::Always);
        self.sys.refresh_processes_specifics(ProcessesToUpdate::Some(&list), true, kind);
        pids.iter()
            .filter_map(|pid| {
                let cwd = self.sys.process(Pid::from_u32(*pid))?.cwd()?;
                Some((*pid, cwd.to_string_lossy().trim_end_matches('/').to_string()))
            })
            .filter(|(_, c)| !c.is_empty())
            .collect()
    }
}

/// End a Claude Code process (SIGTERM, as closing its terminal would),
/// after checking the PID still belongs to Claude Code.
pub fn end_claude(pid: u32) -> Result<(), String> {
    let mut sys = System::new();
    let p = Pid::from_u32(pid);
    sys.refresh_processes_specifics(ProcessesToUpdate::Some(&[p]), true, ProcessRefreshKind::everything());
    let Some(proc_) = sys.process(p) else { return Err("it has already ended".into()) };
    let named = |s: &std::ffi::OsStr| s.to_string_lossy().to_lowercase().contains("claude");
    if !named(proc_.name()) && !proc_.cmd().iter().any(|a| named(a)) {
        return Err(format!("process {pid} isn't Claude Code anymore"));
    }
    // SAFETY: kill(2) with a checked, live PID and a standard signal.
    if unsafe { libc::kill(pid as i32, libc::SIGTERM) } != 0 {
        return Err(format!("couldn't end process {pid}: {}", std::io::Error::last_os_error()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_process_directory() {
        let dir = std::env::temp_dir().canonicalize().unwrap();
        let mut child = std::process::Command::new("sleep").arg("30").current_dir(&dir).spawn().unwrap();
        let cwds = Cwds::default().of(&[child.id()]);
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(cwds.get(&child.id()).map(String::as_str), Some(dir.to_string_lossy().trim_end_matches('/')));
    }

    #[test]
    fn ends_only_claude() {
        let mut other = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        assert!(end_claude(other.id()).unwrap_err().contains("isn't Claude Code"));
        other.kill().unwrap();
        other.wait().unwrap();

        let dir = std::env::temp_dir().join(format!("aulora-end-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let claude = dir.join("claude");
        // A script, so its command line says claude (macOS kills copies
        // of signed system binaries).
        std::fs::write(&claude, "#!/bin/sh\nwhile :; do sleep 1; done\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut child = std::process::Command::new(&claude).spawn().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(100));
        end_claude(child.id()).unwrap();
        let status = child.wait().unwrap();
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(status.signal(), Some(libc::SIGTERM));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
