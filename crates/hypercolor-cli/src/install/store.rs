use std::fs;
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use hypercolor_platform_fs::{
    DirectoryAuthority, DirectoryEntryKind, DirectoryEntryMetadata, ExclusiveDirectory,
    PublicDirectoryAuthority, ReadOnlyDirectoryAuthority,
};

use super::model::{InstallJournalV1, UnitId, active_target};

const INSTALL_LOCK_FILE: &str = "install.lock";
const ANCHORED_INSTALL_LOCK_FILE: &str = ".hypercolor-release-install.lock";
const INSTALL_JOURNAL_FILE: &str = "install-journal.json";
const UNITS_DIRECTORY: &str = "units";
const MAX_JOURNAL_STAGE_ATTEMPTS: usize = 128;
static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone)]
pub struct InstallStore {
    root: PathBuf,
    max_journal_bytes: usize,
}

impl InstallStore {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>, max_journal_bytes: usize) -> Self {
        Self {
            root: root.into(),
            max_journal_bytes,
        }
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn journal_path(&self) -> PathBuf {
        self.root.join(INSTALL_JOURNAL_FILE)
    }

    #[must_use]
    pub fn lock_path(&self) -> PathBuf {
        self.root.join(INSTALL_LOCK_FILE)
    }

    #[must_use]
    pub fn active_path(&self) -> PathBuf {
        self.root.join("active")
    }

    /// Return the canonical path of one digest-named immutable unit.
    #[must_use]
    pub fn unit_path(&self, unit: &UnitId) -> PathBuf {
        self.root.join(UNITS_DIRECTORY).join(unit.as_str())
    }

    pub fn acquire_lock(&self) -> Result<InstallLock, InstallStoreError> {
        fs::create_dir_all(&self.root).map_err(InstallStoreError::CreateRoot)?;
        let gate = ExclusiveDirectory::try_acquire(&self.root, Path::new(INSTALL_LOCK_FILE))
            .map_err(InstallStoreError::AcquireLock)?
            .ok_or(InstallStoreError::LockContended)?;
        let directory = gate
            .root_directory()
            .map_err(InstallStoreError::OpenRootAuthority)?;
        Ok(InstallLock {
            root: self.root.clone(),
            gate,
            directory,
        })
    }

    /// Acquire one user-scoped install lock before durably bootstrapping the
    /// exact store root beneath retained no-follow directory authorities.
    ///
    /// The anchor must already exist. Missing store components are monotone
    /// scaffolding and are intentionally retained after later failures. Callers
    /// must validate the candidate release before invoking this method.
    ///
    /// # Errors
    ///
    /// Returns an error for an unsafe root, missing non-root bootstrap anchor,
    /// lock contention, ancestry drift, unsafe existing components, directory
    /// creation failure, or failure to retain the final store inode.
    pub fn acquire_anchored_lock(&self, anchor: &Path) -> Result<InstallLock, InstallStoreError> {
        validate_bootstrap_root(&self.root)?;
        validate_bootstrap_root(anchor)?;
        let relative =
            self.root
                .strip_prefix(anchor)
                .map_err(|_| InstallStoreError::RootOutsideAnchor {
                    root: self.root.clone(),
                    anchor: anchor.to_path_buf(),
                })?;
        if relative.as_os_str().is_empty() {
            return Err(InstallStoreError::RootOutsideAnchor {
                root: self.root.clone(),
                anchor: anchor.to_path_buf(),
            });
        }
        let anchor_preflight =
            ReadOnlyDirectoryAuthority::open(anchor).map_err(InstallStoreError::BootstrapRoot)?;
        let anchor_metadata = anchor_preflight
            .metadata()
            .map_err(InstallStoreError::BootstrapRoot)?;
        require_safe_bootstrap_directory(anchor_metadata, anchor)?;
        let bootstrap =
            ExclusiveDirectory::try_acquire(anchor, Path::new(ANCHORED_INSTALL_LOCK_FILE))
                .map_err(InstallStoreError::AcquireLock)?
                .ok_or(InstallStoreError::LockContended)?;
        require_same_directory_identity(
            anchor_metadata,
            bootstrap
                .root_directory()
                .and_then(|directory| directory.metadata())
                .map_err(InstallStoreError::OpenRootAuthority)?,
        )?;
        let bootstrapped = bootstrap_store_root(&bootstrap, anchor, relative)?;
        let gate = ExclusiveDirectory::try_acquire(&self.root, Path::new(INSTALL_LOCK_FILE))
            .map_err(InstallStoreError::AcquireLock)?
            .ok_or(InstallStoreError::LockContended)?;
        let directory = gate
            .root_directory()
            .map_err(InstallStoreError::OpenRootAuthority)?;
        require_same_directory_identity(
            directory
                .metadata()
                .map_err(InstallStoreError::OpenRootAuthority)?,
            bootstrapped
                .metadata()
                .map_err(InstallStoreError::BootstrapRoot)?,
        )?;
        Ok(InstallLock {
            root: self.root.clone(),
            gate,
            directory,
        })
    }

    pub fn active_unit(&self, lock: &InstallLock) -> Result<Option<UnitId>, InstallStoreError> {
        let directory = self.authority(lock)?;
        let Some(target) = directory
            .read_symlink(Path::new("active"))
            .map_err(InstallStoreError::InspectActive)?
        else {
            return Ok(None);
        };
        let mut components = target.components();
        let units = components
            .next()
            .and_then(|component| component.as_os_str().to_str());
        let unit = components
            .next()
            .and_then(|component| component.as_os_str().to_str());
        if units != Some(UNITS_DIRECTORY) || components.next().is_some() {
            return Err(InstallStoreError::InvalidActiveLink(format!(
                "unexpected active target {}",
                target.display()
            )));
        }
        let id = UnitId::new(unit.unwrap_or_default()).map_err(|error| {
            InstallStoreError::InvalidActiveLink(format!("invalid active unit: {error}"))
        })?;
        Ok(Some(id))
    }

    pub fn set_active(
        &self,
        unit: Option<&UnitId>,
        lock: &InstallLock,
    ) -> Result<(), InstallStoreError> {
        let directory = self.authority(lock)?;
        match unit {
            Some(unit) => {
                directory
                    .durable_replace_symlink(&active_target(unit), Path::new("active"))
                    .map_err(InstallStoreError::SwitchActive)?;
            }
            None => {
                directory
                    .durable_remove_file(Path::new("active"))
                    .map_err(InstallStoreError::SwitchActive)?;
            }
        }
        Ok(())
    }

    pub fn load_journal(
        &self,
        lock: &InstallLock,
    ) -> Result<Option<InstallJournalV1>, InstallStoreError> {
        let directory = self.authority(lock)?;
        let mut file = match directory.open_file(Path::new(INSTALL_JOURNAL_FILE)) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(InstallStoreError::OpenJournal(error)),
        };
        let metadata = file.metadata().map_err(InstallStoreError::ReadJournal)?;
        if !metadata.is_file() || metadata.len() > self.max_journal_bytes as u64 {
            return Err(InstallStoreError::JournalTooLarge {
                limit: self.max_journal_bytes,
            });
        }
        let mut bytes = Vec::new();
        file.by_ref()
            .take((self.max_journal_bytes + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(InstallStoreError::ReadJournal)?;
        if bytes.len() > self.max_journal_bytes {
            return Err(InstallStoreError::JournalTooLarge {
                limit: self.max_journal_bytes,
            });
        }
        let journal: InstallJournalV1 =
            serde_json::from_slice(&bytes).map_err(InstallStoreError::DecodeJournal)?;
        journal
            .validate()
            .map_err(InstallStoreError::InvalidJournal)?;
        Ok(Some(journal))
    }

    pub fn write_journal(
        &self,
        journal: &InstallJournalV1,
        lock: &InstallLock,
    ) -> Result<(), InstallStoreError> {
        let directory = self.authority(lock)?;
        journal
            .validate()
            .map_err(InstallStoreError::InvalidJournal)?;
        let bytes = serde_json::to_vec(journal).map_err(InstallStoreError::EncodeJournal)?;
        if bytes.len() > self.max_journal_bytes {
            return Err(InstallStoreError::JournalTooLarge {
                limit: self.max_journal_bytes,
            });
        }
        let temporary = stage_journal(directory, &bytes, &TEMPORARY_SEQUENCE)?;
        if let Err(error) =
            directory.durable_replace_file(&temporary, Path::new(INSTALL_JOURNAL_FILE))
        {
            let _ = directory.durable_remove_file(&temporary);
            return Err(InstallStoreError::ReplaceJournal(error));
        }
        Ok(())
    }

    fn authority<'a>(
        &self,
        lock: &'a InstallLock,
    ) -> Result<&'a DirectoryAuthority, InstallStoreError> {
        if lock.root != self.root {
            return Err(InstallStoreError::WrongLock);
        }
        Ok(&lock.directory)
    }

    pub(crate) fn units_authority(
        &self,
        lock: &InstallLock,
    ) -> Result<DirectoryAuthority, InstallStoreError> {
        let root = self.authority(lock)?;
        match root
            .entry_metadata(Path::new(UNITS_DIRECTORY))
            .map_err(InstallStoreError::InspectUnits)?
        {
            None => root
                .create_child_directory(Path::new(UNITS_DIRECTORY))
                .map_err(InstallStoreError::CreateUnits),
            Some(metadata) if metadata.kind() == DirectoryEntryKind::Directory => root
                .open_child_directory(Path::new(UNITS_DIRECTORY))
                .map_err(InstallStoreError::OpenUnits),
            Some(_) => Err(InstallStoreError::InvalidUnitsDirectory),
        }
    }

    /// Open one immutable unit through this transaction's retained authority.
    ///
    /// The caller must use the lock acquired from this store. The returned
    /// capability shares that lock's operation gate and never reopens the
    /// install root pathname.
    ///
    /// # Errors
    ///
    /// Returns an error for a foreign lock, invalid units directory, missing
    /// unit, or a unit entry that is not an exact child directory.
    pub fn open_unit_directory(
        &self,
        lock: &InstallLock,
        unit: &UnitId,
    ) -> Result<DirectoryAuthority, InstallStoreError> {
        self.units_authority(lock)?
            .open_child_directory(Path::new(unit.as_str()))
            .map_err(InstallStoreError::OpenUnit)
    }
}

fn stage_journal(
    directory: &DirectoryAuthority,
    bytes: &[u8],
    sequence: &AtomicU64,
) -> Result<PathBuf, InstallStoreError> {
    for _ in 0..MAX_JOURNAL_STAGE_ATTEMPTS {
        let sequence = sequence.fetch_add(1, Ordering::Relaxed);
        let temporary = PathBuf::from(format!(
            ".{INSTALL_JOURNAL_FILE}.{}.{}",
            std::process::id(),
            sequence
        ));
        match directory.write_secret(&temporary, bytes) {
            Ok(()) => return Ok(temporary),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(InstallStoreError::WriteJournal(error)),
        }
    }
    Err(InstallStoreError::JournalStageCollisions {
        attempts: MAX_JOURNAL_STAGE_ATTEMPTS,
    })
}

fn validate_bootstrap_root(root: &Path) -> Result<(), InstallStoreError> {
    if !root.is_absolute()
        || root.components().any(|component| {
            !matches!(
                component,
                std::path::Component::RootDir | std::path::Component::Normal(_)
            )
        })
    {
        return Err(InstallStoreError::InvalidBootstrapRoot(root.to_path_buf()));
    }
    Ok(())
}

fn bootstrap_store_root(
    gate: &ExclusiveDirectory,
    anchor: &Path,
    relative: &Path,
) -> Result<PublicDirectoryAuthority, InstallStoreError> {
    let retained = gate
        .root_directory()
        .map_err(InstallStoreError::OpenRootAuthority)?;
    let mut authority = gate
        .open_public_directory(anchor)
        .map_err(InstallStoreError::BootstrapRoot)?;
    require_same_directory_identity(
        retained
            .metadata()
            .map_err(InstallStoreError::OpenRootAuthority)?,
        authority
            .metadata()
            .map_err(InstallStoreError::BootstrapRoot)?,
    )?;
    require_safe_bootstrap_directory(
        authority
            .metadata()
            .map_err(InstallStoreError::BootstrapRoot)?,
        anchor,
    )?;
    let mut current = anchor.to_path_buf();
    for component in relative.components() {
        let std::path::Component::Normal(name) = component else {
            return Err(InstallStoreError::InvalidBootstrapRoot(
                anchor.join(relative),
            ));
        };
        current.push(name);
        authority = match authority.open_child_directory(Path::new(name)) {
            Ok(child) => {
                require_safe_bootstrap_directory(
                    child.metadata().map_err(InstallStoreError::BootstrapRoot)?,
                    &current,
                )?;
                child
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => authority
                .durable_ensure_child_directory(Path::new(name), 0o755)
                .map_err(InstallStoreError::BootstrapRoot)?,
            Err(error) => return Err(InstallStoreError::BootstrapRoot(error)),
        };
    }
    Ok(authority)
}

fn require_safe_bootstrap_directory(
    metadata: DirectoryEntryMetadata,
    path: &Path,
) -> Result<(), InstallStoreError> {
    if metadata.kind() != DirectoryEntryKind::Directory
        || metadata.mode() & 0o700 != 0o700
        || metadata.mode() & 0o022 != 0
    {
        return Err(InstallStoreError::UnsafeBootstrapDirectory(
            path.to_path_buf(),
        ));
    }
    Ok(())
}

fn require_same_directory_identity(
    retained: DirectoryEntryMetadata,
    public: DirectoryEntryMetadata,
) -> Result<(), InstallStoreError> {
    if retained.kind() != DirectoryEntryKind::Directory
        || public.kind() != DirectoryEntryKind::Directory
        || retained.device() != public.device()
        || retained.inode() != public.inode()
    {
        return Err(InstallStoreError::StoreRootIdentityMismatch);
    }
    Ok(())
}

#[derive(Debug)]
pub struct InstallLock {
    root: PathBuf,
    gate: ExclusiveDirectory,
    directory: DirectoryAuthority,
}

impl InstallLock {
    /// Open the canonical store root only when it still names this lock's
    /// retained store inode.
    ///
    /// # Errors
    ///
    /// Returns an error when the canonical root ancestry changed, the path now
    /// names another inode, or retained-handle inspection fails.
    pub fn open_store_public_directory(
        &self,
    ) -> Result<PublicDirectoryAuthority, InstallStoreError> {
        let public = self
            .gate
            .open_public_directory(&self.root)
            .map_err(InstallStoreError::OpenPublicDirectory)?;
        require_same_directory_identity(
            self.directory
                .metadata()
                .map_err(InstallStoreError::OpenRootAuthority)?,
            public
                .metadata()
                .map_err(InstallStoreError::OpenPublicDirectory)?,
        )?;
        Ok(public)
    }

    /// Open one public directory under this transaction's retained authority.
    ///
    /// The returned capability shares the install lock and operation gate, so
    /// public launcher mutations cannot escape the transaction's authority.
    ///
    /// # Errors
    ///
    /// Returns an error when the absolute directory cannot be opened without
    /// following links or its ancestry cannot be retained exactly.
    pub fn open_public_directory(
        &self,
        directory: &Path,
    ) -> Result<PublicDirectoryAuthority, InstallStoreError> {
        self.gate
            .open_public_directory(directory)
            .map_err(InstallStoreError::OpenPublicDirectory)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum InstallStoreError {
    #[error("failed to create install state directory: {0}")]
    CreateRoot(io::Error),
    #[error("failed to acquire install transaction authority: {0}")]
    AcquireLock(io::Error),
    #[error("install transaction lock is already held")]
    LockContended,
    #[error("install transaction authority belongs to another prefix")]
    WrongLock,
    #[error("failed to retain the install root authority: {0}")]
    OpenRootAuthority(io::Error),
    #[error("install store root is not one safe absolute path: {}", .0.display())]
    InvalidBootstrapRoot(PathBuf),
    #[error(
        "install store root {} is outside retained bootstrap anchor {}",
        root.display(),
        anchor.display()
    )]
    RootOutsideAnchor { root: PathBuf, anchor: PathBuf },
    #[error("install store bootstrap directory is not safely owned: {}", .0.display())]
    UnsafeBootstrapDirectory(PathBuf),
    #[error("canonical install store path no longer names the retained store inode")]
    StoreRootIdentityMismatch,
    #[error("failed to bootstrap the retained install root: {0}")]
    BootstrapRoot(io::Error),
    #[error("failed to retain a public layout directory: {0}")]
    OpenPublicDirectory(io::Error),
    #[error("failed to inspect the immutable unit directory: {0}")]
    InspectUnits(io::Error),
    #[error("the immutable unit path is not a directory")]
    InvalidUnitsDirectory,
    #[error("failed to create the immutable unit directory: {0}")]
    CreateUnits(io::Error),
    #[error("failed to open the immutable unit directory: {0}")]
    OpenUnits(io::Error),
    #[error("failed to open the exact immutable unit: {0}")]
    OpenUnit(io::Error),
    #[error("failed to inspect active unit: {0}")]
    InspectActive(io::Error),
    #[error("invalid active unit link: {0}")]
    InvalidActiveLink(String),
    #[error("failed to switch active unit: {0}")]
    SwitchActive(io::Error),
    #[error("failed to open install journal without following links: {0}")]
    OpenJournal(io::Error),
    #[error("failed to read install journal: {0}")]
    ReadJournal(io::Error),
    #[error("install journal exceeds {limit} bytes")]
    JournalTooLarge { limit: usize },
    #[error("failed to decode install journal: {0}")]
    DecodeJournal(serde_json::Error),
    #[error("invalid install journal: {0}")]
    InvalidJournal(super::model::InstallModelError),
    #[error("failed to encode install journal: {0}")]
    EncodeJournal(serde_json::Error),
    #[error("failed to write staged install journal: {0}")]
    WriteJournal(io::Error),
    #[error("could not allocate a unique staged install journal after {attempts} attempts")]
    JournalStageCollisions { attempts: usize },
    #[error("failed to publish install journal: {0}")]
    ReplaceJournal(io::Error),
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::AtomicU64;

    use super::{
        ANCHORED_INSTALL_LOCK_FILE, INSTALL_JOURNAL_FILE, InstallStore, UnitId, stage_journal,
    };

    #[test]
    fn stale_stage_from_reused_pid_does_not_block_journal_staging() {
        let root = tempfile::tempdir().expect("temporary install root");
        let store = InstallStore::new(root.path(), 1024);
        let lock = store.acquire_lock().expect("install authority");
        let sequence = AtomicU64::new(0);
        let stale = format!(".{INSTALL_JOURNAL_FILE}.{}.0", std::process::id());
        lock.directory
            .write_secret(stale.as_ref(), b"crash residue")
            .expect("seed stale journal stage");

        let staged = stage_journal(&lock.directory, b"current journal", &sequence)
            .expect("collision retry must stage the current journal");

        assert_eq!(
            staged,
            PathBuf::from(format!(".{INSTALL_JOURNAL_FILE}.{}.1", std::process::id()))
        );
    }

    #[test]
    fn anchored_lock_bootstraps_and_retains_the_exact_store_inode() {
        let workspace = tempfile::Builder::new()
            .prefix("anchored-install-store-")
            .tempdir_in(std::env::current_dir().expect("current directory"))
            .expect("temporary anchored store workspace");
        let home = workspace.path().join("home");
        fs::create_dir(&home).expect("create retained HOME anchor");
        fs::create_dir(home.join(".local")).expect("create existing user directory");
        fs::set_permissions(home.join(".local"), fs::Permissions::from_mode(0o700))
            .expect("set existing user directory mode");
        let root = home.join(".local/lib/hypercolor");
        let store = InstallStore::new(&root, 1024);
        let lock = store
            .acquire_anchored_lock(&home)
            .expect("anchored install authority");
        let retained_public = lock
            .open_store_public_directory()
            .expect("canonical path names retained store");

        assert!(home.join(ANCHORED_INSTALL_LOCK_FILE).is_file());
        assert!(root.join("install.lock").is_file());
        assert_eq!(
            fs::metadata(home.join(".local"))
                .expect("existing user directory metadata")
                .permissions()
                .mode()
                & 0o7777,
            0o700
        );
        assert_eq!(
            retained_public.metadata().expect("public store metadata"),
            lock.directory.metadata().expect("retained store metadata")
        );
        assert!(matches!(
            store.acquire_anchored_lock(&home),
            Err(super::InstallStoreError::LockContended)
        ));
        assert!(matches!(
            store.acquire_lock(),
            Err(super::InstallStoreError::LockContended)
        ));

        let displaced = home.join("displaced-local");
        fs::rename(home.join(".local"), &displaced).expect("displace canonical store ancestry");
        fs::create_dir_all(&root).expect("create attacker replacement store path");
        lock.open_store_public_directory()
            .expect_err("replacement path must not become the public store authority");
        let unit = UnitId::new("a".repeat(64)).expect("unit ID");
        store
            .set_active(Some(&unit), &lock)
            .expect("switch retained active entry");

        assert_eq!(
            fs::read_link(displaced.join("lib/hypercolor/active"))
                .expect("retained active symlink"),
            Path::new("units").join(unit.as_str())
        );
        assert!(!root.join("active").exists());
    }

    #[test]
    fn anchored_lock_rejects_store_roots_outside_the_anchor() {
        let workspace = tempfile::Builder::new()
            .prefix("anchored-install-store-boundary-")
            .tempdir_in(std::env::current_dir().expect("current directory"))
            .expect("temporary anchored store workspace");
        let home = workspace.path().join("home");
        let outside = workspace.path().join("outside/hypercolor");
        fs::create_dir(&home).expect("create retained HOME anchor");
        let store = InstallStore::new(&outside, 1024);

        assert!(matches!(
            store.acquire_anchored_lock(&home),
            Err(super::InstallStoreError::RootOutsideAnchor { .. })
        ));
        assert!(!outside.exists());
        assert!(!home.join(ANCHORED_INSTALL_LOCK_FILE).exists());

        let equal = InstallStore::new(&home, 1024);
        assert!(matches!(
            equal.acquire_anchored_lock(&home),
            Err(super::InstallStoreError::RootOutsideAnchor { .. })
        ));
        assert!(!home.join(ANCHORED_INSTALL_LOCK_FILE).exists());
        assert!(!home.join("install.lock").exists());
    }

    #[test]
    fn anchored_lock_rejects_writable_existing_store_ancestors() {
        for unsafe_depth in 0..3 {
            let workspace = tempfile::Builder::new()
                .prefix("anchored-install-store-mode-")
                .tempdir_in(std::env::current_dir().expect("current directory"))
                .expect("temporary anchored store workspace");
            let home = workspace.path().join("home");
            fs::create_dir(&home).expect("create retained HOME anchor");
            let components = [".local", "lib", "hypercolor"];
            let mut unsafe_path = home.clone();
            for (depth, component) in components.iter().enumerate().take(unsafe_depth + 1) {
                unsafe_path.push(component);
                fs::create_dir(&unsafe_path).expect("create existing store component");
                let mode = if depth == unsafe_depth { 0o777 } else { 0o755 };
                fs::set_permissions(&unsafe_path, fs::Permissions::from_mode(mode))
                    .expect("set existing store component mode");
            }
            let root = home.join(".local/lib/hypercolor");
            let store = InstallStore::new(&root, 1024);

            assert!(matches!(
                store.acquire_anchored_lock(&home),
                Err(super::InstallStoreError::UnsafeBootstrapDirectory(ref path))
                    if path == &unsafe_path
            ));
            assert!(!root.join("install.lock").exists());
            if let Some(next) = components.get(unsafe_depth + 1) {
                assert!(!unsafe_path.join(next).exists());
            }
        }
    }
}
