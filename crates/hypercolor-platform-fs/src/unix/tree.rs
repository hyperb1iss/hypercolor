use std::ffi::OsString;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

mod authority;
mod publication;
mod replacement;
mod staging;
mod traversal;

pub use replacement::MAX_EXACT_ENTRY_BYTES;
pub(super) use traversal::write_secret_contents;

const SECRET_FILE_MODE: u32 = 0o600;
const PRIVATE_DIRECTORY_MODE: u32 = 0o700;
const PERMISSION_BITS: u32 = 0o777;
const ALL_PERMISSION_BITS: u32 = 0o7777;
const STAGING_NAME_PREFIX: &str = ".hypercolor-stage-";
#[cfg(target_os = "linux")]
const DIRECTORY_BUFFER_BYTES: usize = 16 * 1024;
static SYMLINK_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Exclusive mutation authority for one opened Unix directory.
///
/// Every process mutating entries governed by this capability must acquire the
/// same `lock_name`. Operations stay relative to the opened directory handle,
/// so renaming or replacing the pathname used to acquire it cannot redirect a
/// later mutation or durability barrier.
#[derive(Debug)]
pub struct ExclusiveDirectory {
    pub(super) shared: Arc<ExclusiveDirectoryShared>,
}

#[derive(Debug)]
pub(super) struct ExclusiveDirectoryShared {
    pub(super) directory: File,
    pub(super) _lock: File,
    pub(super) lock_name: OsString,
    pub(super) operation: Mutex<()>,
}

/// Handle-relative authority for one opened Unix directory.
///
/// Authorities originate from an [`ExclusiveDirectory`] and keep its lock and
/// operation gate alive. Child traversal, inspection, and mutation never
/// reopen the pathname that originally named the install root.
#[derive(Debug)]
pub struct DirectoryAuthority {
    pub(super) directory: File,
    pub(super) shared: Arc<ExclusiveDirectoryShared>,
    pub(super) protected_name: Option<OsString>,
}

/// An ancestry-anchored mutation authority for one absolute public directory.
///
/// The authority retains the global [`ExclusiveDirectory`] lock without
/// creating another lock file in the public directory. Every mutation proves
/// that each opened ancestor still occupies its original absolute pathname.
#[derive(Debug)]
pub struct PublicDirectoryAuthority {
    pub(super) directory: File,
    pub(super) ancestry: Vec<DirectoryAnchor>,
    pub(super) shared: Arc<ExclusiveDirectoryShared>,
}

#[derive(Debug)]
pub(super) struct DirectoryAnchor {
    pub(super) parent: File,
    pub(super) name: OsString,
    pub(super) expected: DirectoryEntryMetadata,
}

/// Read-only handle-relative authority for one opened Unix directory.
///
/// This capability never creates a lock entry and exposes no mutation
/// operations. Child traversal remains relative to the original no-follow
/// directory handle even if the pathname used to open it is later replaced.
#[derive(Debug)]
pub struct ReadOnlyDirectoryAuthority {
    pub(super) directory: File,
}

/// Exact supported state of one public directory entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExactEntry {
    /// No entry exists under the governed name.
    Absent,
    /// One single-link regular file with exact bytes and ordinary mode.
    RegularFile {
        /// Exact ordinary permission bits.
        mode: u32,
        /// Exact byte length.
        size: u64,
        /// SHA-256 of the exact opened file contents.
        sha256: [u8; 32],
        /// Filesystem device identity used for conditional mutation.
        device: u64,
        /// Filesystem inode identity used for conditional mutation.
        inode: u64,
    },
    /// One symbolic link with its exact uninterpreted target.
    Symlink {
        /// Exact target bytes represented as a platform path.
        target: PathBuf,
        /// Filesystem device identity used for conditional mutation.
        device: u64,
        /// Filesystem inode identity used for conditional mutation.
        inode: u64,
    },
}

/// Content to publish as one exact public directory entry.
#[derive(Debug, Clone, Copy)]
pub enum EntryReplacement<'a> {
    /// A regular file with exact bytes and ordinary mode.
    RegularFile {
        /// Exact ordinary permission bits for the published file.
        mode: u32,
        /// Complete file contents.
        contents: &'a [u8],
    },
    /// A stable symbolic link target.
    Symlink {
        /// Absolute target or relative target using normal and parent
        /// components. The target is stored without dereferencing it.
        target: &'a Path,
    },
}

/// One opened regular file paired with metadata from the same file handle.
#[derive(Debug)]
pub struct OpenedRegularFile {
    pub(super) file: File,
    pub(super) metadata: DirectoryEntryMetadata,
}

impl OpenedRegularFile {
    /// Return metadata obtained with `fstat` from this exact open file.
    #[must_use]
    pub fn metadata(&self) -> DirectoryEntryMetadata {
        self.metadata
    }

    /// Borrow the opened file.
    #[must_use]
    pub fn file(&self) -> &File {
        &self.file
    }

    /// Mutably borrow the opened file for streaming reads.
    pub fn file_mut(&mut self) -> &mut File {
        &mut self.file
    }

    /// Consume the proof wrapper and return the opened file.
    #[must_use]
    pub fn into_file(self) -> File {
        self.file
    }
}

/// Capability for one unpublished private staging directory.
///
/// Only [`DirectoryAuthority::create_private_staging_directory`] constructs
/// this type. Cleanup and publication consume the capability, so an arbitrary
/// or already-published immutable unit cannot be targeted by those operations.
#[derive(Debug)]
pub struct PrivateStagingDirectory {
    pub(super) parent: File,
    pub(super) name: OsString,
    pub(super) directory: DirectoryAuthority,
    pub(super) protected_name: Option<OsString>,
}

/// The no-follow type of one directory entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectoryEntryKind {
    /// A regular file.
    RegularFile,
    /// A directory.
    Directory,
    /// A symbolic link.
    SymbolicLink,
    /// A socket, device, FIFO, or another unsupported special entry.
    Special,
}

/// No-follow metadata for one entry beneath a [`DirectoryAuthority`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirectoryEntryMetadata {
    pub(super) kind: DirectoryEntryKind,
    pub(super) mode: u32,
    pub(super) size: u64,
    pub(super) link_count: u64,
    pub(super) device: u64,
    pub(super) inode: u64,
}

impl DirectoryEntryMetadata {
    /// Return the entry type.
    #[must_use]
    pub fn kind(self) -> DirectoryEntryKind {
        self.kind
    }

    /// Return all permission bits, including set-ID and sticky bits.
    #[must_use]
    pub fn mode(self) -> u32 {
        self.mode
    }

    /// Return the entry size in bytes.
    #[must_use]
    pub fn size(self) -> u64 {
        self.size
    }

    /// Return the filesystem link count.
    #[must_use]
    pub fn link_count(self) -> u64 {
        self.link_count
    }

    /// Return the filesystem device number.
    #[must_use]
    pub fn device(self) -> u64 {
        self.device
    }

    /// Return the inode number.
    #[must_use]
    pub fn inode(self) -> u64 {
        self.inode
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::ffi::OsStr;
    use std::fs::{self, File};
    use std::io;
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use super::publication::durable_publish_directory_with;
    use super::publication::{PublicationRecoverySlot, RECOVERY_NAME_PREFIX};
    use super::traversal::{metadata_for_file, open_directory_at};
    use rustix::fs::Mode;

    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    fn recovery_entries(root: &Path) -> Vec<PathBuf> {
        fs::read_dir(root)
            .expect("enumerate test root")
            .map(|entry| entry.expect("read test root entry"))
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(RECOVERY_NAME_PREFIX)
            })
            .map(|entry| entry.path())
            .collect()
    }

    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    #[derive(Clone, Copy, Debug)]
    enum RecoveryPlaceholderDrift {
        Hardlink,
        Mode,
        Size,
    }

    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    fn mutate_recovery_placeholder(root: &Path, drift: RecoveryPlaceholderDrift) -> PathBuf {
        let placeholder = recovery_entries(root)
            .into_iter()
            .next()
            .expect("reserved recovery placeholder");
        match drift {
            RecoveryPlaceholderDrift::Hardlink => {
                fs::hard_link(&placeholder, root.join("attacker-hardlink"))
                    .expect("hardlink recovery placeholder");
            }
            RecoveryPlaceholderDrift::Mode => {
                fs::set_permissions(&placeholder, fs::Permissions::from_mode(0o644))
                    .expect("chmod recovery placeholder");
            }
            RecoveryPlaceholderDrift::Size => {
                fs::write(&placeholder, b"attacker data").expect("write recovery placeholder");
            }
        }
        placeholder
    }

    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    #[test]
    fn publish_or_remove_cleans_staging_after_previsibility_failure() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let root =
            super::ExclusiveDirectory::try_acquire(directory.path(), Path::new("install.lock"))
                .expect("acquire root")
                .expect("uncontended root")
                .root_directory()
                .expect("root authority");
        let staging = root
            .create_private_staging_directory(Path::new(".hypercolor-stage-fail-before"))
            .expect("create staging");
        staging
            .publish_or_remove_with(
                Path::new("unit"),
                || Err(io::Error::other("injected previsibility failure")),
                || Ok(()),
                |directory| directory.sync_all(),
            )
            .expect_err("injected publication failure");

        assert!(!directory.path().join("unit").exists());
        assert!(
            !directory
                .path()
                .join(".hypercolor-stage-fail-before")
                .exists()
        );
        assert!(recovery_entries(directory.path()).is_empty());
    }

    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    #[test]
    fn publish_or_remove_cleans_rolled_back_staging_after_sync_failure() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let root =
            super::ExclusiveDirectory::try_acquire(directory.path(), Path::new("install.lock"))
                .expect("acquire root")
                .expect("uncontended root")
                .root_directory()
                .expect("root authority");
        let staging = root
            .create_private_staging_directory(Path::new(".hypercolor-stage-fail-sync"))
            .expect("create staging");
        staging
            .publish_or_remove_with(
                Path::new("unit"),
                || Ok(()),
                || Ok(()),
                |_| Err(io::Error::other("injected parent sync failure")),
            )
            .expect_err("injected sync failure");

        assert!(!directory.path().join("unit").exists());
        assert!(
            !directory
                .path()
                .join(".hypercolor-stage-fail-sync")
                .exists()
        );
        assert!(recovery_entries(directory.path()).is_empty());
    }

    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    fn assert_recovery_drift_survives(
        root: &Path,
        placeholder: &Path,
        drift: RecoveryPlaceholderDrift,
    ) {
        let metadata = fs::metadata(placeholder).expect("inspect drifted recovery placeholder");
        match drift {
            RecoveryPlaceholderDrift::Hardlink => {
                assert_eq!(metadata.nlink(), 2);
                assert!(root.join("attacker-hardlink").exists());
            }
            RecoveryPlaceholderDrift::Mode => assert_eq!(metadata.mode() & 0o7777, 0o644),
            RecoveryPlaceholderDrift::Size => {
                assert_eq!(
                    fs::read(placeholder).expect("read attacker data"),
                    b"attacker data"
                );
            }
        }
    }

    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    #[test]
    fn recovery_reservation_cleans_owned_slot_after_post_create_failure() {
        let root = tempfile::tempdir().expect("temporary directory");
        let parent = File::open(root.path()).expect("open parent directory");

        let error = PublicationRecoverySlot::reserve_with(
            &parent,
            || Ok(()),
            || Err(io::Error::other("injected post-create reservation failure")),
        )
        .expect_err("injected reservation failure must propagate");

        assert_eq!(
            error.to_string(),
            "injected post-create reservation failure"
        );
        assert!(recovery_entries(root.path()).is_empty());
    }

    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    #[test]
    fn recovery_reservation_cleans_owned_slot_after_injected_fstat_failure() {
        let root = tempfile::tempdir().expect("temporary directory");
        let parent = File::open(root.path()).expect("open parent directory");

        let error = PublicationRecoverySlot::reserve_with(
            &parent,
            || {
                Err(io::Error::other(
                    "injected recovery placeholder fstat failure",
                ))
            },
            || Ok(()),
        )
        .expect_err("injected fstat failure must propagate");

        assert_eq!(
            error.to_string(),
            "injected recovery placeholder fstat failure"
        );
        assert!(recovery_entries(root.path()).is_empty());
    }

    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    #[test]
    fn recovery_reservation_survives_restrictive_umask() {
        const CHILD_ENV: &str = "HYPERCOLOR_RECOVERY_UMASK_CHILD";
        if std::env::var_os(CHILD_ENV).is_some() {
            let root = tempfile::tempdir().expect("temporary directory");
            fs::create_dir(root.path().join("staged")).expect("create staged directory");
            let parent = File::open(root.path()).expect("open parent directory");
            let staged = open_directory_at(&parent, OsStr::new("staged")).expect("open staged");
            let expected = metadata_for_file(&staged).expect("inspect staged");
            let prior_umask = rustix::process::umask(Mode::from_raw_mode(0o777));
            let result = durable_publish_directory_with(
                &parent,
                OsStr::new("staged"),
                OsStr::new("published"),
                expected,
                || Ok(()),
                || Ok(()),
                |_| Ok(()),
            );
            rustix::process::umask(prior_umask);
            result.expect("publish with restrictive umask");
            assert!(root.path().join("published").is_dir());
            assert!(recovery_entries(root.path()).is_empty());
            return;
        }

        let output = Command::new(std::env::current_exe().expect("current test executable"))
            .args([
                "--exact",
                "unix::tree::tests::recovery_reservation_survives_restrictive_umask",
                "--nocapture",
            ])
            .env(CHILD_ENV, "1")
            .output()
            .expect("run isolated restrictive-umask test");
        assert!(
            output.status.success(),
            "restrictive-umask child failed:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    #[test]
    fn recovery_reservation_cleans_owned_slot_after_injected_fchmod_failure() {
        let root = tempfile::tempdir().expect("temporary directory");
        let parent = File::open(root.path()).expect("open parent directory");

        PublicationRecoverySlot::reserve_with_steps(
            &parent,
            || Ok(()),
            || {
                Err(io::Error::other(
                    "injected recovery placeholder fchmod failure",
                ))
            },
            || Ok(()),
        )
        .expect_err("injected fchmod failure must propagate");

        assert!(recovery_entries(root.path()).is_empty());
    }

    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    #[test]
    fn recovery_reservation_leaves_renamed_create_time_entry_untouched() {
        let root = tempfile::tempdir().expect("temporary directory");
        let parent = File::open(root.path()).expect("open parent directory");
        let displaced = root.path().join("attacker-displaced");

        PublicationRecoverySlot::reserve_with_steps(
            &parent,
            || Ok(()),
            || {
                let placeholder = recovery_entries(root.path())
                    .into_iter()
                    .next()
                    .expect("reserved recovery placeholder");
                fs::rename(placeholder, &displaced)?;
                Err(io::Error::other(
                    "injected recovery placeholder name failure",
                ))
            },
            || Ok(()),
        )
        .expect_err("injected name failure must propagate");

        assert!(recovery_entries(root.path()).is_empty());
        assert!(displaced.is_file());
    }

    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    #[test]
    fn recovery_reservation_refuses_drifted_placeholder_cleanup() {
        for drift in [
            RecoveryPlaceholderDrift::Hardlink,
            RecoveryPlaceholderDrift::Mode,
            RecoveryPlaceholderDrift::Size,
        ] {
            let root = tempfile::tempdir().expect("temporary directory");
            let parent = File::open(root.path()).expect("open parent directory");
            let placeholder = RefCell::new(None);

            PublicationRecoverySlot::reserve_with(
                &parent,
                || Ok(()),
                || {
                    placeholder.replace(Some(mutate_recovery_placeholder(root.path(), drift)));
                    Err(io::Error::other("injected cleanup after metadata drift"))
                },
            )
            .expect_err("drifted reservation cleanup must fail closed");

            let placeholder = placeholder
                .into_inner()
                .expect("drifted recovery placeholder path");
            assert_recovery_drift_survives(root.path(), &placeholder, drift);
        }
    }

    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    #[test]
    fn publication_refuses_steady_state_placeholder_drift() {
        for drift in [
            RecoveryPlaceholderDrift::Hardlink,
            RecoveryPlaceholderDrift::Mode,
            RecoveryPlaceholderDrift::Size,
        ] {
            let root = tempfile::tempdir().expect("temporary directory");
            fs::create_dir(root.path().join("staged")).expect("create staged directory");
            let parent = File::open(root.path()).expect("open parent directory");
            let staged = open_directory_at(&parent, OsStr::new("staged")).expect("open staged");
            let expected = metadata_for_file(&staged).expect("inspect staged");
            let placeholder = RefCell::new(None);

            durable_publish_directory_with(
                &parent,
                OsStr::new("staged"),
                OsStr::new("published"),
                expected,
                || {
                    placeholder.replace(Some(mutate_recovery_placeholder(root.path(), drift)));
                    Ok(())
                },
                || Ok(()),
                |_| Ok(()),
            )
            .expect_err("steady-state placeholder drift must fail closed");

            assert!(!root.path().join("published").exists());
            assert!(root.path().join("staged").is_dir());
            let placeholder = placeholder
                .into_inner()
                .expect("drifted recovery placeholder path");
            assert_recovery_drift_survives(root.path(), &placeholder, drift);
        }
    }

    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    #[test]
    fn directory_publication_sync_runs_after_no_replace_rename() {
        let root = tempfile::tempdir().expect("temporary directory");
        fs::create_dir(root.path().join("staged")).expect("create staged directory");
        let parent = File::open(root.path()).expect("open parent directory");
        let staged = open_directory_at(&parent, OsStr::new("staged")).expect("open staged");
        let expected = metadata_for_file(&staged).expect("inspect staged");
        let sync_called = Cell::new(false);

        durable_publish_directory_with(
            &parent,
            OsStr::new("staged"),
            OsStr::new("published"),
            expected,
            || Ok(()),
            || Ok(()),
            |directory| {
                assert!(directory.metadata()?.is_dir());
                assert!(root.path().join("published").is_dir());
                assert!(!root.path().join("staged").exists());
                sync_called.set(true);
                Ok(())
            },
        )
        .expect("publish and sync directory");

        assert!(sync_called.get());
        assert!(recovery_entries(root.path()).is_empty());
    }

    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    #[test]
    fn directory_publication_rolls_back_a_post_rename_inode_swap() {
        let root = tempfile::tempdir().expect("temporary directory");
        fs::create_dir(root.path().join("staged")).expect("create staged directory");
        let parent = File::open(root.path()).expect("open parent directory");
        let staged = open_directory_at(&parent, OsStr::new("staged")).expect("open staged");
        let expected = metadata_for_file(&staged).expect("inspect staged");

        durable_publish_directory_with(
            &parent,
            OsStr::new("staged"),
            OsStr::new("published"),
            expected,
            || Ok(()),
            || {
                fs::rename(root.path().join("published"), root.path().join("displaced"))?;
                fs::create_dir(root.path().join("published"))
            },
            |_| Ok(()),
        )
        .expect_err("published inode substitution must fail closed");

        assert!(!root.path().join("published").exists());
        assert!(root.path().join("displaced").is_dir());
        assert!(root.path().join("staged").is_dir());
        let displaced = fs::metadata(root.path().join("displaced")).expect("inspect displaced");
        let staged = fs::metadata(root.path().join("staged")).expect("inspect rollback entry");
        assert_ne!(
            std::os::unix::fs::MetadataExt::ino(&displaced),
            std::os::unix::fs::MetadataExt::ino(&staged)
        );
        assert!(recovery_entries(root.path()).is_empty());
    }

    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    #[test]
    fn directory_publication_uses_reserved_slot_when_fresh_names_are_occupied() {
        let root = tempfile::tempdir().expect("temporary directory");
        fs::create_dir(root.path().join("staged")).expect("create staged directory");
        let parent = File::open(root.path()).expect("open parent directory");
        let staged = open_directory_at(&parent, OsStr::new("staged")).expect("open staged");
        let expected = metadata_for_file(&staged).expect("inspect staged");
        let collision_entries = RefCell::new(Vec::new());

        durable_publish_directory_with(
            &parent,
            OsStr::new("staged"),
            OsStr::new("published"),
            expected,
            || {
                for sequence in 0..256 {
                    let collision = root.path().join(format!(
                        "{RECOVERY_NAME_PREFIX}{}-{sequence}",
                        std::process::id()
                    ));
                    match fs::create_dir(&collision) {
                        Ok(()) => {
                            fs::write(collision.join("unrelated"), b"unrelated recovery entry")?;
                            collision_entries.borrow_mut().push(collision);
                        }
                        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                        Err(error) => return Err(error),
                    }
                }
                Ok(())
            },
            || {
                fs::rename(root.path().join("published"), root.path().join("displaced"))?;
                fs::create_dir(root.path().join("published"))?;
                fs::write(
                    root.path().join("published/attacker"),
                    b"published attacker",
                )?;
                fs::create_dir(root.path().join("staged"))?;
                fs::write(root.path().join("staged/attacker"), b"staging attacker")
            },
            |_| Ok(()),
        )
        .expect_err("occupied staging must force quarantine recovery");

        assert!(!root.path().join("published").exists());
        assert_eq!(
            fs::read(root.path().join("staged/attacker")).expect("read staging attacker"),
            b"staging attacker"
        );
        let displaced = fs::metadata(root.path().join("displaced")).expect("inspect displaced");
        assert_eq!(
            std::os::unix::fs::MetadataExt::dev(&displaced),
            expected.device
        );
        assert_eq!(
            std::os::unix::fs::MetadataExt::ino(&displaced),
            expected.inode
        );
        let collision_entries = collision_entries.into_inner();
        assert!(collision_entries.len() >= 128);
        for collision in collision_entries {
            assert_eq!(
                fs::read(collision.join("unrelated")).expect("read unrelated recovery entry"),
                b"unrelated recovery entry"
            );
        }
        let quarantine = recovery_entries(root.path())
            .into_iter()
            .find(|entry| entry.join("attacker").is_file())
            .expect("unverified destination quarantine");
        assert_eq!(
            fs::read(quarantine.join("attacker")).expect("read quarantined attacker"),
            b"published attacker"
        );
    }

    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    #[test]
    fn directory_publication_sync_failure_rolls_back_visibility() {
        let root = tempfile::tempdir().expect("temporary directory");
        fs::create_dir(root.path().join("staged")).expect("create staged directory");
        let parent = File::open(root.path()).expect("open parent directory");
        let staged = open_directory_at(&parent, OsStr::new("staged")).expect("open staged");
        let expected = metadata_for_file(&staged).expect("inspect staged");

        let error = durable_publish_directory_with(
            &parent,
            OsStr::new("staged"),
            OsStr::new("published"),
            expected,
            || Ok(()),
            || Ok(()),
            |_| Err(io::Error::other("injected publication sync failure")),
        )
        .expect_err("parent sync failure must roll back visibility");

        assert_eq!(error.to_string(), "injected publication sync failure");
        assert!(!root.path().join("published").exists());
        let restored = fs::metadata(root.path().join("staged")).expect("inspect restored staging");
        assert_eq!(
            std::os::unix::fs::MetadataExt::dev(&restored),
            expected.device
        );
        assert_eq!(
            std::os::unix::fs::MetadataExt::ino(&restored),
            expected.inode
        );
        assert!(recovery_entries(root.path()).is_empty());
    }

    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    #[test]
    fn directory_publication_rolls_back_a_pre_rename_source_swap() {
        let root = tempfile::tempdir().expect("temporary directory");
        fs::create_dir(root.path().join("staged")).expect("create staged directory");
        let parent = File::open(root.path()).expect("open parent directory");
        let staged = open_directory_at(&parent, OsStr::new("staged")).expect("open staged");
        let expected = metadata_for_file(&staged).expect("inspect staged");

        durable_publish_directory_with(
            &parent,
            OsStr::new("staged"),
            OsStr::new("published"),
            expected,
            || {
                fs::rename(root.path().join("staged"), root.path().join("displaced"))?;
                fs::create_dir(root.path().join("staged"))
            },
            || Ok(()),
            |_| Ok(()),
        )
        .expect_err("source inode substitution must fail closed");

        assert!(!root.path().join("published").exists());
        assert!(root.path().join("displaced").is_dir());
        assert!(root.path().join("staged").is_dir());
        let displaced = fs::metadata(root.path().join("displaced")).expect("inspect displaced");
        let staged = fs::metadata(root.path().join("staged")).expect("inspect rollback entry");
        assert_ne!(
            std::os::unix::fs::MetadataExt::ino(&displaced),
            std::os::unix::fs::MetadataExt::ino(&staged)
        );
        assert!(recovery_entries(root.path()).is_empty());
    }
}
