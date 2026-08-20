use std::ffi::OsStr;
use std::io;
use std::path::Path;
use std::sync::Arc;

use super::publication::durable_publish_directory_with;
use super::traversal::{entry_name, metadata_for_file, remove_directory_tree};
use super::{DirectoryAuthority, PrivateStagingDirectory, STAGING_NAME_PREFIX};

impl PrivateStagingDirectory {
    /// Borrow the staging directory authority for population and validation.
    #[must_use]
    pub fn directory(&self) -> &DirectoryAuthority {
        &self.directory
    }

    /// Recursively remove this exact unpublished staging directory.
    ///
    /// The stored directory handle must still match its private staging name.
    /// Symbolic links, hardlinks, special files, and source-name swaps fail
    /// closed.
    ///
    /// # Errors
    ///
    /// Returns invalid-input when the staging name no longer identifies this
    /// exact directory or contains unsafe members. Returns the operating-system
    /// error when traversal, removal, or durability fails.
    pub fn remove(self) -> io::Result<()> {
        let _operation = self.directory.operation_guard()?;
        self.remove_exact()
    }

    /// Atomically publish this exact staging directory without replacement.
    ///
    /// Publication consumes the private staging capability, proves that its
    /// source name still identifies the held directory handle, performs a
    /// no-replace rename, reopens and proves the destination inode, verifies
    /// the staging name disappeared, and then syncs the parent directory.
    ///
    /// # Errors
    ///
    /// Returns invalid-input for an unsafe destination, held lock entry, source
    /// swap, or failed post-publication identity proof. Returns the
    /// operating-system error when publication or durability fails.
    pub fn publish(self, destination: &Path) -> io::Result<DirectoryAuthority> {
        self.publish_with(
            destination,
            || Ok(()),
            || Ok(()),
            |directory| directory.sync_all(),
        )
    }

    /// Atomically publish this exact staging directory or remove it on error.
    ///
    /// This operation has the same no-replace and post-publication proof
    /// contract as [`Self::publish`]. When publication fails while the exact
    /// staged inode is still recoverable through its private name, the tree is
    /// removed before the error returns. A cleanup failure is combined with
    /// the publication error and fails closed.
    ///
    /// # Errors
    ///
    /// Returns invalid-input for an unsafe destination, held lock entry,
    /// source swap, or failed identity proof. Returns the operating-system
    /// error when publication, recovery, cleanup, or durability fails.
    pub fn publish_or_remove(self, destination: &Path) -> io::Result<DirectoryAuthority> {
        let destination = entry_name(destination, "published directory name")?;
        if self.protected_name.as_deref() == Some(destination) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "refusing to mutate the held directory lock entry",
            ));
        }
        let _operation = self.directory.operation_guard()?;
        match self.publish_inner(
            destination,
            || Ok(()),
            || Ok(()),
            |directory| directory.sync_all(),
        ) {
            Ok(published) => Ok(self.published_authority(published)),
            Err(publication_error) => match self.remove_exact() {
                Ok(()) => Err(publication_error),
                Err(cleanup_error) => Err(io::Error::other(format!(
                    "{publication_error}; exact private staging cleanup failed: {cleanup_error}"
                ))),
            },
        }
    }

    fn publish_with(
        self,
        destination: &Path,
        before_rename: impl FnOnce() -> io::Result<()>,
        after_rename: impl FnOnce() -> io::Result<()>,
        sync: impl FnOnce(&std::fs::File) -> io::Result<()>,
    ) -> io::Result<DirectoryAuthority> {
        let destination = entry_name(destination, "published directory name")?;
        if self.protected_name.as_deref() == Some(destination) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "refusing to mutate the held directory lock entry",
            ));
        }
        let _operation = self.directory.operation_guard()?;
        let published = self.publish_inner(destination, before_rename, after_rename, sync)?;
        Ok(self.published_authority(published))
    }

    fn publish_inner(
        &self,
        destination: &OsStr,
        before_rename: impl FnOnce() -> io::Result<()>,
        after_rename: impl FnOnce() -> io::Result<()>,
        sync: impl FnOnce(&std::fs::File) -> io::Result<()>,
    ) -> io::Result<std::fs::File> {
        let expected = metadata_for_file(&self.directory.directory)?;
        durable_publish_directory_with(
            &self.parent,
            &self.name,
            destination,
            expected,
            before_rename,
            after_rename,
            sync,
        )
    }

    fn published_authority(&self, published: std::fs::File) -> DirectoryAuthority {
        DirectoryAuthority {
            directory: published,
            shared: Arc::clone(&self.directory.shared),
            protected_name: None,
        }
    }

    fn remove_exact(&self) -> io::Result<()> {
        let expected = metadata_for_file(&self.directory.directory)?;
        remove_directory_tree(&self.parent, &self.name, expected)
    }

    #[cfg(test)]
    pub(super) fn publish_or_remove_with(
        self,
        destination: &Path,
        before_rename: impl FnOnce() -> io::Result<()>,
        after_rename: impl FnOnce() -> io::Result<()>,
        sync: impl FnOnce(&std::fs::File) -> io::Result<()>,
    ) -> io::Result<DirectoryAuthority> {
        let destination = entry_name(destination, "published directory name")?;
        let _operation = self.directory.operation_guard()?;
        match self.publish_inner(destination, before_rename, after_rename, sync) {
            Ok(published) => Ok(self.published_authority(published)),
            Err(publication_error) => match self.remove_exact() {
                Ok(()) => Err(publication_error),
                Err(cleanup_error) => Err(io::Error::other(format!(
                    "{publication_error}; exact private staging cleanup failed: {cleanup_error}"
                ))),
            },
        }
    }
}

pub(super) fn validate_staging_name(path: &Path) -> io::Result<&OsStr> {
    let name = entry_name(path, "private staging directory name")?;
    let Some(name) = name.to_str() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "private staging directory name must be ASCII",
        ));
    };
    let suffix = name.strip_prefix(STAGING_NAME_PREFIX).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "private staging directory name has the wrong namespace",
        )
    })?;
    if suffix.is_empty()
        || !suffix
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "private staging directory suffix is invalid",
        ));
    }
    Ok(OsStr::new(name))
}
