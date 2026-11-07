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
    WindowsDirectory, WindowsDirectoryAccess, WindowsEntryKind, WindowsFileLock, WindowsSid,
    windows_file_security,
};

use super::installation::{WINDOWS_INSTALLATION_FILE, WindowsInstallation};
use super::machine_layout::BOOTSTRAP_DIRECTORY;
use super::ownership::{WindowsDirectoryRefusal, WindowsOwnershipPolicy};
use super::update_pipe::{UPDATE_PIPE_RECORD, UpdatePipeName};
use crate::model::{InstallDisposition, InstallJournalV1, InstallModelError, UnitId};
use crate::release_selection::{ReleaseRecords, decode_active_pointer, encode_active_pointer};

const INSTALL_LOCK_FILE: &str = "install.lock";
const INSTALL_JOURNAL_FILE: &str = "install-journal.json";
const ACTIVE_POINTER_FILE: &str = "active.json";
const MAX_ACTIVE_POINTER_BYTES: usize = 4 * 1024;
const MAX_INSTALLATION_BYTES: usize = 4 * 1024;
/// The activator's records, and the pipe record the service publishes.
const ACTIVATOR_DIRECTORY: &str = "activator";
/// The records the daemon keeps as the recorded user.
const COORDINATOR_DIRECTORY: &str = "coordinator";
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
        let release = self.open_root(&self.root, WindowsDirectoryAccess::Write)?;
        let state = self.open_root(&self.state_root, WindowsDirectoryAccess::Write)?;
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

    /// Prove both roots and their ancestors safe for a reader that holds no
    /// lock: a program outside any unit, running as whoever started it.
    ///
    /// # Errors
    /// Returns an error for a missing or unsafe root.
    pub fn release_records(&self) -> Result<WindowsReleaseRecords, InstallStoreError> {
        let release = self.open_root(&self.root, WindowsDirectoryAccess::Read)?;
        let state = self.open_root(&self.state_root, WindowsDirectoryAccess::Read)?;
        Ok(WindowsReleaseRecords {
            store: self.clone(),
            release,
            state,
        })
    }

    fn activator_directory(
        &self,
        state: &WindowsDirectory,
    ) -> Result<WindowsDirectory, InstallStoreError> {
        let name = OsStr::new(ACTIVATOR_DIRECTORY);
        let path = self.state_root.join(ACTIVATOR_DIRECTORY);
        let directory = match state.entry_metadata(name) {
            Ok(None) => {
                state.create_child_directory_with_security(name, &self.ownership.units_sddl())
            }
            Ok(Some(_)) => state.open_child_directory(name, WindowsDirectoryAccess::Write),
            Err(error) => Err(error),
        }
        .map_err(|source| self.open_root_error(&path, source))?;
        self.require_installer_owned(&directory, &path)?;
        Ok(directory)
    }

    fn open_root(
        &self,
        root: &Path,
        access: WindowsDirectoryAccess,
    ) -> Result<WindowsDirectory, InstallStoreError> {
        self.require_safe_ancestors(root)?;
        let directory = WindowsDirectory::open(root, access)
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

    /// Prove every directory above the release root safe, and the root
    /// itself no link, so a program that goes on to create or change it by
    /// path reaches exactly what it named.
    ///
    /// # Errors
    /// Returns an error for a link anywhere on the path, or an ancestor an
    /// untrusted principal could replace.
    pub(crate) fn require_safe_release_path(&self) -> Result<(), InstallStoreError> {
        self.require_safe_ancestors(&self.root)?;
        match WindowsDirectory::open(&self.root, WindowsDirectoryAccess::Read) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            opened => opened
                .map(drop)
                .map_err(|source| self.open_root_error(&self.root, source)),
        }
    }

    fn require_safe_ancestors(&self, root: &Path) -> Result<(), InstallStoreError> {
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
        Ok(())
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
        decode_active_pointer(&bytes)
            .map(Some)
            .map_err(InstallStoreError::InvalidActivePointer)
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
                let bytes = encode_active_pointer(unit)
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

    /// What the installer recorded, or `None` before it did.
    ///
    /// # Errors
    /// Returns an error for a foreign lock or an unsafe or invalid record.
    pub fn installation(
        &self,
        lock: &InstallLock,
    ) -> Result<Option<WindowsInstallation>, InstallStoreError> {
        let state = self.state_directory(lock)?;
        read_installation(self, state)
    }

    /// Write the installation's record in one durable step.
    ///
    /// # Errors
    /// Returns an error for a foreign lock or a failed replacement.
    pub fn record_installation(
        &self,
        installation: &WindowsInstallation,
        lock: &InstallLock,
    ) -> Result<(), InstallStoreError> {
        let state = self.state_directory(lock)?;
        let bytes = installation
            .encode()
            .map_err(|error| InstallStoreError::WriteRecord(io::Error::other(error)))?;
        replace_durably(state, WINDOWS_INSTALLATION_FILE, &bytes)
            .map_err(InstallStoreError::WriteRecord)
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

    /// Create the directory the recorded user's daemon keeps its update
    /// records in, which only that user and the machine may reach. The
    /// state root admits nobody else, so one already there is this one.
    ///
    /// # Errors
    /// Returns an error for a foreign lock or a failed creation.
    pub(crate) fn prepare_coordinator_directory(
        &self,
        lock: &InstallLock,
        user_sid: &str,
    ) -> Result<(), InstallStoreError> {
        let state = self.state_directory(lock)?;
        let name = OsStr::new(COORDINATOR_DIRECTORY);
        let path = self.state_root.join(COORDINATOR_DIRECTORY);
        let exists = state
            .entry_metadata(name)
            .map_err(|source| self.open_root_error(&path, source))?
            .is_some();
        if !exists {
            let sddl = format!("O:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FA;;;{user_sid})");
            state
                .create_child_directory_with_security(name, &sddl)
                .map_err(|source| self.open_root_error(&path, source))?;
        }
        Ok(())
    }

    /// The directory holding the recovery bootstrap's slots, created under
    /// the units directory's own DACL when it is absent.
    ///
    /// # Errors
    /// Returns an error for a foreign lock, an entry that is not a plain
    /// directory, or a directory an untrusted principal can change.
    pub(crate) fn bootstrap_directory(
        &self,
        lock: &InstallLock,
    ) -> Result<WindowsDirectory, InstallStoreError> {
        let release = self.release_directory(lock)?;
        let name = OsStr::new(BOOTSTRAP_DIRECTORY);
        let path = self.root.join(BOOTSTRAP_DIRECTORY);
        let directory = match release.entry_metadata(name) {
            Ok(None) => {
                release.create_child_directory_with_security(name, &self.ownership.units_sddl())
            }
            Ok(Some(_)) => release.open_child_directory(name, WindowsDirectoryAccess::Write),
            Err(error) => Err(error),
        }
        .map_err(|source| self.open_root_error(&path, source))?;
        self.require_installer_owned(&directory, &path)?;
        Ok(directory)
    }

    /// Remove the slots, which are gone already when the directory is.
    ///
    /// # Errors
    /// Returns an error for a foreign lock or a failed removal.
    pub(crate) fn remove_bootstrap_directory(
        &self,
        lock: &InstallLock,
    ) -> Result<(), InstallStoreError> {
        let release = self.release_directory(lock)?;
        let name = OsStr::new(BOOTSTRAP_DIRECTORY);
        let remove = || -> io::Result<()> {
            if release.entry_metadata(name)?.is_some() {
                release.remove_tree(name)?;
            }
            Ok(())
        };
        remove().map_err(|source| InstallStoreError::RemoveUnit {
            name: BOOTSTRAP_DIRECTORY.to_owned(),
            source,
        })
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

/// The journal, the pointer and the units, for a reader that holds no lock.
///
/// Every read goes through the root handles this retained, and every
/// record and program is judged by the handle its contents come from.
#[derive(Debug)]
pub struct WindowsReleaseRecords {
    store: InstallStore,
    release: WindowsDirectory,
    state: WindowsDirectory,
}

impl WindowsReleaseRecords {
    /// The path of `program` at the root of `unit`, once the units
    /// directory, the unit and the program are all proven to be the
    /// installer's own.
    ///
    /// # Errors
    /// Returns an error for a missing or unsafe unit or program. A prior
    /// that is gone stays an error: nothing else may stand in for it.
    pub fn unit_program(&self, unit: &UnitId, program: &str) -> Result<PathBuf, InstallStoreError> {
        let store = &self.store;
        let units_path = store.root.join(UNITS_DIRECTORY);
        let units = self
            .release
            .open_child_directory(OsStr::new(UNITS_DIRECTORY), WindowsDirectoryAccess::Read)
            .map_err(InstallStoreError::OpenUnits)?;
        let security = units
            .security()
            .map_err(|source| store.open_root_error(&units_path, source))?;
        store
            .ownership
            .require_units_directory(&security)
            .map_err(|refusal| InstallStoreError::Untrusted(units_path, refusal))?;
        let unit_path = store.unit_path(unit);
        let directory = units
            .open_child_directory(OsStr::new(unit.as_str()), WindowsDirectoryAccess::Read)
            .map_err(InstallStoreError::OpenUnit)?;
        store.require_installer_owned(&directory, &unit_path)?;
        let program_path = unit_path.join(program);
        let file = directory
            .open_file(OsStr::new(program))
            .map_err(InstallStoreError::OpenUnit)?;
        store.require_trusted_file(&file, &program_path)?;
        Ok(program_path)
    }

    /// Publish the update pipe's name to `user` and nobody else, in one
    /// step, so a reader never finds half a record.
    ///
    /// # Errors
    /// Returns an error for an unsafe state root or a failed write.
    pub fn publish_update_pipe(
        &self,
        name: &UpdatePipeName,
        user: &WindowsSid,
    ) -> Result<(), InstallStoreError> {
        let store = &self.store;
        let state = store.open_root(&store.state_root, WindowsDirectoryAccess::Write)?;
        let directory = store.activator_directory(&state)?;
        let bytes = name
            .encode_record()
            .map_err(|error| InstallStoreError::PublishPipe(io::Error::other(error)))?;
        let sddl = format!("D:P(A;;FA;;;SY)(A;;FR;;;{})", user.to_string_form());
        replace_durably_with(&directory, UPDATE_PIPE_RECORD, &bytes, Some(&sddl))
            .map_err(InstallStoreError::PublishPipe)
    }

    /// What the installer recorded about this installation.
    ///
    /// # Errors
    /// Returns an error for a missing, unsafe or invalid record.
    pub fn installation(&self) -> Result<WindowsInstallation, InstallStoreError> {
        read_installation(&self.store, &self.state)?.ok_or_else(|| {
            InstallStoreError::InvalidInstallation("the record is missing".to_owned())
        })
    }

    fn read_trusted(&self, name: &str, limit: usize) -> Result<Option<Vec<u8>>, InstallStoreError> {
        read_trusted(&self.store, &self.state, name, limit)
    }
}

fn read_installation(
    store: &InstallStore,
    state: &WindowsDirectory,
) -> Result<Option<WindowsInstallation>, InstallStoreError> {
    read_trusted(
        store,
        state,
        WINDOWS_INSTALLATION_FILE,
        MAX_INSTALLATION_BYTES,
    )?
    .map(|bytes| {
        WindowsInstallation::decode(&bytes).map_err(InstallStoreError::InvalidInstallation)
    })
    .transpose()
}

fn read_trusted(
    store: &InstallStore,
    state: &WindowsDirectory,
    name: &str,
    limit: usize,
) -> Result<Option<Vec<u8>>, InstallStoreError> {
    let file = match state.open_file(OsStr::new(name)) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(InstallStoreError::ReadRecord(error)),
    };
    store.require_trusted_file(&file, &store.state_root.join(name))?;
    let bytes = read_bounded(file, limit)
        .map_err(InstallStoreError::ReadRecord)?
        .ok_or(InstallStoreError::RecordTooLarge { limit })?;
    Ok(Some(bytes))
}

impl ReleaseRecords for WindowsReleaseRecords {
    fn read_journal(&mut self) -> io::Result<Option<Vec<u8>>> {
        self.read_trusted(INSTALL_JOURNAL_FILE, self.store.max_journal_bytes)
            .map_err(io::Error::other)
    }

    fn read_pointer(&mut self) -> io::Result<Option<Vec<u8>>> {
        self.read_trusted(ACTIVE_POINTER_FILE, MAX_ACTIVE_POINTER_BYTES)
            .map_err(io::Error::other)
    }
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
pub(super) fn replace_durably(
    directory: &WindowsDirectory,
    name: &str,
    bytes: &[u8],
) -> io::Result<()> {
    replace_durably_with(directory, name, bytes, None)
}

/// [`replace_durably`], for a file that carries exactly `sddl` when one is
/// given and whatever the directory passes on otherwise.
fn replace_durably_with(
    directory: &WindowsDirectory,
    name: &str,
    bytes: &[u8],
    sddl: Option<&str>,
) -> io::Result<()> {
    for _ in 0..MAX_STAGE_ATTEMPTS {
        let sequence = TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temporary = format!(".{name}.{}.{sequence}", std::process::id());
        let created = match sddl {
            Some(sddl) => directory.create_file_with_security(OsStr::new(&temporary), sddl),
            None => directory.create_file(OsStr::new(&temporary)),
        };
        let mut file = match created {
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
    #[error("failed to read an installation record: {0}")]
    ReadRecord(io::Error),
    #[error("failed to write an installation record: {0}")]
    WriteRecord(io::Error),
    #[error("an installation record exceeds {limit} bytes")]
    RecordTooLarge { limit: usize },
    #[error("invalid installation record: {0}")]
    InvalidInstallation(String),
    #[error("failed to publish the update pipe: {0}")]
    PublishPipe(io::Error),
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
