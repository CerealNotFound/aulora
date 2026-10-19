//! The aulora daemon. Receives hook events over a Unix socket, keeps the
//! state of every Claude Code session, and streams snapshots to clients.

pub mod git;
pub mod procs;
pub mod store;
