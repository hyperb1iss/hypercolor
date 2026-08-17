//! Global output power API contracts.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Desired global output power state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum OutputPowerMode {
    /// Render and deliver live output.
    Running,
    /// Preserve live state while holding outputs at their off frame.
    Paused,
}

/// Observed global output power state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum OutputPowerStatus {
    /// Render and deliver live output.
    Running,
    /// Preserve live state while holding outputs at their off frame.
    Paused,
    /// The active effect was destructively stopped.
    Stopped,
}

/// Request for `PUT /api/v1/output/power`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SetOutputPowerRequest {
    pub state: OutputPowerMode,
}

/// Current global output power state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct OutputPowerResponse {
    pub state: OutputPowerStatus,
}

/// The one output resource — `GET /api/v1/output` (Spec 78 §4).
///
/// Power and brightness in one home. The routes this merges
/// (`/output/power`, `/settings/brightness`, pause/resume) and the
/// power types above die with wave 78.2.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct OutputResource {
    pub power: OutputPowerMode,
    /// Global brightness, `0.0..=1.0`.
    pub brightness: f32,
}

/// `PATCH /api/v1/output` — partial: either or both fields.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct OutputPatchRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power: Option<OutputPowerMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brightness: Option<f32>,
}
