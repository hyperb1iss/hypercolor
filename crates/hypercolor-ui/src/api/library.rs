//! Library API — presets and favorites.

use gloo_net::http::Request;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::ApiEnvelope;

// ── Preset Types ────────────────────────────────────────────────────────────

/// Preset summary from `GET /api/v1/library/presets`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PresetSummary {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub effect_id: String,
    #[serde(default)]
    pub controls: HashMap<String, serde_json::Value>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub created_at_ms: u64,
    #[serde(default)]
    pub updated_at_ms: u64,
}

/// Paginated preset list response.
#[derive(Debug, Deserialize)]
pub struct PresetListResponse {
    pub items: Vec<PresetSummary>,
}

/// Request body for creating a preset.
#[derive(Debug, Serialize)]
pub struct CreatePresetRequest {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub effect: String,
    pub controls: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
}

// ── Favorite Types ──────────────────────────────────────────────────────────

/// Favorite entry from `GET /api/v1/library/favorites`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FavoriteSummary {
    pub effect_id: String,
    pub effect_name: String,
    pub added_at_ms: u64,
}

/// Paginated favorites list response.
#[derive(Debug, Deserialize)]
pub struct FavoriteListResponse {
    pub items: Vec<FavoriteSummary>,
}

// ── Preset Functions ────────────────────────────────────────────────────────

/// Fetch all saved presets.
pub async fn fetch_presets() -> Result<Vec<PresetSummary>, String> {
    let resp = Request::get("/api/v1/library/presets")
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }

    let envelope: ApiEnvelope<PresetListResponse> =
        resp.json().await.map_err(|e| format!("Parse error: {e}"))?;

    Ok(envelope.data.items)
}

/// Create a new preset from current control values.
pub async fn create_preset(req: &CreatePresetRequest) -> Result<PresetSummary, String> {
    let body = serde_json::to_string(req).map_err(|e| format!("Serialize error: {e}"))?;

    let resp = Request::post("/api/v1/library/presets")
        .header("Content-Type", "application/json")
        .body(body)
        .map_err(|e| format!("Request error: {e}"))?
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 201 && resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }

    let envelope: ApiEnvelope<PresetSummary> =
        resp.json().await.map_err(|e| format!("Parse error: {e}"))?;

    Ok(envelope.data)
}

/// Apply a saved preset by ID.
pub async fn apply_preset(id: &str) -> Result<(), String> {
    let url = format!("/api/v1/library/presets/{id}/apply");
    let resp = Request::post(&url)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }
    Ok(())
}

/// Update an existing preset (name, controls, etc.).
pub async fn update_preset(id: &str, req: &CreatePresetRequest) -> Result<PresetSummary, String> {
    let url = format!("/api/v1/library/presets/{id}");
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

    let envelope: ApiEnvelope<PresetSummary> =
        resp.json().await.map_err(|e| format!("Parse error: {e}"))?;

    Ok(envelope.data)
}

/// Delete a preset by ID.
pub async fn delete_preset(id: &str) -> Result<(), String> {
    let url = format!("/api/v1/library/presets/{id}");
    let resp = Request::delete(&url)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }
    Ok(())
}

// ── Favorite Functions ──────────────────────────────────────────────────────

/// Fetch all favorited effect IDs.
pub async fn fetch_favorites() -> Result<Vec<FavoriteSummary>, String> {
    let resp = Request::get("/api/v1/library/favorites")
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }

    let envelope: ApiEnvelope<FavoriteListResponse> =
        resp.json().await.map_err(|e| format!("Parse error: {e}"))?;

    Ok(envelope.data.items)
}

/// Add an effect to favorites.
pub async fn add_favorite(effect_id: &str) -> Result<(), String> {
    let body = serde_json::json!({ "effect": effect_id });

    let resp = Request::post("/api/v1/library/favorites")
        .header("Content-Type", "application/json")
        .body(serde_json::to_string(&body).map_err(|e| format!("Serialize error: {e}"))?)
        .map_err(|e| format!("Request error: {e}"))?
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }
    Ok(())
}

/// Remove an effect from favorites.
pub async fn remove_favorite(effect_id: &str) -> Result<(), String> {
    let url = format!("/api/v1/library/favorites/{effect_id}");
    let resp = Request::delete(&url)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }
    Ok(())
}
