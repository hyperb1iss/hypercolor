//! Permanent discovery authority at the historical installation journal path.

use std::io::{self, Read as _};
use std::path::{Path, PathBuf};

use hypercolor_platform_fs::{DirectoryAuthority, ExactEntry, PublicDirectoryAuthority};
use uuid::Uuid;

use super::super::{
    INSTALL_JOURNAL_SCHEMA_VERSION, InstallJournalV1, InstallLock, InstallPlatform,
    InstallPlatformError, InstallStore, InstallStoreError, MAX_INSTALL_JOURNAL_BYTES,
    MAX_MANAGED_INSTALL_JOURNAL_BYTES,
};
use super::location::{InstallLocationError, LinuxInstallLocation};
use super::locator_receipt::{AdoptionPreparation, MAX_PREPARATION_BYTES, RECEIPT_NAME};

#[path = "election.rs"]
mod election;
#[path = "locator_preparation.rs"]
mod preparation;
pub use election::{
    LinuxInstallElection, LinuxManagedAuthority, elect_linux_installation,
    elect_linux_installation_with,
};

const LOCATOR_NAME: &str = "install-journal.json";
/// Adoption target recorded beside the locator before any new root exists.
///
/// Only managed-aware installers read it. It keeps an interrupted adoption on
/// the roots it started with, whatever the environment says on a rerun, and
/// lets uninstall find roots prepared before the locator was published.
const INTENT_NAME: &str = "managed-adoption.json";
const MAX_LOCATOR_BYTES: u64 = MAX_INSTALL_JOURNAL_BYTES as u64;

#[cfg(test)]
#[path = "locator_tests.rs"]
mod tests;

/// Authority selected by the fixed historical journal, without fallback.
#[derive(Debug)]
pub enum LinuxInstallAuthority {
    Legacy(Option<InstallJournalV1>),
    Managed(LinuxInstallLocation),
}

/// Failure to prove the permanent discovery authority.
#[derive(Debug, thiserror::Error)]
pub enum LinuxLocatorError {
    #[error("locator requires the exact historical installation lock")]
    WrongLock,
    #[error("installation locator is oversized, malformed or changed during observation")]
    InvalidLocator,
    #[error("managed installation preparation is incomplete or has another identity")]
    Unprepared,
    #[error("managed installation authority cannot be replaced")]
    AlreadyManaged,
    #[error(
        "installation directory {path} is not writable only by you: {refusal}",
        path = .0.display(),
        refusal = .1
    )]
    UnsafeDirectory(PathBuf, crate::install::DirectoryRefusal),
    #[error("installation locator filesystem operation failed: {0}")]
    Io(#[from] io::Error),
    #[error("installation locator JSON is invalid: {0}")]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Location(#[from] InstallLocationError),
    #[error(transparent)]
    Store(#[from] InstallStoreError),
    #[error("legacy platform preparation could not be proven: {0}")]
    Platform(#[from] InstallPlatformError),
}

/// Retained historical directory guarded by its original install lock.
#[derive(Debug)]
pub struct LinuxInstallLocator {
    home: PathBuf,
    public: PublicDirectoryAuthority,
    directory: DirectoryAuthority,
}

impl LinuxInstallLocator {
    /// Retain the permanent locator under the exact historical store lock.
    ///
    /// # Errors
    /// Returns an error for a foreign lock or unsafe directory ancestry.
    pub fn retain(home: &Path, lock: &InstallLock) -> Result<Self, LinuxLocatorError> {
        let root = home.join(".local/lib/hypercolor");
        if !lock.guards_roots(&root, &root) {
            return Err(LinuxLocatorError::WrongLock);
        }
        Ok(Self {
            home: home.to_path_buf(),
            public: lock.open_public_directory(&root)?,
            directory: lock
                .open_public_directory(&root)?
                .into_directory_authority()?,
        })
    }

    /// Read the selected authority. Invalid V2 never becomes legacy V1.
    ///
    /// # Errors
    /// Returns an error for unknown schemas, malformed journals or changed files.
    pub fn read(&self) -> Result<LinuxInstallAuthority, LinuxLocatorError> {
        let (_, bytes) = self.observe()?;
        self.decode(bytes)
    }

    fn decode(&self, bytes: Option<Vec<u8>>) -> Result<LinuxInstallAuthority, LinuxLocatorError> {
        Self::decode_at(&self.home, bytes)
    }

    fn decode_at(
        home: &Path,
        bytes: Option<Vec<u8>>,
    ) -> Result<LinuxInstallAuthority, LinuxLocatorError> {
        let Some(bytes) = bytes else {
            return Ok(LinuxInstallAuthority::Legacy(None));
        };
        let value: serde_json::Value = serde_json::from_slice(&bytes)?;
        match value
            .get("schema_version")
            .and_then(serde_json::Value::as_u64)
        {
            Some(schema) if schema == u64::from(INSTALL_JOURNAL_SCHEMA_VERSION) => {
                let journal: InstallJournalV1 = serde_json::from_slice(&bytes)?;
                journal
                    .validate()
                    .map_err(|_| LinuxLocatorError::InvalidLocator)?;
                Ok(LinuxInstallAuthority::Legacy(Some(journal)))
            }
            Some(2) => Ok(LinuxInstallAuthority::Managed(LinuxInstallLocation::parse(
                &bytes, home,
            )?)),
            _ => Err(LinuxLocatorError::InvalidLocator),
        }
    }

    /// Publish prepared managed authority without reverting after visibility.
    ///
    /// The state journal and installation identity must already be durable.
    /// Any error leaves activation forbidden; callers reread the locator and
    /// retry its directory barrier before proceeding under managed authority.
    ///
    /// # Errors
    /// Returns an error for incomplete preparation, existing managed authority,
    /// changed ancestry or failed publication/durability. An error never means
    /// that the old authority remains selected.
    pub fn publish_prepared(
        &self,
        location: &LinuxInstallLocation,
        state_store: &InstallStore,
        state_lock: &InstallLock,
        platform: &mut impl InstallPlatform,
    ) -> Result<(), LinuxLocatorError> {
        self.publish_prepared_with(
            location,
            state_store,
            state_lock,
            platform,
            DirectoryAuthority::durable_replace_file,
        )
    }

    fn publish_prepared_with(
        &self,
        location: &LinuxInstallLocation,
        state_store: &InstallStore,
        state_lock: &InstallLock,
        platform: &mut impl InstallPlatform,
        publish: impl FnOnce(&DirectoryAuthority, &Path, &Path) -> io::Result<()>,
    ) -> Result<(), LinuxLocatorError> {
        preparation::require_state(location, state_store, state_lock)?;
        let journal = state_store
            .load_journal(state_lock)?
            .ok_or(LinuxLocatorError::Unprepared)?;
        preparation::require_initial(&journal)?;
        let expected = self.capture_preparation(location, &journal)?;
        let roots = location.retain_existing(&self.home, state_lock)?;
        let state = state_lock.open_public_directory(location.state_root())?;
        let mut receipt = state.open_regular_file(Path::new(RECEIPT_NAME))?;
        preparation::require_file_owner(receipt.metadata(), location.uid())?;
        let mut receipt_bytes = Vec::new();
        receipt
            .file_mut()
            .take(MAX_PREPARATION_BYTES + 1)
            .read_to_end(&mut receipt_bytes)?;
        if receipt_bytes.len() as u64 > MAX_PREPARATION_BYTES
            || serde_json::from_slice::<AdoptionPreparation>(&receipt_bytes)? != expected
        {
            return Err(LinuxLocatorError::Unprepared);
        }
        let mut identity = state.open_regular_file(Path::new("installation.json"))?;
        preparation::require_file_owner(identity.metadata(), location.uid())?;
        let mut bytes = Vec::new();
        identity
            .file_mut()
            .take(MAX_LOCATOR_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_LOCATOR_BYTES
            || LinuxInstallLocation::parse(&bytes, &self.home)? != *location
        {
            return Err(LinuxLocatorError::Unprepared);
        }
        preparation::validate_platform(platform, &journal)?;
        receipt.file().sync_all()?;
        identity.file().sync_all()?;
        let journal_file = state.open_regular_file(Path::new(LOCATOR_NAME))?;
        preparation::require_file_owner(journal_file.metadata(), location.uid())?;
        journal_file.file().sync_all()?;
        state_lock
            .open_public_directory(location.state_root())?
            .into_directory_authority()?
            .sync()?;
        state.validate_ancestry()?;
        if state_store.load_journal(state_lock)?.as_ref() != Some(&journal) {
            return Err(LinuxLocatorError::Unprepared);
        }
        let bytes = serde_json::to_vec(location)?;
        let staging = PathBuf::from(format!(".managed-location-{}.tmp", Uuid::new_v4()));
        self.directory.create_regular_file(
            &staging,
            0o600,
            bytes.len() as u64,
            &mut bytes.as_slice(),
        )?;
        roots.validate()?;
        if self.capture_preparation(location, &journal)? != expected {
            return Err(LinuxLocatorError::InvalidLocator);
        }
        self.public.validate_ancestry()?;
        // This existing primitive deliberately preserves a visible replacement
        // when its parent fsync fails. Never use rollback-on-error publication.
        publish(&self.directory, &staging, Path::new(LOCATOR_NAME))?;
        self.public.validate_ancestry()?;
        Ok(())
    }

    /// The adoption target recorded before any new root was prepared.
    ///
    /// # Errors
    /// Refuses a foreign-owned, writable, oversized or invalid intent.
    pub fn adoption_intent(&self) -> Result<Option<LinuxInstallLocation>, LinuxLocatorError> {
        let mut file = match self.public.open_regular_file(Path::new(INTENT_NAME)) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let mut bytes = Vec::new();
        file.file_mut()
            .take(MAX_LOCATOR_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_LOCATOR_BYTES {
            return Err(LinuxLocatorError::InvalidLocator);
        }
        let location = LinuxInstallLocation::parse(&bytes, &self.home)?;
        preparation::require_file_owner(file.metadata(), location.uid())?;
        Ok(Some(location))
    }

    /// Durably record `proposed` as the adoption target unless one exists.
    ///
    /// Returns the recorded target, which wins over `proposed`. The caller
    /// holds the historical lock, so no other installer races the record.
    ///
    /// # Errors
    /// Refuses managed authority or a failed durable write.
    pub fn record_adoption_intent(
        &self,
        proposed: LinuxInstallLocation,
    ) -> Result<LinuxInstallLocation, LinuxLocatorError> {
        if matches!(self.read()?, LinuxInstallAuthority::Managed(_)) {
            return Err(LinuxLocatorError::AlreadyManaged);
        }
        if let Some(recorded) = self.adoption_intent()? {
            return Ok(recorded);
        }
        let bytes = serde_json::to_vec(&proposed)?;
        self.public.validate_ancestry()?;
        self.directory.create_regular_file(
            Path::new(INTENT_NAME),
            0o600,
            bytes.len() as u64,
            &mut bytes.as_slice(),
        )?;
        self.public.validate_ancestry()?;
        Ok(proposed)
    }

    /// Retry the locator directory barrier before any managed transitions.
    ///
    /// # Errors
    /// Returns an error unless the exact managed identity is visible and its
    /// original directory ancestry and successful fsync are proven.
    pub fn confirm_durable(
        &self,
        expected: &LinuxInstallLocation,
    ) -> Result<(), LinuxLocatorError> {
        match self.read()? {
            LinuxInstallAuthority::Managed(actual) if actual == *expected => {}
            _ => return Err(LinuxLocatorError::InvalidLocator),
        }
        self.public.validate_ancestry()?;
        let locator_file = self.public.open_regular_file(Path::new(LOCATOR_NAME))?;
        preparation::require_file_owner(locator_file.metadata(), expected.uid())?;
        self.directory.sync()?;
        self.public.validate_ancestry()?;
        match self.read()? {
            LinuxInstallAuthority::Managed(actual) if actual == *expected => {}
            _ => return Err(LinuxLocatorError::InvalidLocator),
        }
        Ok(())
    }

    fn observe(&self) -> Result<(ExactEntry, Option<Vec<u8>>), LinuxLocatorError> {
        let name = Path::new(LOCATOR_NAME);
        let exact = self.public.observe_entry(name)?;
        if exact == ExactEntry::Absent {
            return Ok((exact, None));
        }
        let mut file = self.public.open_regular_file(name)?;
        if file.metadata().size() > MAX_LOCATOR_BYTES {
            return Err(LinuxLocatorError::InvalidLocator);
        }
        let mut bytes = Vec::new();
        file.file_mut()
            .take(MAX_LOCATOR_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_LOCATOR_BYTES || self.public.observe_entry(name)? != exact {
            return Err(LinuxLocatorError::InvalidLocator);
        }
        Ok((exact, Some(bytes)))
    }
}
