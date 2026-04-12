//! Effect-related API types and fetch functions.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use hypercolor_types::effect::{ControlDefinition, ControlValue, PresetTemplate};

use super::client;

// ── Types ───────────────────────────────────────────────────────────────────

/// Effect list item from `GET /api/v1/effects`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EffectSummary {
    pub id: String,
    pub name: String,
    pub description: String,
    pub author: String,
    pub category: String,
    pub source: String,
    pub runnable: bool,
    pub tags: Vec<String>,
    pub version: String,
    #[serde(default)]
    pub audio_reactive: bool,
}

/// Paginated effect list response.
#[derive(Debug, Deserialize)]
pub struct EffectListResponse {
    pub items: Vec<EffectSummary>,
}

/// Active effect response from `GET /api/v1/effects/active`.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ActiveEffectResponse {
    pub id: String,
    pub name: String,
    pub state: String,
    #[serde(default)]
    pub controls: Vec<ControlDefinition>,
    #[serde(default)]
    pub control_values: HashMap<String, ControlValue>,
    #[serde(default)]
    pub active_preset_id: Option<String>,
}

/// Detailed effect payload from `GET /api/v1/effects/:id`.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct EffectDetailResponse {
    pub id: String,
    pub name: String,
    pub description: String,
    pub author: String,
    pub category: String,
    pub source: String,
    pub runnable: bool,
    pub tags: Vec<String>,
    pub version: String,
    pub audio_reactive: bool,
    #[serde(default)]
    pub controls: Vec<ControlDefinition>,
    #[serde(default)]
    pub presets: Vec<PresetTemplate>,
    #[serde(default)]
    pub active_control_values: Option<HashMap<String, ControlValue>>,
}

// ── Fetch Functions ─────────────────────────────────────────────────────────

/// Fetch all registered effects.
pub async fn fetch_effects() -> Result<Vec<EffectSummary>, String> {
    let list: EffectListResponse = client::fetch_json("/api/v1/effects").await?;
    Ok(list.items)
}

/// Fetch effects filtered to a single category.
pub async fn fetch_effects_by_category(category: &str) -> Result<Vec<EffectSummary>, String> {
    let url = format!("/api/v1/effects?category={category}");
    let list: EffectListResponse = client::fetch_json(&url).await?;
    Ok(list.items)
}

/// Fetch the currently active effect, if any.
pub async fn fetch_active_effect() -> Result<Option<ActiveEffectResponse>, String> {
    client::fetch_json_optional("/api/v1/effects/active")
        .await
        .map_err(Into::into)
}

/// Fetch detailed metadata for one effect.
pub async fn fetch_effect_detail(id: &str) -> Result<EffectDetailResponse, String> {
    client::fetch_json(&format!("/api/v1/effects/{id}"))
        .await
        .map_err(Into::into)
}

/// Fetch the bundled (effect-defined) presets for an effect.
pub async fn fetch_bundled_presets(id: &str) -> Result<Vec<PresetTemplate>, String> {
    let detail = fetch_effect_detail(id).await?;
    Ok(detail.presets)
}

/// Apply an effect by ID or name.
pub async fn apply_effect(id: &str) -> Result<(), String> {
    client::post_empty(&format!("/api/v1/effects/{id}/apply"))
        .await
        .map_err(Into::into)
}

/// Stop the currently active effect.
pub async fn stop_effect() -> Result<(), String> {
    client::post_empty("/api/v1/effects/stop")
        .await
        .map_err(Into::into)
}

/// Update effect control parameters.
pub async fn update_controls(controls: &serde_json::Value) -> Result<(), String> {
    let body = serde_json::json!({ "controls": controls });
    client::patch_json_discard("/api/v1/effects/current/controls", &body)
        .await
        .map_err(Into::into)
}

/// Reset all controls on the active effect to their defaults.
pub async fn reset_controls() -> Result<(), String> {
    client::post_empty("/api/v1/effects/current/reset")
        .await
        .map_err(Into::into)
}
