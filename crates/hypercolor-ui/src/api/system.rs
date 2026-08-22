//! System status API.

pub use hypercolor_types::api::system::{
    GpuCompositorProbeStatus, InputSourceIssueStatus, InputSourcePlatformStatus, InputSourceStatus,
    InputStatus, MacosArchitecture, MacosAuthorizationState, MacosCapabilityOwner,
    MacosDaemonHandoverPhase, MacosDaemonOwnerConflictStatus,
    MacosDaemonOwnerRecoveryRequiredStatus, MacosDaemonOwnershipStatus, MacosInputTelemetry,
    MacosProtectedSourceState, MacosScreenTelemetry, MacosScreenTiming, MacosSelectionState,
    MacosTahoeCapabilities, MacosTahoeSelectionCapabilities, MacosTiming, RenderAccelerationStatus,
    RenderLoopStatus, ServerInfo, SystemResource, SystemStatus,
};
use hypercolor_types::sensor::SystemSnapshot;

use super::{ApiError, ApiResult, client};

// ── Fetch Functions ─────────────────────────────────────────────────────────

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
