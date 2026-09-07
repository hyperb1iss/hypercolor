use serde::{Deserialize, Serialize};

use crate::coordinator_error::MacosOwnerCoordinatorError;
use crate::error::MacosOwnerStoreError;
use crate::model::{
    MACOS_HANDOVER_JOURNAL_SCHEMA_VERSION, MAX_MACOS_HANDOVER_OPERATIONS, MacosAutostartStates,
    MacosDaemonOwner, MacosExternalOwnerMode, MacosHandoverOperation, MacosHandoverPhase,
};
use crate::validation::validate_version;

/// Stable, path-free identifier for one handover transaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct MacosHandoverTransactionId(pub(crate) String);

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
}

pub(crate) const fn external_owner_mode(owner: MacosDaemonOwner) -> Option<MacosExternalOwnerMode> {
    match owner {
        MacosDaemonOwner::DirectLaunchd => Some(MacosExternalOwnerMode::DirectLaunchd),
        MacosDaemonOwner::Homebrew => Some(MacosExternalOwnerMode::Homebrew),
        MacosDaemonOwner::AppSidecar | MacosDaemonOwner::Standalone => None,
    }
}

pub(crate) fn forward_operations(
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

pub(crate) fn rollback_operations(
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

pub(crate) const fn autostart_operations_for(
    owner: MacosDaemonOwner,
) -> [MacosHandoverOperation; 3] {
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

pub(crate) const fn autostart_operations_from(
    states: MacosAutostartStates,
) -> [MacosHandoverOperation; 3] {
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

pub(crate) const fn flush_stop_operation(
    owner: MacosDaemonOwner,
) -> Result<MacosHandoverOperation, MacosOwnerCoordinatorError> {
    match owner {
        MacosDaemonOwner::AppSidecar => Ok(MacosHandoverOperation::FlushAndStopAppSidecar {}),
        MacosDaemonOwner::DirectLaunchd => Ok(MacosHandoverOperation::FlushAndStopDirectLaunchd {}),
        MacosDaemonOwner::Homebrew => Ok(MacosHandoverOperation::FlushAndStopHomebrew {}),
        MacosDaemonOwner::Standalone => Err(MacosOwnerCoordinatorError::StandaloneCannotBeSelected),
    }
}

pub(crate) const fn start_operation(
    owner: MacosDaemonOwner,
) -> Result<MacosHandoverOperation, MacosOwnerCoordinatorError> {
    match owner {
        MacosDaemonOwner::AppSidecar => Ok(MacosHandoverOperation::StartAppSidecar {}),
        MacosDaemonOwner::DirectLaunchd => Ok(MacosHandoverOperation::StartDirectLaunchd {}),
        MacosDaemonOwner::Homebrew => Ok(MacosHandoverOperation::StartHomebrew {}),
        MacosDaemonOwner::Standalone => Err(MacosOwnerCoordinatorError::StandaloneCannotBeSelected),
    }
}

pub(crate) fn validate_handover_journal(
    journal: &MacosHandoverJournal,
) -> Result<(), MacosOwnerStoreError> {
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

fn is_valid_transaction_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}
