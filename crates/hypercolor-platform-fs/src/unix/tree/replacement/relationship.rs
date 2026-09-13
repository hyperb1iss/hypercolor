use std::io;
use std::sync::Arc;

use super::super::PublicDirectoryAuthority;
use super::super::traversal::metadata_for_file;

impl PublicDirectoryAuthority {
    /// Test whether this directory is the other directory or lies below it.
    ///
    /// Comparison uses retained device/inode identities throughout the original
    /// ancestry, so distinct path spellings do not hide an aliased ancestor.
    /// Both authorities must share the same exclusive operation gate.
    ///
    /// # Errors
    /// Returns an error for different gates, changed ancestry or failed handle
    /// inspection. A failed comparison must not be treated as disjointness.
    pub fn is_within(&self, ancestor: &Self) -> io::Result<bool> {
        if !Arc::ptr_eq(&self.shared, &ancestor.shared) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "directory relationship requires one exclusive authority",
            ));
        }
        let _operation = self.operation_guard()?;
        self.validate_ancestry_inner()?;
        ancestor.validate_ancestry_inner()?;
        let expected = metadata_for_file(&ancestor.directory)?;
        let contained = self.ancestry.iter().any(|entry| {
            entry.expected.device() == expected.device()
                && entry.expected.inode() == expected.inode()
        });
        self.validate_ancestry_inner()?;
        ancestor.validate_ancestry_inner()?;
        Ok(contained)
    }
}
