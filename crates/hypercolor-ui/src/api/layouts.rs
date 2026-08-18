//! Layout-related API types and fetch functions.

use super::client;

pub use hypercolor_types::api::layouts::{
    CreateLayoutRequest, LayoutListResponse, LayoutSummary, UpdateLayoutRequest,
};

// ── Fetch Functions ─────────────────────────────────────────────────────────

/// Fetch all spatial layouts.
pub async fn fetch_layouts() -> Result<Vec<LayoutSummary>, String> {
    let list: LayoutListResponse = client::fetch_json("/api/v1/layouts").await?;
    Ok(list.items)
}

/// Fetch a single layout with full zone data.
pub async fn fetch_layout(id: &str) -> Result<hypercolor_types::spatial::SpatialLayout, String> {
    client::fetch_json(&format!("/api/v1/layouts/{id}"))
        .await
        .map_err(Into::into)
}

/// Fetch the currently active layout.
pub async fn fetch_active_layout() -> Result<hypercolor_types::spatial::SpatialLayout, String> {
    client::fetch_json("/api/v1/layouts/active")
        .await
        .map_err(Into::into)
}

/// Create a new layout.
pub async fn create_layout(req: &CreateLayoutRequest) -> Result<LayoutSummary, String> {
    client::post_json("/api/v1/layouts", req)
        .await
        .map_err(Into::into)
}

/// Update a layout (metadata + optionally zones).
pub async fn update_layout(id: &str, req: &UpdateLayoutRequest) -> Result<LayoutSummary, String> {
    client::put_json(&format!("/api/v1/layouts/{id}"), req)
        .await
        .map_err(Into::into)
}

/// Apply a layout to the spatial engine.
pub async fn apply_layout(id: &str) -> Result<(), String> {
    client::post_empty(&format!("/api/v1/layouts/{id}/apply"))
        .await
        .map_err(Into::into)
}

/// Push a layout to the spatial engine for live preview (no persistence).
pub async fn preview_layout(
    layout: &hypercolor_types::spatial::SpatialLayout,
) -> Result<(), String> {
    client::put_json_discard("/api/v1/layouts/active/preview", layout)
        .await
        .map_err(Into::into)
}

/// Delete a layout.
pub async fn delete_layout(id: &str) -> Result<(), String> {
    client::delete_empty(&format!("/api/v1/layouts/{id}"))
        .await
        .map_err(Into::into)
}
