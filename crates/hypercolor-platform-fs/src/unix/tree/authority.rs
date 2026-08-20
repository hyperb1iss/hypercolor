use std::ffi::{OsStr, OsString};
use std::fs::{self, File, TryLockError};
use std::io::{self, Read};
use std::os::unix::ffi::OsStringExt as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, MutexGuard};

use rustix::fs::{
    AtFlags, CWD, FileType, Mode, OFlags, mkdirat, openat, readlinkat, renameat, statat, symlinkat,
    unlinkat,
};
use rustix::io::Errno;

use super::staging::validate_staging_name;
use super::traversal::{
    copy_exact, directory_entries, duplicate_directory, entry_metadata_at, entry_name,
    metadata_for_file, open_directory_at, open_regular_file_at, rustix_mode, set_exact_mode,
    validate_mode, validate_symlink_target, write_secret_contents,
};
use super::{
    DirectoryAuthority, DirectoryEntryKind, DirectoryEntryMetadata, ExclusiveDirectory,
    ExclusiveDirectoryShared, OpenedRegularFile, PRIVATE_DIRECTORY_MODE, PrivateStagingDirectory,
    ReadOnlyDirectoryAuthority, SECRET_FILE_MODE, SYMLINK_SEQUENCE,
};

impl ReadOnlyDirectoryAuthority {
    /// Open a source root without following its final path component.
    ///
    /// The pathname is resolved only by this constructor. Every later
    /// traversal and inspection stays relative to the retained directory
    /// handle.
    ///
    /// # Errors
    ///
    /// Returns the operating-system error when `directory` cannot be opened
    /// as a no-follow directory with ordinary permission bits.
    pub fn open(directory: &Path) -> io::Result<Self> {
        let directory = openat(
            CWD,
            directory,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .map(File::from)
        .map_err(io::Error::from)?;
        let metadata = metadata_for_file(&directory)?;
        if metadata.kind != DirectoryEntryKind::Directory || metadata.mode & !0o777 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "source root is not a directory with ordinary permission bits",
            ));
        }
        Ok(Self { directory })
    }

    /// Return metadata obtained with `fstat` from this exact directory handle.
    ///
    /// # Errors
    ///
    /// Returns the operating-system error when handle inspection fails.
    pub fn metadata(&self) -> io::Result<DirectoryEntryMetadata> {
        metadata_for_file(&self.directory)
    }

    /// Open one normal child directory without following a symbolic link.
    ///
    /// # Errors
    ///
    /// Returns invalid-input when `name` is not one normal component or the
    /// entry has an unsafe type or mode. Returns the operating-system error
    /// when opening or inspecting it fails.
    pub fn open_child_directory(&self, name: &Path) -> io::Result<Self> {
        let name = entry_name(name, "directory name")?;
        Ok(Self {
            directory: open_directory_at(&self.directory, name)?,
        })
    }

    /// Enumerate normal child entry names through the retained handle.
    ///
    /// Dot entries are omitted and names are returned in raw-byte order.
    ///
    /// # Errors
    ///
    /// Returns the operating-system error when enumeration fails, or
    /// invalid-data for a non-normal entry name.
    pub fn entries(&self) -> io::Result<Vec<OsString>> {
        directory_entries(&self.directory)
    }

    /// Inspect one normal child without following a symbolic link.
    ///
    /// Missing entries return `Ok(None)`.
    ///
    /// # Errors
    ///
    /// Returns invalid-input when `name` is not one normal component and the
    /// operating-system error when metadata inspection fails.
    pub fn entry_metadata(&self, name: &Path) -> io::Result<Option<DirectoryEntryMetadata>> {
        let name = entry_name(name, "entry name")?;
        entry_metadata_at(&self.directory, name)
    }

    /// Open one single-link regular child without following symbolic links.
    ///
    /// The returned metadata is obtained from the same opened file handle.
    ///
    /// # Errors
    ///
    /// Returns invalid-input for a non-normal name, unsafe type, link count,
    /// or mode. Returns the operating-system error when opening fails.
    pub fn open_regular_file(&self, name: &Path) -> io::Result<OpenedRegularFile> {
        let name = entry_name(name, "file name")?;
        open_regular_file_at(&self.directory, name)
    }
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
                shared: Arc::new(ExclusiveDirectoryShared {
                    directory,
                    _lock: lock,
                    lock_name: lock_name.to_os_string(),
                    operation: Mutex::new(()),
                }),
            })),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(error)) => Err(error),
        }
    }

    /// Open the governed root as a handle-relative directory authority.
    ///
    /// The returned authority retains this directory's install lock and uses
    /// the same in-process operation gate.
    ///
    /// # Errors
    ///
    /// Returns the operating-system error when the opened root directory
    /// cannot be duplicated without following links.
    pub fn root_directory(&self) -> io::Result<DirectoryAuthority> {
        let _operation = self.operation_guard()?;
        Ok(DirectoryAuthority {
            directory: duplicate_directory(&self.shared.directory)?,
            shared: Arc::clone(&self.shared),
            protected_name: Some(self.shared.lock_name.clone()),
        })
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
            &self.shared.directory,
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
        let target =
            readlinkat(&self.shared.directory, name, Vec::new()).map_err(io::Error::from)?;
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
            &self.shared.directory,
            name,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::RUSR | Mode::WUSR,
        )
        .map(File::from)
        .map_err(io::Error::from)?;
        if let Err(error) = write_secret_contents(&mut file, contents) {
            drop(file);
            let _ = unlinkat(&self.shared.directory, name, AtFlags::empty());
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
        renameat(
            &self.shared.directory,
            source,
            &self.shared.directory,
            destination,
        )
        .map_err(io::Error::from)?;
        self.shared.directory.sync_all()
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
            match symlinkat(target, &self.shared.directory, candidate.as_str()) {
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
            &self.shared.directory,
            staged.as_str(),
            &self.shared.directory,
            destination,
        ) {
            let _ = unlinkat(&self.shared.directory, staged.as_str(), AtFlags::empty());
            return Err(io::Error::from(error));
        }
        self.shared.directory.sync_all()
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
        unlinkat(&self.shared.directory, name, AtFlags::empty()).map_err(io::Error::from)?;
        self.shared.directory.sync_all()?;
        Ok(true)
    }

    fn mutable_entry_name<'a>(&self, path: &'a Path, description: &str) -> io::Result<&'a OsStr> {
        let name = entry_name(path, description)?;
        if name == self.shared.lock_name {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "refusing to mutate the held directory lock entry",
            ));
        }
        Ok(name)
    }

    pub(super) fn operation_guard(&self) -> io::Result<MutexGuard<'_, ()>> {
        self.shared
            .operation
            .lock()
            .map_err(|_| io::Error::other("exclusive directory operation gate is poisoned"))
    }

    fn entry_type(&self, name: &OsStr) -> io::Result<Option<FileType>> {
        match statat(&self.shared.directory, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(metadata) => Ok(Some(FileType::from_raw_mode(metadata.st_mode))),
            Err(Errno::NOENT) => Ok(None),
            Err(error) => Err(io::Error::from(error)),
        }
    }
}

impl DirectoryAuthority {
    /// Duplicate this exact opened directory as a read-only authority.
    ///
    /// The returned capability remains bound to the same directory inode even
    /// after the pathname that originally named it is renamed or replaced. It
    /// exposes no mutation operations and does not retain the install lock.
    ///
    /// # Errors
    ///
    /// Returns the operating-system error when the directory handle cannot be
    /// duplicated.
    pub fn read_only(&self) -> io::Result<ReadOnlyDirectoryAuthority> {
        let _operation = self.operation_guard()?;
        Ok(ReadOnlyDirectoryAuthority {
            directory: duplicate_directory(&self.directory)?,
        })
    }

    /// Return metadata obtained with `fstat` from this exact directory handle.
    ///
    /// # Errors
    ///
    /// Returns the operating-system error when handle inspection fails.
    pub fn metadata(&self) -> io::Result<DirectoryEntryMetadata> {
        let _operation = self.operation_guard()?;
        metadata_for_file(&self.directory)
    }

    /// Open one normal child directory without following a symbolic link.
    ///
    /// The returned authority shares the root install lock and operation gate.
    ///
    /// # Errors
    ///
    /// Returns an invalid-input error when `name` is not one normal component.
    /// Returns the operating-system error when the entry cannot be opened as a
    /// no-follow directory.
    pub fn open_child_directory(&self, name: &Path) -> io::Result<Self> {
        let name = entry_name(name, "directory name")?;
        let _operation = self.operation_guard()?;
        Ok(Self {
            directory: open_directory_at(&self.directory, name)?,
            shared: Arc::clone(&self.shared),
            protected_name: None,
        })
    }

    /// Create one private normal child directory and sync both directories.
    ///
    /// The entry must not already exist. New directories use exact mode
    /// `0700`; callers may apply their final manifest mode after populating the
    /// tree with [`Self::set_mode`].
    ///
    /// # Errors
    ///
    /// Returns an invalid-input error when `name` is not one normal component
    /// or names the held lock. Returns the operating-system error when secure
    /// creation, opening, permission setting, or durability fails.
    pub fn create_child_directory(&self, name: &Path) -> io::Result<Self> {
        let name = self.mutable_entry_name(name, "directory name")?;
        let _operation = self.operation_guard()?;
        mkdirat(&self.directory, name, rustix_mode(PRIVATE_DIRECTORY_MODE)?)
            .map_err(io::Error::from)?;

        let directory = match open_directory_at(&self.directory, name) {
            Ok(directory) => directory,
            Err(error) => {
                let _ = unlinkat(&self.directory, name, AtFlags::REMOVEDIR);
                return Err(error);
            }
        };
        if let Err(error) =
            set_exact_mode(&directory, PRIVATE_DIRECTORY_MODE).and_then(|()| directory.sync_all())
        {
            drop(directory);
            let _ = unlinkat(&self.directory, name, AtFlags::REMOVEDIR);
            return Err(error);
        }
        self.directory.sync_all()?;
        Ok(Self {
            directory,
            shared: Arc::clone(&self.shared),
            protected_name: None,
        })
    }

    /// Create one unpublished private staging directory capability.
    ///
    /// `name` must be one normal component beginning with
    /// `.hypercolor-stage-` and containing a nonempty ASCII alphanumeric,
    /// hyphen, or underscore suffix. The name must not already exist.
    ///
    /// # Errors
    ///
    /// Returns invalid-input for an unsafe staging name. Returns the
    /// operating-system error when the private directory or retained parent
    /// handle cannot be created durably.
    pub fn create_private_staging_directory(
        &self,
        name: &Path,
    ) -> io::Result<PrivateStagingDirectory> {
        let name = validate_staging_name(name)?;
        let directory = self.create_child_directory(Path::new(name))?;
        Ok(PrivateStagingDirectory {
            parent: duplicate_directory(&self.directory)?,
            name: name.to_os_string(),
            directory,
            protected_name: self.protected_name.clone(),
        })
    }

    /// Enumerate normal child entry names through the opened directory.
    ///
    /// Dot entries are omitted. Returned names are sorted by their raw
    /// operating-system byte representation for deterministic validation.
    ///
    /// # Errors
    ///
    /// Returns the operating-system error when directory enumeration fails, or
    /// invalid-data when the operating system returns a non-normal entry name.
    pub fn entries(&self) -> io::Result<Vec<OsString>> {
        let _operation = self.operation_guard()?;
        directory_entries(&self.directory)
    }

    /// Inspect one normal child without following a symbolic link.
    ///
    /// Missing entries return `Ok(None)`.
    ///
    /// # Errors
    ///
    /// Returns an invalid-input error when `name` is not one normal component.
    /// Returns the operating-system error when metadata inspection fails.
    pub fn entry_metadata(&self, name: &Path) -> io::Result<Option<DirectoryEntryMetadata>> {
        let name = entry_name(name, "entry name")?;
        let _operation = self.operation_guard()?;
        entry_metadata_at(&self.directory, name)
    }

    /// Open one single-link regular child without following symbolic links.
    ///
    /// # Errors
    ///
    /// Returns an invalid-input error when `name` is not one normal component,
    /// names a non-regular entry, names a regular file with a link count other
    /// than one, or contains set-ID or sticky permission bits. Returns the
    /// operating-system error when opening or inspecting the file fails.
    pub fn open_regular_file(&self, name: &Path) -> io::Result<OpenedRegularFile> {
        let name = entry_name(name, "file name")?;
        let _operation = self.operation_guard()?;
        open_regular_file_at(&self.directory, name)
    }

    /// Create, copy, chmod, and sync one new regular child file.
    ///
    /// `mode` accepts only permission bits in `0o000..=0o777`. Exactly
    /// `expected_size` bytes must be copied and the source must then be at EOF.
    /// The destination must not exist. The new file and its parent directory
    /// are synced before success is returned.
    ///
    /// # Errors
    ///
    /// Returns an invalid-input error for an unsafe name, held lock entry, or
    /// mode containing non-permission bits. Returns the operating-system error
    /// when creation, copying, permission setting, or durability fails.
    pub fn create_regular_file(
        &self,
        name: &Path,
        mode: u32,
        expected_size: u64,
        source: &mut impl Read,
    ) -> io::Result<DirectoryEntryMetadata> {
        validate_mode(mode)?;
        let name = self.mutable_entry_name(name, "file name")?;
        let _operation = self.operation_guard()?;
        let mut file = openat(
            &self.directory,
            name,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            rustix_mode(SECRET_FILE_MODE)?,
        )
        .map(File::from)
        .map_err(io::Error::from)?;
        let result = set_exact_mode(&file, mode)
            .and_then(|()| copy_exact(source, &mut file, expected_size))
            .and_then(|()| file.sync_all())
            .and_then(|()| metadata_for_file(&file));
        let metadata = match result {
            Ok(metadata) => metadata,
            Err(error) => {
                drop(file);
                let _ = unlinkat(&self.directory, name, AtFlags::empty());
                return Err(error);
            }
        };
        if metadata.kind != DirectoryEntryKind::RegularFile
            || metadata.link_count != 1
            || metadata.mode != mode
            || metadata.size != expected_size
        {
            drop(file);
            let _ = unlinkat(&self.directory, name, AtFlags::empty());
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "created file metadata does not satisfy the regular-file contract",
            ));
        }
        self.directory.sync_all()?;
        Ok(metadata)
    }

    /// Set this directory's exact permission bits and sync its metadata.
    ///
    /// # Errors
    ///
    /// Returns invalid-input when `mode` contains non-permission bits. Returns
    /// the operating-system error when chmod or the durability barrier fails.
    pub fn set_mode(&self, mode: u32) -> io::Result<()> {
        validate_mode(mode)?;
        let _operation = self.operation_guard()?;
        set_exact_mode(&self.directory, mode)?;
        self.directory.sync_all()
    }

    /// Sync this opened directory.
    ///
    /// # Errors
    ///
    /// Returns the operating-system error when the durability barrier fails.
    pub fn sync(&self) -> io::Result<()> {
        let _operation = self.operation_guard()?;
        self.directory.sync_all()
    }

    fn mutable_entry_name<'a>(&self, path: &'a Path, description: &str) -> io::Result<&'a OsStr> {
        let name = entry_name(path, description)?;
        if self.protected_name.as_deref() == Some(name) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "refusing to mutate the held directory lock entry",
            ));
        }
        Ok(name)
    }

    pub(super) fn operation_guard(&self) -> io::Result<MutexGuard<'_, ()>> {
        self.shared
            .operation
            .lock()
            .map_err(|_| io::Error::other("exclusive directory operation gate is poisoned"))
    }
}
