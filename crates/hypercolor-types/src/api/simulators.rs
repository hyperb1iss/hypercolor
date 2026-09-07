//! Simulated-display API contracts — `/api/v1/simulators/*`.

use serde::{Deserialize, Serialize};

use crate::device::DeviceId;

/// One simulated display as `/api/v1/simulators/displays` renders it.
///
/// This is both the stored configuration and the resource every route
/// in the family returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct SimulatedDisplay {
    pub id: DeviceId,
    pub name: String,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub circular: bool,
    #[serde(default = "default_simulated_display_enabled")]
    pub enabled: bool,
}

const fn default_simulated_display_enabled() -> bool {
    true
}

/// Request body for `POST /api/v1/simulators/displays`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct CreateSimulatedDisplayRequest {
    pub name: String,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub circular: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

/// Request body for `PATCH /api/v1/simulators/displays/{id}`.
///
/// Omitted fields leave the simulated display untouched.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct UpdateSimulatedDisplayRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub circular: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

/// Response from `DELETE /api/v1/simulators/displays/{id}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct DeleteSimulatedDisplayResponse {
    pub id: DeviceId,
    pub deleted: bool,
}
