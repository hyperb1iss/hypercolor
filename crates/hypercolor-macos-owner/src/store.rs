use std::fs::{self, OpenOptions};
use std::path::PathBuf;

use crate::coordinator_error::MacosOwnerExecutionError;
use crate::error::MacosOwnerStoreError;
#[cfg(target_os = "macos")]
use crate::guard::MacosDaemonGuard;
use crate::journal::{MacosHandoverJournal, MacosHandoverTransactionId, validate_handover_journal};
#[cfg(target_os = "macos")]
use crate::model::MacosServerSessionId;
use crate::model::{
    MACOS_DAEMON_SESSION_ATTESTATION_FILE_NAME, MACOS_HANDOVER_JOURNAL_FILE_NAME,
    MACOS_HANDOVER_JOURNAL_SCHEMA_VERSION, MACOS_OWNER_COORDINATION_LOCK_FILE_NAME,
    MACOS_OWNER_RECORD_FILE_NAME, MacosConflictUpdate, MacosDaemonOwner,
    MacosDaemonSessionAttestation, MacosExternalOwnerMode, MacosHandoverPhase,
    MacosOwnerConflictRecord, MacosOwnerIdentity, MacosOwnerIncarnation, MacosOwnerRecord,
};
#[cfg(target_os = "macos")]
use crate::store_io::sync_parent_directory;
use crate::store_io::{
    CoordinationLock, read_daemon_session_attestation, read_handover_journal, read_owner_record,
    successor_owner_record, write_json_atomic,
};
#[cfg(target_os = "macos")]
use crate::validation::validate_daemon_session_attestation;

/// Durable owner state rooted in an explicit per-user data directory.
#[derive(Debug, Clone)]
pub struct MacosOwnerStore {
    data_dir: PathBuf,
}

impl MacosOwnerStore {
    /// Construct a store without reading or creating any files.
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
        }
    }

    /// Return the owner-record path.
    pub fn owner_record_path(&self) -> PathBuf {
        self.data_dir.join(MACOS_OWNER_RECORD_FILE_NAME)
    }

    /// Return the handover-journal path.
    pub fn handover_journal_path(&self) -> PathBuf {
        self.data_dir.join(MACOS_HANDOVER_JOURNAL_FILE_NAME)
    }

    /// Return the daemon-session attestation path.
    pub fn daemon_session_attestation_path(&self) -> PathBuf {
        self.data_dir
            .join(MACOS_DAEMON_SESSION_ATTESTATION_FILE_NAME)
    }

    /// Return the stable lock path shared by every writer.
    pub fn coordination_lock_path(&self) -> PathBuf {
        self.data_dir.join(MACOS_OWNER_COORDINATION_LOCK_FILE_NAME)
    }

    /// Load and validate the current owner record.
    pub fn load_owner_record(&self) -> Result<Option<MacosOwnerRecord>, MacosOwnerStoreError> {
        read_owner_record(&self.owner_record_path())
    }

    /// Load a private session attestation only when its exact owner is current.
    ///
    /// Artifact presence is not ownership authority. Callers that need owner
    /// authority must independently verify the canonical daemon guard.
    pub fn load_daemon_session_attestation(
        &self,
    ) -> Result<Option<MacosDaemonSessionAttestation>, MacosOwnerStoreError> {
        let _lock = self.acquire_coordination_lock()?;
        let Some(attestation) =
            read_daemon_session_attestation(&self.daemon_session_attestation_path())?
        else {
            return Ok(None);
        };
        let current = read_owner_record(&self.owner_record_path())?
            .ok_or(MacosOwnerStoreError::MissingOwnerRecord)?;
        if attestation.owner_incarnation() != current.incarnation() {
            return Err(MacosOwnerStoreError::InvalidArtifact {
                artifact: "daemon session attestation",
                detail: "owner topology, epoch, or identity is not current",
            });
        }
        Ok(Some(attestation))
    }

    /// Publish a new private process session for the exact guard-winning owner.
    #[cfg(target_os = "macos")]
    pub fn publish_daemon_session_attestation(
        &self,
        _guard: &MacosDaemonGuard,
        expected_owner: &MacosOwnerIncarnation,
    ) -> Result<MacosDaemonSessionAttestation, MacosOwnerStoreError> {
        let _lock = self.acquire_coordination_lock()?;
        let current = read_owner_record(&self.owner_record_path())?
            .ok_or(MacosOwnerStoreError::MissingOwnerRecord)?;
        if current.incarnation() != *expected_owner {
            return Err(MacosOwnerStoreError::InvalidArtifact {
                artifact: "daemon session attestation",
                detail: "current owner does not match the guard-winning incarnation",
            });
        }
        let attestation = MacosDaemonSessionAttestation::generate(&current)?;
        validate_daemon_session_attestation(&attestation)?;
        write_json_atomic(
            &self.data_dir,
            &self.daemon_session_attestation_path(),
            "daemon session attestation",
            &attestation,
        )?;
        Ok(attestation)
    }

    /// Clear only the exact current owner's matching process session.
    #[cfg(target_os = "macos")]
    pub fn clear_daemon_session_attestation(
        &self,
        expected_owner: &MacosOwnerIncarnation,
        expected_session: &MacosServerSessionId,
    ) -> Result<bool, MacosOwnerStoreError> {
        let _lock = self.acquire_coordination_lock()?;
        let current = read_owner_record(&self.owner_record_path())?
            .ok_or(MacosOwnerStoreError::MissingOwnerRecord)?;
        if current.incarnation() != *expected_owner {
            return Err(MacosOwnerStoreError::InvalidArtifact {
                artifact: "daemon session attestation",
                detail: "current owner does not match the clearing incarnation",
            });
        }
        let path = self.daemon_session_attestation_path();
        let Some(attestation) = read_daemon_session_attestation(&path)? else {
            return Ok(false);
        };
        if attestation.owner_incarnation() != *expected_owner
            || attestation.server_session_id != *expected_session
        {
            return Err(MacosOwnerStoreError::InvalidArtifact {
                artifact: "daemon session attestation",
                detail: "identity, epoch, or server session does not match",
            });
        }
        fs::remove_file(&path).map_err(|source| {
            MacosOwnerStoreError::RemoveSessionAttestation {
                path: path.clone(),
                source,
            }
        })?;
        sync_parent_directory(&self.data_dir)?;
        Ok(true)
    }

    /// Issue one stop request only while the exact owner publication remains current.
    ///
    /// The callback runs under the same coordination lock used by owner publication,
    /// so it must not write through this store or wait for the daemon guard.
    ///
    /// # Errors
    ///
    /// Returns an error when the owner record is unavailable or no longer matches,
    /// when the coordination lock cannot be acquired, or when the request fails.
    pub fn request_stop_if_current(
        &self,
        expected: &MacosOwnerIncarnation,
        request: impl FnOnce() -> Result<(), MacosOwnerExecutionError>,
    ) -> Result<(), MacosOwnerExecutionError> {
        let _lock = self
            .acquire_coordination_lock()
            .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
        let current = read_owner_record(&self.owner_record_path())
            .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?
            .ok_or_else(|| MacosOwnerExecutionError::new("macOS owner record is unavailable"))?;
        if current.incarnation() != *expected {
            return Err(MacosOwnerExecutionError::new(
                "macOS owner incarnation changed before the stop request",
            ));
        }
        request()
    }

    /// Publish a newly acquired owner and advance the durable owner epoch.
    ///
    /// The locked record supplies the persisted external-owner mode and any
    /// distinct contender, so publication cannot overwrite a concurrent choice
    /// or erase a contender that arrived before the winning owner published.
    pub fn publish_owner(
        &self,
        active_owner: MacosDaemonOwner,
        active_identity: MacosOwnerIdentity,
    ) -> Result<MacosOwnerRecord, MacosOwnerStoreError> {
        let _lock = self.acquire_coordination_lock()?;
        let path = self.owner_record_path();
        let record = match read_owner_record(&path)? {
            Some(previous) => successor_owner_record(previous, active_owner, active_identity)?,
            None => MacosOwnerRecord::new(active_owner, active_identity, None),
        };
        write_json_atomic(&self.data_dir, &path, "owner record", &record)?;
        Ok(record)
    }

    /// Publish an owner that already holds the authoritative daemon guard.
    ///
    /// The guard token permits repair of a corrupt diagnostic owner record.
    /// Ordinary store mutations continue to reject the same invalid bytes.
    #[cfg(target_os = "macos")]
    pub fn publish_guard_winner(
        &self,
        _guard: &MacosDaemonGuard,
        active_owner: MacosDaemonOwner,
        active_identity: MacosOwnerIdentity,
    ) -> Result<MacosOwnerRecord, MacosOwnerStoreError> {
        let _lock = self.acquire_coordination_lock()?;
        let path = self.owner_record_path();
        let previous = read_owner_record(&path).ok().flatten();
        let record = previous
            .and_then(|previous| {
                successor_owner_record(previous, active_owner, active_identity.clone()).ok()
            })
            .unwrap_or_else(|| MacosOwnerRecord::new(active_owner, active_identity, None));
        write_json_atomic(&self.data_dir, &path, "owner record", &record)?;
        Ok(record)
    }

    /// Record a distinct contender or coalesce one already observed this epoch.
    pub fn record_conflict(
        &self,
        contender_owner: MacosDaemonOwner,
        contender_identity: MacosOwnerIdentity,
        observed_at_ms: u64,
    ) -> Result<MacosConflictUpdate, MacosOwnerStoreError> {
        let _lock = self.acquire_coordination_lock()?;
        let path = self.owner_record_path();
        let mut record =
            read_owner_record(&path)?.ok_or(MacosOwnerStoreError::MissingOwnerRecord)?;
        let conflict = MacosOwnerConflictRecord {
            active_owner: record.active_owner,
            active_epoch: record.owner_epoch,
            contender_owner,
            contender_identity,
            observed_at_ms,
        };
        if record
            .conflict
            .as_ref()
            .is_some_and(|existing| existing.has_same_identity(&conflict))
        {
            return Ok(MacosConflictUpdate::Coalesced(record.snapshot()));
        }
        record.conflict = Some(conflict);
        write_json_atomic(&self.data_dir, &path, "owner record", &record)?;
        Ok(MacosConflictUpdate::Recorded(record.snapshot()))
    }

    /// Clear the current conflict without changing the owner epoch.
    pub fn clear_conflict(&self) -> Result<MacosOwnerRecord, MacosOwnerStoreError> {
        let _lock = self.acquire_coordination_lock()?;
        let path = self.owner_record_path();
        let mut record =
            read_owner_record(&path)?.ok_or(MacosOwnerStoreError::MissingOwnerRecord)?;
        if record.conflict.take().is_some() {
            write_json_atomic(&self.data_dir, &path, "owner record", &record)?;
        }
        Ok(record)
    }

    /// Persist or clear the selected external-owner mode.
    pub fn set_external_owner_mode(
        &self,
        selected_external_owner: Option<MacosExternalOwnerMode>,
    ) -> Result<MacosOwnerRecord, MacosOwnerStoreError> {
        let _lock = self.acquire_coordination_lock()?;
        let path = self.owner_record_path();
        let mut record =
            read_owner_record(&path)?.ok_or(MacosOwnerStoreError::MissingOwnerRecord)?;
        if record.selected_external_owner != selected_external_owner {
            record.selected_external_owner = selected_external_owner;
            write_json_atomic(&self.data_dir, &path, "owner record", &record)?;
        }
        Ok(record)
    }

    /// Load and validate the current handover journal.
    pub fn load_handover_journal(
        &self,
    ) -> Result<Option<MacosHandoverJournal>, MacosOwnerStoreError> {
        read_handover_journal(&self.handover_journal_path())
    }

    /// Begin a handover unless a nonterminal journal requires recovery.
    pub fn begin_handover(
        &self,
        mut journal: MacosHandoverJournal,
    ) -> Result<MacosHandoverJournal, MacosOwnerStoreError> {
        let _lock = self.acquire_coordination_lock()?;
        let path = self.handover_journal_path();
        if let Some(existing) = read_handover_journal(&path)?
            && !existing.phase.is_terminal()
        {
            return Err(MacosOwnerStoreError::HandoverAlreadyPending {
                transaction_id: existing.transaction_id.0,
            });
        }
        validate_handover_journal(&journal)?;
        journal.schema_version = MACOS_HANDOVER_JOURNAL_SCHEMA_VERSION;
        journal.journal_revision = 1;
        journal.phase = MacosHandoverPhase::Prepared;
        write_json_atomic(&self.data_dir, &path, "handover journal", &journal)?;
        Ok(journal)
    }

    /// Durably advance one handover phase under one read-modify-write lock hold.
    pub fn advance_handover(
        &self,
        transaction_id: &MacosHandoverTransactionId,
        phase: MacosHandoverPhase,
    ) -> Result<MacosHandoverJournal, MacosOwnerStoreError> {
        let _lock = self.acquire_coordination_lock()?;
        let path = self.handover_journal_path();
        let mut journal =
            read_handover_journal(&path)?.ok_or(MacosOwnerStoreError::MissingHandoverJournal)?;
        if journal.transaction_id != *transaction_id {
            return Err(MacosOwnerStoreError::HandoverTransactionMismatch);
        }
        if journal.phase.is_terminal() {
            return Err(MacosOwnerStoreError::TerminalHandover {
                transaction_id: journal.transaction_id.0,
            });
        }
        journal.journal_revision = journal
            .journal_revision
            .checked_add(1)
            .ok_or(MacosOwnerStoreError::JournalRevisionOverflow)?;
        journal.phase = phase;
        write_json_atomic(&self.data_dir, &path, "handover journal", &journal)?;
        Ok(journal)
    }

    /// Atomically advance one handover phase when its predecessor still matches.
    pub fn advance_handover_from(
        &self,
        transaction_id: &MacosHandoverTransactionId,
        expected_phase: MacosHandoverPhase,
        phase: MacosHandoverPhase,
    ) -> Result<MacosHandoverJournal, MacosOwnerStoreError> {
        let _lock = self.acquire_coordination_lock()?;
        let path = self.handover_journal_path();
        let mut journal =
            read_handover_journal(&path)?.ok_or(MacosOwnerStoreError::MissingHandoverJournal)?;
        if journal.transaction_id != *transaction_id {
            return Err(MacosOwnerStoreError::HandoverTransactionMismatch);
        }
        if journal.phase != expected_phase {
            return Err(MacosOwnerStoreError::HandoverPhaseChanged {
                expected: expected_phase,
                found: journal.phase,
            });
        }
        if journal.phase.is_terminal() {
            return Err(MacosOwnerStoreError::TerminalHandover {
                transaction_id: journal.transaction_id.0,
            });
        }
        journal.journal_revision = journal
            .journal_revision
            .checked_add(1)
            .ok_or(MacosOwnerStoreError::JournalRevisionOverflow)?;
        journal.phase = phase;
        write_json_atomic(&self.data_dir, &path, "handover journal", &journal)?;
        Ok(journal)
    }

    pub(crate) fn bind_requested_epoch(
        &self,
        transaction_id: &MacosHandoverTransactionId,
        requested_owner: MacosDaemonOwner,
        requested_epoch: u64,
    ) -> Result<MacosHandoverJournal, MacosOwnerStoreError> {
        let _lock = self.acquire_coordination_lock()?;
        let path = self.handover_journal_path();
        let mut journal =
            read_handover_journal(&path)?.ok_or(MacosOwnerStoreError::MissingHandoverJournal)?;
        if journal.transaction_id != *transaction_id {
            return Err(MacosOwnerStoreError::HandoverTransactionMismatch);
        }
        if journal.phase.is_terminal() {
            return Ok(journal);
        }
        if journal
            .contender_epoch
            .is_some_and(|epoch| epoch > journal.active_epoch)
        {
            return if journal.contender_epoch == Some(requested_epoch) {
                Ok(journal)
            } else {
                Err(MacosOwnerStoreError::InvalidArtifact {
                    artifact: "handover journal",
                    detail: "requested owner epoch changed after it was bound",
                })
            };
        }
        if requested_owner != journal.requested_owner || requested_epoch <= journal.active_epoch {
            return Err(MacosOwnerStoreError::InvalidArtifact {
                artifact: "handover journal",
                detail: "requested owner incarnation does not match the transaction",
            });
        }
        journal.journal_revision = journal
            .journal_revision
            .checked_add(1)
            .ok_or(MacosOwnerStoreError::JournalRevisionOverflow)?;
        journal.contender_epoch = Some(requested_epoch);
        validate_handover_journal(&journal)?;
        write_json_atomic(&self.data_dir, &path, "handover journal", &journal)?;
        Ok(journal)
    }

    fn acquire_coordination_lock(&self) -> Result<CoordinationLock, MacosOwnerStoreError> {
        fs::create_dir_all(&self.data_dir).map_err(|source| {
            MacosOwnerStoreError::CreateDirectory {
                path: self.data_dir.clone(),
                source,
            }
        })?;
        let path = self.coordination_lock_path();
        let mut options = OpenOptions::new();
        options.create(true).read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file =
            options
                .open(&path)
                .map_err(|source| MacosOwnerStoreError::OpenCoordinationLock {
                    path: path.clone(),
                    source,
                })?;
        file.lock()
            .map_err(|source| MacosOwnerStoreError::AcquireCoordinationLock { path, source })?;
        Ok(CoordinationLock { file })
    }
}
