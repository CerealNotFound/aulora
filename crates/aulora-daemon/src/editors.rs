//! Editor extensions (VS Code, Cursor) that can type into their terminals.
//!
//! An extension connects, lists its window's terminals with their shell
//! PIDs, and keeps the connection open. A Claude session belongs to the
//! terminal whose shell is one of its process ancestors. To send a prompt,
//! the daemon asks that extension to type it (`terminal.sendText`), so it
//! shows up in the editor exactly as if typed there.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use aulora_core::proto::{EditorTerminal, ServerMsg};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
use tokio::sync::{mpsc, oneshot};

struct Editor {
    app: String,
    terminals: Vec<EditorTerminal>,
    out: mpsc::Sender<ServerMsg>,
}

/// Where a session can be typed into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub conn: u64,
    pub terminal: u32,
    pub app: String,
}

#[derive(Default)]
pub struct Editors {
    editors: Mutex<HashMap<u64, Editor>>,
    pending: Mutex<HashMap<u64, oneshot::Sender<Option<String>>>>,
    next_request: AtomicU64,
    /// Session id → where it can be typed into.
    reach: Mutex<HashMap<String, Target>>,
    sys: Mutex<Option<System>>,
}

/// How far up the process tree a terminal's shell can be from Claude
/// (shell → claude, or shell → npx → node → claude).
const MAX_DEPTH: usize = 8;

impl Editors {
    pub fn register(&self, conn: u64, app: String, terminals: Vec<EditorTerminal>, out: mpsc::Sender<ServerMsg>) {
        self.editors.lock().unwrap().insert(conn, Editor { app, terminals, out });
    }

    pub fn unregister(&self, conn: u64) -> bool {
        let gone = self.editors.lock().unwrap().remove(&conn).is_some();
        if gone {
            self.reach.lock().unwrap().retain(|_, t| t.conn != conn);
        }
        gone
    }

    pub fn any(&self) -> bool {
        !self.editors.lock().unwrap().is_empty()
    }

    pub fn target(&self, session: &str) -> Option<Target> {
        self.reach.lock().unwrap().get(session).cloned()
    }

    /// The editor app each session can be typed into, for the snapshot.
    pub fn apps(&self) -> HashMap<String, String> {
        self.reach.lock().unwrap().iter().map(|(s, t)| (s.clone(), t.app.clone())).collect()
    }

    /// Match live sessions (id, Claude PID) to editor terminals. Returns
    /// true if anything changed.
    pub fn recompute(&self, sessions: &[(String, u32)]) -> bool {
        let terminals: HashMap<u32, (u64, u32, String)> = self
            .editors
            .lock()
            .unwrap()
            .iter()
            .flat_map(|(conn, e)| e.terminals.iter().map(move |t| (t.pid, (*conn, t.id, e.app.clone()))))
            .collect();
        let mut reach = HashMap::new();
        if !terminals.is_empty() && !sessions.is_empty() {
            let mut guard = self.sys.lock().unwrap();
            let sys = guard.get_or_insert_with(System::new);
            sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
            for (id, pid) in sessions {
                let found = ancestors(sys, *pid).into_iter().find_map(|p| terminals.get(&p));
                if let Some((conn, terminal, app)) = found {
                    reach.insert(id.clone(), Target { conn: *conn, terminal: *terminal, app: app.clone() });
                }
            }
        }
        let mut cur = self.reach.lock().unwrap();
        let changed = *cur != reach;
        *cur = reach;
        changed
    }

    /// Ask the editor to type `text` into the session's terminal, and wait
    /// for it to say it did.
    pub async fn type_into(&self, target: &Target, text: String) -> Result<(), String> {
        let out = self.editors.lock().unwrap().get(&target.conn).map(|e| e.out.clone());
        let Some(out) = out else { return Err(format!("{} closed", target.app)) };
        let request_id = self.next_request.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(request_id, tx);
        let msg = ServerMsg::Type { request_id, terminal_id: target.terminal, text };
        if out.send(msg).await.is_err() {
            self.pending.lock().unwrap().remove(&request_id);
            return Err(format!("{} closed", target.app));
        }
        match tokio::time::timeout(Duration::from_secs(5), rx).await {
            Ok(Ok(None)) => Ok(()),
            Ok(Ok(Some(e))) => Err(e),
            _ => {
                self.pending.lock().unwrap().remove(&request_id);
                Err(format!("{} didn't answer", target.app))
            }
        }
    }

    /// Ask the editor to show the session's terminal.
    pub async fn show(&self, target: &Target) -> Result<(), String> {
        let out = self.editors.lock().unwrap().get(&target.conn).map(|e| e.out.clone());
        let Some(out) = out else { return Err(format!("{} closed", target.app)) };
        out.send(ServerMsg::Show { terminal_id: target.terminal }).await.map_err(|_| format!("{} closed", target.app))
    }

    pub fn typed(&self, request_id: u64, error: Option<String>) {
        if let Some(tx) = self.pending.lock().unwrap().remove(&request_id) {
            let _ = tx.send(error);
        }
    }
}

/// `pid` and its parents, nearest first.
fn ancestors(sys: &System, pid: u32) -> Vec<u32> {
    let mut out = vec![pid];
    let mut cur = Pid::from_u32(pid);
    for _ in 0..MAX_DEPTH {
        match sys.process(cur).and_then(|p| p.parent()) {
            Some(parent) if parent.as_u32() > 1 => {
                out.push(parent.as_u32());
                cur = parent;
            }
            _ => break,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_match_the_terminal_whose_shell_runs_them() {
        // This test process stands in for the terminal's shell; a child
        // stands in for Claude.
        let mut child = std::process::Command::new("sleep").arg("5").spawn().unwrap();
        let editors = Editors::default();
        let (out, _rx) = mpsc::channel(4);
        let terminal = EditorTerminal { id: 7, pid: std::process::id(), name: "zsh".into() };
        editors.register(1, "Visual Studio Code".into(), vec![terminal], out);
        let sessions = vec![("s1".to_string(), child.id()), ("s2".to_string(), 1)];
        assert!(editors.recompute(&sessions));
        assert_eq!(editors.target("s1"), Some(Target { conn: 1, terminal: 7, app: "Visual Studio Code".into() }));
        assert_eq!(editors.target("s2"), None);
        assert!(!editors.recompute(&sessions), "nothing changed");
        assert!(editors.unregister(1));
        assert_eq!(editors.target("s1"), None);
        let _ = child.kill();
        let _ = child.wait();
    }

    #[tokio::test]
    async fn typing_waits_for_the_editor() {
        let editors = std::sync::Arc::new(Editors::default());
        let (out, mut rx) = mpsc::channel(4);
        editors.register(3, "Cursor".into(), vec![], out);
        let target = Target { conn: 3, terminal: 1, app: "Cursor".into() };
        let e2 = editors.clone();
        let editor = tokio::spawn(async move {
            let Some(ServerMsg::Type { request_id, text, .. }) = rx.recv().await else { panic!("no request") };
            assert_eq!(text, "hi");
            e2.typed(request_id, None);
        });
        assert_eq!(editors.type_into(&target, "hi".into()).await, Ok(()));
        editor.await.unwrap();
        editors.unregister(3);
        assert!(editors.type_into(&target, "hi".into()).await.is_err());
    }
}
