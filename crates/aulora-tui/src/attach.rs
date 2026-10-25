//! `aulora attach`: put a pane in the current terminal, tmux-attach style.
//!
//! Bytes pass through untouched in both directions, so everything the
//! program asks of the terminal (mouse, bracketed paste, cursor keys)
//! works as if it ran here directly. Ctrl-Q detaches.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use aulora_core::proto::{self, ClientMsg, ServerMsg};
use ratatui::crossterm::{execute, terminal};

pub const DETACH_KEY: u8 = 0x11; // Ctrl-Q

/// Undo modes the program may have switched on in this terminal.
const RESET_MODES: &str = "\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?2004l\x1b[?1l\x1b>\x1b[0m\x1b[?25h";

fn send(w: &Mutex<UnixStream>, msg: &ClientMsg) -> std::io::Result<()> {
    let mut line = serde_json::to_vec(msg).map_err(std::io::Error::other)?;
    line.push(b'\n');
    w.lock().unwrap().write_all(&line)
}

fn restore() {
    let mut out = std::io::stdout();
    let _ = out.write_all(RESET_MODES.as_bytes());
    let _ = execute!(out, terminal::LeaveAlternateScreen);
    let _ = terminal::disable_raw_mode();
}

pub fn attach(socket: &Path, pane_id: &str, label: &str) -> Result<()> {
    let stream = UnixStream::connect(socket).context("the aulora daemon is not running")?;
    let reader = stream.try_clone()?;
    let writer = Arc::new(Mutex::new(stream));
    let (cols, rows) = terminal::size().unwrap_or((120, 36));
    send(&writer, &ClientMsg::Attach { pane_id: pane_id.to_string(), terminal: true })?;
    send(&writer, &ClientMsg::Resize { pane_id: pane_id.to_string(), cols, rows })?;

    // Wait for the first screen before taking over the terminal, so a bad
    // pane id fails cleanly.
    let mut lines = BufReader::new(reader);
    let mut first = String::new();
    lines.read_line(&mut first)?;
    let first_data = match serde_json::from_str::<ServerMsg>(&first)? {
        ServerMsg::PaneScreen { data, .. } => proto::decode(&data),
        ServerMsg::Error { message } => bail!(message),
        other => bail!("unexpected reply from daemon: {other:?}"),
    };

    terminal::enable_raw_mode()?;
    execute!(std::io::stdout(), terminal::EnterAlternateScreen)?;
    {
        let mut out = std::io::stdout();
        out.write_all(&first_data)?;
        out.flush()?;
    }

    let exited = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicBool::new(false));

    // Daemon → terminal.
    {
        let exited = exited.clone();
        let done = done.clone();
        std::thread::spawn(move || {
            let mut out = std::io::stdout();
            let mut line = String::new();
            loop {
                line.clear();
                match lines.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                match serde_json::from_str::<ServerMsg>(&line) {
                    Ok(ServerMsg::PaneScreen { data, .. } | ServerMsg::PaneOutput { data, .. }) => {
                        let _ = out.write_all(&proto::decode(&data));
                        let _ = out.flush();
                    }
                    Ok(ServerMsg::PaneExited { code, .. }) => {
                        exited.store(true, Ordering::SeqCst);
                        let _ = write!(
                            out,
                            "\r\n\x1b[0m\x1b[2m[aulora] process exited with code {code}; press any key to leave\x1b[0m"
                        );
                        let _ = out.flush();
                    }
                    _ => {}
                }
            }
            // Connection gone (daemon stopped) while still attached.
            if !done.load(Ordering::SeqCst) {
                restore();
                eprintln!("aulora: lost the connection to the daemon");
                std::process::exit(1);
            }
        });
    }

    // Follow terminal resizes.
    {
        let writer = writer.clone();
        let done = done.clone();
        let id = pane_id.to_string();
        std::thread::spawn(move || {
            let mut last = (cols, rows);
            while !done.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(150));
                if let Ok(size) = terminal::size()
                    && size != last
                {
                    last = size;
                    let _ = send(&writer, &ClientMsg::Resize { pane_id: id.clone(), cols: size.0, rows: size.1 });
                }
            }
        });
    }

    // Terminal → daemon.
    let mut stdin = std::io::stdin();
    let mut buf = [0u8; 8192];
    loop {
        let n = stdin.read(&mut buf)?;
        if n == 0 || exited.load(Ordering::SeqCst) {
            break;
        }
        let chunk = &buf[..n];
        let (data, detach) = match chunk.iter().position(|b| *b == DETACH_KEY) {
            Some(i) => (&chunk[..i], true),
            None => (chunk, false),
        };
        if !data.is_empty() {
            send(&writer, &ClientMsg::Input { pane_id: pane_id.to_string(), data: proto::encode(data) })?;
        }
        if detach {
            break;
        }
    }
    done.store(true, Ordering::SeqCst);
    let _ = send(&writer, &ClientMsg::Detach { pane_id: pane_id.to_string() });
    restore();
    if exited.load(Ordering::SeqCst) {
        println!("pane {label} exited");
    } else {
        println!("detached from {label} (still running; `aulora attach {pane_id}` to return)");
    }
    Ok(())
}
