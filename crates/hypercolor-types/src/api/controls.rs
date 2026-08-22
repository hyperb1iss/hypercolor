//! Control-surface API contracts — `/api/v1/control-surfaces/*`.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::controls::{ControlSurfaceDocument, ControlValueMap};

/// Response body for `GET /api/v1/control-surfaces`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ControlSurfaceListResponse {
    pub surfaces: Vec<ControlSurfaceDocument>,
}

/// Query parameters for `GET /api/v1/control-surfaces`.
///
/// At least one selector must be present; a query that names neither a
/// device nor a driver is rejected.
#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema, utoipa::IntoParams,
)]
pub struct ControlSurfaceListQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub driver_id: Option<String>,
    /// Also include the device's owning driver surface.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_driver: Option<bool>,
}

/// Request body for
/// `POST /api/v1/control-surfaces/{id}/actions/{action}`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct InvokeControlActionRequest {
    #[serde(default)]
    pub input: ControlValueMap,
}
