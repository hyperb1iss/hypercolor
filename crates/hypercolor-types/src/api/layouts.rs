//! Spatial layout API contracts — `/api/v1/layouts/*`.

use serde::{Deserialize, Serialize};

use crate::api::envelope::ListResponse;
use crate::spatial::{Output, SpatialLayout};

/// Summary row from `GET /api/v1/layouts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct LayoutSummary {
    pub id: String,
    pub name: String,
    pub canvas_width: u32,
    pub canvas_height: u32,
    pub zone_count: usize,
    #[serde(default)]
    pub is_active: bool,
}

/// Paginated response from `GET /api/v1/layouts`.
pub type LayoutListResponse = ListResponse<LayoutSummary>;

/// Query parameters for `GET /api/v1/layouts`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema, utoipa::IntoParams))]
pub struct LayoutListQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    /// Restrict the list to the layout the daemon is rendering with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<bool>,
}

/// Request body for `POST /api/v1/layouts`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct CreateLayoutRequest {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canvas_width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canvas_height: Option<u32>,
}

/// Request body for `PUT /api/v1/layouts/{id}`.
///
/// Omitted fields leave the stored layout untouched; a present `zones`
/// list replaces the layout's outputs wholesale.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct UpdateLayoutRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canvas_width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canvas_height: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zones: Option<Vec<Output>>,
}

/// Response from `POST /api/v1/layouts/{id}/apply`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct ApplyLayoutResponse {
    pub layout: SpatialLayout,
    pub applied: bool,
    pub persistence_pending: bool,
}

/// Response from `PUT /api/v1/layouts/active/preview`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct PreviewLayoutResponse {
    pub previewing: bool,
}

/// Response from `DELETE /api/v1/layouts/{id}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct DeleteLayoutResponse {
    pub id: String,
    pub deleted: bool,
    pub persistence_pending: bool,
}
