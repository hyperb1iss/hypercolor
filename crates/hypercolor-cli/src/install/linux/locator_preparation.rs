use std::io::{self, Read as _};
use std::path::Path;

use crate::install::{
    InstallAction, InstallDisposition, InstallJournalV1, InstallLock, InstallPlatform,
    InstallStore, PlatformCheckpoint,
};

use super::super::locator_receipt::{AdoptionPreparation, MAX_PREPARATION_BYTES, RECEIPT_NAME};
use super::{LinuxInstallAuthority, LinuxInstallLocation, LinuxInstallLocator, LinuxLocatorError};

impl LinuxInstallLocator {
    /// Bind exact legacy observations to the intended initial managed journal.
    ///
    /// The receipt is durable before the caller writes the state journal. A
    /// retry may reuse only the identical receipt and unchanged legacy state.
    /// Existing state journals without that receipt cannot be adopted.
    ///
    /// # Errors
    /// Returns an error for changed legacy state, an unrelated orphan journal,
    /// refused platform proof or failed durable receipt publication.
    pub fn prepare_adoption(
        &self,
        location: &LinuxInstallLocation,
        journal: &InstallJournalV1,
        state_store: &InstallStore,
        state_lock: &InstallLock,
        platform: &mut impl InstallPlatform,
    ) -> Result<(), LinuxLocatorError> {
        require_initial(journal)?;
        require_state(location, state_store, state_lock)?;
        let expected = self.capture_preparation(location, journal)?;
        validate_platform(platform, journal)?;
        let state = state_lock.open_public_directory(location.state_root())?;
        match state.open_regular_file(Path::new(RECEIPT_NAME)) {
            Ok(mut file) => {
                require_file_owner(file.metadata(), location.uid())?;
                let mut bytes = Vec::new();
                file.file_mut()
                    .take(MAX_PREPARATION_BYTES + 1)
                    .read_to_end(&mut bytes)?;
                if bytes.len() as u64 > MAX_PREPARATION_BYTES
                    || serde_json::from_slice::<AdoptionPreparation>(&bytes)? != expected
                {
                    return Err(LinuxLocatorError::Unprepared);
                }
                if let Some(existing) = state_store.load_journal(state_lock)?
                    && existing != *journal
                {
                    return Err(LinuxLocatorError::Unprepared);
                }
                file.file().sync_all()?;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if state_store.load_journal(state_lock)?.is_some() {
                    return Err(LinuxLocatorError::Unprepared);
                }
                let bytes = serde_json::to_vec(&expected)?;
                if bytes.len() as u64 > MAX_PREPARATION_BYTES {
                    return Err(LinuxLocatorError::Unprepared);
                }
                state_lock
                    .open_public_directory(location.state_root())?
                    .into_directory_authority()?
                    .create_regular_file(
                        Path::new(RECEIPT_NAME),
                        0o600,
                        bytes.len() as u64,
                        &mut bytes.as_slice(),
                    )?;
            }
            Err(error) => return Err(error.into()),
        }
        state_lock
            .open_public_directory(location.state_root())?
            .into_directory_authority()?
            .sync()?;
        state.validate_ancestry()?;
        if self.capture_preparation(location, journal)? != expected {
            return Err(LinuxLocatorError::Unprepared);
        }
        Ok(())
    }

    /// Read an exact initial proposal retained before the state journal exists.
    ///
    /// This does not authorize publication; the live platform proof must still
    /// pass prepare_adoption and publish_prepared after restoring its bindings.
    ///
    /// # Errors
    /// Refuses corrupt receipts, inconsistent hashes or changed legacy authority.
    pub fn prepared_journal(
        &self,
        location: &LinuxInstallLocation,
        state_store: &InstallStore,
        state_lock: &InstallLock,
    ) -> Result<Option<InstallJournalV1>, LinuxLocatorError> {
        require_state(location, state_store, state_lock)?;
        let state = state_lock.open_public_directory(location.state_root())?;
        let mut file = match state.open_regular_file(Path::new(RECEIPT_NAME)) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        require_file_owner(file.metadata(), location.uid())?;
        let mut bytes = Vec::new();
        file.file_mut()
            .take(MAX_PREPARATION_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_PREPARATION_BYTES {
            return Err(LinuxLocatorError::Unprepared);
        }
        let receipt: AdoptionPreparation = serde_json::from_slice(&bytes)?;
        require_initial(&receipt.initial_journal)?;
        if self.capture_preparation(location, &receipt.initial_journal)? != receipt {
            return Err(LinuxLocatorError::Unprepared);
        }
        state.validate_ancestry()?;
        Ok(Some(receipt.initial_journal))
    }

    pub(super) fn capture_preparation(
        &self,
        location: &LinuxInstallLocation,
        journal: &InstallJournalV1,
    ) -> Result<AdoptionPreparation, LinuxLocatorError> {
        let (exact, bytes) = self.observe()?;
        match self.decode(bytes)? {
            LinuxInstallAuthority::Legacy(old) => {
                if old.is_some_and(|old| {
                    matches!(
                        old.disposition,
                        InstallDisposition::Forward | InstallDisposition::Rollback
                    )
                }) {
                    return Err(LinuxLocatorError::Unprepared);
                }
            }
            LinuxInstallAuthority::Managed(_) => return Err(LinuxLocatorError::AlreadyManaged),
        }
        AdoptionPreparation::capture(
            location.installation_id(),
            journal,
            &exact,
            &self.public.observe_entry(Path::new("active"))?,
        )
    }
}

pub(super) fn require_initial(journal: &InstallJournalV1) -> Result<(), LinuxLocatorError> {
    journal
        .validate()
        .map_err(|_| LinuxLocatorError::Unprepared)?;
    if journal.disposition != InstallDisposition::Forward
        || journal.next_action != Some(InstallAction::PreflightCandidate)
        || journal.revision != 1
        || journal.layout_operation_index != 0
    {
        return Err(LinuxLocatorError::Unprepared);
    }
    Ok(())
}

pub(super) fn require_state(
    location: &LinuxInstallLocation,
    store: &InstallStore,
    lock: &InstallLock,
) -> Result<(), LinuxLocatorError> {
    if store.root() != location.release_root()
        || store.state_root() != location.state_root()
        || !lock.guards_roots(location.release_root(), location.state_root())
    {
        return Err(LinuxLocatorError::Unprepared);
    }
    Ok(())
}

pub(super) fn validate_platform(
    platform: &mut impl InstallPlatform,
    journal: &InstallJournalV1,
) -> Result<(), LinuxLocatorError> {
    platform.validate_transaction_plan(
        &journal.prior_platform,
        &journal.target_platform,
        &journal.transition_states,
        journal.layout_operation_count,
        &journal.platform_record,
    )?;
    if platform.inspect()? != journal.prior_platform {
        return Err(LinuxLocatorError::Unprepared);
    }
    if !platform.matches_exact_state(
        PlatformCheckpoint::PriorOriginal,
        &journal.prior_platform,
        0,
        &journal.platform_record,
        None,
    )? {
        return Err(LinuxLocatorError::Unprepared);
    }
    Ok(())
}

pub(super) fn require_file_owner(
    metadata: hypercolor_platform_fs::DirectoryEntryMetadata,
    uid: u32,
) -> Result<(), LinuxLocatorError> {
    if metadata.owner_uid() != uid
        || !metadata.is_owned_by_current_user()
        || metadata.mode() & 0o022 != 0
    {
        return Err(LinuxLocatorError::Unprepared);
    }
    Ok(())
}
