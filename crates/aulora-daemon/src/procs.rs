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
