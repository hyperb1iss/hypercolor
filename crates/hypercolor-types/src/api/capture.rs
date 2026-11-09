//! Protected input and screen-capture REST contracts.

use serde::{Deserialize, Serialize};

use crate::api::envelope::ListResponse;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ProtectedSourceGrantOwner {
    AppSidecar,
    App,
    LaunchdService,
    HomebrewService,
    Broker,
    Standalone,
    PlatformBackend,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct CaptureAuthorizationResponse {
    pub authorized: bool,
    pub grant_owner: ProtectedSourceGrantOwner,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct CapturePickerResponse {
    pub picking: bool,
    pub grant_owner: ProtectedSourceGrantOwner,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct CaptureMonitor {
    pub index: usize,
    pub id: String,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub primary: bool,
    pub value: String,
}

/// Response body for `GET /api/v1/capture/monitors`.
pub type CaptureMonitorListResponse = ListResponse<CaptureMonitor>;
