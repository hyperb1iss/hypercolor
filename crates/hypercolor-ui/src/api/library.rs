//! Library API — presets and favorites.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::client;

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
    let list: PresetListResponse = client::fetch_json("/api/v1/library/presets").await?;
    Ok(list.items)
}

/// Create a new preset from current control values.
pub async fn create_preset(req: &CreatePresetRequest) -> Result<PresetSummary, String> {
    client::post_json("/api/v1/library/presets", req)
        .await
        .map_err(Into::into)
}

/// Apply a saved preset by ID.
pub async fn apply_preset(id: &str) -> Result<(), String> {
    client::post_empty(&format!("/api/v1/library/presets/{id}/apply"))
        .await
        .map_err(Into::into)
}

/// Update an existing preset (name, controls, etc.).
pub async fn update_preset(id: &str, req: &CreatePresetRequest) -> Result<PresetSummary, String> {
    client::put_json(&format!("/api/v1/library/presets/{id}"), req)
        .await
        .map_err(Into::into)
}

/// Delete a preset by ID.
pub async fn delete_preset(id: &str) -> Result<(), String> {
    client::delete_empty(&format!("/api/v1/library/presets/{id}"))
        .await
        .map_err(Into::into)
}

// ── Favorite Functions ──────────────────────────────────────────────────────

/// Fetch all favorited effect IDs.
pub async fn fetch_favorites() -> Result<Vec<FavoriteSummary>, String> {
    let list: FavoriteListResponse = client::fetch_json("/api/v1/library/favorites").await?;
    Ok(list.items)
}

/// Add an effect to favorites.
pub async fn add_favorite(effect_id: &str) -> Result<(), String> {
    client::post_json_discard(
        "/api/v1/library/favorites",
        &serde_json::json!({ "effect": effect_id }),
    )
    .await
    .map_err(Into::into)
}

/// Remove an effect from favorites.
pub async fn remove_favorite(effect_id: &str) -> Result<(), String> {
    client::delete_empty(&format!("/api/v1/library/favorites/{effect_id}"))
        .await
        .map_err(Into::into)
}
