//! The release store on Windows: immutable units under the release root and
//! the transaction lock, journal and active pointer under the state root.
//!
//! Every operation goes through directory handles the lock retained, so a
//! path swapped after acquisition cannot redirect it. The active pointer is
//! a file, `active.json`, replaced in one step like the journal, so
//! switching releases needs no symlink privilege.

use std::ffi::OsStr;
use std::io::{self, Read as _, Write as _};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use hypercolor_platform_fs::{
    WindowsDirectory, WindowsDirectoryAccess, WindowsEntryKind, WindowsFileLock,
    windows_file_security,
};
use serde::{Deserialize, Serialize};

use super::ownership::{WindowsDirectoryRefusal, WindowsOwnershipPolicy};
use crate::model::{InstallDisposition, InstallJournalV1, InstallModelError, UnitId};

const INSTALL_LOCK_FILE: &str = "install.lock";
const INSTALL_JOURNAL_FILE: &str = "install-journal.json";
const ACTIVE_POINTER_FILE: &str = "active.json";
const ACTIVE_POINTER_SCHEMA_VERSION: u32 = 1;
const MAX_ACTIVE_POINTER_BYTES: usize = 4 * 1024;
const UNITS_DIRECTORY: &str = "units";
/// File systems whose rename and flush give the journal and pointer one
/// durable step.
const DURABLE_FILE_SYSTEMS: [&str; 2] = ["NTFS", "ReFS"];
const MAX_STAGE_ATTEMPTS: usize = 128;
/// Directory name prefixes of interrupted unit staging and removal. They
/// are only ever created and removed under the installation lock, so any
/// found while holding it belong to a process that died.
const LEFTOVER_UNIT_PREFIXES: [&str; 2] = [".hypercolor-stage-", ".hypercolor-removing-"];
static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// The two roots of one Windows installation and the policy that guards
/// them.
#[derive(Debug, Clone)]
pub struct InstallStore {
    root: PathBuf,
    state_root: PathBuf,
    max_journal_bytes: usize,
    ownership: WindowsOwnershipPolicy,
}

/// The pointer that names the active unit.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ActivePointer {
    schema_version: u32,
    unit: UnitId,
}

impl InstallStore {
    /// Use separate immutable-release and mutable-state roots.
    ///
    /// Both must be absolute paths of plain names, distinct and not nested.
    /// Acquisition additionally proves their ownership and their ancestors'.
    ///
    /// # Errors
    /// Returns an error for an invalid or overlapping root.
    pub fn with_roots(
        release_root: impl Into<PathBuf>,
        state_root: impl Into<PathBuf>,
        max_journal_bytes: usize,
    ) -> Result<Self, InstallStoreError> {
        let root = release_root.into();
        let state_root = state_root.into();
        validate_root(&root)?;
        validate_root(&state_root)?;
        if nested(&root, &state_root) || nested(&state_root, &root) {
            return Err(InstallStoreError::OverlappingRoots);
        }
        Ok(Self {
            root,
            state_root,
            max_journal_bytes,
            ownership: WindowsOwnershipPolicy::machine(),
        })
    }

    /// Replace the policy that decides who may own and change the roots.
    #[must_use]
    pub fn with_ownership_policy(mut self, ownership: WindowsOwnershipPolicy) -> Self {
        self.ownership = ownership;
        self
    }

    /// The policy applied to the roots, their ancestors and the units
    /// directory.
    #[must_use]
    pub fn ownership_policy(&self) -> &WindowsOwnershipPolicy {
        &self.ownership
    }

    /// Directory holding the immutable units.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Directory holding the transaction lock, journal and active pointer.
    #[must_use]
    pub fn state_root(&self) -> &Path {
        &self.state_root
    }

    #[must_use]
    pub fn journal_path(&self) -> PathBuf {
        self.state_root.join(INSTALL_JOURNAL_FILE)
    }

    #[must_use]
    pub fn lock_path(&self) -> PathBuf {
        self.state_root.join(INSTALL_LOCK_FILE)
    }

    #[must_use]
    pub fn active_path(&self) -> PathBuf {
        self.state_root.join(ACTIVE_POINTER_FILE)
    }

    /// Return the canonical path of one digest-named immutable unit.
    #[must_use]
    pub fn unit_path(&self, unit: &UnitId) -> PathBuf {
        self.root.join(UNITS_DIRECTORY).join(unit.as_str())
    }

    /// Prove both roots and their ancestors safe, then take the transaction
    /// lock without waiting.
    ///
    /// # Errors
    /// Returns [`InstallStoreError::LockContended`] when another process
    /// holds the lock, and an error for a missing, unsafe or aliased root.
    pub fn acquire_lock(&self) -> Result<InstallLock, InstallStoreError> {
        let release = self.open_root(&self.root)?;
        let state = self.open_root(&self.state_root)?;
        let release_identity = release
            .metadata()
            .map_err(|source| self.open_root_error(&self.root, source))?;
        let state_identity = state
            .metadata()
            .map_err(|source| self.open_root_error(&self.state_root, source))?;
        if release_identity.same_entry(state_identity) {
            return Err(InstallStoreError::OverlappingRoots);
        }
        let lock = state
            .try_lock_file(OsStr::new(INSTALL_LOCK_FILE))
            .map_err(InstallStoreError::AcquireLock)?
            .ok_or(InstallStoreError::LockContended)?;
        // Whoever could write the lock file could also lock it first.
        self.require_trusted_file(lock.file(), &self.lock_path())?;
        Ok(InstallLock {
            root: self.root.clone(),
            state_root: self.state_root.clone(),
            _lock: lock,
            release,
            state,
        })
    }

    fn open_root(&self, root: &Path) -> Result<WindowsDirectory, InstallStoreError> {
        let mut ancestor = PathBuf::new();
        let mut components = root.components().peekable();
        while let Some(component) = components.next() {
            ancestor.push(component.as_os_str());
            if matches!(component, Component::Prefix(_)) || components.peek().is_none() {
                continue;
            }
            let directory = WindowsDirectory::open(&ancestor, WindowsDirectoryAccess::Read)
                .map_err(|source| self.open_root_error(&ancestor, source))?;
            let security = directory
                .security()
                .map_err(|source| self.open_root_error(&ancestor, source))?;
            self.ownership
                .require_ancestor(&security)
                .map_err(|refusal| InstallStoreError::Untrusted(ancestor.clone(), refusal))?;
        }
        let directory = WindowsDirectory::open(root, WindowsDirectoryAccess::Write)
            .map_err(|source| self.open_root_error(root, source))?;
        self.require_installer_owned(&directory, root)?;
        let file_system = directory
            .file_system_name()
            .map_err(|source| self.open_root_error(root, source))?;
        if !DURABLE_FILE_SYSTEMS.contains(&file_system.as_str()) {
            return Err(InstallStoreError::UnsupportedFileSystem {
                path: root.to_path_buf(),
                file_system,
            });
        }
        Ok(directory)
    }

    fn require_installer_owned(
        &self,
        directory: &WindowsDirectory,
        path: &Path,
    ) -> Result<(), InstallStoreError> {
        let security = directory
            .security()
            .map_err(|source| self.open_root_error(path, source))?;
        self.ownership
            .require_installer_owned(&security)
            .map_err(|refusal| InstallStoreError::Untrusted(path.to_path_buf(), refusal))
    }

    /// Require that only trusted principals can change a control file,
    /// judged through the handle its contents are read from.
    fn require_trusted_file(
        &self,
        file: &std::fs::File,
        path: &Path,
    ) -> Result<(), InstallStoreError> {
        let security =
            windows_file_security(file).map_err(|source| self.open_root_error(path, source))?;
        self.ownership
            .require_installer_owned(&security)
            .map_err(|refusal| InstallStoreError::Untrusted(path.to_path_buf(), refusal))
    }

    #[allow(clippy::unused_self)]
    fn open_root_error(&self, path: &Path, source: io::Error) -> InstallStoreError {
        InstallStoreError::OpenRoot {
            path: path.to_path_buf(),
            source,
        }
    }

    /// The unit the active pointer names, or `None` before the first commit.
    ///
    /// # Errors
    /// Returns an error for a foreign lock or an unreadable or invalid
    /// pointer.
    pub fn active_unit(&self, lock: &InstallLock) -> Result<Option<UnitId>, InstallStoreError> {
        let state = self.state_directory(lock)?;
        let file = match state.open_file(OsStr::new(ACTIVE_POINTER_FILE)) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(InstallStoreError::InspectActive(error)),
        };
        self.require_trusted_file(&file, &self.active_path())?;
        let bytes = read_bounded(file, MAX_ACTIVE_POINTER_BYTES)
            .map_err(InstallStoreError::InspectActive)?
            .ok_or_else(|| {
                InstallStoreError::InvalidActivePointer(format!(
                    "the pointer exceeds {MAX_ACTIVE_POINTER_BYTES} bytes"
                ))
            })?;
        let pointer: ActivePointer = serde_json::from_slice(&bytes)
            .map_err(|error| InstallStoreError::InvalidActivePointer(error.to_string()))?;
        if pointer.schema_version != ACTIVE_POINTER_SCHEMA_VERSION {
            return Err(InstallStoreError::InvalidActivePointer(format!(
                "unsupported pointer schema {}",
                pointer.schema_version
            )));
        }
        Ok(Some(pointer.unit))
    }

    /// Point the installation at `unit`, or at nothing, in one durable step.
    ///
    /// # Errors
    /// Returns an error for a foreign lock or a failed replacement.
    pub fn set_active(
        &self,
        unit: Option<&UnitId>,
        lock: &InstallLock,
    ) -> Result<(), InstallStoreError> {
        let state = self.state_directory(lock)?;
        match unit {
            Some(unit) => {
                let bytes = serde_json::to_vec(&ActivePointer {
                    schema_version: ACTIVE_POINTER_SCHEMA_VERSION,
                    unit: unit.clone(),
                })
                .map_err(|error| InstallStoreError::SwitchActive(io::Error::other(error)))?;
                replace_durably(state, ACTIVE_POINTER_FILE, &bytes)
                    .map_err(InstallStoreError::SwitchActive)
            }
            None => match state.remove_file(OsStr::new(ACTIVE_POINTER_FILE)) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(InstallStoreError::SwitchActive(error)),
            },
        }
    }

    /// Read and validate the journal, or `None` when no transaction ran.
    ///
    /// # Errors
    /// Returns an error for a foreign lock or an unreadable, oversized or
    /// invalid journal.
    pub fn load_journal(
        &self,
        lock: &InstallLock,
    ) -> Result<Option<InstallJournalV1>, InstallStoreError> {
        let state = self.state_directory(lock)?;
        let file = match state.open_file(OsStr::new(INSTALL_JOURNAL_FILE)) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(InstallStoreError::OpenJournal(error)),
        };
        self.require_trusted_file(&file, &self.journal_path())?;
        let bytes = read_bounded(file, self.max_journal_bytes)
            .map_err(InstallStoreError::ReadJournal)?
            .ok_or(InstallStoreError::JournalTooLarge {
                limit: self.max_journal_bytes,
            })?;
        let journal: InstallJournalV1 =
            serde_json::from_slice(&bytes).map_err(InstallStoreError::DecodeJournal)?;
        journal
            .validate()
            .map_err(InstallStoreError::InvalidJournal)?;
        Ok(Some(journal))
    }

    /// Validate and durably replace the journal.
    ///
    /// # Errors
    /// Returns an error for a foreign lock, an invalid or oversized journal,
    /// or a failed write or replacement.
    pub fn write_journal(
        &self,
        journal: &InstallJournalV1,
        lock: &InstallLock,
    ) -> Result<(), InstallStoreError> {
        let state = self.state_directory(lock)?;
        journal
            .validate()
            .map_err(InstallStoreError::InvalidJournal)?;
        let bytes = serde_json::to_vec(journal).map_err(InstallStoreError::EncodeJournal)?;
        if bytes.len() > self.max_journal_bytes {
            return Err(InstallStoreError::JournalTooLarge {
                limit: self.max_journal_bytes,
            });
        }
        replace_durably(state, INSTALL_JOURNAL_FILE, &bytes)
            .map_err(InstallStoreError::WriteJournal)
    }

    /// Open one immutable unit read-only through the lock's release root.
    ///
    /// # Errors
    /// Returns an error for a foreign lock, an unsafe units directory, or a
    /// missing unit or one that is not a plain directory.
    pub fn open_unit_directory(
        &self,
        lock: &InstallLock,
        unit: &UnitId,
    ) -> Result<WindowsDirectory, InstallStoreError> {
        let units = self
            .units_directory(lock, false)?
            .ok_or_else(|| InstallStoreError::OpenUnit(io::ErrorKind::NotFound.into()))?;
        units
            .open_child_directory(OsStr::new(unit.as_str()), WindowsDirectoryAccess::Read)
            .map_err(InstallStoreError::OpenUnit)
    }

    /// The units directory for writing, created when `create` asks and it
    /// is absent; `None` when it is absent and `create` is false.
    ///
    /// # Errors
    /// Returns an error for a foreign lock, a units entry that is not a
    /// plain directory, or a units directory an untrusted principal can
    /// change.
    pub(crate) fn units_directory(
        &self,
        lock: &InstallLock,
        create: bool,
    ) -> Result<Option<WindowsDirectory>, InstallStoreError> {
        let release = self.release_directory(lock)?;
        let name = OsStr::new(UNITS_DIRECTORY);
        let units = match release
            .entry_metadata(name)
            .map_err(InstallStoreError::InspectUnits)?
        {
            None if create => release
                .create_child_directory_with_security(name, &self.ownership.units_sddl())
                .map_err(InstallStoreError::CreateUnits)?,
            None => return Ok(None),
            Some(metadata) if metadata.kind() == WindowsEntryKind::Directory => release
                .open_child_directory(name, WindowsDirectoryAccess::Write)
                .map_err(InstallStoreError::OpenUnits)?,
            Some(_) => return Err(InstallStoreError::InvalidUnitsDirectory),
        };
        let path = self.root.join(UNITS_DIRECTORY);
        let security = units
            .security()
            .map_err(|source| self.open_root_error(&path, source))?;
        self.ownership
            .require_units_directory(&security)
            .map_err(|refusal| InstallStoreError::Untrusted(path, refusal))?;
        Ok(Some(units))
    }

    /// Units this store keeps whatever a caller asks: the active unit and
    /// every unit the journal names unless it committed.
    ///
    /// # Errors
    /// Returns an error for a foreign lock or unreadable records.
    pub fn referenced_units(&self, lock: &InstallLock) -> Result<Vec<UnitId>, InstallStoreError> {
        let mut referenced: Vec<UnitId> = self.active_unit(lock)?.into_iter().collect();
        if let Some(journal) = self.load_journal(lock)?
            && journal.disposition != InstallDisposition::Committed
        {
            for unit in journal.named_units() {
                if !referenced.contains(&unit) {
                    referenced.push(unit);
                }
            }
        }
        Ok(referenced)
    }

    /// Durably remove one installed unit that nothing references.
    ///
    /// Returns `false` when the unit is not installed.
    ///
    /// # Errors
    /// Refuses a referenced unit without any effect, and returns an error
    /// when removal fails.
    pub fn remove_unit(
        &self,
        lock: &InstallLock,
        unit: &UnitId,
    ) -> Result<bool, InstallStoreError> {
        if self.referenced_units(lock)?.contains(unit) {
            return Err(InstallStoreError::UnitReferenced(unit.as_str().to_owned()));
        }
        let Some(units) = self.units_directory(lock, false)? else {
            return Ok(false);
        };
        let name = OsStr::new(unit.as_str());
        match units.entry_metadata(name) {
            Ok(None) => return Ok(false),
            Ok(Some(_)) => {}
            Err(source) => {
                return Err(InstallStoreError::RemoveUnit {
                    name: unit.as_str().to_owned(),
                    source,
                });
            }
        }
        units
            .remove_tree(name)
            .map(|()| true)
            .map_err(|source| InstallStoreError::RemoveUnit {
                name: unit.as_str().to_owned(),
                source,
            })
    }

    /// Remove every installed unit except the referenced ones and `retain`,
    /// then every leftover of interrupted staging, removal and pointer or
    /// journal writes.
    ///
    /// Entries whose names this installer never creates are left alone. An
    /// entry that refuses removal is reported and collection continues.
    ///
    /// # Errors
    /// Returns an error for a foreign lock, unreadable records, or a
    /// directory that cannot be listed.
    pub fn collect_units(
        &self,
        lock: &InstallLock,
        retain: &[UnitId],
    ) -> Result<UnitCollection, InstallStoreError> {
        let referenced = self.referenced_units(lock)?;
        let mut collection = UnitCollection::default();
        if let Some(units) = self.units_directory(lock, false)? {
            for name in units.entries().map_err(InstallStoreError::InspectUnits)? {
                let Some(name) = name.to_str() else {
                    continue;
                };
                let path = self.root.join(UNITS_DIRECTORY).join(name);
                let unit = UnitId::new(name).ok();
                let leftover = LEFTOVER_UNIT_PREFIXES
                    .iter()
                    .any(|prefix| name.starts_with(prefix));
                if unit
                    .as_ref()
                    .is_some_and(|unit| referenced.contains(unit) || retain.contains(unit))
                    || unit.is_none() && !leftover
                {
                    continue;
                }
                match units.remove_tree(OsStr::new(name)) {
                    Ok(()) => match unit {
                        Some(unit) => collection.removed_units.push(unit),
                        None => collection.removed_leftovers.push(path),
                    },
                    Err(error) => collection.refused.push((path, error.to_string())),
                }
            }
        }
        let state = self.state_directory(lock)?;
        let stage_prefixes = [
            format!(".{INSTALL_JOURNAL_FILE}."),
            format!(".{ACTIVE_POINTER_FILE}."),
        ];
        for name in state.entries().map_err(InstallStoreError::InspectState)? {
            let Some(name) = name.to_str() else {
                continue;
            };
            if !stage_prefixes.iter().any(|prefix| name.starts_with(prefix)) {
                continue;
            }
            let path = self.state_root.join(name);
            match state.remove_file(OsStr::new(name)) {
                Ok(()) => collection.removed_leftovers.push(path),
                Err(error) => collection.refused.push((path, error.to_string())),
            }
        }
        Ok(collection)
    }

    fn release_directory<'a>(
        &self,
        lock: &'a InstallLock,
    ) -> Result<&'a WindowsDirectory, InstallStoreError> {
        if !lock.guards_roots(&self.root, &self.state_root) {
            return Err(InstallStoreError::WrongLock);
        }
        Ok(&lock.release)
    }

    fn state_directory<'a>(
        &self,
        lock: &'a InstallLock,
    ) -> Result<&'a WindowsDirectory, InstallStoreError> {
        self.release_directory(lock)?;
        Ok(&lock.state)
    }
}

/// What one collection removed, and what it could not.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UnitCollection {
    /// Installed units removed because nothing references them.
    pub removed_units: Vec<UnitId>,
    /// Leftovers of interrupted staging, removal and pointer or journal
    /// writes.
    pub removed_leftovers: Vec<PathBuf>,
    /// Entries that looked like the installer's but could not be removed,
    /// with the reason.
    pub refused: Vec<(PathBuf, String)>,
}

/// The held transaction lock and the root handles it retained.
#[derive(Debug)]
pub struct InstallLock {
    root: PathBuf,
    state_root: PathBuf,
    _lock: WindowsFileLock,
    release: WindowsDirectory,
    state: WindowsDirectory,
}

impl InstallLock {
    pub(crate) fn guards_roots(&self, release_root: &Path, state_root: &Path) -> bool {
        self.root == release_root && self.state_root == state_root
    }
}

/// Write `bytes` to a fresh temporary file beside `name`, flush it, then
/// rename it over `name` and flush the directory.
fn replace_durably(directory: &WindowsDirectory, name: &str, bytes: &[u8]) -> io::Result<()> {
    for _ in 0..MAX_STAGE_ATTEMPTS {
        let sequence = TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temporary = format!(".{name}.{}.{sequence}", std::process::id());
        let mut file = match directory.create_file(OsStr::new(&temporary)) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        };
        let written = file.write_all(bytes).and_then(|()| file.sync_all());
        drop(file);
        let result =
            written.and_then(|()| directory.rename(OsStr::new(&temporary), OsStr::new(name), true));
        if result.is_err() {
            let _ = directory.remove_file(OsStr::new(&temporary));
        }
        return result;
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!(
            "could not allocate a temporary name beside {name} after {MAX_STAGE_ATTEMPTS} attempts"
        ),
    ))
}

/// Read at most `limit` bytes; `None` when the file holds more.
fn read_bounded(file: std::fs::File, limit: usize) -> io::Result<Option<Vec<u8>>> {
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    Ok((bytes.len() <= limit).then_some(bytes))
}

fn validate_root(root: &Path) -> Result<(), InstallStoreError> {
    let mut components = root.components();
    let valid = matches!(components.next(), Some(Component::Prefix(_)))
        && matches!(components.next(), Some(Component::RootDir))
        && {
            let rest = components.collect::<Vec<_>>();
            !rest.is_empty()
                && rest
                    .iter()
                    .all(|component| matches!(component, Component::Normal(_)))
        };
    if valid {
        Ok(())
    } else {
        Err(InstallStoreError::InvalidRoot(root.to_path_buf()))
    }
}

/// Whether `inner` is `outer` or lies beneath it, comparing names the way
/// a case-insensitive volume does.
fn nested(outer: &Path, inner: &Path) -> bool {
    let fold = |path: &Path| {
        path.components()
            .map(|component| component.as_os_str().to_string_lossy().to_lowercase())
            .collect::<Vec<_>>()
    };
    let outer = fold(outer);
    let inner = fold(inner);
    inner.len() >= outer.len() && inner[..outer.len()] == outer[..]
}

/// Release store failure on Windows.
#[derive(Debug, thiserror::Error)]
pub enum InstallStoreError {
    #[error("release and state roots overlap or alias the same directory")]
    OverlappingRoots,
    #[error("install store root is not one absolute path of plain names: {}", .0.display())]
    InvalidRoot(PathBuf),
    #[error("failed to open install directory {}: {source}", path.display())]
    OpenRoot {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("install entry {path} cannot be trusted: {refusal}", path = .0.display(), refusal = .1)]
    Untrusted(PathBuf, WindowsDirectoryRefusal),
    #[error("install root {} is on {file_system}, not NTFS or ReFS", path.display())]
    UnsupportedFileSystem { path: PathBuf, file_system: String },
    #[error("failed to acquire install transaction lock: {0}")]
    AcquireLock(io::Error),
    #[error("install transaction lock is already held")]
    LockContended,
    #[error("install transaction lock belongs to another store")]
    WrongLock,
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
    #[error("failed to read the active unit pointer: {0}")]
    InspectActive(io::Error),
    #[error("invalid active unit pointer: {0}")]
    InvalidActivePointer(String),
    #[error("failed to switch the active unit: {0}")]
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
    InvalidJournal(InstallModelError),
    #[error("failed to encode install journal: {0}")]
    EncodeJournal(serde_json::Error),
    #[error("failed to write install journal durably: {0}")]
    WriteJournal(io::Error),
    #[error("installed unit {0} is still referenced and cannot be removed")]
    UnitReferenced(String),
    #[error("failed to remove {name}: {source}")]
    RemoveUnit {
        name: String,
        #[source]
        source: io::Error,
    },
    #[error("failed to inspect the install state directory: {0}")]
    InspectState(io::Error),
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
