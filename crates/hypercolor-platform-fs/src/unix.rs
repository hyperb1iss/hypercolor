use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Write as _};
use std::os::unix::ffi::OsStringExt as _;
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

use rustix::fs::{
    AtFlags, CWD, FileType, Mode, OFlags, openat, readlinkat, renameat, statat, symlinkat, unlinkat,
};
use rustix::io::Errno;

const SECRET_FILE_MODE: u32 = 0o600;
static SYMLINK_SEQUENCE: AtomicU64 = AtomicU64::new(0);
#[cfg(any(target_os = "macos", target_os = "ios"))]
const OPEN_NO_FOLLOW: i32 = 0x0100;
#[cfg(any(target_os = "linux", target_os = "android"))]
const OPEN_NO_FOLLOW: i32 = 0x0002_0000;

pub(super) fn durable_replace(source: &Path, destination: &Path) -> io::Result<()> {
    durable_replace_with(source, destination, sync_directory)
}

/// Exclusive mutation authority for one opened Unix directory.
///
/// Every process mutating entries governed by this capability must acquire the
/// same `lock_name`. Operations stay relative to the opened directory handle,
/// so renaming or replacing the pathname used to acquire it cannot redirect a
/// later mutation or durability barrier.
#[derive(Debug)]
pub struct ExclusiveDirectory {
    directory: File,
    _lock: File,
    lock_name: OsString,
    operation: Mutex<()>,
}

impl ExclusiveDirectory {
    /// Try to acquire exclusive mutation authority for `directory`.
    ///
    /// Returns `Ok(None)` when another process holds `lock_name`.
    ///
    /// # Errors
    ///
    /// Returns an invalid-input error when `lock_name` is not one normal path
    /// component. Returns the operating-system error when the directory or
    /// lock cannot be opened or the lock operation fails.
    pub fn try_acquire(directory: &Path, lock_name: &Path) -> io::Result<Option<Self>> {
        let lock_name = entry_name(lock_name, "lock name")?;
        let directory = openat(
            CWD,
            directory,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .map(File::from)
        .map_err(io::Error::from)?;
        let lock = openat(
            &directory,
            lock_name,
            OFlags::RDWR | OFlags::CREATE | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::RUSR | Mode::WUSR,
        )
        .map(File::from)
        .map_err(io::Error::from)?;
        lock.set_permissions(fs::Permissions::from_mode(SECRET_FILE_MODE))?;
        match lock.try_lock() {
            Ok(()) => Ok(Some(Self {
                directory,
                _lock: lock,
                lock_name: lock_name.to_os_string(),
                operation: Mutex::new(()),
            })),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(error)) => Err(error),
        }
    }

    /// Open one regular file without following a symbolic link.
    ///
    /// # Errors
    ///
    /// Returns an invalid-input error when `name` is not one normal path
    /// component. Returns the operating-system error when the entry cannot be
    /// opened as a regular no-follow file.
    pub fn open_file(&self, name: &Path) -> io::Result<File> {
        let name = entry_name(name, "file name")?;
        let _operation = self.operation_guard()?;
        let file = openat(
            &self.directory,
            name,
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .map(File::from)
        .map_err(io::Error::from)?;
        if !file.metadata()?.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "entry is not a regular file",
            ));
        }
        Ok(file)
    }

    /// Read one symbolic-link target through the opened directory authority.
    ///
    /// Missing entries return `Ok(None)`. Existing non-symbolic-link entries
    /// are rejected.
    ///
    /// # Errors
    ///
    /// Returns an invalid-input error when `name` is not one normal path
    /// component or names a non-symbolic-link entry. Returns the
    /// operating-system error when inspection or reading fails.
    pub fn read_symlink(&self, name: &Path) -> io::Result<Option<PathBuf>> {
        let name = entry_name(name, "symbolic-link name")?;
        let _operation = self.operation_guard()?;
        let Some(file_type) = self.entry_type(name)? else {
            return Ok(None);
        };
        if !file_type.is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "entry is not a symbolic link",
            ));
        }
        let target = readlinkat(&self.directory, name, Vec::new()).map_err(io::Error::from)?;
        Ok(Some(PathBuf::from(std::ffi::OsString::from_vec(
            target.into_bytes(),
        ))))
    }

    /// Create and sync one private file without replacing an existing entry.
    ///
    /// # Errors
    ///
    /// Returns an invalid-input error when `name` is not one normal path
    /// component. Returns the operating-system error when creation, writing,
    /// syncing, or cleanup fails.
    pub fn write_secret(&self, name: &Path, contents: &[u8]) -> io::Result<()> {
        let name = self.mutable_entry_name(name, "file name")?;
        let _operation = self.operation_guard()?;
        let mut file = openat(
            &self.directory,
            name,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::RUSR | Mode::WUSR,
        )
        .map(File::from)
        .map_err(io::Error::from)?;
        if let Err(error) = write_secret_contents(&mut file, contents) {
            drop(file);
            let _ = unlinkat(&self.directory, name, AtFlags::empty());
            return Err(error);
        }
        Ok(())
    }

    /// Atomically replace one file entry with another and sync the directory.
    ///
    /// # Errors
    ///
    /// Returns an invalid-input error when either name is not one normal path
    /// component. Returns the operating-system error when replacement or the
    /// durability barrier fails.
    pub fn durable_replace_file(&self, source: &Path, destination: &Path) -> io::Result<()> {
        let source = self.mutable_entry_name(source, "source name")?;
        let destination = self.mutable_entry_name(destination, "destination name")?;
        let _operation = self.operation_guard()?;
        renameat(&self.directory, source, &self.directory, destination).map_err(io::Error::from)?;
        self.directory.sync_all()
    }

    /// Atomically replace one symbolic link and sync the directory.
    ///
    /// `target` must be a nonempty relative path containing only normal
    /// components. `destination` must be absent or an existing symbolic link.
    ///
    /// # Errors
    ///
    /// Returns an invalid-input error for an unsafe target, invalid destination
    /// name, or non-symbolic-link destination. Returns the operating-system
    /// error when staging, replacement, cleanup, or syncing fails.
    pub fn durable_replace_symlink(&self, target: &Path, destination: &Path) -> io::Result<()> {
        validate_symlink_target(target)?;
        let destination = self.mutable_entry_name(destination, "symbolic-link destination")?;
        let _operation = self.operation_guard()?;
        if let Some(file_type) = self.entry_type(destination)?
            && !file_type.is_symlink()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "refusing to replace a non-symbolic-link destination",
            ));
        }

        let mut staged = None;
        for _ in 0..128 {
            let sequence = SYMLINK_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let candidate = format!(
                ".{}.link.{}.{}",
                destination.to_string_lossy(),
                std::process::id(),
                sequence
            );
            match symlinkat(target, &self.directory, candidate.as_str()) {
                Ok(()) => {
                    staged = Some(candidate);
                    break;
                }
                Err(Errno::EXIST) => {}
                Err(error) => return Err(io::Error::from(error)),
            }
        }
        let staged = staged.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::AlreadyExists,
                "could not allocate a unique staged symbolic link",
            )
        })?;
        if let Err(error) = renameat(
            &self.directory,
            staged.as_str(),
            &self.directory,
            destination,
        ) {
            let _ = unlinkat(&self.directory, staged.as_str(), AtFlags::empty());
            return Err(io::Error::from(error));
        }
        self.directory.sync_all()
    }

    /// Remove one file or symbolic link and sync the directory.
    ///
    /// Missing entries return `Ok(false)`. Directories are rejected.
    ///
    /// # Errors
    ///
    /// Returns an invalid-input error when `name` is not one normal path
    /// component or names a directory. Returns the operating-system error when
    /// inspection, removal, or syncing fails.
    pub fn durable_remove_file(&self, name: &Path) -> io::Result<bool> {
        let name = self.mutable_entry_name(name, "file name")?;
        let _operation = self.operation_guard()?;
        let Some(file_type) = self.entry_type(name)? else {
            return Ok(false);
        };
        if file_type.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "refusing to remove a directory through the file removal boundary",
            ));
        }
        unlinkat(&self.directory, name, AtFlags::empty()).map_err(io::Error::from)?;
        self.directory.sync_all()?;
        Ok(true)
    }

    fn mutable_entry_name<'a>(&self, path: &'a Path, description: &str) -> io::Result<&'a OsStr> {
        let name = entry_name(path, description)?;
        if name == self.lock_name {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "refusing to mutate the held directory lock entry",
            ));
        }
        Ok(name)
    }

    fn operation_guard(&self) -> io::Result<MutexGuard<'_, ()>> {
        self.operation
            .lock()
            .map_err(|_| io::Error::other("exclusive directory operation gate is poisoned"))
    }

    fn entry_type(&self, name: &OsStr) -> io::Result<Option<FileType>> {
        match statat(&self.directory, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(metadata) => Ok(Some(FileType::from_raw_mode(metadata.st_mode))),
            Err(Errno::NOENT) => Ok(None),
            Err(error) => Err(io::Error::from(error)),
        }
    }
}

fn entry_name<'a>(path: &'a Path, description: &str) -> io::Result<&'a OsStr> {
    let mut components = path.components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(name)), None) => Ok(name),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{description} must be one normal path component"),
        )),
    }
}

fn validate_symlink_target(target: &Path) -> io::Result<()> {
    if target.as_os_str().is_empty()
        || target
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "symbolic-link target must be a nonempty normal relative path",
        ));
    }
    Ok(())
}

fn durable_replace_with(
    source: &Path,
    destination: &Path,
    sync: impl FnOnce(File) -> io::Result<()>,
) -> io::Result<()> {
    let parent = File::open(destination_parent(destination))?;
    fs::rename(source, destination)?;
    sync(parent)
}

pub(super) fn write_secret(path: &Path, contents: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(SECRET_FILE_MODE)
        .open(path)?;
    if let Err(error) = write_secret_contents(&mut file, contents) {
        drop(file);
        drop(fs::remove_file(path));
        return Err(error);
    }
    Ok(())
}

fn write_secret_contents(file: &mut File, contents: &[u8]) -> io::Result<()> {
    file.set_permissions(fs::Permissions::from_mode(SECRET_FILE_MODE))?;
    file.write_all(contents)?;
    file.sync_all()
}

pub(super) fn open_no_follow(path: &Path) -> io::Result<File> {
    #[cfg(any(
        target_os = "android",
        target_os = "ios",
        target_os = "linux",
        target_os = "macos"
    ))]
    {
        OpenOptions::new()
            .read(true)
            .custom_flags(OPEN_NO_FOLLOW)
            .open(path)
    }

    #[cfg(not(any(
        target_os = "android",
        target_os = "ios",
        target_os = "linux",
        target_os = "macos"
    )))]
    {
        let _ = path;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "symlink-refusing file open is unsupported on this Unix platform",
        ))
    }
}

fn destination_parent(destination: &Path) -> PathBuf {
    destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf()
}

fn sync_directory(directory: File) -> io::Result<()> {
    directory.sync_all()
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    #[test]
    fn replacement_runs_parent_sync_after_rename() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let source = directory.path().join("source.tmp");
        let destination = directory.path().join("state.json");
        fs::write(&source, b"new").expect("write source");
        fs::write(&destination, b"old").expect("write destination");
        let sync_called = Cell::new(false);

        durable_replace_with(&source, &destination, |parent| {
            assert!(parent.metadata()?.is_dir());
            assert_eq!(fs::read(&destination)?, b"new");
            sync_called.set(true);
            Ok(())
        })
        .expect("replace and sync destination");

        assert!(sync_called.get());
    }

    #[test]
    fn parent_sync_failure_is_reported_after_atomic_replacement() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let source = directory.path().join("source.tmp");
        let destination = directory.path().join("state.json");
        fs::write(&source, b"new").expect("write source");
        fs::write(&destination, b"old").expect("write destination");

        let error = durable_replace_with(&source, &destination, |_| {
            Err(io::Error::other("injected parent sync failure"))
        })
        .expect_err("parent sync failure must propagate");

        assert_eq!(error.to_string(), "injected parent sync failure");
        assert_eq!(fs::read(&destination).expect("read destination"), b"new");
        assert!(!source.exists());
    }
}
