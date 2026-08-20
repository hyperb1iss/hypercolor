use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io;
use std::path::Path;
use std::sync::atomic::Ordering;

use rustix::fs::{RenameFlags, renameat_with};

use super::super::traversal::{
    directory_is_empty, entry_metadata_at, entry_name, metadata_for_file, open_directory_at,
    require_same_entry, unsafe_entry,
};
use super::super::{
    DirectoryEntryKind, DirectoryEntryMetadata, ExactDirectoryEntry, PublicDirectoryAuthority,
};
use super::staging::{MAX_STAGE_ATTEMPTS, STAGE_SEQUENCE};

const RECOVERY_NAME_PREFIX: &str = ".hypercolor-public-directory-recovery-";
const QUARANTINE_NAME_PREFIX: &str = ".hypercolor-public-directory-quarantine-";

impl PublicDirectoryAuthority {
    /// Observe one public child as absent or an exact empty directory.
    ///
    /// The directory is opened without following links and its full name and
    /// handle identity is proved around an allocation-free emptiness check.
    ///
    /// # Errors
    ///
    /// Returns invalid-input for an unsafe name, non-directory, special mode,
    /// or nonempty directory. Returns an error when ancestry or identity
    /// changes during observation.
    pub fn observe_empty_child_directory(&self, name: &Path) -> io::Result<ExactDirectoryEntry> {
        self.observe_empty_child_directory_with(name, || Ok(()))
    }

    /// Durably remove one exact empty public child directory.
    ///
    /// The expected state must be a fresh exact observation from the same
    /// public pathname. Removal first renames the retained child to a recovery
    /// name in the same parent, proves its exact identity and emptiness there,
    /// and syncs the parent while recovery remains possible. The exact empty
    /// directory remains as a hidden tombstone because POSIX has no fd-bound
    /// directory removal operation. The absent public name is the replay-safe
    /// durable result; each successful call retains exactly one tombstone.
    ///
    /// # Errors
    ///
    /// Returns invalid-input for an absent expectation or unsafe name. Returns
    /// an error when ancestry, identity, emptiness, recovery, or durability
    /// proof fails. Safe rollback restores the expected directory. Changed
    /// recovery or destination state remains under a recovery or quarantine
    /// name instead of being deleted or promoted.
    pub fn durable_remove_empty_child_directory(
        &self,
        name: &Path,
        expected: &ExactDirectoryEntry,
    ) -> io::Result<()> {
        self.remove_empty_child_directory_with(
            name,
            expected,
            |_, _| Ok(()),
            || Ok(()),
            |directory| directory.sync_all(),
        )
    }

    pub(super) fn observe_empty_child_directory_with(
        &self,
        name: &Path,
        after_open: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<ExactDirectoryEntry> {
        let name = entry_name(name, "public empty-directory name")?;
        let _operation = self.operation_guard()?;
        self.validate_ancestry_inner()?;
        let Some(named) = entry_metadata_at(&self.directory, name)? else {
            self.validate_ancestry_inner()?;
            return Ok(ExactDirectoryEntry::Absent);
        };
        require_supported_directory(named, "public empty-directory observation refused entry")?;
        let child = open_directory_at(&self.directory, name)?;
        after_open()?;
        let observed = require_exact_empty_directory(
            &self.directory,
            name,
            &child,
            named,
            "public empty-directory changed during observation",
        )?;
        self.validate_ancestry_inner()?;
        Ok(exact_directory_entry(observed))
    }

    pub(super) fn remove_empty_child_directory_with(
        &self,
        name: &Path,
        expected: &ExactDirectoryEntry,
        before_visibility: impl FnOnce(&OsStr, &OsStr) -> io::Result<()>,
        after_visibility: impl FnOnce() -> io::Result<()>,
        sync: impl Fn(&File) -> io::Result<()>,
    ) -> io::Result<()> {
        let name = entry_name(name, "public empty-directory removal destination")?;
        if matches!(expected, ExactDirectoryEntry::Absent) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "exact public directory removal requires an existing entry",
            ));
        }
        let _operation = self.operation_guard()?;
        remove_empty_child_directory(
            self,
            name,
            expected,
            before_visibility,
            after_visibility,
            sync,
        )
    }
}

struct RetainedEmptyDirectory {
    directory: File,
    metadata: DirectoryEntryMetadata,
}

impl RetainedEmptyDirectory {
    fn open(parent: &File, name: &OsStr, expected: &ExactDirectoryEntry) -> io::Result<Self> {
        let named = entry_metadata_at(parent, name)?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "public empty-directory removal destination disappeared",
            )
        })?;
        require_expected_directory(
            named,
            expected,
            "public empty-directory removal destination drifted",
        )?;
        let directory = open_directory_at(parent, name)?;
        let metadata = require_exact_empty_directory(
            parent,
            name,
            &directory,
            named,
            "public empty-directory changed while retaining its handle",
        )?;
        Ok(Self {
            directory,
            metadata,
        })
    }

    fn validate_at(
        &self,
        parent: &File,
        name: &OsStr,
        expected: &ExactDirectoryEntry,
        message: &'static str,
    ) -> io::Result<()> {
        require_expected_directory(self.metadata, expected, message)?;
        require_exact_empty_directory(parent, name, &self.directory, self.metadata, message)?;
        Ok(())
    }
}

fn remove_empty_child_directory(
    authority: &PublicDirectoryAuthority,
    destination: &OsStr,
    expected: &ExactDirectoryEntry,
    before_visibility: impl FnOnce(&OsStr, &OsStr) -> io::Result<()>,
    after_visibility: impl FnOnce() -> io::Result<()>,
    sync: impl Fn(&File) -> io::Result<()>,
) -> io::Result<()> {
    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    {
        authority.validate_ancestry_inner()?;
        let retained = RetainedEmptyDirectory::open(&authority.directory, destination, expected)?;
        let recovery = reserve_absent_name(&authority.directory, RECOVERY_NAME_PREFIX)?;
        let quarantine = reserve_absent_name(&authority.directory, QUARANTINE_NAME_PREFIX)?;
        before_visibility(&recovery, &quarantine)?;
        authority.validate_ancestry_inner()?;
        retained.validate_at(
            &authority.directory,
            destination,
            expected,
            "public empty-directory changed before removal visibility",
        )?;
        require_absent(
            &authority.directory,
            &recovery,
            "directory recovery name appeared",
        )?;
        require_absent(
            &authority.directory,
            &quarantine,
            "directory quarantine name appeared",
        )?;
        renameat_with(
            &authority.directory,
            destination,
            &authority.directory,
            &recovery,
            RenameFlags::NOREPLACE,
        )
        .map_err(io::Error::from)?;
        let proof = (|| {
            after_visibility()?;
            authority.validate_ancestry_inner()?;
            retained.validate_at(
                &authority.directory,
                &recovery,
                expected,
                "recovered public empty-directory changed after visibility",
            )?;
            require_absent(
                &authority.directory,
                destination,
                "public empty-directory destination reappeared after visibility",
            )?;
            sync(&authority.directory)?;
            authority.validate_ancestry_inner()?;
            retained.validate_at(
                &authority.directory,
                &recovery,
                expected,
                "recovered public empty-directory changed after durability",
            )?;
            require_absent(
                &authority.directory,
                destination,
                "public empty-directory destination reappeared after durability",
            )
        })();
        if let Err(error) = proof {
            return rollback_empty_directory_removal(
                authority,
                destination,
                &recovery,
                &quarantine,
                expected,
                &retained,
                error,
            );
        }
        Ok(())
    }

    #[cfg(not(any(target_vendor = "apple", target_os = "linux")))]
    {
        let _ = (
            authority,
            destination,
            expected,
            before_visibility,
            after_visibility,
            sync,
        );
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "exact public directory removal is unsupported on this Unix platform",
        ))
    }
}

fn rollback_empty_directory_removal<T>(
    authority: &PublicDirectoryAuthority,
    destination: &OsStr,
    recovery: &OsStr,
    quarantine: &OsStr,
    expected: &ExactDirectoryEntry,
    retained: &RetainedEmptyDirectory,
    proof_error: io::Error,
) -> io::Result<T> {
    if let Err(changed) = retained.validate_at(
        &authority.directory,
        recovery,
        expected,
        "public empty-directory recovery source changed",
    ) {
        return Err(io::Error::other(format!(
            "{proof_error}; changed public directory remains quarantined: {changed}"
        )));
    }
    if entry_metadata_at(&authority.directory, destination)?.is_none() {
        renameat_with(
            &authority.directory,
            recovery,
            &authority.directory,
            destination,
            RenameFlags::NOREPLACE,
        )
        .map_err(io::Error::from)?;
        authority.directory.sync_all()?;
        retained.validate_at(
            &authority.directory,
            destination,
            expected,
            "restored public empty-directory changed during rollback",
        )?;
        return Err(proof_error);
    }
    require_absent(
        &authority.directory,
        quarantine,
        "public directory quarantine name appeared during rollback",
    )?;
    renameat_with(
        &authority.directory,
        destination,
        &authority.directory,
        quarantine,
        RenameFlags::NOREPLACE,
    )
    .map_err(io::Error::from)?;
    if let Err(error) = renameat_with(
        &authority.directory,
        recovery,
        &authority.directory,
        destination,
        RenameFlags::NOREPLACE,
    ) {
        return Err(io::Error::other(format!(
            "{proof_error}; exact directory and unverified destination remain quarantined: {error}"
        )));
    }
    authority.directory.sync_all()?;
    retained.validate_at(
        &authority.directory,
        destination,
        expected,
        "restored public empty-directory changed after quarantine rollback",
    )?;
    Err(io::Error::other(format!(
        "{proof_error}; unverified directory destination quarantined as {}",
        quarantine.to_string_lossy()
    )))
}

fn reserve_absent_name(parent: &File, prefix: &str) -> io::Result<OsString> {
    for _ in 0..MAX_STAGE_ATTEMPTS {
        let sequence = STAGE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let name = OsString::from(format!("{prefix}{}-{sequence}", std::process::id()));
        if entry_metadata_at(parent, &name)?.is_none() {
            return Ok(name);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not reserve a public directory recovery name",
    ))
}

fn require_exact_empty_directory(
    parent: &File,
    name: &OsStr,
    directory: &File,
    expected: DirectoryEntryMetadata,
    message: &'static str,
) -> io::Result<DirectoryEntryMetadata> {
    let handle = metadata_for_file(directory)?;
    if handle != expected {
        return Err(unsafe_entry(message));
    }
    require_supported_directory(handle, message)?;
    if !directory_is_empty(directory)? {
        return Err(unsafe_entry(message));
    }
    let handle_after = metadata_for_file(directory)?;
    if handle_after != expected {
        return Err(unsafe_entry(message));
    }
    let named = entry_metadata_at(parent, name)?
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, message))?;
    if named != expected {
        return Err(unsafe_entry(message));
    }
    require_same_entry(expected, named, message)?;
    Ok(handle_after)
}

fn require_supported_directory(
    metadata: DirectoryEntryMetadata,
    message: &'static str,
) -> io::Result<()> {
    if metadata.kind != DirectoryEntryKind::Directory || metadata.mode & !0o777 != 0 {
        return Err(unsafe_entry(message));
    }
    Ok(())
}

fn require_expected_directory(
    metadata: DirectoryEntryMetadata,
    expected: &ExactDirectoryEntry,
    message: &'static str,
) -> io::Result<()> {
    let ExactDirectoryEntry::Empty {
        mode,
        device,
        inode,
    } = expected
    else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "exact public directory state must describe an empty directory",
        ));
    };
    require_supported_directory(metadata, message)?;
    if metadata.mode != *mode || metadata.device != *device || metadata.inode != *inode {
        return Err(unsafe_entry(message));
    }
    Ok(())
}

fn exact_directory_entry(metadata: DirectoryEntryMetadata) -> ExactDirectoryEntry {
    ExactDirectoryEntry::Empty {
        mode: metadata.mode,
        device: metadata.device,
        inode: metadata.inode,
    }
}

fn require_absent(parent: &File, name: &OsStr, message: &'static str) -> io::Result<()> {
    if entry_metadata_at(parent, name)?.is_some() {
        return Err(unsafe_entry(message));
    }
    Ok(())
}
