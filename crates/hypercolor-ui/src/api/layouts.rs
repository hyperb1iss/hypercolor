//! Layout-related API types and fetch functions.

use super::{ApiResult, client};

pub use hypercolor_types::api::layouts::{LayoutListResponse, LayoutSummary, UpdateLayoutRequest};

// ── Fetch Functions ─────────────────────────────────────────────────────────

/// Fetch all spatial layouts.
pub async fn fetch_layouts() -> ApiResult<Vec<LayoutSummary>> {
    let list: LayoutListResponse = client::fetch_json("/api/v1/layouts").await?;
    Ok(list.items)
}

/// Fetch the currently active layout.
pub async fn fetch_active_layout() -> ApiResult<hypercolor_types::spatial::SpatialLayout> {
    client::fetch_json("/api/v1/layouts/active").await
}

/// Update a layout (metadata + optionally zones).
pub async fn update_layout(id: &str, req: &UpdateLayoutRequest) -> ApiResult<LayoutSummary> {
    client::put_json(&format!("/api/v1/layouts/{id}"), req).await
}

/// Apply a layout to the spatial engine.
pub async fn apply_layout(id: &str) -> ApiResult<()> {
    client::post_empty(&format!("/api/v1/layouts/{id}/apply")).await
}
