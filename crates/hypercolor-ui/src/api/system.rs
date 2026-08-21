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

use super::client;

// ── Fetch Functions ─────────────────────────────────────────────────────────

/// Fetch system status.
pub async fn fetch_status() -> Result<SystemStatus, String> {
    let system: SystemResource = client::fetch_json("/api/v1/system")
        .await
        .map_err(String::from)?;
    system
        .status
        .ok_or_else(|| "System status requires daemon read access".to_owned())
}

/// Fetch the latest system sensor snapshot.
pub async fn fetch_system_sensors() -> Result<SystemSnapshot, String> {
    client::fetch_json("/api/v1/system/sensors")
        .await
        .map_err(Into::into)
}
