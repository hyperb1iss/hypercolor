//! Prepare managed authority while the historical installer still owns activation.

use std::path::{Path, PathBuf};

use super::super::{
    InstallCoordinator, InstallCoordinatorError, InstallDisposition, InstallJournalV1, InstallLock,
    InstallPlatform, InstallPlatformError, InstallRequest, InstallStore, InstallStoreError,
    ReleasePayloadError, UnitRecord, copy_installed_release_unit,
};
use super::command::LinuxInstallCheckpoint;
use super::{
    LinuxInstallAuthority, LinuxInstallElection, LinuxInstallLocation, LinuxInstallLocator,
    LinuxLocatorError, LinuxManagedAuthority, retain_linux_unit,
};

#[derive(Debug, thiserror::Error)]
pub enum LinuxAdoptionError {
    #[error("legacy transaction must be recovered before managed adoption")]
    LegacyRecoveryRequired,
    #[error("managed adoption preparation belongs to another authority or candidate")]
    ConflictingPreparation,
    #[error(transparent)]
    Locator(#[from] LinuxLocatorError),
    #[error(transparent)]
    Store(#[from] InstallStoreError),
    #[error(transparent)]
    Payload(#[from] ReleasePayloadError),
    #[error(transparent)]
    Platform(#[from] InstallPlatformError),
    #[error(transparent)]
    Coordinator(#[from] InstallCoordinatorError),
    #[error("adoption stopped at a durable checkpoint: {0}")]
    Stopped(String),
}

/// A private managed preparation retaining old then new locks until publication.
pub struct LinuxAdoption {
    home: PathBuf,
    old_lock: InstallLock,
    locator: LinuxInstallLocator,
    location: LinuxInstallLocation,
    store: InstallStore,
    lock: InstallLock,
    prior: Option<UnitRecord>,
}

impl LinuxAdoption {
    /// Bootstrap recorded roots and copy the original active unit privately.
    ///
    /// The new pointer initially names a verified copy of the old logical unit.
    /// The old physical pointer and all public launcher/layout entries stay put.
    ///
    /// # Errors
    /// Refuses pending legacy transactions, unsafe roots and conflicting state.
    pub fn begin(
        home: &Path,
        elected: LinuxInstallElection,
        proposed: LinuxInstallLocation,
    ) -> Result<Self, LinuxAdoptionError> {
        Self::begin_observed(home, elected, proposed, &mut |_| Ok(()))
    }

    /// Begin adoption and report each durable preparation boundary.
    ///
    /// `observe` runs after the recorded roots and identity, the copied prior
    /// unit, and the new prior pointer each become durable. An observer error
    /// stops preparation at that boundary without further writes.
    ///
    /// # Errors
    /// Returns the same refusals as [`Self::begin`] or the observer's error.
    pub fn begin_observed(
        home: &Path,
        elected: LinuxInstallElection,
        proposed: LinuxInstallLocation,
        observe: &mut dyn FnMut(LinuxInstallCheckpoint) -> Result<(), LinuxAdoptionError>,
    ) -> Result<Self, LinuxAdoptionError> {
        let LinuxInstallElection::Legacy {
            store: old,
            lock: old_lock,
            locator,
        } = elected
        else {
            return Err(LinuxAdoptionError::ConflictingPreparation);
        };
        let old_root = home.join(".local/lib/hypercolor");
        if old.root() != old_root
            || old.state_root() != old_root
            || !old_lock.guards_roots(&old_root, &old_root)
        {
            return Err(LinuxAdoptionError::ConflictingPreparation);
        }
        // Rebind the fixed path to the elected lock rather than trusting a
        // separately supplied locator capability in a constructed enum value.
        drop(locator);
        let locator = LinuxInstallLocator::retain(home, &old_lock)?;
        match locator.read()? {
            LinuxInstallAuthority::Legacy(Some(journal))
                if matches!(
                    journal.disposition,
                    InstallDisposition::Forward | InstallDisposition::Rollback
                ) =>
            {
                return Err(LinuxAdoptionError::LegacyRecoveryRequired);
            }
            LinuxInstallAuthority::Legacy(_) => {}
            LinuxInstallAuthority::Managed(_) => {
                return Err(LinuxAdoptionError::ConflictingPreparation);
            }
        }
        let original_id = old.active_unit(&old_lock)?;
        let prior = original_id
            .as_ref()
            .map(|id| retain_linux_unit(&old, &old_lock, id))
            .transpose()?;
        let (location, store, lock) =
            super::adoption_roots::prepare_roots(home, &old_lock, proposed)?;
        observe(LinuxInstallCheckpoint::RootsBootstrapped)?;
        if let Some(prior) = &prior {
            copy_installed_release_unit(&store, &lock, prior)?;
            observe(LinuxInstallCheckpoint::PriorCopied)?;
        }
        // Before publication nothing reads the new pointer, so it simply
        // follows the historical active unit, including after another
        // installer changed that unit between interrupted attempts.
        if store.active_unit(&lock)? != original_id {
            store.set_active(original_id.as_ref(), &lock)?;
            observe(LinuxInstallCheckpoint::PriorActivated)?;
        }
        Ok(Self {
            home: home.to_owned(),
            old_lock,
            locator,
            location,
            store,
            lock,
            prior,
        })
    }

    #[must_use]
    pub fn store(&self) -> &InstallStore {
        &self.store
    }
    #[must_use]
    pub fn lock(&self) -> &InstallLock {
        &self.lock
    }
    #[must_use]
    pub fn location(&self) -> &LinuxInstallLocation {
        &self.location
    }
    #[must_use]
    pub fn original_prior(&self) -> Option<&UnitRecord> {
        self.prior.as_ref()
    }

    /// Return the identical initial proposal for cold prior-role reconstruction.
    ///
    /// # Errors
    /// Refuses an unbound journal or disagreement between the two durable copies.
    pub fn prepared_journal(&self) -> Result<Option<InstallJournalV1>, LinuxAdoptionError> {
        let receipt = self
            .locator
            .prepared_journal(&self.location, &self.store, &self.lock)?;
        let journal = self.store.load_journal(&self.lock)?;
        match (receipt, journal) {
            (None, None) => Ok(None),
            (Some(receipt), None) => Ok(Some(receipt)),
            (Some(receipt), Some(journal)) if receipt == journal => Ok(Some(journal)),
            _ => Err(LinuxAdoptionError::ConflictingPreparation),
        }
    }

    /// Discard an unpublished preparation that can no longer be resumed.
    ///
    /// # Errors
    /// Refuses once authority is managed or the journal has advanced.
    pub fn discard_unpublished_preparation(&self) -> Result<(), LinuxAdoptionError> {
        self.locator
            .discard_preparation(&self.location, &self.store, &self.lock)?;
        Ok(())
    }

    /// Prepare or resume the identical initial journal and persist its receipt.
    ///
    /// # Errors
    /// Refuses another candidate, changed old state or a journal lacking its
    /// original receipt. No service or public layout transition occurs here.
    pub fn prepare(
        &self,
        platform: &mut impl InstallPlatform,
        request: InstallRequest,
    ) -> Result<InstallJournalV1, LinuxAdoptionError> {
        let journal = if let Some(existing) = self.prepared_journal()? {
            if existing.candidate_unit != *request.candidate.id() {
                return Err(LinuxAdoptionError::ConflictingPreparation);
            }
            existing
        } else {
            InstallCoordinator::new(&self.store, platform).prepare_with_lock(request, &self.lock)?
        };
        self.locator.prepare_adoption(
            &self.location,
            &journal,
            &self.store,
            &self.lock,
            platform,
        )?;
        Ok(journal)
    }

    /// Publish prepared authority and hand back only the managed state lock.
    ///
    /// The existing coordinator recovery path drives activation afterward. Any
    /// error forbids activation, including an error after locator visibility.
    ///
    /// # Errors
    /// Refuses changed preparation, failed durability or failed managed rebind.
    pub fn publish(
        self,
        journal: &InstallJournalV1,
        platform: &mut impl InstallPlatform,
    ) -> Result<LinuxInstallElection, LinuxAdoptionError> {
        self.locator.prepare_adoption(
            &self.location,
            journal,
            &self.store,
            &self.lock,
            platform,
        )?;
        self.store.write_journal(journal, &self.lock)?;
        self.locator
            .publish_prepared(&self.location, &self.store, &self.lock, platform)?;
        self.locator.confirm_durable(&self.location)?;
        let authority =
            LinuxManagedAuthority::retain(&self.home, &self.store, &self.lock, self.location)?;
        drop(self.locator);
        drop(self.old_lock);
        Ok(LinuxInstallElection::Managed {
            store: self.store,
            lock: self.lock,
            authority,
        })
    }
}
