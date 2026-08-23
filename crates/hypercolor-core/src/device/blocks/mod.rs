//! ROLI Blocks device backend — IPC bridge to blocksd.
//!
//! Communicates with the blocksd daemon over a Unix domain socket to drive
//! ROLI Lightpad, LUMI Keys, and Seaboard Blocks as pixel-addressable RGB
//! surfaces. See spec 30 for full protocol details.

mod backend;
mod connection;
mod scanner;
mod types;

pub use backend::BlocksBackend;
pub use scanner::BlocksScanner;
pub use types::RoliBlockType;
