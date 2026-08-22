//! Layout-related API types and fetch functions.

use super::{ApiResult, client};

pub use hypercolor_types::api::layouts::{
    CreateLayoutRequest, LayoutListResponse, LayoutSummary, UpdateLayoutRequest,
};

// ── Fetch Functions ─────────────────────────────────────────────────────────

/// Fetch all spatial layouts.
pub async fn fetch_layouts() -> ApiResult<Vec<LayoutSummary>> {
    let list: LayoutListResponse = client::fetch_json("/api/v1/layouts").await?;
    Ok(list.items)
}

/// Fetch a single layout with full zone data.
pub async fn fetch_layout(id: &str) -> ApiResult<hypercolor_types::spatial::SpatialLayout> {
    client::fetch_json(&format!("/api/v1/layouts/{id}")).await
}

/// Fetch the currently active layout.
pub async fn fetch_active_layout() -> ApiResult<hypercolor_types::spatial::SpatialLayout> {
    client::fetch_json("/api/v1/layouts/active").await
}

/// Create a new layout.
pub async fn create_layout(req: &CreateLayoutRequest) -> ApiResult<LayoutSummary> {
    client::post_json("/api/v1/layouts", req).await
}

/// Update a layout (metadata + optionally zones).
pub async fn update_layout(id: &str, req: &UpdateLayoutRequest) -> ApiResult<LayoutSummary> {
    client::put_json(&format!("/api/v1/layouts/{id}"), req).await
}

/// Apply a layout to the spatial engine.
pub async fn apply_layout(id: &str) -> ApiResult<()> {
    client::post_empty(&format!("/api/v1/layouts/{id}/apply")).await
}

/// Push a layout to the spatial engine for live preview (no persistence).
pub async fn preview_layout(layout: &hypercolor_types::spatial::SpatialLayout) -> ApiResult<()> {
    client::put_json_discard("/api/v1/layouts/active/preview", layout).await
}

/// Delete a layout.
pub async fn delete_layout(id: &str) -> ApiResult<()> {
    client::delete_empty(&format!("/api/v1/layouts/{id}")).await
}
