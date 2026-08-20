use std::ffi::OsString;
use std::fs::File;
use std::io;

use super::super::traversal::bounded_directory_entries;
use super::super::{DirectoryAuthority, PublicDirectoryAuthority};

/// Maximum number of normal child names returned by bounded enumeration.
pub const MAX_PUBLIC_DIRECTORY_CHILD_COUNT: usize = 1024;

/// Maximum aggregate encoded bytes across bounded enumeration results.
pub const MAX_PUBLIC_DIRECTORY_CHILD_NAMES_BYTES: usize = 64 * 1024;

impl DirectoryAuthority {
    /// Enumerate a bounded, byte-sorted snapshot of direct child names.
    ///
    /// Names are returned as platform strings so callers can pass them to
    /// handle-relative observation and open operations without reopening the
    /// directory pathname. This operation does not recurse or inspect child
    /// entry kinds.
    ///
    /// # Errors
    ///
    /// Returns an error when a name is not one normal path component, the
    /// count or aggregate-name-byte bound is exceeded, the two directory scans
    /// disagree, the shared operation gate is poisoned, or enumeration fails.
    pub fn child_names(&self) -> io::Result<Vec<OsString>> {
        self.child_names_with(|| Ok(()))
    }

    pub(super) fn child_names_with(
        &self,
        after_first_scan: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<Vec<OsString>> {
        let _operation = self.operation_guard()?;
        confirmed_child_names(
            &self.directory,
            after_first_scan,
            "directory child names changed during enumeration",
        )
    }
}

impl PublicDirectoryAuthority {
    /// Enumerate a bounded, byte-sorted snapshot of direct child names.
    ///
    /// Names are returned as platform strings so callers can pass them to
    /// handle-relative observation and open operations without reopening the
    /// public directory pathname. This operation does not recurse or inspect
    /// child entry kinds.
    ///
    /// # Errors
    ///
    /// Returns an error when ancestry changes, a name is not one normal path
    /// component, the count or aggregate-name-byte bound is exceeded, the two
    /// directory scans disagree, or directory enumeration fails.
    pub fn child_names(&self) -> io::Result<Vec<OsString>> {
        self.child_names_with(|| Ok(()))
    }

    pub(super) fn child_names_with(
        &self,
        after_first_scan: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<Vec<OsString>> {
        let _operation = self.operation_guard()?;
        self.validate_ancestry_inner()?;
        let names = confirmed_child_names(
            &self.directory,
            after_first_scan,
            "public directory child names changed during enumeration",
        )?;
        self.validate_ancestry_inner()?;
        Ok(names)
    }
}

fn confirmed_child_names(
    directory: &File,
    after_first_scan: impl FnOnce() -> io::Result<()>,
    changed_message: &'static str,
) -> io::Result<Vec<OsString>> {
    let names = bounded_directory_entries(
        directory,
        MAX_PUBLIC_DIRECTORY_CHILD_COUNT,
        MAX_PUBLIC_DIRECTORY_CHILD_NAMES_BYTES,
    )?;
    after_first_scan()?;
    let confirmed = bounded_directory_entries(
        directory,
        MAX_PUBLIC_DIRECTORY_CHILD_COUNT,
        MAX_PUBLIC_DIRECTORY_CHILD_NAMES_BYTES,
    )?;
    if names != confirmed {
        return Err(io::Error::new(io::ErrorKind::InvalidData, changed_message));
    }
    Ok(names)
}
