#![deny(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]
#![cfg_attr(not(target_os = "windows"), forbid(unsafe_code))]

//! Audited platform filesystem operations for Hypercolor.

use std::io;
use std::path::Path;

#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "windows")]
pub use windows::DestinationIdentity;

/// Atomically replace `destination` with `source` on the same filesystem.
///
/// Windows replacement requests write-through durability from the operating
/// system. Unix callers remain responsible for syncing the parent directory
/// after this rename succeeds.
///
/// # Errors
///
/// Returns the platform filesystem error when the replacement fails.
pub fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        windows::replace_file(source, destination)
    }

    #[cfg(not(target_os = "windows"))]
    {
        std::fs::rename(source, destination)
    }
}
