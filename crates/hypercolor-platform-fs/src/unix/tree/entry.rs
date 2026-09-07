use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io;
use std::os::unix::ffi::OsStringExt as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use rustix::fs::{
    AtFlags, FileType, Mode, OFlags, openat, readlinkat, renameat, statat, symlinkat, unlinkat,
};
use rustix::io::Errno;

use super::traversal::{
    entry_metadata_at, entry_name, metadata_for_file, validate_symlink_target,
    write_secret_contents,
};
use super::{DirectoryAuthority, ExclusiveDirectory, ExclusiveDirectoryShared, SYMLINK_SEQUENCE};

impl ExclusiveDirectory {
    /// Open one regular file without following a symbolic link.
    ///
    /// # Errors
    ///
    /// Returns invalid-input when `name` is not one normal component. Returns
    /// the operating-system error when the entry cannot be opened as a regular
    /// no-follow file.
    pub fn open_file(&self, name: &Path) -> io::Result<File> {
        let name = entry_name(name, "file name")?;
        let _operation = self.operation_guard()?;
        open_file_at(&self.shared.directory, name)
    }

    /// Read one symbolic-link target through the opened directory authority.
    ///
    /// Missing entries return `Ok(None)`. Existing non-symbolic-link entries
    /// are rejected.
    ///
    /// # Errors
    ///
    /// Returns invalid-input for an unsafe name or non-symbolic-link entry.
    /// Returns the operating-system error when inspection or reading fails.
    pub fn read_symlink(&self, name: &Path) -> io::Result<Option<PathBuf>> {
        let name = entry_name(name, "symbolic-link name")?;
        let _operation = self.operation_guard()?;
        read_symlink_at(&self.shared.directory, name)
    }

    /// Create and sync one private file without replacing an existing entry.
    ///
    /// # Errors
    ///
    /// Returns invalid-input for an unsafe name or held-lock alias. Returns the
    /// operating-system error when creation, writing, syncing, or cleanup fails.
    pub fn write_secret(&self, name: &Path, contents: &[u8]) -> io::Result<()> {
        let name = entry_name(name, "file name")?;
        let _operation = self.operation_guard()?;
        require_mutable_entry(
            &self.shared.directory,
            &self.shared,
            Some(&self.shared.lock_name),
            name,
        )?;
        write_secret_at(&self.shared.directory, name, contents)
    }

    /// Atomically replace one file entry with another and sync the directory.
    ///
    /// # Errors
    ///
    /// Returns invalid-input for an unsafe name or held-lock alias. Returns the
    /// operating-system error when replacement or the durability barrier fails.
    pub fn durable_replace_file(&self, source: &Path, destination: &Path) -> io::Result<()> {
        let source = entry_name(source, "source name")?;
        let destination = entry_name(destination, "destination name")?;
        let _operation = self.operation_guard()?;
        require_mutable_entry(
            &self.shared.directory,
            &self.shared,
            Some(&self.shared.lock_name),
            source,
        )?;
        require_mutable_entry(
            &self.shared.directory,
            &self.shared,
            Some(&self.shared.lock_name),
            destination,
        )?;
        durable_replace_file_at(&self.shared.directory, source, destination)
    }

    /// Atomically replace one symbolic link and sync the directory.
    ///
    /// `target` must satisfy the internal install-store link contract.
    /// `destination` must be absent or an existing symbolic link.
    ///
    /// # Errors
    ///
    /// Returns invalid-input for an unsafe target, destination, or held-lock
    /// alias. Returns the operating-system error when replacement or syncing
    /// fails.
    pub fn durable_replace_symlink(&self, target: &Path, destination: &Path) -> io::Result<()> {
        let destination = entry_name(destination, "symbolic-link destination")?;
        let _operation = self.operation_guard()?;
        require_mutable_entry(
            &self.shared.directory,
            &self.shared,
            Some(&self.shared.lock_name),
            destination,
        )?;
        durable_replace_symlink_at(&self.shared.directory, target, destination)
    }

    /// Remove one file or symbolic link and sync the directory.
    ///
    /// Missing entries return `Ok(false)`. Directories are rejected.
    ///
    /// # Errors
    ///
    /// Returns invalid-input for an unsafe name, held-lock alias, or directory.
    /// Returns the operating-system error when removal or syncing fails.
    pub fn durable_remove_file(&self, name: &Path) -> io::Result<bool> {
        let name = entry_name(name, "file name")?;
        let _operation = self.operation_guard()?;
        require_mutable_entry(
            &self.shared.directory,
            &self.shared,
            Some(&self.shared.lock_name),
            name,
        )?;
        durable_remove_file_at(&self.shared.directory, name)
    }
}

impl DirectoryAuthority {
    /// Open one regular file without following a symbolic link.
    ///
    /// The returned descriptor stays bound to the opened inode after a later
    /// name replacement.
    ///
    /// # Errors
    ///
    /// Returns invalid-input when `name` is not one normal component. Returns
    /// the operating-system error when the entry cannot be opened as a regular
    /// no-follow file.
    pub fn open_file(&self, name: &Path) -> io::Result<File> {
        let name = entry_name(name, "file name")?;
        let _operation = self.operation_guard()?;
        open_file_at(&self.directory, name)
    }

    /// Read one symbolic-link target through the retained directory handle.
    ///
    /// Missing entries return `Ok(None)`. Existing non-symbolic-link entries
    /// are rejected.
    ///
    /// # Errors
    ///
    /// Returns invalid-input for an unsafe name or non-symbolic-link entry.
    /// Returns the operating-system error when inspection or reading fails.
    pub fn read_symlink(&self, name: &Path) -> io::Result<Option<PathBuf>> {
        let name = entry_name(name, "symbolic-link name")?;
        let _operation = self.operation_guard()?;
        read_symlink_at(&self.directory, name)
    }

    /// Create and sync one private file without replacing an existing entry.
    ///
    /// # Errors
    ///
    /// Returns invalid-input for an unsafe name or held-lock alias. Returns the
    /// operating-system error when creation, writing, syncing, or cleanup fails.
    pub fn write_secret(&self, name: &Path, contents: &[u8]) -> io::Result<()> {
        let name = entry_name(name, "file name")?;
        let _operation = self.operation_guard()?;
        require_mutable_entry(
            &self.directory,
            &self.shared,
            self.protected_name.as_deref(),
            name,
        )?;
        write_secret_at(&self.directory, name, contents)
    }

    /// Atomically replace one file entry with another and sync the directory.
    ///
    /// # Errors
    ///
    /// Returns invalid-input for an unsafe name or held-lock alias. Returns the
    /// operating-system error when replacement or the durability barrier fails.
    pub fn durable_replace_file(&self, source: &Path, destination: &Path) -> io::Result<()> {
        let source = entry_name(source, "source name")?;
        let destination = entry_name(destination, "destination name")?;
        let _operation = self.operation_guard()?;
        require_mutable_entry(
            &self.directory,
            &self.shared,
            self.protected_name.as_deref(),
            source,
        )?;
        require_mutable_entry(
            &self.directory,
            &self.shared,
            self.protected_name.as_deref(),
            destination,
        )?;
        durable_replace_file_at(&self.directory, source, destination)
    }

    /// Atomically replace one symbolic link and sync the directory.
    ///
    /// `target` must satisfy the internal install-store link contract.
    /// `destination` must be absent or an existing symbolic link.
    ///
    /// # Errors
    ///
    /// Returns invalid-input for an unsafe target, destination, or held-lock
    /// alias. Returns the operating-system error when replacement or syncing
    /// fails.
    pub fn durable_replace_symlink(&self, target: &Path, destination: &Path) -> io::Result<()> {
        let destination = entry_name(destination, "symbolic-link destination")?;
        let _operation = self.operation_guard()?;
        require_mutable_entry(
            &self.directory,
            &self.shared,
            self.protected_name.as_deref(),
            destination,
        )?;
        durable_replace_symlink_at(&self.directory, target, destination)
    }

    /// Remove one file or symbolic link and sync the directory.
    ///
    /// Missing entries return `Ok(false)`. Directories are rejected.
    ///
    /// # Errors
    ///
    /// Returns invalid-input for an unsafe name, held-lock alias, or directory.
    /// Returns the operating-system error when removal or syncing fails.
    pub fn durable_remove_file(&self, name: &Path) -> io::Result<bool> {
        let name = entry_name(name, "file name")?;
        let _operation = self.operation_guard()?;
        require_mutable_entry(
            &self.directory,
            &self.shared,
            self.protected_name.as_deref(),
            name,
        )?;
        durable_remove_file_at(&self.directory, name)
    }
}

pub(super) fn protected_name_for_directory(
    directory: &File,
    shared: &ExclusiveDirectoryShared,
) -> io::Result<Option<OsString>> {
    let actual = metadata_for_file(directory)?;
    let lock_parent = metadata_for_file(&shared.directory)?;
    Ok(
        (actual.device == lock_parent.device && actual.inode == lock_parent.inode)
            .then(|| shared.lock_name.clone()),
    )
}

pub(super) fn require_mutable_entry(
    directory: &File,
    shared: &ExclusiveDirectoryShared,
    protected_name: Option<&OsStr>,
    name: &OsStr,
) -> io::Result<()> {
    if protected_name == Some(name) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "refusing to mutate the held directory lock entry",
        ));
    }
    let Some(entry) = entry_metadata_at(directory, name)? else {
        return Ok(());
    };
    let lock = metadata_for_file(&shared._lock)?;
    if entry.device == lock.device && entry.inode == lock.inode {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "refusing to mutate an alias of the held directory lock entry",
        ));
    }
    Ok(())
}

pub(super) fn open_file_at(directory: &File, name: &OsStr) -> io::Result<File> {
    let file = openat(
        directory,
        name,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
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

pub(super) fn read_symlink_at(directory: &File, name: &OsStr) -> io::Result<Option<PathBuf>> {
    let Some(file_type) = entry_type(directory, name)? else {
        return Ok(None);
    };
    if !file_type.is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "entry is not a symbolic link",
        ));
    }
    let target = readlinkat(directory, name, Vec::new()).map_err(io::Error::from)?;
    Ok(Some(PathBuf::from(OsString::from_vec(target.into_bytes()))))
}

pub(super) fn write_secret_at(directory: &File, name: &OsStr, contents: &[u8]) -> io::Result<()> {
    let mut file = openat(
        directory,
        name,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::RUSR | Mode::WUSR,
    )
    .map(File::from)
    .map_err(io::Error::from)?;
    if let Err(error) = write_secret_contents(&mut file, contents) {
        drop(file);
        let _ = unlinkat(directory, name, AtFlags::empty());
        return Err(error);
    }
    Ok(())
}

pub(super) fn durable_replace_file_at(
    directory: &File,
    source: &OsStr,
    destination: &OsStr,
) -> io::Result<()> {
    renameat(directory, source, directory, destination).map_err(io::Error::from)?;
    directory.sync_all()
}

pub(super) fn durable_replace_symlink_at(
    directory: &File,
    target: &Path,
    destination: &OsStr,
) -> io::Result<()> {
    validate_symlink_target(target)?;
    if let Some(file_type) = entry_type(directory, destination)?
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
        match symlinkat(target, directory, candidate.as_str()) {
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
    if let Err(error) = renameat(directory, staged.as_str(), directory, destination) {
        let _ = unlinkat(directory, staged.as_str(), AtFlags::empty());
        return Err(io::Error::from(error));
    }
    directory.sync_all()
}

pub(super) fn durable_remove_file_at(directory: &File, name: &OsStr) -> io::Result<bool> {
    let Some(file_type) = entry_type(directory, name)? else {
        return Ok(false);
    };
    if file_type.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "refusing to remove a directory through the file removal boundary",
        ));
    }
    unlinkat(directory, name, AtFlags::empty()).map_err(io::Error::from)?;
    directory.sync_all()?;
    Ok(true)
}

fn entry_type(directory: &File, name: &OsStr) -> io::Result<Option<FileType>> {
    match statat(directory, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(metadata) => Ok(Some(FileType::from_raw_mode(metadata.st_mode))),
        Err(Errno::NOENT) => Ok(None),
        Err(error) => Err(io::Error::from(error)),
    }
}
