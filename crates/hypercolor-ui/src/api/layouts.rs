//! Layout-related API types and fetch functions.

use gloo_net::http::Request;
use serde::{Deserialize, Serialize};

use super::client;

// ── Types ───────────────────────────────────────────────────────────────────

/// Layout summary from `GET /api/v1/layouts`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LayoutSummary {
    pub id: String,
    pub name: String,
    pub canvas_width: u32,
    pub canvas_height: u32,
    pub zone_count: usize,
    #[serde(default)]
    pub is_active: bool,
}

/// Paginated layout list response.
#[derive(Debug, Deserialize)]
pub struct LayoutListResponse {
    pub items: Vec<LayoutSummary>,
}

/// Request body for creating a layout.
#[derive(Debug, Serialize)]
pub struct CreateLayoutRequest {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub canvas_width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub canvas_height: Option<u32>,
}

/// Request body for updating a layout.
#[derive(Debug, Serialize)]
pub struct UpdateLayoutApiRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub canvas_width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub canvas_height: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zones: Option<Vec<hypercolor_types::spatial::DeviceZone>>,
}

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
pub async fn update_layout(
    id: &str,
    req: &UpdateLayoutApiRequest,
) -> Result<LayoutSummary, String> {
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

/// Delete a layout. Uses raw request because the daemon returns a
/// structured error body with a user-facing message on failure.
pub async fn delete_layout(id: &str) -> Result<(), String> {
    let url = format!("/api/v1/layouts/{id}");
    let resp = Request::delete(&url)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        let msg = resp
            .json::<serde_json::Value>()
            .await
            .ok()
            .and_then(|v| v["error"]["message"].as_str().map(String::from))
            .unwrap_or_else(|| format!("HTTP {}", resp.status()));
        return Err(msg);
    }
    Ok(())
}
