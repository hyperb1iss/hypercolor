use std::io;
use std::path::Path;

use super::super::traversal::{
    entry_metadata_at, entry_name, metadata_for_file, open_regular_file_at, unsafe_entry,
};
use super::super::{
    DirectoryEntryKind, DirectoryEntryMetadata, OpenedRegularFile, PublicDirectoryAuthority,
};
use super::exact::require_exact_metadata;

impl PublicDirectoryAuthority {
    /// Open one public regular file through this anchored authority.
    ///
    /// The returned file is opened without following symbolic links. Its
    /// metadata comes from the same retained file handle, and acquisition
    /// proves that the public name denoted that handle before returning.
    /// Callers can snapshot the retained file without reopening its pathname.
    ///
    /// # Errors
    ///
    /// Returns invalid-input for an unsafe name, non-regular entry, hardlink,
    /// or special permission bits. Returns an error when ancestry or entry
    /// identity changes during acquisition.
    pub fn open_regular_file(&self, name: &Path) -> io::Result<OpenedRegularFile> {
        self.open_regular_file_with(name, || Ok(()), || Ok(()))
    }

    pub(super) fn open_regular_file_with(
        &self,
        name: &Path,
        after_observation: impl FnOnce() -> io::Result<()>,
        after_open: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<OpenedRegularFile> {
        let name = entry_name(name, "public regular file name")?;
        let _operation = self.operation_guard()?;
        self.validate_ancestry_inner()?;
        let named_before = entry_metadata_at(&self.directory, name)?.ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "public regular file is missing")
        })?;
        require_public_regular_metadata(named_before)?;
        after_observation()?;

        let opened = open_regular_file_at(&self.directory, name)?;
        require_exact_metadata(
            named_before,
            opened.metadata,
            "public regular file changed before opening",
        )?;
        after_open()?;

        let handle_after = metadata_for_file(opened.file())?;
        require_exact_metadata(
            opened.metadata,
            handle_after,
            "public regular file handle metadata changed during acquisition",
        )?;
        let named_after = entry_metadata_at(&self.directory, name)?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "public regular file disappeared during acquisition",
            )
        })?;
        require_exact_metadata(
            handle_after,
            named_after,
            "public regular file name changed during acquisition",
        )?;
        self.validate_ancestry_inner()?;
        Ok(opened)
    }
}

fn require_public_regular_metadata(metadata: DirectoryEntryMetadata) -> io::Result<()> {
    if metadata.kind != DirectoryEntryKind::RegularFile
        || metadata.link_count != 1
        || metadata.mode & !0o777 != 0
    {
        return Err(unsafe_entry(
            "public read entry is not a single-link regular file with ordinary permission bits",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io;
    use std::os::unix::fs::{FileTypeExt as _, PermissionsExt as _};
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use crate::unix::tree::{ExclusiveDirectory, PublicDirectoryAuthority};

    struct Fixture {
        _temporary: tempfile::TempDir,
        lock_root: PathBuf,
        public: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let temporary = tempfile::tempdir().expect("temporary directory");
            let canonical = fs::canonicalize(temporary.path()).expect("canonical temporary root");
            let lock_root = canonical.join("lock");
            let public = canonical.join("public");
            fs::create_dir(&lock_root).expect("create lock root");
            fs::create_dir(&public).expect("create public root");
            Self {
                _temporary: temporary,
                lock_root,
                public,
            }
        }

        fn authority(&self) -> (ExclusiveDirectory, PublicDirectoryAuthority) {
            let lock = ExclusiveDirectory::try_acquire(&self.lock_root, Path::new("install.lock"))
                .expect("acquire lock")
                .expect("uncontended lock");
            let authority = lock
                .open_public_directory(&self.public)
                .expect("open public directory");
            (lock, authority)
        }
    }

    fn write_mode(path: &Path, bytes: &[u8], mode: u32) {
        fs::write(path, bytes).expect("write fixture file");
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("set fixture mode");
    }

    #[test]
    fn detects_parent_replacement_at_each_acquisition_phase() {
        for replace_after_open in [false, true] {
            let fixture = Fixture::new();
            write_mode(&fixture.public.join("entry"), b"trusted", 0o644);
            let (_lock, authority) = fixture.authority();
            let detached = fixture.public.with_extension("detached");
            let replace_parent = || {
                fs::rename(&fixture.public, &detached)?;
                fs::create_dir(&fixture.public)
            };

            authority
                .open_regular_file_with(
                    Path::new("entry"),
                    || {
                        if replace_after_open {
                            Ok(())
                        } else {
                            replace_parent()
                        }
                    },
                    || {
                        if replace_after_open {
                            replace_parent()
                        } else {
                            Ok(())
                        }
                    },
                )
                .expect_err("renamed public parent must fail anchored acquisition");

            assert!(!fixture.public.join("entry").exists());
            assert_eq!(
                fs::read(detached.join("entry")).expect("read detached trusted entry"),
                b"trusted"
            );
        }
    }

    #[test]
    fn detects_entry_replacement_at_each_acquisition_phase() {
        for replace_after_open in [false, true] {
            let fixture = Fixture::new();
            let entry = fixture.public.join("entry");
            let displaced = fixture.public.join("displaced-entry");
            write_mode(&entry, b"trusted!", 0o644);
            let (_lock, authority) = fixture.authority();
            let replace_entry = || {
                fs::rename(&entry, &displaced)?;
                write_mode(&entry, b"attacker", 0o644);
                Ok(())
            };

            authority
                .open_regular_file_with(
                    Path::new("entry"),
                    || {
                        if replace_after_open {
                            Ok(())
                        } else {
                            replace_entry()
                        }
                    },
                    || {
                        if replace_after_open {
                            replace_entry()
                        } else {
                            Ok(())
                        }
                    },
                )
                .expect_err("replaced public entry must fail handle acquisition");

            assert_eq!(fs::read(&entry).expect("read attacker entry"), b"attacker");
            assert_eq!(
                fs::read(&displaced).expect("read displaced trusted entry"),
                b"trusted!"
            );
        }
    }

    #[test]
    fn rejects_fifo_replacement_without_blocking() {
        let fixture = Fixture::new();
        let entry = fixture.public.join("entry");
        write_mode(&entry, b"trusted", 0o644);
        let (_lock, authority) = fixture.authority();

        authority
            .open_regular_file_with(
                Path::new("entry"),
                || {
                    fs::remove_file(&entry)?;
                    let status = Command::new("mkfifo").arg(&entry).status()?;
                    if status.success() {
                        Ok(())
                    } else {
                        Err(io::Error::other("mkfifo fixture command failed"))
                    }
                },
                || Ok(()),
            )
            .expect_err("FIFO replacement must be rejected without blocking");

        assert!(
            fs::symlink_metadata(entry)
                .expect("inspect FIFO")
                .file_type()
                .is_fifo()
        );
    }
}
