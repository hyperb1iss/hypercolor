use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use hypercolor_types::service::ProtectedControlCredential;
use serde::{Deserialize, Serialize};

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
pub(crate) const MACOS_SERVER_SESSION_ID_PREFIX: &str = "hc_session_";
pub(crate) const MACOS_SERVER_SESSION_ID_BYTES: usize = 16;

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
    pub(crate) fn has_same_identity(&self, other: &Self) -> bool {
        self.active_owner == other.active_owner
            && self.active_epoch == other.active_epoch
            && self.contender_owner == other.contender_owner
            && self.contender_identity.executable_path == other.contender_identity.executable_path
            && self.contender_identity.designated_requirement_hash
                == other.contender_identity.designated_requirement_hash
    }

    pub(crate) const fn snapshot(&self) -> MacosOwnerConflict {
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
pub struct MacosServerSessionId(pub(crate) String);

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
    pub protected_control_credential: ProtectedControlCredential,
}

impl MacosDaemonSessionAttestation {
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

pub(crate) fn format_hex_token(prefix: &str, bytes: &[u8]) -> String {
    const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut value = String::with_capacity(prefix.len() + bytes.len() * 2);
    value.push_str(prefix);
    for byte in bytes {
        value.push(char::from(HEX_DIGITS[usize::from(byte >> 4)]));
        value.push(char::from(HEX_DIGITS[usize::from(byte & 0x0f)]));
    }
    value
}
