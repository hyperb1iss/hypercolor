//! Portable archive transfer and explicit restore confirmation.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct BackupPreview {
    pub operation_id: Uuid,
    pub name: String,
    pub scenes: u64,
    pub layouts: u64,
    pub effects: u64,
    pub media: u64,
    pub favorites: u64,
    pub includes_preferences: bool,
    pub unresolved_devices: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct BackupApplyRequest {
    pub operation_id: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct BackupApplyResponse {
    pub operation_id: Uuid,
    pub unresolved_devices: Vec<String>,
}
