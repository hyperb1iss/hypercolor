//! ROLI Blocks device backend — IPC bridge to blocksd.
//!
//! Communicates with the blocksd daemon over a Unix domain socket to drive
//! ROLI Lightpad, LUMI Keys, and Seaboard Blocks as pixel-addressable RGB
//! surfaces. See spec 30 for full protocol details.

mod backend;
#[allow(dead_code)] // Phase 3 methods (subscribe, read_event) not yet used
mod connection;
mod types;

pub use backend::BlocksBackend;
pub use types::RoliBlockType;
