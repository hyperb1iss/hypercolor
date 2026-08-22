//! Simulated-display API contracts — `/api/v1/simulators/*`.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::device::DeviceId;

/// Request body for `POST /api/v1/simulators/displays`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
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
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DeleteSimulatedDisplayResponse {
    pub id: DeviceId,
    pub deleted: bool,
}
