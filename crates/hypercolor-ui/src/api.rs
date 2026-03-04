//! REST API client — thin wrappers around the daemon's HTTP endpoints.

use gloo_net::http::Request;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use hypercolor_types::effect::{ControlDefinition, ControlValue};

// ── API Response Types ──────────────────────────────────────────────────────

/// Mirrors the daemon's envelope: `{ "data": T, "meta": { ... } }`.
#[derive(Debug, Deserialize)]
pub struct ApiEnvelope<T> {
    pub data: T,
}

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
    pub active_control_values: Option<HashMap<String, ControlValue>>,
}

/// System status from `GET /api/v1/status`.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct SystemStatus {
    pub running: bool,
    pub version: String,
    pub uptime_seconds: u64,
    pub device_count: usize,
    pub effect_count: usize,
    pub active_effect: Option<String>,
}

// ── Device Types ────────────────────────────────────────────────────────────

/// Device zone summary.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ZoneSummary {
    pub id: String,
    pub name: String,
    pub led_count: usize,
    pub topology: String,
}

/// Device summary from `GET /api/v1/devices`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeviceSummary {
    pub id: String,
    pub name: String,
    pub backend: String,
    pub status: String,
    #[serde(default)]
    pub firmware_version: Option<String>,
    pub total_leds: usize,
    #[serde(default)]
    pub zones: Vec<ZoneSummary>,
}

/// Paginated device list response.
#[derive(Debug, Deserialize)]
pub struct DeviceListResponse {
    pub items: Vec<DeviceSummary>,
}

// ── Layout Types ────────────────────────────────────────────────────────────

/// Layout summary from `GET /api/v1/layouts`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LayoutSummary {
    pub id: String,
    pub name: String,
    pub canvas_width: u32,
    pub canvas_height: u32,
    pub zone_count: usize,
}

/// Paginated layout list response.
#[derive(Debug, Deserialize)]
pub struct LayoutListResponse {
    pub items: Vec<LayoutSummary>,
}

// ── Fetch Functions ─────────────────────────────────────────────────────────

/// Fetch all registered effects.
pub async fn fetch_effects() -> Result<Vec<EffectSummary>, String> {
    let resp = Request::get("/api/v1/effects")
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }

    let envelope: ApiEnvelope<EffectListResponse> =
        resp.json().await.map_err(|e| format!("Parse error: {e}"))?;

    Ok(envelope.data.items)
}

/// Fetch the currently active effect, if any.
pub async fn fetch_active_effect() -> Result<Option<ActiveEffectResponse>, String> {
    let resp = Request::get("/api/v1/effects/active")
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() == 404 {
        return Ok(None);
    }
    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }

    let envelope: ApiEnvelope<ActiveEffectResponse> =
        resp.json().await.map_err(|e| format!("Parse error: {e}"))?;

    Ok(Some(envelope.data))
}

/// Fetch detailed metadata for one effect.
pub async fn fetch_effect_detail(id: &str) -> Result<EffectDetailResponse, String> {
    let url = format!("/api/v1/effects/{id}");
    let resp = Request::get(&url)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }

    let envelope: ApiEnvelope<EffectDetailResponse> =
        resp.json().await.map_err(|e| format!("Parse error: {e}"))?;

    Ok(envelope.data)
}

/// Apply an effect by ID or name.
pub async fn apply_effect(id: &str) -> Result<(), String> {
    let url = format!("/api/v1/effects/{id}/apply");
    let resp = Request::post(&url)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }
    Ok(())
}

/// Stop the currently active effect.
pub async fn stop_effect() -> Result<(), String> {
    let resp = Request::post("/api/v1/effects/stop")
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }
    Ok(())
}

/// Fetch system status.
pub async fn fetch_status() -> Result<SystemStatus, String> {
    let resp = Request::get("/api/v1/status")
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }

    let envelope: ApiEnvelope<SystemStatus> =
        resp.json().await.map_err(|e| format!("Parse error: {e}"))?;

    Ok(envelope.data)
}

// ── Device Fetch Functions ──────────────────────────────────────────────────

/// Fetch all tracked devices.
pub async fn fetch_devices() -> Result<Vec<DeviceSummary>, String> {
    let resp = Request::get("/api/v1/devices")
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }

    let envelope: ApiEnvelope<DeviceListResponse> =
        resp.json().await.map_err(|e| format!("Parse error: {e}"))?;

    Ok(envelope.data.items)
}

/// Trigger device discovery scan.
pub async fn discover_devices() -> Result<(), String> {
    let resp = Request::post("/api/v1/devices/discover")
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 && resp.status() != 202 {
        return Err(format!("HTTP {}", resp.status()));
    }
    Ok(())
}

// ── Layout Fetch Functions ─────────────────────────────────────────────────

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

// ── Preset Types ────────────────────────────────────────────────────────

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

// ── Preset Fetch Functions ──────────────────────────────────────────────

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

/// Reset all controls on the active effect to their defaults.
pub async fn reset_controls() -> Result<(), String> {
    let resp = Request::post("/api/v1/effects/current/reset")
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }
    Ok(())
}

/// Update effect control parameters.
pub async fn update_controls(controls: &serde_json::Value) -> Result<(), String> {
    let url = "/api/v1/effects/current/controls";
    let body = serde_json::json!({ "controls": controls });

    let resp = Request::patch(url)
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
