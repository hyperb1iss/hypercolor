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
