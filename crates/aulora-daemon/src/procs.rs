use std::collections::HashMap;

use aulora_core::model::{Ms, Server};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

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

/// Finds what panes started that left their shell: servers that put
/// themselves in the background or ran under nohup. They still carry
/// the AULORA_PANE_ID a pane gives everything started in it. Only those
/// listening on a TCP port count, so agents like ssh-agent don't.
#[derive(Default)]
pub struct Escaped {
    sys: System,
    /// Ports by process (and start time), and when they were last looked up.
    ports: HashMap<(u32, u64), (Ms, Vec<u16>)>,
}

#[derive(Debug, Default)]
pub struct Scan {
    /// Servers by pane id.
    pub servers: HashMap<String, Vec<Server>>,
    /// How many child processes each shell has.
    pub children: HashMap<u32, usize>,
}

impl Escaped {
    /// `panes`: each running pane's id and its shell's pid.
    pub fn scan(&mut self, panes: &[(String, u32)], now: Ms) -> Scan {
        let mut scan = Scan::default();
        if panes.is_empty() {
            return scan;
        }
        // A process's environment doesn't change: read it once.
        let kind = ProcessRefreshKind::nothing().with_environ(UpdateKind::OnlyIfNotSet);
        self.sys.refresh_processes_specifics(ProcessesToUpdate::All, true, kind);
        let procs = self.sys.processes();
        let shells: HashMap<u32, &str> = panes.iter().map(|(id, pid)| (*pid, id.as_str())).collect();
        let ids: HashMap<&str, u32> = panes.iter().map(|(id, pid)| (id.as_str(), *pid)).collect();
        let own = std::process::id();
        let mut found = Vec::new();
        for (pid, p) in procs {
            let pid = pid.as_u32();
            if let Some(parent) = p.parent().map(|p| p.as_u32()).filter(|p| shells.contains_key(p)) {
                *scan.children.entry(parent).or_default() += 1;
            }
            if pid == own {
                continue;
            }
            let Some(pane) = p.environ().iter().find_map(|e| e.to_str()?.strip_prefix("AULORA_PANE_ID=")) else {
                continue;
            };
            let Some(&shell) = ids.get(pane) else { continue };
            if !in_tree(procs, pid, shell) {
                found.push((pid, p.start_time(), p.name().to_string_lossy().to_string(), pane.to_string()));
            }
        }
        let alive: std::collections::HashSet<(u32, u64)> = found.iter().map(|(p, t, ..)| (*p, *t)).collect();
        self.ports.retain(|k, _| alive.contains(k));
        // Look up ports for new ones, and again every 10 s for those not
        // listening yet (a server can take a while to bind).
        let due: Vec<u32> = found
            .iter()
            .filter(|(p, t, ..)| {
                self.ports.get(&(*p, *t)).is_none_or(|(at, ports)| ports.is_empty() && now - at > 10_000)
            })
            .map(|(p, ..)| *p)
            .collect();
        if !due.is_empty() {
            let listening = listening(&due);
            for (pid, start, ..) in &found {
                if due.contains(pid) {
                    self.ports.insert((*pid, *start), (now, listening.get(pid).cloned().unwrap_or_default()));
                }
            }
        }
        for (pid, start, name, pane) in found {
            let ports = self.ports.get(&(pid, start)).map(|(_, p)| p.clone()).unwrap_or_default();
            if !ports.is_empty() {
                scan.servers.entry(pane).or_default().push(Server { pid, name, ports });
            }
        }
        for list in scan.servers.values_mut() {
            list.sort_by_key(|s| s.pid);
        }
        scan
    }
}

/// Whether `pid` is `root` or runs under it.
fn in_tree(procs: &HashMap<Pid, sysinfo::Process>, pid: u32, root: u32) -> bool {
    let mut at = pid;
    for _ in 0..64 {
        if at == root {
            return true;
        }
        match procs.get(&Pid::from_u32(at)).and_then(|p| p.parent()) {
            Some(parent) if parent.as_u32() != at => at = parent.as_u32(),
            _ => return false,
        }
    }
    false
}

/// The TCP ports each of `pids` listens on.
#[cfg(target_os = "macos")]
fn listening(pids: &[u32]) -> HashMap<u32, Vec<u16>> {
    let list: Vec<String> = pids.iter().map(u32::to_string).collect();
    let out = std::process::Command::new("lsof")
        .args(["-nP", "-a", "-iTCP", "-sTCP:LISTEN", "-Fpn", "-p", &list.join(",")])
        .stderr(std::process::Stdio::null())
        .output();
    let Ok(out) = out else { return HashMap::new() };
    parse_lsof(&String::from_utf8_lossy(&out.stdout))
}

/// The TCP ports each of `pids` listens on, from /proc.
#[cfg(not(target_os = "macos"))]
fn listening(pids: &[u32]) -> HashMap<u32, Vec<u16>> {
    // Listening sockets' inodes and ports.
    let mut sockets: HashMap<String, u16> = HashMap::new();
    for table in ["/proc/net/tcp", "/proc/net/tcp6"] {
        let Ok(text) = std::fs::read_to_string(table) else { continue };
        for line in text.lines().skip(1) {
            let f: Vec<&str> = line.split_whitespace().collect();
            if f.len() > 9 && f[3] == "0A" {
                let port = f[1].rsplit(':').next().and_then(|p| u16::from_str_radix(p, 16).ok());
                if let Some(port) = port {
                    sockets.insert(f[9].to_string(), port);
                }
            }
        }
    }
    let mut out: HashMap<u32, Vec<u16>> = HashMap::new();
    for pid in pids {
        let Ok(fds) = std::fs::read_dir(format!("/proc/{pid}/fd")) else { continue };
        for fd in fds.flatten() {
            let Ok(link) = std::fs::read_link(fd.path()) else { continue };
            let link = link.to_string_lossy();
            if let Some(port) =
                link.strip_prefix("socket:[").and_then(|l| l.strip_suffix(']')).and_then(|i| sockets.get(i))
            {
                let ports = out.entry(*pid).or_default();
                if !ports.contains(port) {
                    ports.push(*port);
                }
            }
        }
    }
    out
}

/// lsof's `-Fpn` output: `p<pid>` lines, then `n<address>:<port>`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn parse_lsof(text: &str) -> HashMap<u32, Vec<u16>> {
    let mut out: HashMap<u32, Vec<u16>> = HashMap::new();
    let mut pid = None;
    for line in text.lines() {
        if let Some(p) = line.strip_prefix('p') {
            pid = p.parse().ok();
        } else if let (Some(n), Some(pid)) = (line.strip_prefix('n'), pid)
            && let Some(port) = n.rsplit(':').next().and_then(|p| p.parse::<u16>().ok())
        {
            let ports: &mut Vec<u16> = out.entry(pid).or_default();
            if !ports.contains(&port) {
                ports.push(port);
            }
        }
    }
    out
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
    fn reads_lsof() {
        let ports = parse_lsof("p12\nf5\nn*:3000\nf6\nn[::1]:3000\np40\nn127.0.0.1:5173\n");
        assert_eq!(ports.get(&12), Some(&vec![3000]));
        assert_eq!(ports.get(&40), Some(&vec![5173]));
    }

    #[test]
    fn finds_a_server_that_left_its_shell() {
        if std::process::Command::new("python3").arg("--version").output().is_err() {
            return;
        }
        let id = format!("test-{}", std::process::id());
        // A shell that starts a server under nohup and exits, as `npm
        // start &` then closing would, or a server that daemonizes.
        let shell = std::process::Command::new("sh")
            .args(["-c", "nohup python3 -m http.server 0 --bind 127.0.0.1 >/dev/null 2>&1 & echo $!"])
            .env("AULORA_PANE_ID", &id)
            .output()
            .unwrap();
        let server: u32 = String::from_utf8_lossy(&shell.stdout).trim().parse().unwrap();
        let mut found = None;
        for _ in 0..50 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            // The pane's shell: this test, which the server isn't under.
            // A new scan each time: the server takes a moment to listen.
            let scan = Escaped::default().scan(&[(id.clone(), std::process::id())], now());
            found = scan.servers.get(&id).cloned();
            if found.is_some() {
                break;
            }
        }
        unsafe { libc::kill(server as i32, libc::SIGKILL) };
        let found = found.expect("found the server");
        assert_eq!(found[0].pid, server);
        assert!(!found[0].ports.is_empty());
    }

    fn now() -> Ms {
        aulora_core::model::now_ms()
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
