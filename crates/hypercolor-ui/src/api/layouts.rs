//! Layout-related API types and fetch functions.

use gloo_net::http::Request;
use serde::{Deserialize, Serialize};

use super::ApiEnvelope;

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
    pub group_count: usize,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub groups: Option<Vec<hypercolor_types::spatial::ZoneGroup>>,
}

// ── Fetch Functions ─────────────────────────────────────────────────────────

/// Fetch all spatial layouts.
pub async fn fetch_layouts() -> Result<Vec<LayoutSummary>, String> {
    let resp = Request::get("/api/v1/layouts")
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }

    let envelope: ApiEnvelope<LayoutListResponse> =
        resp.json().await.map_err(|e| format!("Parse error: {e}"))?;

    Ok(envelope.data.items)
}

/// Fetch a single layout with full zone data.
pub async fn fetch_layout(id: &str) -> Result<hypercolor_types::spatial::SpatialLayout, String> {
    let url = format!("/api/v1/layouts/{id}");
    let resp = Request::get(&url)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }

    let envelope: ApiEnvelope<hypercolor_types::spatial::SpatialLayout> =
        resp.json().await.map_err(|e| format!("Parse error: {e}"))?;

    Ok(envelope.data)
}

/// Fetch the currently active layout.
pub async fn fetch_active_layout() -> Result<hypercolor_types::spatial::SpatialLayout, String> {
    let resp = Request::get("/api/v1/layouts/active")
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }

    let envelope: ApiEnvelope<hypercolor_types::spatial::SpatialLayout> =
        resp.json().await.map_err(|e| format!("Parse error: {e}"))?;

    Ok(envelope.data)
}

/// Create a new layout.
pub async fn create_layout(req: &CreateLayoutRequest) -> Result<LayoutSummary, String> {
    let body = serde_json::to_string(req).map_err(|e| format!("Serialize error: {e}"))?;

    let resp = Request::post("/api/v1/layouts")
        .header("Content-Type", "application/json")
        .body(body)
        .map_err(|e| format!("Request error: {e}"))?
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 && resp.status() != 201 {
        return Err(format!("HTTP {}", resp.status()));
    }

    let envelope: ApiEnvelope<LayoutSummary> =
        resp.json().await.map_err(|e| format!("Parse error: {e}"))?;

    Ok(envelope.data)
}

/// Update a layout (metadata + optionally zones).
pub async fn update_layout(
    id: &str,
    req: &UpdateLayoutApiRequest,
) -> Result<LayoutSummary, String> {
    let url = format!("/api/v1/layouts/{id}");
    let body = serde_json::to_string(req).map_err(|e| format!("Serialize error: {e}"))?;

    let resp = Request::put(&url)
        .header("Content-Type", "application/json")
        .body(body)
        .map_err(|e| format!("Request error: {e}"))?
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }

    let envelope: ApiEnvelope<LayoutSummary> =
        resp.json().await.map_err(|e| format!("Parse error: {e}"))?;

    Ok(envelope.data)
}

/// Apply a layout to the spatial engine.
pub async fn apply_layout(id: &str) -> Result<(), String> {
    let url = format!("/api/v1/layouts/{id}/apply");
    let resp = Request::post(&url)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }
    Ok(())
}

/// Push a layout to the spatial engine for live preview (no persistence).
pub async fn preview_layout(
    layout: &hypercolor_types::spatial::SpatialLayout,
) -> Result<(), String> {
    let body = serde_json::to_string(layout).map_err(|e| format!("Serialize error: {e}"))?;

    let resp = Request::put("/api/v1/layouts/active/preview")
        .header("Content-Type", "application/json")
        .body(body)
        .map_err(|e| format!("Request error: {e}"))?
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }
    Ok(())
}

/// Delete a layout.
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
