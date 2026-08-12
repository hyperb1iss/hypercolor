//! Durable macOS daemon ownership and handover state.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

/// Current owner-record schema version.
pub const MACOS_OWNER_RECORD_SCHEMA_VERSION: u32 = 1;
/// Current handover-journal schema version.
pub const MACOS_HANDOVER_JOURNAL_SCHEMA_VERSION: u32 = 1;
/// Stable owner-record file name within the per-user data directory.
pub const MACOS_OWNER_RECORD_FILE_NAME: &str = "macos-daemon-owner.json";
/// Stable handover-journal file name within the per-user data directory.
pub const MACOS_HANDOVER_JOURNAL_FILE_NAME: &str = "macos-daemon-handover.json";
/// Stable coordination-lock file name shared by both durable artifacts.
pub const MACOS_OWNER_COORDINATION_LOCK_FILE_NAME: &str = "macos-daemon-owner.lock";
/// Maximum UTF-8 byte length for an audit-token identity.
pub const MAX_MACOS_AUDIT_TOKEN_IDENTITY_BYTES: usize = 256;
/// Maximum UTF-8 byte length for a diagnostic executable path.
pub const MAX_MACOS_EXECUTABLE_PATH_BYTES: usize = 4_096;
/// Maximum UTF-8 byte length for a designated-requirement hash.
pub const MAX_MACOS_DESIGNATED_REQUIREMENT_HASH_BYTES: usize = 256;
/// Maximum byte length accepted for either durable JSON artifact.
pub const MAX_MACOS_OWNER_ARTIFACT_BYTES: usize = 256 * 1_024;
/// Maximum number of closed rollback operations in one journal.
pub const MAX_MACOS_HANDOVER_OPERATIONS: usize = 64;
const MAX_TEMPORARY_CREATE_ATTEMPTS: usize = 64;

static TEMPORARY_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// A daemon topology that can own protected macOS capabilities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MacosDaemonOwner {
    /// Daemon supervised by the packaged app.
    AppSidecar,
    /// Daemon managed by Hypercolor's direct per-user launchd service.
    DirectLaunchd,
    /// Daemon managed by Homebrew services.
    Homebrew,
    /// Daemon started directly from a terminal.
    Standalone,
}

/// An external daemon topology selected by the local app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MacosExternalOwnerMode {
    /// Connect to Hypercolor's direct per-user launchd service.
    DirectLaunchd,
    /// Connect to the Homebrew-managed service.
    Homebrew,
}

/// Bounded diagnostic identity for the process that attempted ownership.
///
/// The executable path is diagnostic data only. It is never an executable,
/// command, or recovery authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MacosOwnerIdentity {
    /// Stable representation of the process audit token.
    pub audit_token_identity: String,
    /// Absolute path observed for the process executable.
    pub executable_path: PathBuf,
    /// Hash of the process designated requirement.
    pub designated_requirement_hash: String,
    /// Process identifier observed with this identity.
    pub pid: u32,
}

impl MacosOwnerIdentity {
    /// Validate and construct a diagnostic process identity.
    pub fn new(
        audit_token_identity: impl Into<String>,
        executable_path: impl Into<PathBuf>,
        designated_requirement_hash: impl Into<String>,
        pid: u32,
    ) -> Result<Self, MacosOwnerStoreError> {
        let identity = Self {
            audit_token_identity: audit_token_identity.into(),
            executable_path: executable_path.into(),
            designated_requirement_hash: designated_requirement_hash.into(),
            pid,
        };
        validate_owner_identity(&identity)?;
        Ok(identity)
    }
}

impl<'de> Deserialize<'de> for MacosOwnerIdentity {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawIdentity {
            audit_token_identity: String,
            executable_path: PathBuf,
            designated_requirement_hash: String,
            pid: u32,
        }

        let raw = RawIdentity::deserialize(deserializer)?;
        Self::new(
            raw.audit_token_identity,
            raw.executable_path,
            raw.designated_requirement_hash,
            raw.pid,
        )
        .map_err(serde::de::Error::custom)
    }
}

/// Bounded conflict status for a contender that failed to acquire the guard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MacosOwnerConflict {
    /// Owner holding the guard when the conflict was observed.
    pub active_owner: MacosDaemonOwner,
    /// Active owner's acquisition epoch.
    pub active_epoch: u64,
    /// Topology of the losing contender.
    pub contender_owner: MacosDaemonOwner,
    /// Millisecond timestamp supplied by the observer.
    pub observed_at_ms: u64,
}

/// Durable conflict record including the contender's diagnostic identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MacosOwnerConflictRecord {
    /// Owner holding the guard when the conflict was observed.
    pub active_owner: MacosDaemonOwner,
    /// Active owner's acquisition epoch.
    pub active_epoch: u64,
    /// Topology of the losing contender.
    pub contender_owner: MacosDaemonOwner,
    /// Diagnostic identity of the losing contender.
    pub contender_identity: MacosOwnerIdentity,
    /// Millisecond timestamp supplied by the observer.
    pub observed_at_ms: u64,
}

impl MacosOwnerConflictRecord {
    fn has_same_identity(&self, other: &Self) -> bool {
        self.active_owner == other.active_owner
            && self.active_epoch == other.active_epoch
            && self.contender_owner == other.contender_owner
            && self.contender_identity.executable_path == other.contender_identity.executable_path
            && self.contender_identity.designated_requirement_hash
                == other.contender_identity.designated_requirement_hash
    }

    const fn snapshot(&self) -> MacosOwnerConflict {
        MacosOwnerConflict {
            active_owner: self.active_owner,
            active_epoch: self.active_epoch,
            contender_owner: self.contender_owner,
            observed_at_ms: self.observed_at_ms,
        }
    }
}

/// Bounded status snapshot derived from the durable owner record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MacosOwnerSnapshot {
    /// Current daemon owner.
    pub active_owner: MacosDaemonOwner,
    /// Current owner's acquisition epoch.
    pub owner_epoch: u64,
    /// Latest distinct owner conflict, when present.
    pub conflict: Option<MacosOwnerConflict>,
}

/// Versioned durable owner state for one macOS user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MacosOwnerRecord {
    /// Durable schema version.
    pub schema_version: u32,
    /// Current daemon owner.
    pub active_owner: MacosDaemonOwner,
    /// Diagnostic identity of the current owner process.
    pub active_identity: MacosOwnerIdentity,
    /// Monotonically increasing owner acquisition epoch.
    pub owner_epoch: u64,
    /// Latest distinct losing contender, when present.
    pub conflict: Option<MacosOwnerConflictRecord>,
    /// Persisted app preference for an externally managed daemon.
    pub selected_external_owner: Option<MacosExternalOwnerMode>,
}

impl MacosOwnerRecord {
    /// Construct an initial owner record at epoch one.
    pub const fn new(
        active_owner: MacosDaemonOwner,
        active_identity: MacosOwnerIdentity,
        selected_external_owner: Option<MacosExternalOwnerMode>,
    ) -> Self {
        Self {
            schema_version: MACOS_OWNER_RECORD_SCHEMA_VERSION,
            active_owner,
            active_identity,
            owner_epoch: 1,
            conflict: None,
            selected_external_owner,
        }
    }

    /// Return the bounded status surface for this record.
    pub fn snapshot(&self) -> MacosOwnerSnapshot {
        MacosOwnerSnapshot {
            active_owner: self.active_owner,
            owner_epoch: self.owner_epoch,
            conflict: self
                .conflict
                .as_ref()
                .map(MacosOwnerConflictRecord::snapshot),
        }
    }
}

/// Result of publishing a contender against the current owner epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacosConflictUpdate {
    /// A distinct contender state was durably recorded.
    Recorded(MacosOwnerSnapshot),
    /// The contender matched the existing conflict identity.
    Coalesced(MacosOwnerSnapshot),
}

impl MacosConflictUpdate {
    /// Return the owner snapshot associated with this update.
    pub const fn snapshot(self) -> MacosOwnerSnapshot {
        match self {
            Self::Recorded(snapshot) | Self::Coalesced(snapshot) => snapshot,
        }
    }
}

/// Installed-state snapshot captured before a daemon handover.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MacosAutostartStates {
    /// Whether app-sidecar autostart was enabled.
    pub app_sidecar: bool,
    /// Whether the direct launchd service was enabled.
    pub direct_launchd: bool,
    /// Whether the Homebrew service was enabled.
    pub homebrew: bool,
}

impl MacosAutostartStates {
    /// Construct an installed-state snapshot.
    pub const fn new(app_sidecar: bool, direct_launchd: bool, homebrew: bool) -> Self {
        Self {
            app_sidecar,
            direct_launchd,
            homebrew,
        }
    }
}

/// A validated path-free handover or rollback operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MacosHandoverOperation {
    /// Set app-sidecar autostart state.
    SetAppSidecarAutostart {
        /// Desired installed state.
        enabled: bool,
    },
    /// Flush and stop the app-supervised sidecar.
    FlushAndStopAppSidecar {},
    /// Start the app-supervised sidecar.
    StartAppSidecar {},
    /// Set direct-launchd autostart state.
    SetDirectLaunchdAutostart {
        /// Desired installed state.
        enabled: bool,
    },
    /// Flush and stop the direct launchd service.
    FlushAndStopDirectLaunchd {},
    /// Start the direct launchd service.
    StartDirectLaunchd {},
    /// Set Homebrew-service autostart state.
    SetHomebrewAutostart {
        /// Desired installed state.
        enabled: bool,
    },
    /// Flush and stop the Homebrew service.
    FlushAndStopHomebrew {},
    /// Start the Homebrew service.
    StartHomebrew {},
    /// Await user-directed termination of a standalone owner.
    AwaitStandaloneExit {
        /// Authoritative process identifier shown to the user.
        pid: u32,
    },
}

/// Durable handover phase used to resume or reverse interrupted work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MacosHandoverPhase {
    /// Journal exists and no external mutation has begun.
    Prepared,
    /// Nonselected autostarts have been disabled.
    AutostartsConfigured,
    /// Stop of the outgoing managed owner has been requested.
    StopRequested,
    /// The outgoing managed owner has stopped.
    OutgoingOwnerStopped,
    /// The coordinator is waiting for the instance guard to release.
    AwaitingGuardRelease,
    /// The instance guard is free.
    GuardReleased,
    /// Startup of the requested owner has been requested.
    StartRequested,
    /// The requested owner has started.
    RequestedOwnerStarted,
    /// The requested owner is ready for the ownership commit.
    CommitPending,
    /// The requested owner committed the handover.
    Committed,
    /// Forward progress failed and rollback must begin or resume.
    RollbackPending,
    /// Prior autostart state has been restored.
    RollbackAutostartsRestored,
    /// Stop of a partially started requested owner was requested.
    RollbackStopRequested,
    /// The partially started requested owner has stopped.
    RollbackOwnerStopped,
    /// Rollback is waiting for the instance guard to release.
    RollbackAwaitingGuardRelease,
    /// The instance guard is free for the prior owner.
    RollbackGuardReleased,
    /// Restart of the prior managed owner was requested.
    RollbackStartRequested,
    /// The prior managed owner has restarted.
    PriorOwnerStarted,
    /// The prior owner is ready for the rollback commit.
    RollbackCommitPending,
    /// The prior owner committed rollback completion.
    RolledBack,
}

impl MacosHandoverPhase {
    /// Every stable journal phase, in forward then rollback order.
    pub const ALL: [Self; 20] = [
        Self::Prepared,
        Self::AutostartsConfigured,
        Self::StopRequested,
        Self::OutgoingOwnerStopped,
        Self::AwaitingGuardRelease,
        Self::GuardReleased,
        Self::StartRequested,
        Self::RequestedOwnerStarted,
        Self::CommitPending,
        Self::Committed,
        Self::RollbackPending,
        Self::RollbackAutostartsRestored,
        Self::RollbackStopRequested,
        Self::RollbackOwnerStopped,
        Self::RollbackAwaitingGuardRelease,
        Self::RollbackGuardReleased,
        Self::RollbackStartRequested,
        Self::PriorOwnerStarted,
        Self::RollbackCommitPending,
        Self::RolledBack,
    ];

    /// Whether this phase closes the transaction.
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Committed | Self::RolledBack)
    }
}

/// Stable, path-free identifier for one handover transaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct MacosHandoverTransactionId(String);

impl MacosHandoverTransactionId {
    /// Validate and construct a handover transaction identifier.
    pub fn new(value: impl Into<String>) -> Result<Self, MacosOwnerStoreError> {
        let value = value.into();
        if is_valid_transaction_id(&value) {
            Ok(Self(value))
        } else {
            Err(MacosOwnerStoreError::InvalidTransactionId)
        }
    }

    /// Borrow the validated identifier.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for MacosHandoverTransactionId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

/// Versioned durable journal for a local daemon-owner handover.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MacosHandoverJournal {
    /// Durable schema version.
    pub schema_version: u32,
    /// Monotonic mutation count within this journal transaction.
    pub journal_revision: u64,
    /// Stable transaction identifier.
    pub transaction_id: MacosHandoverTransactionId,
    /// Desired owner after a successful handover.
    pub requested_owner: MacosDaemonOwner,
    /// Owner to restore if the handover rolls back.
    pub prior_owner: MacosDaemonOwner,
    /// Installed states to restore during rollback.
    pub prior_autostart_states: MacosAutostartStates,
    /// Closed operations recovery is permitted to execute.
    pub allowed_rollback_operations: Vec<MacosHandoverOperation>,
    /// Last durably completed transaction phase.
    pub phase: MacosHandoverPhase,
    /// Owner epoch observed before mutation began.
    pub active_epoch: u64,
    /// Contender epoch associated with the request, when one exists.
    pub contender_epoch: Option<u64>,
    /// Standalone process whose user-directed exit is pending.
    pub pending_standalone_pid: Option<u32>,
}

impl MacosHandoverJournal {
    /// Construct a prepared journal. The store assigns its first revision.
    pub fn new(
        transaction_id: MacosHandoverTransactionId,
        requested_owner: MacosDaemonOwner,
        prior_owner: MacosDaemonOwner,
        prior_autostart_states: MacosAutostartStates,
        allowed_rollback_operations: Vec<MacosHandoverOperation>,
        active_epoch: u64,
        contender_epoch: Option<u64>,
        pending_standalone_pid: Option<u32>,
    ) -> Self {
        Self {
            schema_version: MACOS_HANDOVER_JOURNAL_SCHEMA_VERSION,
            journal_revision: 0,
            transaction_id,
            requested_owner,
            prior_owner,
            prior_autostart_states,
            allowed_rollback_operations,
            phase: MacosHandoverPhase::Prepared,
            active_epoch,
            contender_epoch,
            pending_standalone_pid,
        }
    }
}

/// Typed durable owner-store failure.
#[derive(Debug, thiserror::Error)]
pub enum MacosOwnerStoreError {
    /// The explicit data directory could not be created.
    #[error("failed to create macOS owner data directory {path}: {source}")]
    CreateDirectory {
        /// Data directory.
        path: PathBuf,
        /// Filesystem failure.
        #[source]
        source: std::io::Error,
    },
    /// The stable coordination lock could not be opened.
    #[error("failed to open macOS owner coordination lock {path}: {source}")]
    OpenCoordinationLock {
        /// Lock path.
        path: PathBuf,
        /// Filesystem failure.
        #[source]
        source: std::io::Error,
    },
    /// The stable coordination lock could not be acquired.
    #[error("failed to acquire macOS owner coordination lock {path}: {source}")]
    AcquireCoordinationLock {
        /// Lock path.
        path: PathBuf,
        /// Filesystem failure.
        #[source]
        source: std::io::Error,
    },
    /// A durable artifact could not be read.
    #[error("failed to read macOS {artifact} at {path}: {source}")]
    Read {
        /// Artifact kind.
        artifact: &'static str,
        /// Artifact path.
        path: PathBuf,
        /// Filesystem failure.
        #[source]
        source: std::io::Error,
    },
    /// A durable artifact could not be decoded.
    #[error("failed to decode macOS {artifact}: {source}")]
    Decode {
        /// Artifact kind.
        artifact: &'static str,
        /// JSON failure.
        #[source]
        source: serde_json::Error,
    },
    /// A durable artifact has an unsupported schema version.
    #[error("unsupported macOS {artifact} schema version {found}; expected {expected}")]
    UnsupportedVersion {
        /// Artifact kind.
        artifact: &'static str,
        /// Version found on disk.
        found: u32,
        /// Version supported by this build.
        expected: u32,
    },
    /// A durable artifact violates a semantic invariant.
    #[error("invalid macOS {artifact}: {detail}")]
    InvalidArtifact {
        /// Artifact kind.
        artifact: &'static str,
        /// Stable validation detail.
        detail: &'static str,
    },
    /// JSON serialization failed before any bytes were replaced.
    #[error("failed to serialize macOS {artifact}: {source}")]
    Encode {
        /// Artifact kind.
        artifact: &'static str,
        /// JSON failure.
        #[source]
        source: serde_json::Error,
    },
    /// A same-directory temporary file could not be created.
    #[error("failed to create temporary file beside {path}: {source}")]
    CreateTemporary {
        /// Destination path.
        path: PathBuf,
        /// Filesystem failure.
        #[source]
        source: std::io::Error,
    },
    /// A complete temporary artifact could not be written.
    #[error("failed to write temporary file for {path}: {source}")]
    WriteTemporary {
        /// Destination path.
        path: PathBuf,
        /// Filesystem failure.
        #[source]
        source: std::io::Error,
    },
    /// Temporary artifact contents could not be synced.
    #[error("failed to sync temporary file for {path}: {source}")]
    SyncTemporary {
        /// Destination path.
        path: PathBuf,
        /// Filesystem failure.
        #[source]
        source: std::io::Error,
    },
    /// The durable destination could not be atomically replaced.
    #[error("failed to atomically replace {path}: {source}")]
    Replace {
        /// Destination path.
        path: PathBuf,
        /// Filesystem failure.
        #[source]
        source: std::io::Error,
    },
    /// The parent directory could not be synced after replacement.
    #[cfg(unix)]
    #[error("failed to sync parent directory {path}: {source}")]
    SyncDirectory {
        /// Parent directory.
        path: PathBuf,
        /// Filesystem failure.
        #[source]
        source: std::io::Error,
    },
    /// No owner record exists for the requested mutation.
    #[error("macOS owner record does not exist")]
    MissingOwnerRecord,
    /// The owner acquisition epoch cannot advance further.
    #[error("macOS owner epoch overflow")]
    OwnerEpochOverflow,
    /// A nonterminal handover journal must be recovered first.
    #[error("macOS handover {transaction_id} is still pending")]
    HandoverAlreadyPending {
        /// Existing transaction identifier.
        transaction_id: String,
    },
    /// No handover journal exists for the requested mutation.
    #[error("macOS handover journal does not exist")]
    MissingHandoverJournal,
    /// A caller attempted to advance a different transaction.
    #[error("macOS handover transaction does not match the durable journal")]
    HandoverTransactionMismatch,
    /// The handover journal revision cannot advance further.
    #[error("macOS handover journal revision overflow")]
    JournalRevisionOverflow,
    /// A transaction identifier is not a bounded path-free token.
    #[error("macOS handover transaction ID must be 1-64 ASCII letters, digits, '_' or '-'")]
    InvalidTransactionId,
    /// An owner identity field is empty, oversized, or structurally invalid.
    #[error("invalid macOS owner identity field {field}: {detail}")]
    InvalidOwnerIdentity {
        /// Invalid identity field.
        field: &'static str,
        /// Stable validation detail.
        detail: &'static str,
    },
    /// A durable artifact exceeds the bounded decoder input size.
    #[error("macOS {artifact} exceeds the {maximum_bytes}-byte limit")]
    ArtifactTooLarge {
        /// Artifact kind.
        artifact: &'static str,
        /// Maximum accepted byte length.
        maximum_bytes: usize,
    },
    /// A completed or rolled-back transaction cannot be advanced.
    #[error("terminal macOS handover {transaction_id} cannot advance")]
    TerminalHandover {
        /// Completed transaction identifier.
        transaction_id: String,
    },
}

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

    /// Return the stable lock path shared by every writer.
    pub fn coordination_lock_path(&self) -> PathBuf {
        self.data_dir.join(MACOS_OWNER_COORDINATION_LOCK_FILE_NAME)
    }

    /// Load and validate the current owner record.
    pub fn load_owner_record(&self) -> Result<Option<MacosOwnerRecord>, MacosOwnerStoreError> {
        read_owner_record(&self.owner_record_path())
    }

    /// Publish a newly acquired owner and advance the durable owner epoch.
    pub fn publish_owner(
        &self,
        active_owner: MacosDaemonOwner,
        active_identity: MacosOwnerIdentity,
        selected_external_owner: Option<MacosExternalOwnerMode>,
    ) -> Result<MacosOwnerRecord, MacosOwnerStoreError> {
        let _lock = self.acquire_coordination_lock()?;
        let path = self.owner_record_path();
        let record = match read_owner_record(&path)? {
            Some(previous) => MacosOwnerRecord {
                owner_epoch: previous
                    .owner_epoch
                    .checked_add(1)
                    .ok_or(MacosOwnerStoreError::OwnerEpochOverflow)?,
                schema_version: MACOS_OWNER_RECORD_SCHEMA_VERSION,
                active_owner,
                active_identity,
                conflict: None,
                selected_external_owner,
            },
            None => MacosOwnerRecord::new(active_owner, active_identity, selected_external_owner),
        };
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

struct CoordinationLock {
    file: File,
}

impl Drop for CoordinationLock {
    fn drop(&mut self) {
        drop(self.file.unlock());
    }
}

fn read_owner_record(path: &Path) -> Result<Option<MacosOwnerRecord>, MacosOwnerStoreError> {
    let Some(bytes) = read_optional(path, "owner record")? else {
        return Ok(None);
    };
    let record = serde_json::from_slice::<MacosOwnerRecord>(&bytes).map_err(|source| {
        MacosOwnerStoreError::Decode {
            artifact: "owner record",
            source,
        }
    })?;
    validate_owner_record(&record)?;
    Ok(Some(record))
}

fn read_handover_journal(
    path: &Path,
) -> Result<Option<MacosHandoverJournal>, MacosOwnerStoreError> {
    let Some(bytes) = read_optional(path, "handover journal")? else {
        return Ok(None);
    };
    let journal = serde_json::from_slice::<MacosHandoverJournal>(&bytes).map_err(|source| {
        MacosOwnerStoreError::Decode {
            artifact: "handover journal",
            source,
        }
    })?;
    validate_handover_journal(&journal)?;
    Ok(Some(journal))
}

fn read_optional(
    path: &Path,
    artifact: &'static str,
) -> Result<Option<Vec<u8>>, MacosOwnerStoreError> {
    match File::open(path) {
        Ok(file) => {
            let mut bytes = Vec::new();
            file.take((MAX_MACOS_OWNER_ARTIFACT_BYTES + 1) as u64)
                .read_to_end(&mut bytes)
                .map_err(|source| MacosOwnerStoreError::Read {
                    artifact,
                    path: path.to_path_buf(),
                    source,
                })?;
            if bytes.len() > MAX_MACOS_OWNER_ARTIFACT_BYTES {
                return Err(MacosOwnerStoreError::ArtifactTooLarge {
                    artifact,
                    maximum_bytes: MAX_MACOS_OWNER_ARTIFACT_BYTES,
                });
            }
            Ok(Some(bytes))
        }
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(MacosOwnerStoreError::Read {
            artifact,
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn validate_owner_record(record: &MacosOwnerRecord) -> Result<(), MacosOwnerStoreError> {
    validate_version(
        "owner record",
        record.schema_version,
        MACOS_OWNER_RECORD_SCHEMA_VERSION,
    )?;
    if record.owner_epoch == 0 {
        return Err(MacosOwnerStoreError::InvalidArtifact {
            artifact: "owner record",
            detail: "owner_epoch must be positive",
        });
    }
    validate_owner_identity(&record.active_identity)?;
    if let Some(conflict) = &record.conflict
        && (conflict.active_owner != record.active_owner
            || conflict.active_epoch != record.owner_epoch)
    {
        return Err(MacosOwnerStoreError::InvalidArtifact {
            artifact: "owner record",
            detail: "conflict must identify the active owner epoch",
        });
    }
    if let Some(conflict) = &record.conflict {
        validate_owner_identity(&conflict.contender_identity)?;
    }
    Ok(())
}

fn validate_owner_identity(identity: &MacosOwnerIdentity) -> Result<(), MacosOwnerStoreError> {
    validate_bounded_identity_text(
        "audit_token_identity",
        &identity.audit_token_identity,
        MAX_MACOS_AUDIT_TOKEN_IDENTITY_BYTES,
    )?;
    let executable_path =
        identity
            .executable_path
            .to_str()
            .ok_or(MacosOwnerStoreError::InvalidOwnerIdentity {
                field: "executable_path",
                detail: "must be valid UTF-8",
            })?;
    validate_bounded_identity_text(
        "executable_path",
        executable_path,
        MAX_MACOS_EXECUTABLE_PATH_BYTES,
    )?;
    if !identity.executable_path.is_absolute() {
        return Err(MacosOwnerStoreError::InvalidOwnerIdentity {
            field: "executable_path",
            detail: "must be absolute",
        });
    }
    validate_bounded_identity_text(
        "designated_requirement_hash",
        &identity.designated_requirement_hash,
        MAX_MACOS_DESIGNATED_REQUIREMENT_HASH_BYTES,
    )?;
    if identity.pid == 0 {
        return Err(MacosOwnerStoreError::InvalidOwnerIdentity {
            field: "pid",
            detail: "must be positive",
        });
    }
    Ok(())
}

fn validate_bounded_identity_text(
    field: &'static str,
    value: &str,
    maximum_bytes: usize,
) -> Result<(), MacosOwnerStoreError> {
    if value.is_empty() {
        Err(MacosOwnerStoreError::InvalidOwnerIdentity {
            field,
            detail: "must not be empty",
        })
    } else if value.len() > maximum_bytes {
        Err(MacosOwnerStoreError::InvalidOwnerIdentity {
            field,
            detail: "exceeds its byte limit",
        })
    } else {
        Ok(())
    }
}

fn validate_handover_journal(journal: &MacosHandoverJournal) -> Result<(), MacosOwnerStoreError> {
    validate_version(
        "handover journal",
        journal.schema_version,
        MACOS_HANDOVER_JOURNAL_SCHEMA_VERSION,
    )?;
    if !is_valid_transaction_id(journal.transaction_id.as_str()) {
        return Err(MacosOwnerStoreError::InvalidTransactionId);
    }
    if journal.active_epoch == 0 {
        return Err(MacosOwnerStoreError::InvalidArtifact {
            artifact: "handover journal",
            detail: "active_epoch must be positive",
        });
    }
    if journal.allowed_rollback_operations.len() > MAX_MACOS_HANDOVER_OPERATIONS {
        return Err(MacosOwnerStoreError::InvalidArtifact {
            artifact: "handover journal",
            detail: "allowed_rollback_operations exceeds its item limit",
        });
    }
    if journal.pending_standalone_pid == Some(0)
        || journal.allowed_rollback_operations.iter().any(|operation| {
            matches!(
                operation,
                MacosHandoverOperation::AwaitStandaloneExit { pid: 0 }
            )
        })
    {
        return Err(MacosOwnerStoreError::InvalidArtifact {
            artifact: "handover journal",
            detail: "standalone PID must be positive",
        });
    }
    Ok(())
}

fn validate_version(
    artifact: &'static str,
    found: u32,
    expected: u32,
) -> Result<(), MacosOwnerStoreError> {
    if found == expected {
        Ok(())
    } else {
        Err(MacosOwnerStoreError::UnsupportedVersion {
            artifact,
            found,
            expected,
        })
    }
}

fn is_valid_transaction_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn write_json_atomic<T>(
    data_dir: &Path,
    path: &Path,
    artifact: &'static str,
    value: &T,
) -> Result<(), MacosOwnerStoreError>
where
    T: Serialize + ?Sized,
{
    let mut payload = serde_json::to_vec_pretty(value)
        .map_err(|source| MacosOwnerStoreError::Encode { artifact, source })?;
    payload.push(b'\n');
    if payload.len() > MAX_MACOS_OWNER_ARTIFACT_BYTES {
        return Err(MacosOwnerStoreError::ArtifactTooLarge {
            artifact,
            maximum_bytes: MAX_MACOS_OWNER_ARTIFACT_BYTES,
        });
    }
    let (mut temporary, temporary_path) = create_temporary_file(data_dir, path)?;
    let result = (|| {
        temporary
            .write_all(&payload)
            .map_err(|source| MacosOwnerStoreError::WriteTemporary {
                path: path.to_path_buf(),
                source,
            })?;
        temporary
            .sync_all()
            .map_err(|source| MacosOwnerStoreError::SyncTemporary {
                path: path.to_path_buf(),
                source,
            })?;
        drop(temporary);
        hypercolor_platform_fs::replace_file(&temporary_path, path).map_err(|source| {
            MacosOwnerStoreError::Replace {
                path: path.to_path_buf(),
                source,
            }
        })?;
        sync_parent_directory(data_dir)
    })();
    if result.is_err() {
        drop(fs::remove_file(&temporary_path));
    }
    result
}

fn create_temporary_file(
    data_dir: &Path,
    path: &Path,
) -> Result<(File, PathBuf), MacosOwnerStoreError> {
    for _ in 0..MAX_TEMPORARY_CREATE_ATTEMPTS {
        let sequence = TEMPORARY_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temporary_path = data_dir.join(format!(
            ".{}.{}.{}.tmp",
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("macos-owner"),
            std::process::id(),
            sequence
        ));
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&temporary_path) {
            Ok(file) => return Ok((file, temporary_path)),
            Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(source) => {
                return Err(MacosOwnerStoreError::CreateTemporary {
                    path: path.to_path_buf(),
                    source,
                });
            }
        }
    }
    Err(MacosOwnerStoreError::CreateTemporary {
        path: path.to_path_buf(),
        source: std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "temporary file collision limit reached",
        ),
    })
}

#[cfg(unix)]
fn sync_parent_directory(data_dir: &Path) -> Result<(), MacosOwnerStoreError> {
    File::open(data_dir)
        .and_then(|directory| directory.sync_all())
        .map_err(|source| MacosOwnerStoreError::SyncDirectory {
            path: data_dir.to_path_buf(),
            source,
        })
}

#[cfg(not(unix))]
fn sync_parent_directory(_data_dir: &Path) -> Result<(), MacosOwnerStoreError> {
    Ok(())
}
