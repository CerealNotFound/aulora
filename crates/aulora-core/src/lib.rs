//! Shared pieces of aulora: the data model, the wire protocol between the
//! daemon and its clients, the hook-event reducer and the transcript reader.

pub mod model;
pub mod paths;
pub mod proto;
pub mod reduce;
pub mod transcript;
pub mod util;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
