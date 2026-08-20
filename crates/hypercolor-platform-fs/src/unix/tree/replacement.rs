mod child;
mod directory;
mod ensure;
mod exact;
mod operation;
mod read;
mod rollback;
mod staging;

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io;
use std::path::Path;
use std::sync::{Arc, MutexGuard};

use super::traversal::{
    duplicate_directory, entry_metadata_at, entry_name, metadata_for_file, open_directory_at,
    require_same_entry, unsafe_entry,
};
use super::{
    DirectoryAnchor, DirectoryEntryMetadata, EntryReplacement, ExactEntry, PublicDirectoryAuthority,
};
use exact::observe_entry_at;
use operation::{remove_entry_with, replace_entry_with};

pub use exact::MAX_EXACT_ENTRY_BYTES;

impl PublicDirectoryAuthority {
    /// Prove that every retained path component still names its opened inode.
    ///
    /// # Errors
    ///
    /// Returns an error when an ancestor disappeared, changed identity, or can
    /// no longer be inspected without following links.
    pub fn validate_ancestry(&self) -> io::Result<()> {
        let _operation = self.operation_guard()?;
        self.validate_ancestry_inner()
    }

    /// Open one existing normal child directory under this anchored authority.
    ///
    /// The returned child retains the same global lock and extends the exact
    /// ancestry proof by one component.
    ///
    /// # Errors
    ///
    /// Returns invalid-input for an unsafe name or entry. Returns an error when
    /// ancestry validation, no-follow opening, or identity proof fails.
    pub fn open_child_directory(&self, name: &Path) -> io::Result<Self> {
        let name = entry_name(name, "public child directory name")?;
        let _operation = self.operation_guard()?;
        self.validate_ancestry_inner()?;
        let child = open_directory_at(&self.directory, name)?;
        let expected = metadata_for_file(&child)?;
        let named = entry_metadata_at(&self.directory, name)?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "public child directory disappeared during acquisition",
            )
        })?;
        require_same_entry(
            expected,
            named,
            "public child directory changed during acquisition",
        )?;
        self.validate_ancestry_inner()?;
        Ok(Self {
            directory: child,
            ancestry: self.extended_ancestry(name, expected)?,
            shared: Arc::clone(&self.shared),
        })
    }

    /// Durably create one absent normal child directory with an exact mode.
    ///
    /// Creation never walks or creates additional components. The returned
    /// authority retains the same global lock and an extended ancestry proof.
    ///
    /// # Errors
    ///
    /// Returns invalid-input for an unsafe name or mode. Returns an error when
    /// the destination exists, ancestry changes, creation cannot be proven, or
    /// a durability barrier fails. Failed post-creation proof removes only the
    /// exact newly created directory when it remains recoverable.
    pub fn durable_create_child_directory(&self, name: &Path, mode: u32) -> io::Result<Self> {
        self.create_child_directory_with(
            name,
            mode,
            || Ok(()),
            || Ok(()),
            |directory| directory.sync_all(),
        )
    }

    /// Durably ensure one normal child directory exists with an exact mode.
    ///
    /// This monotone operation never removes or rolls back the directory. An
    /// interrupted creation can therefore be healed by replay. Existing
    /// directories are accepted only at the requested mode or at a private
    /// creation mode narrowed by the process umask.
    ///
    /// Namespace exclusion is provided by the global install lock only among
    /// cooperating users of that lock. Noncooperating namespace mutation,
    /// including mutation by the same user ID, is outside this guarantee.
    ///
    /// # Errors
    ///
    /// Returns invalid-input for an unsafe name, mode, or existing entry.
    /// Returns an error when creation, no-follow opening, exact identity proof,
    /// ancestry validation, or a durability barrier fails. The directory is
    /// intentionally retained after any error that follows creation.
    pub fn durable_ensure_child_directory(&self, name: &Path, mode: u32) -> io::Result<Self> {
        self.ensure_child_directory_with(
            name,
            mode,
            || Ok(()),
            || Ok(()),
            || Ok(()),
            |directory| directory.sync_all(),
        )
    }

    /// Observe one public entry as an exact supported state.
    ///
    /// Regular files are hashed from a retained no-follow handle. Symbolic
    /// links are read without dereferencing them. Directories, special files,
    /// hardlinks, unsafe permission bits, and oversized files are rejected.
    ///
    /// # Errors
    ///
    /// Returns invalid-input for an unsafe name or unsupported entry state.
    /// Returns an error when ancestry, entry inspection, or hashing fails.
    pub fn observe_entry(&self, name: &Path) -> io::Result<ExactEntry> {
        let name = entry_name(name, "public entry name")?;
        let _operation = self.operation_guard()?;
        self.validate_ancestry_inner()?;
        let observed = observe_entry_at(&self.directory, name)?;
        self.validate_ancestry_inner()?;
        Ok(observed)
    }

    /// Durably replace one exact public entry with regular bytes or a symlink.
    ///
    /// The expected state must be a fresh exact observation from the same
    /// public pathname. Replacement is staged in this directory. An absent
    /// destination uses a no-replace rename; an existing destination uses an
    /// exchange, proves the displaced exact state, and restores it on any
    /// mismatch. Recovery storage is reserved before public visibility.
    ///
    /// # Errors
    ///
    /// Returns invalid-input for unsafe names, targets, modes, expected states,
    /// or destination drift. Returns an error when staging, exchange, proof,
    /// recovery, or durability fails.
    pub fn durable_replace_entry(
        &self,
        name: &Path,
        expected: &ExactEntry,
        replacement: EntryReplacement<'_>,
    ) -> io::Result<ExactEntry> {
        let name = entry_name(name, "public replacement destination")?;
        let _operation = self.operation_guard()?;
        replace_entry_with(
            self,
            name,
            expected,
            replacement,
            || Ok(()),
            || Ok(()),
            |directory| directory.sync_all(),
        )
    }

    /// Durably remove one exact existing public file or symbolic link.
    ///
    /// Removal exchanges the destination into pre-reserved recovery storage,
    /// proves the displaced state, removes the public placeholder, and syncs
    /// the parent. Drift restores the displaced entry or quarantines an
    /// unverified destination without deleting it.
    ///
    /// # Errors
    ///
    /// Returns invalid-input for an absent expectation, unsafe name, or state
    /// drift. Returns an error when exchange, proof, recovery, or durability
    /// fails.
    pub fn durable_remove_entry(&self, name: &Path, expected: &ExactEntry) -> io::Result<()> {
        let name = entry_name(name, "public removal destination")?;
        if matches!(expected, ExactEntry::Absent) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "exact public removal requires an existing entry",
            ));
        }
        let _operation = self.operation_guard()?;
        remove_entry_with(
            self,
            name,
            expected,
            || Ok(()),
            || Ok(()),
            |directory| directory.sync_all(),
        )
    }

    pub(super) fn validate_ancestry_inner(&self) -> io::Result<()> {
        for anchor in &self.ancestry {
            let current = entry_metadata_at(&anchor.parent, &anchor.name)?.ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "public directory ancestry component disappeared",
                )
            })?;
            require_same_entry(
                anchor.expected,
                current,
                "public directory ancestry identity changed",
            )?;
        }
        let expected = self
            .ancestry
            .last()
            .ok_or_else(|| unsafe_entry("public directory authority has no ancestry proof"))?
            .expected;
        let current = metadata_for_file(&self.directory)?;
        require_same_entry(
            expected,
            current,
            "public directory handle identity changed",
        )
    }

    pub(super) fn operation_guard(&self) -> io::Result<MutexGuard<'_, ()>> {
        self.shared
            .operation
            .lock()
            .map_err(|_| io::Error::other("exclusive directory operation gate is poisoned"))
    }

    fn extended_ancestry(
        &self,
        name: &OsStr,
        expected: DirectoryEntryMetadata,
    ) -> io::Result<Vec<DirectoryAnchor>> {
        let mut ancestry = Vec::with_capacity(self.ancestry.len() + 1);
        for anchor in &self.ancestry {
            ancestry.push(DirectoryAnchor {
                parent: duplicate_directory(&anchor.parent)?,
                name: anchor.name.clone(),
                expected: anchor.expected,
            });
        }
        ancestry.push(DirectoryAnchor {
            parent: duplicate_directory(&self.directory)?,
            name: name.to_os_string(),
            expected,
        });
        Ok(ancestry)
    }

    fn prepare_extended_ancestry(&self, name: &OsStr) -> io::Result<PreparedExtendedAncestry> {
        let mut ancestry = Vec::with_capacity(self.ancestry.len() + 1);
        for anchor in &self.ancestry {
            ancestry.push(DirectoryAnchor {
                parent: duplicate_directory(&anchor.parent)?,
                name: anchor.name.clone(),
                expected: anchor.expected,
            });
        }
        Ok(PreparedExtendedAncestry {
            ancestry,
            final_parent: duplicate_directory(&self.directory)?,
            final_name: name.to_os_string(),
        })
    }
}

#[derive(Debug)]
struct PreparedExtendedAncestry {
    ancestry: Vec<DirectoryAnchor>,
    final_parent: File,
    final_name: OsString,
}

impl PreparedExtendedAncestry {
    fn finish(mut self, expected: DirectoryEntryMetadata) -> Vec<DirectoryAnchor> {
        debug_assert!(self.ancestry.len() < self.ancestry.capacity());
        self.ancestry.push(DirectoryAnchor {
            parent: self.final_parent,
            name: self.final_name,
            expected,
        });
        self.ancestry
    }
}

#[cfg(all(test, any(target_vendor = "apple", target_os = "linux")))]
mod directory_tests;
#[cfg(all(test, any(target_vendor = "apple", target_os = "linux")))]
mod ensure_tests;
#[cfg(all(test, any(target_vendor = "apple", target_os = "linux")))]
mod tests;
