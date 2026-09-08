//! Guided OpenRGB setup contracts shared by REST, MCP, and clients.

use serde::{Deserialize, Serialize};

use crate::api::devices::DeviceCoverageRow;
use crate::config::DriverConfigEntry;

/// Host and bridge state for guided setup.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct OpenRgbStatus {
    pub compiled: bool,
    pub enabled: bool,
    pub platform: String,
    pub binary_path: Option<String>,
    pub binary_version: Option<String>,
    pub bridge_config: DriverConfigEntry,
    pub probes: Vec<OpenRgbEndpointStatus>,
    pub install_hints: Vec<OpenRgbInstallHint>,
    pub permission_checks: Vec<OpenRgbPermissionStatus>,
    pub coverage: Vec<DeviceCoverageRow>,
    pub output_disabled_count: usize,
}

/// SDK handshake result for one configured endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct OpenRgbEndpointStatus {
    pub endpoint: String,
    pub reachable: bool,
    pub protocol_version: Option<u32>,
    pub controller_count: Option<u32>,
    pub error: Option<String>,
}

/// Installation command and platform guidance suitable for display.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct OpenRgbInstallHint {
    pub command: String,
    pub note: String,
}

/// A host prerequisite and its actionable remedy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct OpenRgbPermissionStatus {
    pub id: String,
    pub ok: bool,
    pub detail: String,
    pub remedy: Option<String>,
}
