//! SQLite persistence on its own thread, so the async side never blocks on disk.

use std::path::Path;
use std::sync::mpsc;

use aulora_core::model::{FeedEvent, Ms, Session};
use rusqlite::{Connection, params};

pub enum StoreCmd {
    Upsert(Box<Session>),
    Delete(String),
    Feed(FeedEvent),
    /// Answer once everything sent before has been written.
    Flush(mpsc::Sender<()>),
}

pub struct Loaded {
    pub sessions: Vec<Session>,
    pub feed: Vec<FeedEvent>,
}

fn open(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=NORMAL;
         CREATE TABLE IF NOT EXISTS sessions (
             id TEXT PRIMARY KEY,
             updated_at INTEGER NOT NULL,
             data TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS feed (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             at INTEGER NOT NULL,
             data TEXT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS feed_at ON feed(at);",
    )?;
    Ok(conn)
}

/// Load recent state, then start the writer thread.
pub fn start(path: &Path, keep_since: Ms, feed_limit: usize) -> anyhow::Result<(Loaded, mpsc::Sender<StoreCmd>)> {
    let conn = open(path)?;
    conn.execute("DELETE FROM sessions WHERE updated_at < ?1", params![keep_since])?;
    conn.execute("DELETE FROM feed WHERE at < ?1", params![keep_since])?;

    let sessions = {
        let mut st = conn.prepare("SELECT data FROM sessions")?;
        let rows = st.query_map([], |r| r.get::<_, String>(0))?;
        rows.filter_map(|r| r.ok()).filter_map(|d| serde_json::from_str(&d).ok()).collect()
    };
    let mut feed: Vec<FeedEvent> = {
        let mut st = conn.prepare("SELECT data FROM feed ORDER BY id DESC LIMIT ?1")?;
        let rows = st.query_map(params![feed_limit as i64], |r| r.get::<_, String>(0))?;
        rows.filter_map(|r| r.ok()).filter_map(|d| serde_json::from_str(&d).ok()).collect()
    };
    feed.reverse();

    let (tx, rx) = mpsc::channel::<StoreCmd>();
    std::thread::Builder::new().name("aulora-store".into()).spawn(move || writer(conn, rx))?;
    Ok((Loaded { sessions, feed }, tx))
}

fn writer(conn: Connection, rx: mpsc::Receiver<StoreCmd>) {
    while let Ok(first) = rx.recv() {
        // Batch whatever else is queued into one transaction.
        let mut batch = vec![first];
        while let Ok(more) = rx.try_recv() {
            batch.push(more);
        }
        let mut flushed = Vec::new();
        let res = (|| -> rusqlite::Result<()> {
            let tx = conn.unchecked_transaction()?;
            for cmd in batch {
                match cmd {
                    StoreCmd::Flush(done) => flushed.push(done),
                    StoreCmd::Upsert(s) => {
                        let data = serde_json::to_string(&*s).unwrap_or_default();
                        tx.execute(
                            "INSERT INTO sessions (id, updated_at, data) VALUES (?1, ?2, ?3)
                             ON CONFLICT(id) DO UPDATE SET updated_at = excluded.updated_at, data = excluded.data",
                            params![s.id, s.last_event_at, data],
                        )?;
                    }
                    StoreCmd::Delete(id) => {
                        tx.execute("DELETE FROM sessions WHERE id = ?1", params![id])?;
                    }
                    StoreCmd::Feed(f) => {
                        let data = serde_json::to_string(&f).unwrap_or_default();
                        tx.execute("INSERT INTO feed (at, data) VALUES (?1, ?2)", params![f.at, data])?;
                    }
                }
            }
            tx.commit()
        })();
        if let Err(e) = res {
            tracing::warn!("store write failed: {e}");
        }
        for done in flushed {
            let _ = done.send(());
        }
    }
}
