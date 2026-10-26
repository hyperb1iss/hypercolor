#![deny(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]
#![cfg_attr(not(target_os = "windows"), forbid(unsafe_code))]

//! Audited platform filesystem operations for Hypercolor.

use std::fs::File;
use std::io;
use std::path::Path;

#[cfg(unix)]
mod unix;

pub mod user_dirs;

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
pub use windows::{
    DestinationIdentity, WindowsDirectory, WindowsDirectoryAccess, WindowsEntryKind,
    WindowsEntryMetadata, WindowsFileLock, WindowsGrant, WindowsSecurity, WindowsSid,
};

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

/// Open an existing regular file so it can be synced, without following a
/// final symlink or Windows reparse point.
///
/// Unix opens read-only, which is enough to sync, and non-blocking, so a
/// FIFO at the path is refused instead of waiting for a writer. Windows
/// opens with write access, since it flushes only through such a handle.
///
/// # Errors
///
/// Returns the platform filesystem error when the path cannot be opened, the
/// final path component is a symlink or reparse point, or what it names is not
/// a regular file.
pub fn open_regular_no_follow_for_sync(path: &Path) -> io::Result<File> {
    #[cfg(unix)]
    let file = unix::open_no_follow_nonblocking(path)?;

    #[cfg(target_os = "windows")]
    let file = windows::open_no_follow_with_write(path, true)?;

    #[cfg(not(any(unix, target_os = "windows")))]
    let file: File = {
        let _ = path;
        return Err(unsupported("symlink-refusing file open"));
    };

    if file.metadata()?.is_file() {
        Ok(file)
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "refusing to open something other than a regular file",
        ))
    }
}

/// Which file this is, plus a change stamp that moves whenever it changes.
///
/// The stamp is the inode change time on Unix and `ChangeTime` with the file
/// id on Windows. A write, truncation, rename, or permission change moves it,
/// and on Unix unprivileged code cannot set it back, so restoring a file's
/// bytes and modification time still leaves a different stamp. Equal stamps
/// mean the same file, unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileChangeStamp {
    volume: u64,
    file_id: u128,
    len: u64,
    changed: i128,
    modified: i128,
    mode: u32,
}

impl FileChangeStamp {
    /// Whether both stamps describe the same file, changed or not.
    #[must_use]
    pub fn same_file(&self, other: &Self) -> bool {
        self.volume == other.volume && self.file_id == other.file_id
    }

    /// The file's length in bytes when stamped.
    #[must_use]
    pub const fn byte_len(&self) -> u64 {
        self.len
    }

    /// The Unix permission bits the file carried when stamped.
    #[cfg(unix)]
    #[must_use]
    pub const fn permission_bits(&self) -> u32 {
        self.mode & 0o7777
    }
}

/// Stamp an open file.
///
/// Returns `Ok(None)` on platforms that offer no change stamp, where callers
/// must assume the file may have changed.
///
/// # Errors
///
/// Returns the platform error when the file's metadata cannot be read.
pub fn file_change_stamp(file: &File) -> io::Result<Option<FileChangeStamp>> {
    #[cfg(unix)]
    {
        unix::file_change_stamp(file).map(Some)
    }

    #[cfg(target_os = "windows")]
    {
        windows::file_change_stamp(file).map(Some)
    }

    #[cfg(not(any(unix, target_os = "windows")))]
    {
        let _ = file;
        Ok(None)
    }
}

/// Stamp the file at `path`, following a final symlink.
///
/// Returns `Ok(None)` on platforms that offer no change stamp.
///
/// # Errors
///
/// Returns the platform error when the file cannot be opened or its metadata
/// cannot be read, including when it does not exist.
pub fn path_change_stamp(path: &Path) -> io::Result<Option<FileChangeStamp>> {
    #[cfg(unix)]
    {
        unix::path_change_stamp(path).map(Some)
    }

    #[cfg(target_os = "windows")]
    {
        windows::path_change_stamp(path).map(Some)
    }

    #[cfg(not(any(unix, target_os = "windows")))]
    {
        let _ = path;
        Ok(None)
    }
}

/// Atomically rename `source` over `destination`, leaving the directory
/// durability barrier to the caller.
///
/// On Unix the replacement is visible once this returns but not durable
/// until the caller syncs the destination's parent directory, so a caller
/// that owns that barrier itself does not pay for a second one. Windows
/// requests write-through from the operating system, exactly as
/// [`durable_replace`] does.
///
/// # Errors
///
/// Returns the platform filesystem error when the rename fails, in which
/// case the destination is unchanged.
pub fn rename_replacing(source: &Path, destination: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        unix::rename_replacing(source, destination)
    }

    #[cfg(target_os = "windows")]
    {
        windows::durable_replace(source, destination)
    }

    #[cfg(not(any(unix, target_os = "windows")))]
    {
        let _ = (source, destination);
        Err(unsupported("file replacement"))
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
