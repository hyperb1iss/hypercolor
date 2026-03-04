//! REST API client — thin wrappers around the daemon's HTTP endpoints.

use gloo_net::http::Request;
use serde::{Deserialize, Serialize};

// ── API Response Types ──────────────────────────────────────────────────────

/// Mirrors the daemon's envelope: `{ "status": "ok", "data": T }`.
#[derive(Debug, Deserialize)]
pub struct ApiEnvelope<T> {
    #[allow(dead_code)]
    pub status: String,
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

    let envelope: ApiEnvelope<EffectListResponse> = resp
        .json()
        .await
        .map_err(|e| format!("Parse error: {e}"))?;

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

    let envelope: ApiEnvelope<ActiveEffectResponse> = resp
        .json()
        .await
        .map_err(|e| format!("Parse error: {e}"))?;

    Ok(Some(envelope.data))
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

    let envelope: ApiEnvelope<SystemStatus> = resp
        .json()
        .await
        .map_err(|e| format!("Parse error: {e}"))?;

    Ok(envelope.data)
}

/// Update effect control parameters.
pub async fn update_controls(
    effect_id: &str,
    controls: &serde_json::Value,
) -> Result<(), String> {
    let url = format!("/api/v1/effects/{effect_id}/apply");
    let body = serde_json::json!({ "controls": controls });

    let resp = Request::post(&url)
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
