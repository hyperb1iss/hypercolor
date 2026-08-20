//! Durable macOS daemon ownership and handover state.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};

mod direct_launchd;

#[cfg(target_os = "macos")]
pub use direct_launchd::NativeMacosDirectLaunchdInspector;
pub use direct_launchd::{
    MACOS_DIRECT_LAUNCHD_LABEL, MacosDirectLaunchdExecutableExpectation,
    MacosDirectLaunchdInspector, MacosDirectLaunchdOwnerProof,
    MacosDirectLaunchdPublicationExpectation, MacosDirectLaunchdState,
    corroborate_direct_launchd_owner, corroborate_newer_direct_launchd_owner,
    parse_direct_launchd_service_state, wait_for_exact_direct_launchd_publication,
};

/// Current owner-record schema version.
pub const MACOS_OWNER_RECORD_SCHEMA_VERSION: u32 = 1;
/// Current handover-journal schema version.
pub const MACOS_HANDOVER_JOURNAL_SCHEMA_VERSION: u32 = 1;
/// Current daemon-session attestation schema version.
pub const MACOS_DAEMON_SESSION_ATTESTATION_SCHEMA_VERSION: u32 = 1;
/// Stable owner-record file name within the per-user data directory.
pub const MACOS_OWNER_RECORD_FILE_NAME: &str = "macos-daemon-owner.json";
/// Stable handover-journal file name within the per-user data directory.
pub const MACOS_HANDOVER_JOURNAL_FILE_NAME: &str = "macos-daemon-handover.json";
/// Stable daemon-session attestation file name within the per-user data directory.
pub const MACOS_DAEMON_SESSION_ATTESTATION_FILE_NAME: &str = "macos-daemon-session.json";
/// Stable coordination-lock file name shared by both durable artifacts.
pub const MACOS_OWNER_COORDINATION_LOCK_FILE_NAME: &str = "macos-daemon-owner.lock";
/// Tauri product name and app-sidecar LaunchAgent label.
pub const MACOS_APP_PRODUCT_NAME: &str = "Hypercolor";
/// LaunchAgent property-list file installed by Tauri autostart.
pub const MACOS_APP_LAUNCH_AGENT_PLIST_FILE_NAME: &str = "Hypercolor.plist";
/// Main executable location within the signed Tauri app bundle.
pub const MACOS_APP_BUNDLE_EXECUTABLE_RELATIVE_PATH: &str = "Contents/MacOS/Hypercolor";
/// Binary names the app bundle's main executable may carry. Tauri names
/// the `.app` folder after the product but keeps the cargo binary name
/// for the executable, so real bundles ship `hypercolor-app`; the
/// product-named form is accepted for a future renamed bundle.
pub const MACOS_APP_BUNDLE_BINARY_NAMES: [&str; 2] = ["hypercolor-app", MACOS_APP_PRODUCT_NAME];
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
/// Maximum wait for a managed owner to release or acquire the daemon guard.
pub const MACOS_MANAGED_HANDOVER_TIMEOUT: Duration = Duration::from_secs(10);
/// Maximum wait for user-directed standalone-owner termination.
pub const MACOS_STANDALONE_HANDOVER_TIMEOUT: Duration = Duration::from_mins(1);
const MAX_TEMPORARY_CREATE_ATTEMPTS: usize = 64;
const MACOS_SERVER_SESSION_ID_PREFIX: &str = "hc_session_";
const MACOS_PROTECTED_CONTROL_CREDENTIAL_PREFIX: &str = "hc_pc_";
const MACOS_SERVER_SESSION_ID_BYTES: usize = 16;
const MACOS_PROTECTED_CONTROL_CREDENTIAL_BYTES: usize = 32;

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

/// Path-free status for a nonterminal journal this daemon cannot complete.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MacosOwnerRecoveryRequired {
    /// Owner requested by the pending handover.
    pub requested_owner: MacosDaemonOwner,
    /// Owner restored if the pending handover rolls back.
    pub prior_owner: MacosDaemonOwner,
    /// Durable phase at which local coordinator recovery must resume.
    pub phase: MacosHandoverPhase,
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
    /// Nonterminal handover this daemon is not authorized to complete.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_required: Option<MacosOwnerRecoveryRequired>,
}

impl MacosOwnerSnapshot {
    /// Attach path-free recovery status after incoming-daemon reconciliation.
    #[must_use]
    pub const fn with_recovery_required(
        mut self,
        recovery_required: Option<MacosOwnerRecoveryRequired>,
    ) -> Self {
        self.recovery_required = recovery_required;
        self
    }
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
            recovery_required: None,
        }
    }

    /// Return the complete durable identity of this owner acquisition.
    pub fn incarnation(&self) -> MacosOwnerIncarnation {
        MacosOwnerIncarnation {
            owner: self.active_owner,
            owner_epoch: self.owner_epoch,
            identity: self.active_identity.clone(),
        }
    }
}

/// Exact durable identity of one owner acquisition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MacosOwnerIncarnation {
    /// Topology that acquired the canonical daemon guard.
    pub owner: MacosDaemonOwner,
    /// Monotonic acquisition epoch published by that owner.
    pub owner_epoch: u64,
    /// Full process identity published for the acquisition.
    pub identity: MacosOwnerIdentity,
}

/// Per-process identifier exposed by the daemon discovery endpoint.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct MacosServerSessionId(String);

impl MacosServerSessionId {
    /// Construct a canonical session identifier from 128 bits of entropy.
    #[must_use]
    pub fn from_bytes(bytes: [u8; MACOS_SERVER_SESSION_ID_BYTES]) -> Self {
        Self(format_hex_token(MACOS_SERVER_SESSION_ID_PREFIX, &bytes))
    }

    /// Borrow the canonical session identifier.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for MacosServerSessionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("MacosServerSessionId")
            .field(&self.0)
            .finish()
    }
}

impl<'de> Deserialize<'de> for MacosServerSessionId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        validate_hex_token(
            &value,
            MACOS_SERVER_SESSION_ID_PREFIX,
            MACOS_SERVER_SESSION_ID_BYTES,
            "server_session_id must be a canonical 128-bit token",
        )
        .map_err(serde::de::Error::custom)?;
        Ok(Self(value))
    }
}

/// Private 256-bit bearer credential for one daemon process session.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct MacosProtectedControlCredential(String);

impl MacosProtectedControlCredential {
    /// Construct a canonical protected-control credential from 256 bits.
    #[must_use]
    pub fn from_bytes(bytes: [u8; MACOS_PROTECTED_CONTROL_CREDENTIAL_BYTES]) -> Self {
        Self(format_hex_token(
            MACOS_PROTECTED_CONTROL_CREDENTIAL_PREFIX,
            &bytes,
        ))
    }

    /// Explicitly expose the bearer value for authenticated local transport.
    #[must_use]
    pub fn expose_secret(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for MacosProtectedControlCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("MacosProtectedControlCredential([REDACTED])")
    }
}

impl<'de> Deserialize<'de> for MacosProtectedControlCredential {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        validate_hex_token(
            &value,
            MACOS_PROTECTED_CONTROL_CREDENTIAL_PREFIX,
            MACOS_PROTECTED_CONTROL_CREDENTIAL_BYTES,
            "protected_control_credential must be a canonical 256-bit token",
        )
        .map_err(serde::de::Error::custom)?;
        Ok(Self(value))
    }
}

/// Private process-session proof derived from canonical daemon ownership.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MacosDaemonSessionAttestation {
    /// Durable schema version.
    pub schema_version: u32,
    /// Topology holding the canonical daemon guard.
    pub owner: MacosDaemonOwner,
    /// Exact owner epoch current when this session was published.
    pub owner_epoch: u64,
    /// Full process identity current when this session was published.
    pub owner_identity: MacosOwnerIdentity,
    /// Per-process identifier safe to expose from `GET /system`.
    pub server_session_id: MacosServerSessionId,
    /// Private bearer credential accepted only from a loopback peer.
    pub protected_control_credential: MacosProtectedControlCredential,
}

impl MacosDaemonSessionAttestation {
    #[cfg(target_os = "macos")]
    fn generate(record: &MacosOwnerRecord) -> Result<Self, MacosOwnerStoreError> {
        let mut entropy =
            [0_u8; MACOS_SERVER_SESSION_ID_BYTES + MACOS_PROTECTED_CONTROL_CREDENTIAL_BYTES];
        File::open("/dev/urandom")
            .and_then(|mut source| source.read_exact(&mut entropy))
            .map_err(|source| MacosOwnerStoreError::Read {
                artifact: "daemon session entropy",
                path: PathBuf::from("/dev/urandom"),
                source,
            })?;
        let (session_bytes, credential_bytes) = entropy.split_at(MACOS_SERVER_SESSION_ID_BYTES);
        let session_bytes = session_bytes
            .try_into()
            .expect("session entropy slice has the exact array length");
        let credential_bytes = credential_bytes
            .try_into()
            .expect("credential entropy slice has the exact array length");
        Ok(Self {
            schema_version: MACOS_DAEMON_SESSION_ATTESTATION_SCHEMA_VERSION,
            owner: record.active_owner,
            owner_epoch: record.owner_epoch,
            owner_identity: record.active_identity.clone(),
            server_session_id: MacosServerSessionId::from_bytes(session_bytes),
            protected_control_credential: MacosProtectedControlCredential::from_bytes(
                credential_bytes,
            ),
        })
    }

    /// Return the exact owner acquisition that authorized this session.
    #[must_use]
    pub fn owner_incarnation(&self) -> MacosOwnerIncarnation {
        MacosOwnerIncarnation {
            owner: self.owner,
            owner_epoch: self.owner_epoch,
            identity: self.owner_identity.clone(),
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
    /// Closed operations forward recovery is permitted to execute.
    #[serde(default)]
    pub allowed_forward_operations: Vec<MacosHandoverOperation>,
    /// Closed operations recovery is permitted to execute.
    pub allowed_rollback_operations: Vec<MacosHandoverOperation>,
    /// Last durably completed transaction phase.
    pub phase: MacosHandoverPhase,
    /// Owner epoch observed before mutation began.
    pub active_epoch: u64,
    /// Requested owner epoch published during this handover, when one exists.
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
            allowed_forward_operations: Vec::new(),
            allowed_rollback_operations,
            phase: MacosHandoverPhase::Prepared,
            active_epoch,
            contender_epoch,
            pending_standalone_pid,
        }
    }

    /// Construct a complete path-free handover journal for a local owner choice.
    pub fn for_owner_choice(
        transaction_id: MacosHandoverTransactionId,
        requested_owner: MacosDaemonOwner,
        prior_record: &MacosOwnerRecord,
        prior_autostart_states: MacosAutostartStates,
    ) -> Result<Self, MacosOwnerCoordinatorError> {
        if requested_owner == MacosDaemonOwner::Standalone {
            return Err(MacosOwnerCoordinatorError::StandaloneCannotBeSelected);
        }
        let pending_standalone_pid = (prior_record.active_owner == MacosDaemonOwner::Standalone)
            .then_some(prior_record.active_identity.pid);
        let allowed_forward_operations = forward_operations(
            requested_owner,
            prior_record.active_owner,
            pending_standalone_pid,
        );
        let allowed_rollback_operations = rollback_operations(
            requested_owner,
            prior_record.active_owner,
            prior_autostart_states,
        );
        Ok(Self {
            schema_version: MACOS_HANDOVER_JOURNAL_SCHEMA_VERSION,
            journal_revision: 0,
            transaction_id,
            requested_owner,
            prior_owner: prior_record.active_owner,
            prior_autostart_states,
            allowed_forward_operations,
            allowed_rollback_operations,
            phase: MacosHandoverPhase::Prepared,
            active_epoch: prior_record.owner_epoch,
            contender_epoch: None,
            pending_standalone_pid,
        })
    }
}

/// A topology-specific user action returned by the local coordinator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MacosOwnerRemedy {
    /// The standalone owner must be stopped by its terminal user.
    StopStandaloneOwner { pid: u32 },
    /// The standalone capture owner must be restarted by its terminal user.
    RestartStandalone { pid: u32 },
    /// Start the packaged app sidecar locally.
    StartAppSidecar,
    /// Start the direct launchd service locally.
    StartLaunchdService,
    /// Start the Homebrew service locally.
    StartHomebrewService,
}

/// Synchronous result of a local owner selection or recovery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum MacosOwnerCoordinatorOutcome {
    /// The requested owner published a matching durable epoch.
    Active {
        owner: MacosDaemonOwner,
        owner_epoch: u64,
    },
    /// A standalone process still owns the guard and must exit voluntarily.
    PendingStandalone {
        requested_owner: MacosDaemonOwner,
        remedy: MacosOwnerRemedy,
    },
    /// Forward progress failed and the prior managed owner was restored.
    RolledBack {
        prior_owner: MacosDaemonOwner,
        failure: String,
    },
    /// A validated journal belongs to another owner and remains pending.
    RecoveryRequired {
        requested_owner: MacosDaemonOwner,
        prior_owner: MacosDaemonOwner,
        phase: MacosHandoverPhase,
    },
}

/// Closed local process and launcher operations used by the coordinator.
pub trait MacosOwnerExecutor {
    /// Return whether one managed topology is configured for login startup.
    fn autostart_enabled(
        &mut self,
        owner: MacosDaemonOwner,
    ) -> Result<bool, MacosOwnerExecutionError>;

    /// Idempotently set one managed topology's login-start state.
    fn set_autostart(
        &mut self,
        owner: MacosDaemonOwner,
        enabled: bool,
    ) -> Result<(), MacosOwnerExecutionError>;

    /// Verify that this executor can stop one exact managed owner acquisition.
    fn preflight_stop_authority(
        &mut self,
        incarnation: &MacosOwnerIncarnation,
    ) -> Result<(), MacosOwnerExecutionError>;

    /// Flush and stop one exact managed owner acquisition.
    fn flush_and_stop(
        &mut self,
        incarnation: &MacosOwnerIncarnation,
    ) -> Result<(), MacosOwnerExecutionError>;

    /// Idempotently start one managed owner.
    fn start(&mut self, owner: MacosDaemonOwner) -> Result<(), MacosOwnerExecutionError>;

    /// Wait until the canonical daemon guard can be acquired.
    fn wait_for_guard_release(
        &mut self,
        timeout: Duration,
    ) -> Result<bool, MacosOwnerExecutionError>;

    /// Wait for the requested owner to publish an epoch newer than `after_epoch`.
    fn wait_for_owner(
        &mut self,
        owner: MacosDaemonOwner,
        after_epoch: u64,
        timeout: Duration,
    ) -> Result<bool, MacosOwnerExecutionError>;
}

/// Owning handle for the same macOS `flock` used by the final daemon guard.
#[cfg(target_os = "macos")]
#[derive(Debug)]
pub struct MacosDaemonGuard {
    _lock: nix::fcntl::Flock<File>,
}

/// Block until the final macOS daemon guard is acquired.
#[cfg(target_os = "macos")]
pub fn acquire_macos_daemon_guard(
    instance_name: &str,
) -> Result<MacosDaemonGuard, MacosOwnerExecutionError> {
    use nix::errno::Errno;
    use nix::fcntl::{Flock, FlockArg};

    let mut file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(instance_name)
        .map_err(|error| {
            MacosOwnerExecutionError::new(format!("failed to open daemon guard: {error}"))
        })?;
    loop {
        match Flock::lock(file, FlockArg::LockExclusive) {
            Ok(lock) => return Ok(MacosDaemonGuard { _lock: lock }),
            Err((returned, Errno::EINTR)) => file = returned,
            Err((_, error)) => {
                return Err(MacosOwnerExecutionError::new(format!(
                    "failed to acquire daemon guard: {error}"
                )));
            }
        }
    }
}

/// Attempt to acquire the final macOS daemon guard without blocking.
#[cfg(target_os = "macos")]
pub fn try_acquire_macos_daemon_guard(
    instance_name: &str,
) -> Result<Option<MacosDaemonGuard>, MacosOwnerExecutionError> {
    use nix::errno::Errno;
    use nix::fcntl::{Flock, FlockArg};

    let mut file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(instance_name)
        .map_err(|error| {
            MacosOwnerExecutionError::new(format!("failed to open daemon guard: {error}"))
        })?;
    loop {
        match Flock::lock(file, FlockArg::LockExclusiveNonblock) {
            Ok(lock) => return Ok(Some(MacosDaemonGuard { _lock: lock })),
            Err((returned, Errno::EINTR)) => file = returned,
            Err((_, Errno::EAGAIN)) => return Ok(None),
            Err((_, error)) => {
                return Err(MacosOwnerExecutionError::new(format!(
                    "failed to acquire daemon guard: {error}"
                )));
            }
        }
    }
}

/// Request graceful termination through a retained, unreaped child handle.
///
/// # Errors
///
/// Returns an error when the child state cannot be inspected, its identifier
/// cannot be represented by the platform API, or `SIGTERM` cannot be delivered.
#[cfg(target_os = "macos")]
pub fn request_macos_child_termination(
    child: &mut std::process::Child,
) -> Result<(), MacosOwnerExecutionError> {
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;

    if child
        .try_wait()
        .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?
        .is_some()
    {
        return Ok(());
    }
    let pid = i32::try_from(child.id()).map_err(|_| {
        MacosOwnerExecutionError::new("retained child identifier exceeds the macOS process range")
    })?;
    kill(Pid::from_raw(pid), Signal::SIGTERM)
        .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))
}

/// Request graceful termination of a recorded owner process by pid.
///
/// The caller must have verified the exact live process identity against the
/// owner record (including audit token) and observed guard contention before
/// calling: this function delivers `SIGTERM` to whatever currently holds the pid.
///
/// # Errors
///
/// Returns an error when the identifier cannot be represented by the
/// platform API or `SIGTERM` cannot be delivered.
#[cfg(target_os = "macos")]
pub fn request_macos_pid_termination(pid: u32) -> Result<(), MacosOwnerExecutionError> {
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;

    let pid = i32::try_from(pid).map_err(|_| {
        MacosOwnerExecutionError::new("recorded owner identifier exceeds the macOS process range")
    })?;
    kill(Pid::from_raw(pid), Signal::SIGTERM)
        .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))
}

/// Wait until the final single-instance guard can be acquired.
#[cfg(target_os = "macos")]
pub fn wait_for_macos_guard_release(
    timeout: Duration,
    instance_name: &str,
) -> Result<bool, MacosOwnerExecutionError> {
    use std::time::Instant;

    let started = Instant::now();
    loop {
        if try_acquire_macos_daemon_guard(instance_name)?.is_some() {
            return Ok(true);
        }
        let Some(remaining) = timeout.checked_sub(started.elapsed()) else {
            return Ok(false);
        };
        std::thread::sleep(remaining.min(Duration::from_millis(25)));
    }
}

/// Wait for an exact durable owner publication through a native file watch.
pub fn wait_for_owner_publication(
    store: &MacosOwnerStore,
    owner: MacosDaemonOwner,
    after_epoch: u64,
    timeout: Duration,
) -> Result<bool, MacosOwnerExecutionError> {
    use notify::{RecursiveMode, Watcher};
    use std::sync::mpsc;
    use std::time::Instant;

    let matches = || {
        store
            .load_owner_record()
            .map(|record| {
                record.is_some_and(|record| {
                    record.active_owner == owner && record.owner_epoch > after_epoch
                })
            })
            .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))
    };
    if matches()? {
        return Ok(true);
    }
    let owner_path = store.owner_record_path();
    let directory = owner_path
        .parent()
        .ok_or_else(|| MacosOwnerExecutionError::new("owner record has no parent directory"))?
        .to_path_buf();
    fs::create_dir_all(&directory)
        .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
    let (signal_tx, signal_rx) = mpsc::sync_channel(1);
    let watched_path = owner_path.clone();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if event.is_ok_and(|event| event.paths.iter().any(|path| path == &watched_path)) {
            let _ = signal_tx.try_send(());
        }
    })
    .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
    watcher
        .watch(&directory, RecursiveMode::NonRecursive)
        .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
    if matches()? {
        return Ok(true);
    }
    let started = Instant::now();
    loop {
        let Some(remaining) = timeout.checked_sub(started.elapsed()) else {
            return Ok(false);
        };
        match signal_rx.recv_timeout(remaining) {
            Ok(()) if matches()? => return Ok(true),
            Ok(()) => {}
            Err(mpsc::RecvTimeoutError::Timeout) => return Ok(false),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(MacosOwnerExecutionError::new(
                    "owner publication watch disconnected",
                ));
            }
        }
    }
}

/// Bounded failure returned by a typed local operation executor.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{detail}")]
pub struct MacosOwnerExecutionError {
    detail: String,
}

impl MacosOwnerExecutionError {
    /// Construct an executor failure from a bounded operational detail.
    pub fn new(detail: impl Into<String>) -> Self {
        let mut detail = detail.into();
        detail.truncate(4_096);
        Self { detail }
    }
}

/// Typed local coordinator failure.
#[derive(Debug, thiserror::Error)]
pub enum MacosOwnerCoordinatorError {
    /// Durable owner or journal I/O failed.
    #[error(transparent)]
    Store(#[from] MacosOwnerStoreError),
    /// A typed local operation failed before rollback could finish.
    #[error("macOS daemon owner operation {operation:?} failed: {source}")]
    Operation {
        operation: MacosHandoverOperation,
        #[source]
        source: MacosOwnerExecutionError,
    },
    /// Inspecting one launcher's current installed state failed.
    #[error("failed to inspect {owner:?} autostart state: {source}")]
    InspectAutostart {
        owner: MacosDaemonOwner,
        #[source]
        source: MacosOwnerExecutionError,
    },
    /// The durable owner record does not exist.
    #[error("macOS daemon owner selection requires an active owner record")]
    MissingActiveOwner,
    /// Standalone is observable but has no local launcher to select.
    #[error("standalone daemon ownership cannot be selected by a coordinator")]
    StandaloneCannotBeSelected,
    /// Recovery attempted an operation absent from the validated journal.
    #[error("macOS handover journal does not authorize operation {operation:?}")]
    UnauthorizedOperation { operation: MacosHandoverOperation },
    /// A managed owner did not release the guard within ten seconds.
    #[error("macOS daemon guard did not release within the managed handover timeout")]
    GuardReleaseTimeout,
    /// A requested owner did not publish a matching owner epoch in time.
    #[error("requested macOS daemon owner did not publish before startup timeout")]
    OwnerStartupTimeout,
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
    /// A matching daemon-session attestation could not be removed.
    #[error("failed to remove macOS daemon session attestation at {path}: {source}")]
    RemoveSessionAttestation {
        /// Attestation path.
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
    /// A concurrent recovery participant already advanced the journal.
    #[error("macOS handover phase changed from {expected:?} to {found:?}")]
    HandoverPhaseChanged {
        /// Phase expected by the caller.
        expected: MacosHandoverPhase,
        /// Current durable phase.
        found: MacosHandoverPhase,
    },
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

    fn bind_requested_epoch(
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

/// Run a local, synchronous daemon-owner choice.
pub fn choose_daemon_owner(
    store: &MacosOwnerStore,
    executor: &mut impl MacosOwnerExecutor,
    requested_owner: MacosDaemonOwner,
    transaction_id: MacosHandoverTransactionId,
) -> Result<MacosOwnerCoordinatorOutcome, MacosOwnerCoordinatorError> {
    if let Some(existing) = store.load_handover_journal()?
        && !existing.phase.is_terminal()
    {
        let recovered = run_handover(store, executor, existing)?;
        if !matches!(recovered, MacosOwnerCoordinatorOutcome::Active { .. }) {
            return Ok(recovered);
        }
    }

    let prior_record = store
        .load_owner_record()?
        .ok_or(MacosOwnerCoordinatorError::MissingActiveOwner)?;
    if requested_owner != prior_record.active_owner
        && prior_record.active_owner != MacosDaemonOwner::Standalone
    {
        preflight_exact_stop_authority(
            store,
            executor,
            prior_record.active_owner,
            prior_record.owner_epoch,
        )?;
    }
    let prior_autostart_states = MacosAutostartStates::new(
        inspect_autostart(executor, MacosDaemonOwner::AppSidecar)?,
        inspect_autostart(executor, MacosDaemonOwner::DirectLaunchd)?,
        inspect_autostart(executor, MacosDaemonOwner::Homebrew)?,
    );
    let journal = MacosHandoverJournal::for_owner_choice(
        transaction_id,
        requested_owner,
        &prior_record,
        prior_autostart_states,
    )?;
    let journal = store.begin_handover(journal)?;
    run_handover(store, executor, journal)
}

fn inspect_autostart(
    executor: &mut impl MacosOwnerExecutor,
    owner: MacosDaemonOwner,
) -> Result<bool, MacosOwnerCoordinatorError> {
    executor
        .autostart_enabled(owner)
        .map_err(|source| MacosOwnerCoordinatorError::InspectAutostart { owner, source })
}

fn preflight_forward_stop_authority(
    store: &MacosOwnerStore,
    executor: &mut impl MacosOwnerExecutor,
    journal: &MacosHandoverJournal,
) -> Result<ForwardStopPreflight, MacosOwnerCoordinatorError> {
    if journal.requested_owner == journal.prior_owner
        || journal.prior_owner == MacosDaemonOwner::Standalone
    {
        return Ok(ForwardStopPreflight::Ready);
    }
    let operation = flush_stop_operation(journal.prior_owner)?;
    let record =
        store
            .load_owner_record()?
            .ok_or_else(|| MacosOwnerCoordinatorError::Operation {
                operation,
                source: MacosOwnerExecutionError::new(
                    "macOS owner record is unavailable during stop-authority preflight",
                ),
            })?;
    if record.active_owner == journal.prior_owner && record.owner_epoch > journal.active_epoch {
        return Ok(ForwardStopPreflight::PriorOwnerReplaced);
    }
    if record.active_owner != journal.prior_owner || record.owner_epoch != journal.active_epoch {
        return Err(MacosOwnerCoordinatorError::Operation {
            operation,
            source: MacosOwnerExecutionError::new(
                "macOS owner incarnation changed before stop-authority preflight",
            ),
        });
    }
    executor
        .preflight_stop_authority(&record.incarnation())
        .map_err(|source| MacosOwnerCoordinatorError::Operation { operation, source })?;
    Ok(ForwardStopPreflight::Ready)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ForwardStopPreflight {
    Ready,
    PriorOwnerReplaced,
}

fn preflight_exact_stop_authority(
    store: &MacosOwnerStore,
    executor: &mut impl MacosOwnerExecutor,
    owner: MacosDaemonOwner,
    owner_epoch: u64,
) -> Result<(), MacosOwnerCoordinatorError> {
    let operation = flush_stop_operation(owner)?;
    let incarnation = store
        .load_owner_record()?
        .filter(|record| record.active_owner == owner && record.owner_epoch == owner_epoch)
        .map(|record| record.incarnation())
        .ok_or_else(|| MacosOwnerCoordinatorError::Operation {
            operation,
            source: MacosOwnerExecutionError::new(
                "macOS owner incarnation changed before stop-authority preflight",
            ),
        })?;
    executor
        .preflight_stop_authority(&incarnation)
        .map_err(|source| MacosOwnerCoordinatorError::Operation { operation, source })
}

/// Resume the current local transaction before accepting another owner choice.
pub fn recover_daemon_owner(
    store: &MacosOwnerStore,
    executor: &mut impl MacosOwnerExecutor,
) -> Result<Option<MacosOwnerCoordinatorOutcome>, MacosOwnerCoordinatorError> {
    let Some(journal) = store.load_handover_journal()? else {
        return Ok(None);
    };
    if journal.phase.is_terminal() {
        return Ok(None);
    }
    run_handover(store, executor, journal).map(Some)
}

/// Reconcile a journal from a daemon that already holds the process guard.
pub fn recover_incoming_daemon_owner(
    store: &MacosOwnerStore,
    current_owner: MacosDaemonOwner,
) -> Result<Option<MacosOwnerCoordinatorOutcome>, MacosOwnerCoordinatorError> {
    let Some(mut journal) = store.load_handover_journal()? else {
        return Ok(None);
    };
    if journal.phase.is_terminal() {
        return Ok(None);
    }
    if current_owner == journal.requested_owner && requested_owner_can_complete(&journal) {
        return complete_requested_owner_recovery(store, journal).map(Some);
    }
    if current_owner == journal.prior_owner
        && matches!(
            journal.phase,
            MacosHandoverPhase::RollbackStartRequested
                | MacosHandoverPhase::PriorOwnerStarted
                | MacosHandoverPhase::RollbackCommitPending
        )
    {
        if journal.phase == MacosHandoverPhase::RollbackStartRequested {
            journal = advance(store, &journal, MacosHandoverPhase::PriorOwnerStarted)?;
            if let Some(outcome) = terminal_outcome(store, &journal)? {
                return Ok(Some(outcome));
            }
        }
        if journal.phase == MacosHandoverPhase::PriorOwnerStarted {
            journal = advance(store, &journal, MacosHandoverPhase::RollbackCommitPending)?;
            if let Some(outcome) = terminal_outcome(store, &journal)? {
                return Ok(Some(outcome));
            }
        }
        store.set_external_owner_mode(external_owner_mode(journal.prior_owner))?;
        clear_conflict_if_present(store)?;
        let journal = advance(store, &journal, MacosHandoverPhase::RolledBack)?;
        return Ok(Some(terminal_outcome(store, &journal)?.unwrap_or(
            MacosOwnerCoordinatorOutcome::RecoveryRequired {
                requested_owner: journal.requested_owner,
                prior_owner: journal.prior_owner,
                phase: journal.phase,
            },
        )));
    }
    Ok(Some(recovery_required(&journal)))
}

const fn requested_owner_can_complete(journal: &MacosHandoverJournal) -> bool {
    match journal.phase {
        MacosHandoverPhase::AutostartsConfigured
        | MacosHandoverPhase::StartRequested
        | MacosHandoverPhase::RequestedOwnerStarted
        | MacosHandoverPhase::CommitPending => true,
        MacosHandoverPhase::StopRequested
        | MacosHandoverPhase::OutgoingOwnerStopped
        | MacosHandoverPhase::AwaitingGuardRelease
        | MacosHandoverPhase::GuardReleased => journal.pending_standalone_pid.is_none(),
        MacosHandoverPhase::Prepared
        | MacosHandoverPhase::Committed
        | MacosHandoverPhase::RollbackPending
        | MacosHandoverPhase::RollbackAutostartsRestored
        | MacosHandoverPhase::RollbackStopRequested
        | MacosHandoverPhase::RollbackOwnerStopped
        | MacosHandoverPhase::RollbackAwaitingGuardRelease
        | MacosHandoverPhase::RollbackGuardReleased
        | MacosHandoverPhase::RollbackStartRequested
        | MacosHandoverPhase::PriorOwnerStarted
        | MacosHandoverPhase::RollbackCommitPending
        | MacosHandoverPhase::RolledBack => false,
    }
}

fn complete_requested_owner_recovery(
    store: &MacosOwnerStore,
    mut journal: MacosHandoverJournal,
) -> Result<MacosOwnerCoordinatorOutcome, MacosOwnerCoordinatorError> {
    loop {
        if !requested_owner_can_complete(&journal) {
            return Ok(recovery_required(&journal));
        }
        journal = match journal.phase {
            MacosHandoverPhase::AutostartsConfigured => advance(
                store,
                &journal,
                if journal.requested_owner == journal.prior_owner {
                    MacosHandoverPhase::CommitPending
                } else if journal.pending_standalone_pid.is_some() {
                    MacosHandoverPhase::StartRequested
                } else {
                    MacosHandoverPhase::StopRequested
                },
            )?,
            MacosHandoverPhase::StopRequested => {
                advance(store, &journal, MacosHandoverPhase::OutgoingOwnerStopped)?
            }
            MacosHandoverPhase::OutgoingOwnerStopped => {
                advance(store, &journal, MacosHandoverPhase::AwaitingGuardRelease)?
            }
            MacosHandoverPhase::AwaitingGuardRelease => {
                advance(store, &journal, MacosHandoverPhase::GuardReleased)?
            }
            MacosHandoverPhase::GuardReleased => {
                advance(store, &journal, MacosHandoverPhase::StartRequested)?
            }
            MacosHandoverPhase::StartRequested => {
                advance(store, &journal, MacosHandoverPhase::RequestedOwnerStarted)?
            }
            MacosHandoverPhase::RequestedOwnerStarted => {
                advance(store, &journal, MacosHandoverPhase::CommitPending)?
            }
            MacosHandoverPhase::CommitPending => {
                store.set_external_owner_mode(external_owner_mode(journal.requested_owner))?;
                clear_conflict_if_present(store)?;
                let committed = advance(store, &journal, MacosHandoverPhase::Committed)?;
                return Ok(terminal_outcome(store, &committed)?
                    .unwrap_or_else(|| recovery_required(&committed)));
            }
            MacosHandoverPhase::Prepared
            | MacosHandoverPhase::Committed
            | MacosHandoverPhase::RollbackPending
            | MacosHandoverPhase::RollbackAutostartsRestored
            | MacosHandoverPhase::RollbackStopRequested
            | MacosHandoverPhase::RollbackOwnerStopped
            | MacosHandoverPhase::RollbackAwaitingGuardRelease
            | MacosHandoverPhase::RollbackGuardReleased
            | MacosHandoverPhase::RollbackStartRequested
            | MacosHandoverPhase::PriorOwnerStarted
            | MacosHandoverPhase::RollbackCommitPending
            | MacosHandoverPhase::RolledBack => return Ok(recovery_required(&journal)),
        };
    }
}

fn recovery_required(journal: &MacosHandoverJournal) -> MacosOwnerCoordinatorOutcome {
    MacosOwnerCoordinatorOutcome::RecoveryRequired {
        requested_owner: journal.requested_owner,
        prior_owner: journal.prior_owner,
        phase: journal.phase,
    }
}

fn rollback_stop_authority_is_unbound(journal: &MacosHandoverJournal) -> bool {
    journal
        .contender_epoch
        .is_none_or(|epoch| epoch <= journal.active_epoch)
}

fn newer_prior_owner_is_published(
    store: &MacosOwnerStore,
    journal: &MacosHandoverJournal,
) -> Result<bool, MacosOwnerStoreError> {
    Ok(store.load_owner_record()?.is_some_and(|record| {
        record.active_owner == journal.prior_owner && record.owner_epoch > journal.active_epoch
    }))
}

fn terminal_outcome(
    store: &MacosOwnerStore,
    journal: &MacosHandoverJournal,
) -> Result<Option<MacosOwnerCoordinatorOutcome>, MacosOwnerStoreError> {
    match journal.phase {
        MacosHandoverPhase::Committed => {
            let owner_epoch = store
                .load_owner_record()?
                .filter(|record| record.active_owner == journal.requested_owner)
                .map_or(journal.active_epoch, |record| record.owner_epoch);
            Ok(Some(MacosOwnerCoordinatorOutcome::Active {
                owner: journal.requested_owner,
                owner_epoch,
            }))
        }
        MacosHandoverPhase::RolledBack => Ok(Some(MacosOwnerCoordinatorOutcome::RolledBack {
            prior_owner: journal.prior_owner,
            failure: "requested owner failed to become active".to_owned(),
        })),
        _ => Ok(None),
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "the match is the auditable one-to-one encoding of all durable phases"
)]
fn run_handover(
    store: &MacosOwnerStore,
    executor: &mut impl MacosOwnerExecutor,
    mut journal: MacosHandoverJournal,
) -> Result<MacosOwnerCoordinatorOutcome, MacosOwnerCoordinatorError> {
    loop {
        match journal.phase {
            MacosHandoverPhase::Prepared => {
                if let Some(pid) = journal.pending_standalone_pid {
                    require_operation(
                        &journal.allowed_forward_operations,
                        MacosHandoverOperation::AwaitStandaloneExit { pid },
                    )?;
                    journal = advance(store, &journal, MacosHandoverPhase::AwaitingGuardRelease)?;
                } else {
                    if preflight_forward_stop_authority(store, executor, &journal)?
                        == ForwardStopPreflight::PriorOwnerReplaced
                    {
                        journal = begin_rollback(store, &journal)?;
                        continue;
                    }
                    for operation in autostart_operations_for(journal.requested_owner) {
                        if execute_operation(store, executor, &journal, operation, true).is_err() {
                            journal = begin_rollback(store, &journal)?;
                            break;
                        }
                    }
                    if journal.phase == MacosHandoverPhase::Prepared {
                        journal =
                            advance(store, &journal, MacosHandoverPhase::AutostartsConfigured)?;
                    }
                }
            }
            MacosHandoverPhase::AutostartsConfigured => {
                if preflight_forward_stop_authority(store, executor, &journal)?
                    == ForwardStopPreflight::PriorOwnerReplaced
                {
                    journal = begin_rollback(store, &journal)?;
                    continue;
                }
                journal = advance(
                    store,
                    &journal,
                    if journal.requested_owner == journal.prior_owner {
                        MacosHandoverPhase::CommitPending
                    } else if journal.pending_standalone_pid.is_some() {
                        MacosHandoverPhase::StartRequested
                    } else {
                        MacosHandoverPhase::StopRequested
                    },
                )?;
            }
            MacosHandoverPhase::StopRequested => {
                if preflight_forward_stop_authority(store, executor, &journal)?
                    == ForwardStopPreflight::PriorOwnerReplaced
                {
                    journal = begin_rollback(store, &journal)?;
                    continue;
                }
                let operation = flush_stop_operation(journal.prior_owner)?;
                if execute_operation(store, executor, &journal, operation, true).is_err() {
                    journal = begin_rollback(store, &journal)?;
                } else {
                    journal = advance(store, &journal, MacosHandoverPhase::OutgoingOwnerStopped)?;
                }
            }
            MacosHandoverPhase::OutgoingOwnerStopped => {
                journal = advance(store, &journal, MacosHandoverPhase::AwaitingGuardRelease)?;
            }
            MacosHandoverPhase::AwaitingGuardRelease => {
                let standalone_pid = journal.pending_standalone_pid;
                let timeout = if journal.pending_standalone_pid.is_some() {
                    MACOS_STANDALONE_HANDOVER_TIMEOUT
                } else {
                    MACOS_MANAGED_HANDOVER_TIMEOUT
                };
                let released = executor.wait_for_guard_release(timeout);
                if released
                    .as_ref()
                    .is_err_and(|_| journal.pending_standalone_pid.is_some())
                {
                    return Err(MacosOwnerCoordinatorError::Operation {
                        operation: MacosHandoverOperation::AwaitStandaloneExit {
                            pid: standalone_pid.expect("checked standalone handover"),
                        },
                        source: released.expect_err("checked error result"),
                    });
                }
                if released.is_err() {
                    journal = begin_rollback(store, &journal)?;
                    continue;
                }
                if !released.expect("checked successful result") {
                    if journal.pending_standalone_pid.is_some() {
                        return Ok(MacosOwnerCoordinatorOutcome::PendingStandalone {
                            requested_owner: journal.requested_owner,
                            remedy: MacosOwnerRemedy::StopStandaloneOwner {
                                pid: standalone_pid.expect("checked standalone handover"),
                            },
                        });
                    }
                    journal = begin_rollback(store, &journal)?;
                    continue;
                }
                journal = advance(store, &journal, MacosHandoverPhase::GuardReleased)?;
            }
            MacosHandoverPhase::GuardReleased => {
                if journal.pending_standalone_pid.is_some() {
                    for operation in autostart_operations_for(journal.requested_owner) {
                        if let Err(error) =
                            execute_operation(store, executor, &journal, operation, true)
                        {
                            return Err(error);
                        }
                    }
                    journal = advance(store, &journal, MacosHandoverPhase::AutostartsConfigured)?;
                } else {
                    journal = advance(store, &journal, MacosHandoverPhase::StartRequested)?;
                }
            }
            MacosHandoverPhase::StartRequested => {
                let operation = start_operation(journal.requested_owner)?;
                if let Err(error) = execute_operation(store, executor, &journal, operation, true) {
                    if journal.prior_owner == MacosDaemonOwner::Standalone {
                        return Err(error);
                    }
                    journal = bind_requested_epoch_from_record(store, &journal)?;
                    journal = begin_rollback(store, &journal)?;
                    continue;
                }
                journal = advance(store, &journal, MacosHandoverPhase::RequestedOwnerStarted)?;
            }
            MacosHandoverPhase::RequestedOwnerStarted => {
                let started = executor.wait_for_owner(
                    journal.requested_owner,
                    journal.active_epoch,
                    MACOS_MANAGED_HANDOVER_TIMEOUT,
                );
                journal = bind_requested_epoch_from_record(store, &journal)?;
                if started.is_err() && journal.prior_owner == MacosDaemonOwner::Standalone {
                    return Err(MacosOwnerCoordinatorError::Operation {
                        operation: start_operation(journal.requested_owner)
                            .expect("validated requested owner is managed"),
                        source: started.expect_err("checked error result"),
                    });
                }
                if !started.unwrap_or(false) {
                    if journal.prior_owner == MacosDaemonOwner::Standalone {
                        return Err(MacosOwnerCoordinatorError::OwnerStartupTimeout);
                    }
                    journal = begin_rollback(store, &journal)?;
                    continue;
                }
                journal = advance(store, &journal, MacosHandoverPhase::CommitPending)?;
            }
            MacosHandoverPhase::CommitPending => {
                store.set_external_owner_mode(external_owner_mode(journal.requested_owner))?;
                clear_conflict_if_present(store)?;
                journal = advance(store, &journal, MacosHandoverPhase::Committed)?;
            }
            MacosHandoverPhase::Committed => {
                let owner_epoch = store
                    .load_owner_record()?
                    .filter(|record| record.active_owner == journal.requested_owner)
                    .map_or(journal.active_epoch, |record| record.owner_epoch);
                return Ok(MacosOwnerCoordinatorOutcome::Active {
                    owner: journal.requested_owner,
                    owner_epoch,
                });
            }
            MacosHandoverPhase::RollbackPending => {
                if journal.requested_owner != journal.prior_owner
                    && !newer_prior_owner_is_published(store, &journal)?
                    && !rollback_stop_authority_is_unbound(&journal)
                {
                    preflight_exact_stop_authority(
                        store,
                        executor,
                        journal.requested_owner,
                        journal
                            .contender_epoch
                            .expect("checked bound contender epoch"),
                    )?;
                }
                for operation in autostart_operations_from(journal.prior_autostart_states) {
                    execute_operation(store, executor, &journal, operation, false)?;
                }
                journal = advance(
                    store,
                    &journal,
                    MacosHandoverPhase::RollbackAutostartsRestored,
                )?;
            }
            MacosHandoverPhase::RollbackAutostartsRestored => {
                let prior_owner_is_active = newer_prior_owner_is_published(store, &journal)?;
                if journal.requested_owner != journal.prior_owner && !prior_owner_is_active {
                    if rollback_stop_authority_is_unbound(&journal) {
                        return Ok(recovery_required(&journal));
                    }
                    preflight_exact_stop_authority(
                        store,
                        executor,
                        journal.requested_owner,
                        journal
                            .contender_epoch
                            .expect("checked bound contender epoch"),
                    )?;
                }
                journal = advance(
                    store,
                    &journal,
                    if journal.requested_owner == journal.prior_owner || prior_owner_is_active {
                        MacosHandoverPhase::RollbackCommitPending
                    } else {
                        MacosHandoverPhase::RollbackStopRequested
                    },
                )?;
            }
            MacosHandoverPhase::RollbackStopRequested => {
                if rollback_stop_authority_is_unbound(&journal) {
                    return Ok(recovery_required(&journal));
                }
                preflight_exact_stop_authority(
                    store,
                    executor,
                    journal.requested_owner,
                    journal
                        .contender_epoch
                        .expect("checked bound contender epoch"),
                )?;
                let operation = flush_stop_operation(journal.requested_owner)?;
                execute_operation(store, executor, &journal, operation, false)?;
                journal = advance(store, &journal, MacosHandoverPhase::RollbackOwnerStopped)?;
            }
            MacosHandoverPhase::RollbackOwnerStopped => {
                if rollback_stop_authority_is_unbound(&journal) {
                    return Ok(recovery_required(&journal));
                }
                journal = advance(
                    store,
                    &journal,
                    MacosHandoverPhase::RollbackAwaitingGuardRelease,
                )?;
            }
            MacosHandoverPhase::RollbackAwaitingGuardRelease => {
                if rollback_stop_authority_is_unbound(&journal) {
                    return Ok(recovery_required(&journal));
                }
                if !executor
                    .wait_for_guard_release(MACOS_MANAGED_HANDOVER_TIMEOUT)
                    .map_err(|source| MacosOwnerCoordinatorError::Operation {
                        operation: flush_stop_operation(journal.requested_owner)
                            .expect("validated requested owner is managed"),
                        source,
                    })?
                {
                    return Err(MacosOwnerCoordinatorError::GuardReleaseTimeout);
                }
                journal = advance(store, &journal, MacosHandoverPhase::RollbackGuardReleased)?;
            }
            MacosHandoverPhase::RollbackGuardReleased => {
                journal = advance(store, &journal, MacosHandoverPhase::RollbackStartRequested)?;
            }
            MacosHandoverPhase::RollbackStartRequested => {
                let operation = start_operation(journal.prior_owner)?;
                execute_operation(store, executor, &journal, operation, false)?;
                journal = advance(store, &journal, MacosHandoverPhase::PriorOwnerStarted)?;
            }
            MacosHandoverPhase::PriorOwnerStarted => {
                if !executor
                    .wait_for_owner(
                        journal.prior_owner,
                        journal.active_epoch,
                        MACOS_MANAGED_HANDOVER_TIMEOUT,
                    )
                    .map_err(|source| MacosOwnerCoordinatorError::Operation {
                        operation: start_operation(journal.prior_owner)
                            .expect("rollback prior owner is managed"),
                        source,
                    })?
                {
                    return Err(MacosOwnerCoordinatorError::OwnerStartupTimeout);
                }
                journal = advance(store, &journal, MacosHandoverPhase::RollbackCommitPending)?;
            }
            MacosHandoverPhase::RollbackCommitPending => {
                store.set_external_owner_mode(external_owner_mode(journal.prior_owner))?;
                clear_conflict_if_present(store)?;
                journal = advance(store, &journal, MacosHandoverPhase::RolledBack)?;
            }
            MacosHandoverPhase::RolledBack => {
                return Ok(MacosOwnerCoordinatorOutcome::RolledBack {
                    prior_owner: journal.prior_owner,
                    failure: "requested owner failed to become active".to_owned(),
                });
            }
        }
    }
}

fn clear_conflict_if_present(store: &MacosOwnerStore) -> Result<(), MacosOwnerStoreError> {
    if store.load_owner_record()?.is_some() {
        store.clear_conflict()?;
    }
    Ok(())
}

fn advance(
    store: &MacosOwnerStore,
    journal: &MacosHandoverJournal,
    phase: MacosHandoverPhase,
) -> Result<MacosHandoverJournal, MacosOwnerStoreError> {
    match store.advance_handover_from(&journal.transaction_id, journal.phase, phase) {
        Ok(advanced) => Ok(advanced),
        Err(MacosOwnerStoreError::HandoverPhaseChanged { .. }) => store
            .load_handover_journal()?
            .filter(|current| current.transaction_id == journal.transaction_id)
            .ok_or(MacosOwnerStoreError::HandoverTransactionMismatch),
        Err(error) => Err(error),
    }
}

fn begin_rollback(
    store: &MacosOwnerStore,
    journal: &MacosHandoverJournal,
) -> Result<MacosHandoverJournal, MacosOwnerStoreError> {
    advance(store, journal, MacosHandoverPhase::RollbackPending)
}

fn execute_operation(
    store: &MacosOwnerStore,
    executor: &mut impl MacosOwnerExecutor,
    journal: &MacosHandoverJournal,
    operation: MacosHandoverOperation,
    forward: bool,
) -> Result<(), MacosOwnerCoordinatorError> {
    let allowed = if forward {
        &journal.allowed_forward_operations
    } else {
        &journal.allowed_rollback_operations
    };
    require_operation(allowed, operation)?;
    match operation {
        MacosHandoverOperation::SetAppSidecarAutostart { enabled } => {
            executor.set_autostart(MacosDaemonOwner::AppSidecar, enabled)
        }
        MacosHandoverOperation::FlushAndStopAppSidecar {} => flush_and_stop_owner(
            store,
            executor,
            journal,
            MacosDaemonOwner::AppSidecar,
            forward,
        ),
        MacosHandoverOperation::StartAppSidecar {} => executor.start(MacosDaemonOwner::AppSidecar),
        MacosHandoverOperation::SetDirectLaunchdAutostart { enabled } => {
            executor.set_autostart(MacosDaemonOwner::DirectLaunchd, enabled)
        }
        MacosHandoverOperation::FlushAndStopDirectLaunchd {} => flush_and_stop_owner(
            store,
            executor,
            journal,
            MacosDaemonOwner::DirectLaunchd,
            forward,
        ),
        MacosHandoverOperation::StartDirectLaunchd {} => {
            executor.start(MacosDaemonOwner::DirectLaunchd)
        }
        MacosHandoverOperation::SetHomebrewAutostart { enabled } => {
            executor.set_autostart(MacosDaemonOwner::Homebrew, enabled)
        }
        MacosHandoverOperation::FlushAndStopHomebrew {} => flush_and_stop_owner(
            store,
            executor,
            journal,
            MacosDaemonOwner::Homebrew,
            forward,
        ),
        MacosHandoverOperation::StartHomebrew {} => executor.start(MacosDaemonOwner::Homebrew),
        MacosHandoverOperation::AwaitStandaloneExit { .. } => Ok(()),
    }
    .map_err(|source| MacosOwnerCoordinatorError::Operation { operation, source })
}

fn flush_and_stop_owner(
    store: &MacosOwnerStore,
    executor: &mut impl MacosOwnerExecutor,
    journal: &MacosHandoverJournal,
    owner: MacosDaemonOwner,
    forward: bool,
) -> Result<(), MacosOwnerExecutionError> {
    let incarnation = if forward {
        if owner != journal.prior_owner {
            return Err(MacosOwnerExecutionError::new(
                "forward stop does not target the journal's prior owner",
            ));
        }
        store
            .load_owner_record()
            .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?
            .filter(|record| {
                record.active_owner == owner && record.owner_epoch == journal.active_epoch
            })
            .map(|record| record.incarnation())
            .ok_or_else(|| {
                MacosOwnerExecutionError::new(
                    "handover journal has no matching prior owner incarnation",
                )
            })?
    } else {
        if owner != journal.requested_owner {
            return Err(MacosOwnerExecutionError::new(
                "rollback stop does not target the journal's requested owner",
            ));
        }
        let Some(requested_epoch) = journal
            .contender_epoch
            .filter(|epoch| *epoch > journal.active_epoch)
        else {
            return Ok(());
        };
        store
            .load_owner_record()
            .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?
            .filter(|record| record.active_owner == owner && record.owner_epoch == requested_epoch)
            .map(|record| record.incarnation())
            .ok_or_else(|| {
                MacosOwnerExecutionError::new(
                    "handover journal has no matching requested owner incarnation",
                )
            })?
    };
    store.request_stop_if_current(&incarnation, || executor.flush_and_stop(&incarnation))
}

fn bind_requested_epoch_from_record(
    store: &MacosOwnerStore,
    journal: &MacosHandoverJournal,
) -> Result<MacosHandoverJournal, MacosOwnerStoreError> {
    let requested = store
        .load_owner_record()?
        .filter(|record| {
            record.active_owner == journal.requested_owner
                && record.owner_epoch > journal.active_epoch
        })
        .map(|record| (record.active_owner, record.owner_epoch));
    requested.map_or_else(
        || Ok(journal.clone()),
        |(owner, epoch)| store.bind_requested_epoch(&journal.transaction_id, owner, epoch),
    )
}

fn require_operation(
    allowed: &[MacosHandoverOperation],
    operation: MacosHandoverOperation,
) -> Result<(), MacosOwnerCoordinatorError> {
    if allowed.contains(&operation) {
        Ok(())
    } else {
        Err(MacosOwnerCoordinatorError::UnauthorizedOperation { operation })
    }
}

const fn external_owner_mode(owner: MacosDaemonOwner) -> Option<MacosExternalOwnerMode> {
    match owner {
        MacosDaemonOwner::DirectLaunchd => Some(MacosExternalOwnerMode::DirectLaunchd),
        MacosDaemonOwner::Homebrew => Some(MacosExternalOwnerMode::Homebrew),
        MacosDaemonOwner::AppSidecar | MacosDaemonOwner::Standalone => None,
    }
}

fn forward_operations(
    requested_owner: MacosDaemonOwner,
    prior_owner: MacosDaemonOwner,
    pending_standalone_pid: Option<u32>,
) -> Vec<MacosHandoverOperation> {
    let mut operations = autostart_operations_for(requested_owner).to_vec();
    if requested_owner == prior_owner {
        return operations;
    }
    if let Some(pid) = pending_standalone_pid {
        operations.push(MacosHandoverOperation::AwaitStandaloneExit { pid });
    } else if let Ok(stop) = flush_stop_operation(prior_owner) {
        operations.push(stop);
    }
    if let Ok(start) = start_operation(requested_owner) {
        operations.push(start);
    }
    operations
}

fn rollback_operations(
    requested_owner: MacosDaemonOwner,
    prior_owner: MacosDaemonOwner,
    prior_states: MacosAutostartStates,
) -> Vec<MacosHandoverOperation> {
    let mut operations = autostart_operations_from(prior_states).to_vec();
    if requested_owner == prior_owner {
        return operations;
    }
    if let Ok(stop) = flush_stop_operation(requested_owner) {
        operations.push(stop);
    }
    if let Ok(start) = start_operation(prior_owner) {
        operations.push(start);
    }
    operations
}

const fn autostart_operations_for(owner: MacosDaemonOwner) -> [MacosHandoverOperation; 3] {
    [
        MacosHandoverOperation::SetAppSidecarAutostart {
            enabled: matches!(owner, MacosDaemonOwner::AppSidecar),
        },
        MacosHandoverOperation::SetDirectLaunchdAutostart {
            enabled: matches!(owner, MacosDaemonOwner::DirectLaunchd),
        },
        MacosHandoverOperation::SetHomebrewAutostart {
            enabled: matches!(owner, MacosDaemonOwner::Homebrew),
        },
    ]
}

const fn autostart_operations_from(states: MacosAutostartStates) -> [MacosHandoverOperation; 3] {
    [
        MacosHandoverOperation::SetAppSidecarAutostart {
            enabled: states.app_sidecar,
        },
        MacosHandoverOperation::SetDirectLaunchdAutostart {
            enabled: states.direct_launchd,
        },
        MacosHandoverOperation::SetHomebrewAutostart {
            enabled: states.homebrew,
        },
    ]
}

const fn flush_stop_operation(
    owner: MacosDaemonOwner,
) -> Result<MacosHandoverOperation, MacosOwnerCoordinatorError> {
    match owner {
        MacosDaemonOwner::AppSidecar => Ok(MacosHandoverOperation::FlushAndStopAppSidecar {}),
        MacosDaemonOwner::DirectLaunchd => Ok(MacosHandoverOperation::FlushAndStopDirectLaunchd {}),
        MacosDaemonOwner::Homebrew => Ok(MacosHandoverOperation::FlushAndStopHomebrew {}),
        MacosDaemonOwner::Standalone => Err(MacosOwnerCoordinatorError::StandaloneCannotBeSelected),
    }
}

const fn start_operation(
    owner: MacosDaemonOwner,
) -> Result<MacosHandoverOperation, MacosOwnerCoordinatorError> {
    match owner {
        MacosDaemonOwner::AppSidecar => Ok(MacosHandoverOperation::StartAppSidecar {}),
        MacosDaemonOwner::DirectLaunchd => Ok(MacosHandoverOperation::StartDirectLaunchd {}),
        MacosDaemonOwner::Homebrew => Ok(MacosHandoverOperation::StartHomebrew {}),
        MacosDaemonOwner::Standalone => Err(MacosOwnerCoordinatorError::StandaloneCannotBeSelected),
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

fn successor_owner_record(
    previous: MacosOwnerRecord,
    active_owner: MacosDaemonOwner,
    active_identity: MacosOwnerIdentity,
) -> Result<MacosOwnerRecord, MacosOwnerStoreError> {
    let owner_epoch = previous
        .owner_epoch
        .checked_add(1)
        .ok_or(MacosOwnerStoreError::OwnerEpochOverflow)?;
    let conflict = previous
        .conflict
        .filter(|conflict| {
            conflict.contender_owner != active_owner
                || conflict.contender_identity.executable_path != active_identity.executable_path
                || conflict.contender_identity.designated_requirement_hash
                    != active_identity.designated_requirement_hash
        })
        .map(|mut conflict| {
            conflict.active_owner = active_owner;
            conflict.active_epoch = owner_epoch;
            conflict
        });
    Ok(MacosOwnerRecord {
        owner_epoch,
        schema_version: MACOS_OWNER_RECORD_SCHEMA_VERSION,
        active_owner,
        active_identity,
        conflict,
        selected_external_owner: previous.selected_external_owner,
    })
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

fn read_daemon_session_attestation(
    path: &Path,
) -> Result<Option<MacosDaemonSessionAttestation>, MacosOwnerStoreError> {
    let Some(bytes) = read_private_optional(path, "daemon session attestation")? else {
        return Ok(None);
    };
    let attestation =
        serde_json::from_slice::<MacosDaemonSessionAttestation>(&bytes).map_err(|source| {
            MacosOwnerStoreError::Decode {
                artifact: "daemon session attestation",
                source,
            }
        })?;
    validate_daemon_session_attestation(&attestation)?;
    Ok(Some(attestation))
}

fn read_private_optional(
    path: &Path,
    artifact: &'static str,
) -> Result<Option<Vec<u8>>, MacosOwnerStoreError> {
    let path_metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(MacosOwnerStoreError::Read {
                artifact,
                path: path.to_path_buf(),
                source,
            });
        }
    };
    validate_private_file_metadata(&path_metadata, artifact)?;
    let file = File::open(path).map_err(|source| MacosOwnerStoreError::Read {
        artifact,
        path: path.to_path_buf(),
        source,
    })?;
    let file_metadata = file
        .metadata()
        .map_err(|source| MacosOwnerStoreError::Read {
            artifact,
            path: path.to_path_buf(),
            source,
        })?;
    validate_private_file_metadata(&file_metadata, artifact)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if path_metadata.dev() != file_metadata.dev() || path_metadata.ino() != file_metadata.ino()
        {
            return Err(MacosOwnerStoreError::InvalidArtifact {
                artifact,
                detail: "file changed while it was opened",
            });
        }
    }
    read_bounded_file(file, path, artifact).map(Some)
}

fn validate_private_file_metadata(
    metadata: &fs::Metadata,
    artifact: &'static str,
) -> Result<(), MacosOwnerStoreError> {
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(MacosOwnerStoreError::InvalidArtifact {
            artifact,
            detail: "must be a regular file",
        });
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.mode() & 0o777 != 0o600 {
            return Err(MacosOwnerStoreError::InvalidArtifact {
                artifact,
                detail: "mode must be 0600",
            });
        }
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::fs::MetadataExt;
        validate_private_file_uid(
            metadata.uid(),
            nix::unistd::Uid::current().as_raw(),
            artifact,
        )?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn validate_private_file_uid(
    owner_uid: u32,
    current_uid: u32,
    artifact: &'static str,
) -> Result<(), MacosOwnerStoreError> {
    if owner_uid != current_uid {
        return Err(MacosOwnerStoreError::InvalidArtifact {
            artifact,
            detail: "owner UID must match the current user",
        });
    }
    Ok(())
}

fn read_bounded_file(
    file: File,
    path: &Path,
    artifact: &'static str,
) -> Result<Vec<u8>, MacosOwnerStoreError> {
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
    Ok(bytes)
}

fn read_optional(
    path: &Path,
    artifact: &'static str,
) -> Result<Option<Vec<u8>>, MacosOwnerStoreError> {
    match File::open(path) {
        Ok(file) => read_bounded_file(file, path, artifact).map(Some),
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

fn validate_daemon_session_attestation(
    attestation: &MacosDaemonSessionAttestation,
) -> Result<(), MacosOwnerStoreError> {
    validate_version(
        "daemon session attestation",
        attestation.schema_version,
        MACOS_DAEMON_SESSION_ATTESTATION_SCHEMA_VERSION,
    )?;
    if attestation.owner_epoch == 0 {
        return Err(MacosOwnerStoreError::InvalidArtifact {
            artifact: "daemon session attestation",
            detail: "owner_epoch must be positive",
        });
    }
    validate_owner_identity(&attestation.owner_identity)?;
    validate_hex_token(
        attestation.server_session_id.as_str(),
        MACOS_SERVER_SESSION_ID_PREFIX,
        MACOS_SERVER_SESSION_ID_BYTES,
        "server_session_id must be a canonical 128-bit token",
    )?;
    validate_hex_token(
        attestation.protected_control_credential.expose_secret(),
        MACOS_PROTECTED_CONTROL_CREDENTIAL_PREFIX,
        MACOS_PROTECTED_CONTROL_CREDENTIAL_BYTES,
        "protected_control_credential must be a canonical 256-bit token",
    )
}

fn format_hex_token(prefix: &str, bytes: &[u8]) -> String {
    const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut value = String::with_capacity(prefix.len() + bytes.len() * 2);
    value.push_str(prefix);
    for byte in bytes {
        value.push(char::from(HEX_DIGITS[usize::from(byte >> 4)]));
        value.push(char::from(HEX_DIGITS[usize::from(byte & 0x0f)]));
    }
    value
}

fn validate_hex_token(
    value: &str,
    prefix: &str,
    entropy_bytes: usize,
    detail: &'static str,
) -> Result<(), MacosOwnerStoreError> {
    let hex = value
        .strip_prefix(prefix)
        .filter(|hex| hex.len() == entropy_bytes * 2)
        .filter(|hex| {
            hex.bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        });
    if hex.is_none() {
        return Err(MacosOwnerStoreError::InvalidArtifact {
            artifact: "daemon session attestation",
            detail,
        });
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
    if journal.allowed_forward_operations.len() > MAX_MACOS_HANDOVER_OPERATIONS {
        return Err(MacosOwnerStoreError::InvalidArtifact {
            artifact: "handover journal",
            detail: "allowed_forward_operations exceeds its item limit",
        });
    }
    if journal.allowed_rollback_operations.len() > MAX_MACOS_HANDOVER_OPERATIONS {
        return Err(MacosOwnerStoreError::InvalidArtifact {
            artifact: "handover journal",
            detail: "allowed_rollback_operations exceeds its item limit",
        });
    }
    if journal.pending_standalone_pid == Some(0)
        || journal
            .allowed_forward_operations
            .iter()
            .chain(&journal.allowed_rollback_operations)
            .any(|operation| {
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
            Ok(file) => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    file.set_permissions(fs::Permissions::from_mode(0o600))
                        .map_err(|source| MacosOwnerStoreError::CreateTemporary {
                            path: path.to_path_buf(),
                            source,
                        })?;
                }
                return Ok((file, temporary_path));
            }
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
#[expect(
    clippy::unnecessary_wraps,
    reason = "matches the fallible Unix implementation"
)]
fn sync_parent_directory(_data_dir: &Path) -> Result<(), MacosOwnerStoreError> {
    Ok(())
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::validate_private_file_uid;

    #[test]
    fn private_session_file_rejects_another_uid() {
        let error = validate_private_file_uid(501, 502, "daemon session attestation")
            .expect_err("another UID must fail closed");

        assert!(error.to_string().contains("current user"));
    }
}
