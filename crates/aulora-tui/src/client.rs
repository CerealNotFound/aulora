//! Background connection to the daemon. Reconnects forever.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::time::Duration;

use aulora_core::model::Snapshot;
use aulora_core::proto::{ClientMsg, ServerMsg};

pub enum Incoming {
    Snapshot(Box<Snapshot>),
    Connecting,
    Offline(String),
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

/// Fire-and-forget command on its own connection.
pub fn command(socket: &PathBuf, msg: &ClientMsg) {
    if let Ok(mut s) = UnixStream::connect(socket) {
        let _ = s.set_read_timeout(Some(Duration::from_millis(500)));
        if let Ok(mut line) = serde_json::to_string(msg) {
            line.push('\n');
            let _ = s.write_all(line.as_bytes());
            let mut buf = String::new();
            let _ = BufReader::new(s).read_line(&mut buf);
        }
    }
}
