//! Control-surface API contracts — `/api/v1/control-surfaces/*`.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::controls::ControlValueMap;

/// Query parameters for `GET /api/v1/control-surfaces`.
///
/// At least one selector must be present; a query that names neither a
/// device nor a driver is rejected.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
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
/// `POST /api/v1/control-surfaces/{surface_id}/actions/{action_id}`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct InvokeControlActionRequest {
    #[serde(default)]
    pub input: ControlValueMap,
}
