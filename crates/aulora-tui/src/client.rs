//! Background connection to the daemon. Reconnects forever.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::time::Duration;

use aulora_core::model::Snapshot;
use aulora_core::proto::{self, ClientMsg, ServerMsg};

pub enum Incoming {
    Snapshot(Box<Snapshot>),
    Connecting,
    Offline(String),
    PaneScreen { pane_id: String, cols: u16, rows: u16, data: Vec<u8> },
    PaneOutput { pane_id: String, data: Vec<u8> },
    PaneExited { pane_id: String },
}

pub type SpawnFn = Box<dyn Fn() + Send + Sync>;

pub fn spawn(socket: PathBuf, spawn_daemon: Option<SpawnFn>, tx: Sender<Incoming>) {
    std::thread::Builder::new()
        .name("aulora-client".into())
        .spawn(move || {
            let mut tried_spawn = false;
            let mut failures = 0u32;
            loop {
                match UnixStream::connect(&socket) {
                    Ok(stream) => {
                        failures = 0;
                        if let Err(e) = subscribe(stream, &tx)
                            && tx.send(Incoming::Offline(e.to_string())).is_err()
                        {
                            return;
                        }
                    }
                    Err(e) => {
                        failures += 1;
                        if !tried_spawn && let Some(f) = &spawn_daemon {
                            tried_spawn = true;
                            let _ = tx.send(Incoming::Connecting);
                            f();
                            std::thread::sleep(Duration::from_millis(150));
                            continue;
                        }
                        // Give a freshly spawned daemon a moment before reporting.
                        if failures > 8 && tx.send(Incoming::Offline(short_err(&e))).is_err() {
                            return;
                        }
                        if failures > 40 {
                            tried_spawn = false;
                            failures = 9;
                        }
                    }
                }
                std::thread::sleep(Duration::from_millis(250));
            }
        })
        .expect("spawn client thread");
}

fn short_err(e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused => "not running".into(),
        _ => e.to_string(),
    }
}

fn subscribe(mut stream: UnixStream, tx: &Sender<Incoming>) -> std::io::Result<()> {
    stream.write_all(b"{\"type\":\"subscribe\"}\n")?;
    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let line = line?;
        match serde_json::from_str::<ServerMsg>(&line) {
            Ok(ServerMsg::Snapshot(s)) => {
                if tx.send(Incoming::Snapshot(s)).is_err() {
                    return Ok(());
                }
            }
            Ok(_) => {}
            Err(e) => return Err(std::io::Error::other(e)),
        }
    }
    Err(std::io::Error::other("daemon closed the connection"))
}

/// Send one command on its own connection and return the reply.
pub fn command(socket: &PathBuf, msg: &ClientMsg) -> Option<ServerMsg> {
    let mut s = UnixStream::connect(socket).ok()?;
    let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
    let mut line = serde_json::to_string(msg).ok()?;
    line.push('\n');
    s.write_all(line.as_bytes()).ok()?;
    let mut buf = String::new();
    BufReader::new(s).read_line(&mut buf).ok()?;
    serde_json::from_str(&buf).ok()
}

/// A long-lived connection for pane streams: attach, input, resize.
/// It reconnects on its own and re-attaches whatever was attached.
pub struct Stream {
    tx: mpsc::Sender<ClientMsg>,
}

impl Stream {
    pub fn spawn(socket: PathBuf, incoming: Sender<Incoming>) -> Stream {
        let (tx, rx) = mpsc::channel::<ClientMsg>();
        std::thread::Builder::new()
            .name("aulora-stream".into())
            .spawn(move || stream_loop(socket, rx, incoming))
            .expect("spawn stream thread");
        Stream { tx }
    }

    pub fn send(&self, msg: ClientMsg) {
        let _ = self.tx.send(msg);
    }
}

fn stream_loop(socket: PathBuf, rx: mpsc::Receiver<ClientMsg>, incoming: Sender<Incoming>) {
    let mut attached: Option<String> = None;
    let mut size: Option<ClientMsg> = None;
    loop {
        let Ok(stream) = UnixStream::connect(&socket) else {
            // Keep track of what we want while the daemon is away.
            match rx.recv_timeout(Duration::from_millis(300)) {
                Ok(msg) => track(&msg, &mut attached, &mut size),
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
                Err(_) => {}
            }
            continue;
        };
        let Ok(reader) = stream.try_clone() else { continue };
        let alive = Arc::new(AtomicBool::new(true));
        {
            let alive = alive.clone();
            let incoming = incoming.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(reader).lines() {
                    let Ok(line) = line else { break };
                    let msg = match serde_json::from_str::<ServerMsg>(&line) {
                        Ok(ServerMsg::PaneScreen { pane_id, cols, rows, data }) => {
                            Incoming::PaneScreen { pane_id, cols, rows, data: proto::decode(&data) }
                        }
                        Ok(ServerMsg::PaneOutput { pane_id, data }) => {
                            Incoming::PaneOutput { pane_id, data: proto::decode(&data) }
                        }
                        Ok(ServerMsg::PaneExited { pane_id, .. }) => Incoming::PaneExited { pane_id },
                        _ => continue,
                    };
                    if incoming.send(msg).is_err() {
                        break;
                    }
                }
                alive.store(false, Ordering::SeqCst);
            });
        }
        let mut w = stream;
        let mut write = |msg: &ClientMsg| -> bool {
            let Ok(mut line) = serde_json::to_vec(msg) else { return true };
            line.push(b'\n');
            w.write_all(&line).is_ok()
        };
        // Restore what this client had before the connection dropped.
        if let Some(id) = &attached {
            write(&ClientMsg::Attach { pane_id: id.clone() });
        }
        if let Some(r) = &size {
            write(r);
        }
        while alive.load(Ordering::SeqCst) {
            match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(msg) => {
                    track(&msg, &mut attached, &mut size);
                    if !write(&msg) {
                        break;
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
                Err(_) => {}
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn track(msg: &ClientMsg, attached: &mut Option<String>, size: &mut Option<ClientMsg>) {
    match msg {
        ClientMsg::Attach { pane_id } => *attached = Some(pane_id.clone()),
        ClientMsg::Detach { pane_id } if attached.as_deref() == Some(pane_id.as_str()) => *attached = None,
        ClientMsg::Resize { .. } => *size = Some(msg.clone()),
        _ => {}
    }
}
