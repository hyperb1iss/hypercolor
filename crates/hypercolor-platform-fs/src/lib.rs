#![deny(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]
#![cfg_attr(not(target_os = "windows"), forbid(unsafe_code))]

//! Audited platform filesystem operations for Hypercolor.

use std::fs::File;
use std::io;
use std::path::Path;

#[cfg(unix)]
mod unix;

#[cfg(target_os = "windows")]
mod windows;

#[cfg(unix)]
pub use unix::{
    DirectoryAuthority, DirectoryEntryKind, DirectoryEntryMetadata, EntryReplacement,
    ExactDirectoryEntry, ExactEntry, ExclusiveDirectory, MAX_EXACT_ENTRY_BYTES,
    MAX_PUBLIC_DIRECTORY_CHILD_COUNT, MAX_PUBLIC_DIRECTORY_CHILD_NAMES_BYTES, OpenedRegularFile,
    PrivateStagingDirectory, PublicDirectoryAuthority, ReadOnlyDirectoryAuthority,
};
#[cfg(target_os = "windows")]
pub use windows::DestinationIdentity;

/// Atomically replace `destination` with `source` and make the replacement
/// durable.
///
/// Windows replacement requests write-through durability from the operating
/// system. Unix replacement syncs the destination's parent directory before
/// returning success.
///
/// # Errors
///
/// Returns the platform filesystem error when opening the destination parent,
/// replacing the destination, or completing the durability barrier fails.
/// When the final durability barrier fails, the replacement may already be
/// visible.
pub fn durable_replace(source: &Path, destination: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        unix::durable_replace(source, destination)
    }

    #[cfg(target_os = "windows")]
    {
        windows::durable_replace(source, destination)
    }

    #[cfg(not(any(unix, target_os = "windows")))]
    {
        let _ = (source, destination);
        Err(unsupported("durable file replacement"))
    }
}

/// Create a new secret file, write all `contents`, and sync the file.
///
/// The destination must not already exist. Unix files are created with mode
/// `0600` and owned by the effective user that creates them. A failed write or
/// sync removes the newly created file on a best-effort basis.
///
/// # Errors
///
/// Returns the platform filesystem error when the destination already exists,
/// cannot be created securely, cannot be written, or cannot be synced.
pub fn write_secret(path: &Path, contents: &[u8]) -> io::Result<()> {
    #[cfg(unix)]
    {
        unix::write_secret(path, contents)
    }

    #[cfg(target_os = "windows")]
    {
        windows::write_secret(path, contents)
    }

    #[cfg(not(any(unix, target_os = "windows")))]
    {
        let _ = (path, contents);
        Err(unsupported("secret file creation"))
    }
}

/// Open an existing file for reading without following a final symlink.
///
/// # Errors
///
/// Returns the platform filesystem error when the path cannot be opened or the
/// final path component is a symlink or Windows reparse point.
pub fn open_no_follow(path: &Path) -> io::Result<File> {
    #[cfg(unix)]
    {
        unix::open_no_follow(path)
    }

    #[cfg(target_os = "windows")]
    {
        windows::open_no_follow(path)
    }

    #[cfg(not(any(unix, target_os = "windows")))]
    {
        let _ = path;
        Err(unsupported("symlink-refusing file open"))
    }
}

/// Atomically replace `destination` with `source` and make the replacement
/// durable.
///
/// # Errors
///
/// Returns the same errors as [`durable_replace`].
pub fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    durable_replace(source, destination)
}

#[cfg(not(any(unix, target_os = "windows")))]
fn unsupported(operation: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        format!("{operation} is unsupported on this platform"),
    )
}
