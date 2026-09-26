use std::io;
use std::path::Path;

use super::super::traversal::{
    entry_metadata_at, entry_name, remove_owned_directory_tree, unsafe_entry,
};
use super::super::{DirectoryEntryKind, PublicDirectoryAuthority};

impl PublicDirectoryAuthority {
    /// Durably remove one owned child directory tree, if present.
    ///
    /// The child and every entry beneath it must belong to the effective user.
    /// Symbolic links are unlinked and never followed; special files and
    /// multiply linked regular files are refused. Read-only directories are
    /// made owner-writable only as they are emptied. Ancestry is proven before
    /// and after removal and the parent is synced.
    ///
    /// An interrupted removal leaves a smaller owned tree; calling this again
    /// continues from what remains. Returns `false` when the child is absent.
    ///
    /// # Errors
    ///
    /// Returns invalid-input for an unsafe name, a non-directory child, or a
    /// refused entry. Returns an error when ancestry, identity, removal or
    /// durability fails.
    pub fn durable_remove_child_tree(&self, name: &Path) -> io::Result<bool> {
        let name = entry_name(name, "owned tree name")?;
        let _operation = self.operation_guard()?;
        self.validate_ancestry_inner()?;
        let Some(metadata) = entry_metadata_at(&self.directory, name)? else {
            return Ok(false);
        };
        if metadata.kind != DirectoryEntryKind::Directory {
            return Err(unsafe_entry("owned tree root is not a directory"));
        }
        remove_owned_directory_tree(&self.directory, name, metadata)?;
        self.validate_ancestry_inner()?;
        Ok(true)
    }
}
