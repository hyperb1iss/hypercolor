use std::ffi::{OsStr, OsString};
use std::io;
use std::os::unix::ffi::OsStrExt as _;
use std::path::Path;
use std::sync::atomic::Ordering;

use rustix::fs::{AtFlags, RenameFlags, renameat_with, unlinkat};
use rustix::io::Errno;

use super::super::traversal::{
    directory_entries, entry_metadata_at, entry_name, remove_owned_directory_tree, unsafe_entry,
    validate_owned_directory_tree,
};
use super::super::{DirectoryEntryKind, PublicDirectoryAuthority};
use super::staging::{MAX_STAGE_ATTEMPTS, STAGE_SEQUENCE};

const TOMBSTONE_PREFIX: &str = ".hypercolor-removing-";

impl PublicDirectoryAuthority {
    /// Durably remove one owned child directory tree, if present.
    ///
    /// The child is first renamed to a hidden tombstone in the same
    /// directory, so its public name disappears in one step and nothing that
    /// resolves the name, such as a lock file inside it, can be reached while
    /// its contents go. The tombstone and every entry beneath it must belong
    /// to the effective user. Symbolic links are unlinked and never followed;
    /// special files and multiply linked regular files are refused.
    /// Read-only directories are made owner-writable through their proven
    /// handles only as they are emptied.
    ///
    /// The whole tree is proven removable before the rename, so a refused
    /// tree keeps its public name. Tombstones left for the same name by an
    /// interrupted call are removed too, so calling this again always
    /// finishes the removal. Returns `true` when the public name or a
    /// leftover tombstone was removed.
    ///
    /// # Errors
    ///
    /// Returns invalid-input for an unsafe name, a non-directory child, or a
    /// refused entry. Returns an error when ancestry, identity, rename,
    /// removal or durability fails.
    pub fn durable_remove_child_tree(&self, name: &Path) -> io::Result<bool> {
        let name = entry_name(name, "owned tree name")?;
        let _operation = self.operation_guard()?;
        self.validate_ancestry_inner()?;
        let prefix = tombstone_prefix(name);
        let mut removed = false;
        if let Some(metadata) = entry_metadata_at(&self.directory, name)? {
            // Refuse before the rename, so a refused tree keeps its name.
            validate_owned_directory_tree(&self.directory, name, metadata)?;
            self.tombstone(name, &prefix)?;
            removed = true;
        }
        for entry in directory_entries(&self.directory)? {
            if !entry.as_bytes().starts_with(prefix.as_bytes()) {
                continue;
            }
            let Some(metadata) = entry_metadata_at(&self.directory, &entry)? else {
                continue;
            };
            remove_owned_directory_tree(&self.directory, &entry, metadata)?;
            removed = true;
        }
        self.validate_ancestry_inner()?;
        Ok(removed)
    }

    /// Durably remove one owned child directory only while it is empty.
    ///
    /// The kernel refuses the removal atomically when an entry exists, so a
    /// file created concurrently is never deleted. Returns `false` when the
    /// child is absent or not empty.
    ///
    /// # Errors
    ///
    /// Returns invalid-input for an unsafe name or a child that is not an
    /// owned directory, and the operating-system error for any other failure.
    pub fn durable_remove_empty_child(&self, name: &Path) -> io::Result<bool> {
        let name = entry_name(name, "empty directory name")?;
        let _operation = self.operation_guard()?;
        self.validate_ancestry_inner()?;
        let Some(metadata) = entry_metadata_at(&self.directory, name)? else {
            return Ok(false);
        };
        if metadata.kind != DirectoryEntryKind::Directory || !metadata.is_owned_by_current_user() {
            return Err(unsafe_entry("empty child is not an owned directory"));
        }
        match unlinkat(&self.directory, name, AtFlags::REMOVEDIR) {
            Ok(()) => {}
            Err(Errno::NOTEMPTY | Errno::EXIST) => return Ok(false),
            Err(error) => return Err(io::Error::from(error)),
        }
        self.directory.sync_all()?;
        self.validate_ancestry_inner()?;
        Ok(true)
    }

    fn tombstone(&self, name: &OsStr, prefix: &OsStr) -> io::Result<()> {
        for _ in 0..MAX_STAGE_ATTEMPTS {
            let sequence = STAGE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let mut tombstone = prefix.to_os_string();
            tombstone.push(format!("{}-{sequence}", std::process::id()));
            match renameat_with(
                &self.directory,
                name,
                &self.directory,
                &tombstone,
                RenameFlags::NOREPLACE,
            ) {
                Ok(()) => return self.directory.sync_all(),
                Err(Errno::EXIST) => {}
                Err(error) => return Err(io::Error::from(error)),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not allocate a unique removal tombstone",
        ))
    }
}

fn tombstone_prefix(name: &OsStr) -> OsString {
    let mut prefix = OsString::from(TOMBSTONE_PREFIX);
    prefix.push(name);
    prefix.push(".");
    prefix
}
