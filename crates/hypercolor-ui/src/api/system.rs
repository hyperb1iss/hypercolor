//! System status API.

pub use hypercolor_types::api::system::{
    GpuCompositorProbeStatus, InputSourceIssueStatus, InputSourceStatus, InputStatus,
    MacosCapabilityOwner, MacosDaemonHandoverPhase, MacosDaemonOwnerConflictStatus,
    MacosDaemonOwnerRecoveryRequiredStatus, MacosDaemonOwnershipStatus, RenderAccelerationStatus,
    RenderLoopStatus, ServerInfo, SystemResource, SystemStatus,
};
use hypercolor_types::sensor::SystemSnapshot;
pub use hypercolor_types::service::{
    DaemonRunMode, ServiceConflict, ServiceIdentity, ServiceManager, ServiceRecoveryRequired,
    ServiceStatus,
};
pub use hypercolor_types::source_status::{
    SOURCE_DIAGNOSTICS_ENVELOPE_MAX_BYTES, SourceDiagnosticsDisplayField, SourceDiagnosticsEnvelope,
};

use super::{ApiError, ApiResult, client};

/// Adapts canonical platform ownership into the launcher vocabulary used by
/// native restart and owner-selection controls.
pub trait SystemStatusServiceExt {
    #[must_use]
    fn service_status(&self) -> Option<ServiceStatus>;
}

impl SystemStatusServiceExt for SystemStatus {
    /// Adapt the canonical macOS ownership resource into the neutral launcher
    /// vocabulary consumed by native restart and owner-selection controls.
    fn service_status(&self) -> Option<ServiceStatus> {
        let ownership = self.macos_daemon_ownership.as_ref()?;
        let identity = service_identity(ownership.active_owner)?;
        Some(ServiceStatus {
            identity,
            owner_epoch: ownership.owner_epoch,
            conflict: ownership.conflict.as_ref().and_then(|conflict| {
                Some(ServiceConflict {
                    active: service_identity(conflict.active)?,
                    contender: service_identity(conflict.contender)?,
                    observed_at_ms: conflict.observed_at_ms,
                })
            }),
            recovery_required: ownership.recovery_required.as_ref().and_then(|recovery| {
                Some(ServiceRecoveryRequired {
                    requested: service_identity(recovery.requested_owner)?,
                    prior: service_identity(recovery.prior_owner)?,
                    phase: macos_handover_phase(recovery.phase).to_owned(),
                })
            }),
        })
    }
}

fn service_identity(owner: MacosCapabilityOwner) -> Option<ServiceIdentity> {
    match owner {
        MacosCapabilityOwner::AppSidecar => Some(ServiceIdentity::APP_SIDECAR),
        MacosCapabilityOwner::LaunchdService => Some(ServiceIdentity::launchd_direct()),
        MacosCapabilityOwner::HomebrewService => Some(ServiceIdentity::homebrew()),
        MacosCapabilityOwner::Standalone => Some(ServiceIdentity::STANDALONE),
        MacosCapabilityOwner::App | MacosCapabilityOwner::Broker => None,
    }
}

const fn macos_handover_phase(phase: MacosDaemonHandoverPhase) -> &'static str {
    match phase {
        MacosDaemonHandoverPhase::Prepared => "prepared",
        MacosDaemonHandoverPhase::AutostartsConfigured => "autostarts_configured",
        MacosDaemonHandoverPhase::StopRequested => "stop_requested",
        MacosDaemonHandoverPhase::OutgoingOwnerStopped => "outgoing_owner_stopped",
        MacosDaemonHandoverPhase::AwaitingGuardRelease => "awaiting_guard_release",
        MacosDaemonHandoverPhase::GuardReleased => "guard_released",
        MacosDaemonHandoverPhase::StartRequested => "start_requested",
        MacosDaemonHandoverPhase::RequestedOwnerStarted => "requested_owner_started",
        MacosDaemonHandoverPhase::CommitPending => "commit_pending",
        MacosDaemonHandoverPhase::Committed => "committed",
        MacosDaemonHandoverPhase::RollbackPending => "rollback_pending",
        MacosDaemonHandoverPhase::RollbackAutostartsRestored => "rollback_autostarts_restored",
        MacosDaemonHandoverPhase::RollbackStopRequested => "rollback_stop_requested",
        MacosDaemonHandoverPhase::RollbackOwnerStopped => "rollback_owner_stopped",
        MacosDaemonHandoverPhase::RollbackAwaitingGuardRelease => "rollback_awaiting_guard_release",
        MacosDaemonHandoverPhase::RollbackGuardReleased => "rollback_guard_released",
        MacosDaemonHandoverPhase::RollbackStartRequested => "rollback_start_requested",
        MacosDaemonHandoverPhase::PriorOwnerStarted => "prior_owner_started",
        MacosDaemonHandoverPhase::RollbackCommitPending => "rollback_commit_pending",
        MacosDaemonHandoverPhase::RolledBack => "rolled_back",
    }
}

/// Fetch system status.
pub async fn fetch_status() -> ApiResult<SystemStatus> {
    let system: SystemResource = client::fetch_json("/api/v1/system").await?;
    system
        .status
        .ok_or_else(|| ApiError::Parse("System status requires daemon read access".to_owned()))
}

/// Fetch the latest system sensor snapshot.
pub async fn fetch_system_sensors() -> ApiResult<SystemSnapshot> {
    client::fetch_json("/api/v1/system/sensors").await
}
