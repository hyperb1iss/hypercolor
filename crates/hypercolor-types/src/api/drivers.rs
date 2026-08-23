//! Driver module API contracts for `/api/v1/drivers/*`.

use serde::{Deserialize, Serialize};

use crate::api::envelope::ListResponse;
use crate::config::DriverConfigEntry;
use crate::device::{DriverModuleDescriptor, DriverPresentation, DriverProtocolDescriptor};

/// Response for `GET /api/v1/drivers`.
pub type DriverListResponse = ListResponse<DriverSummary>;

/// One registered driver module.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct DriverSummary {
    pub descriptor: DriverModuleDescriptor,
    pub presentation: DriverPresentation,
    pub enabled: bool,
    pub config_key: String,
    #[serde(default)]
    pub protocols: Vec<DriverProtocolDescriptor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_surface_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_surface_path: Option<String>,
}

/// Response for `GET /api/v1/drivers/{id}/config`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct DriverConfigResponse {
    pub driver_id: String,
    pub config_key: String,
    pub configurable: bool,
    pub current: DriverConfigEntry,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<DriverConfigEntry>,
}
