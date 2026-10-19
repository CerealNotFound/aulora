//! Shared pieces of aulora: the data model and the wire protocol between
//! the daemon and its clients.

pub mod model;
pub mod paths;
pub mod proto;
pub mod util;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
