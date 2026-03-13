//! Config and audio device API functions.

use gloo_net::http::Request;
use serde::Deserialize;

use super::ApiEnvelope;

// ── Types ───────────────────────────────────────────────────────────────────

/// Audio device info from `GET /api/v1/audio/devices`.
#[derive(Debug, Clone, Deserialize)]
pub struct AudioDeviceInfo {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AudioDevicesData {
    pub devices: Vec<AudioDeviceInfo>,
    pub current: String,
}

// ── Fetch Functions ─────────────────────────────────────────────────────────

/// Fetch the full daemon config.
pub async fn fetch_config() -> Result<hypercolor_types::config::HypercolorConfig, String> {
    let resp = Request::get("/api/v1/config")
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        return Err(format!("HTTP {}", resp.status()));
    }

    let envelope: ApiEnvelope<hypercolor_types::config::HypercolorConfig> =
        resp.json().await.map_err(|e| format!("Parse error: {e}"))?;

    Ok(envelope.data)
}

/// Set a single config key. Value is JSON-stringified per daemon contract.
pub async fn set_config_value(key: &str, value: &serde_json::Value) -> Result<(), String> {
    let live = key == "audio" || key.starts_with("audio.");
    let body = serde_json::json!({
        "key": key,
        "value": serde_json::to_string(value).unwrap_or_default(),
        "live": live,
    });

    let resp = Request::post("/api/v1/config/set")
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

/// Reset a config key or section to defaults.
pub async fn reset_config_key(key: &str) -> Result<(), String> {
    let body = serde_json::json!({
        "key": key,
        "live": key == "audio" || key.starts_with("audio."),
    });

    let resp = Request::post("/api/v1/config/reset")
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

/// Enumerate available audio devices.
pub async fn fetch_audio_devices() -> Result<AudioDevicesData, String> {
    let resp = Request::get("/api/v1/audio/devices")
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status() != 200 {
        return Ok(AudioDevicesData {
            devices: vec![AudioDeviceInfo {
                id: "default".to_string(),
                name: "Default".to_string(),
                description: "System default".to_string(),
            }],
            current: "default".to_string(),
        });
    }

    let envelope: ApiEnvelope<AudioDevicesData> =
        resp.json().await.map_err(|e| format!("Parse error: {e}"))?;

    Ok(envelope.data)
}
