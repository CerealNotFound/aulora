//! Shared pieces of aulora: the data model, the wire protocol between the
//! daemon and its clients, and the hook-event reducer.

pub mod model;
pub mod paths;
pub mod proto;
pub mod reduce;
pub mod util;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
