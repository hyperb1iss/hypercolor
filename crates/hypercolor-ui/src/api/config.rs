//! Config and audio device API functions.

use serde::Deserialize;

use super::client;

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
}

// ── Fetch Functions ─────────────────────────────────────────────────────────

/// Fetch the full daemon config.
pub async fn fetch_config() -> Result<hypercolor_types::config::HypercolorConfig, String> {
    client::fetch_json("/api/v1/config")
        .await
        .map_err(Into::into)
}

/// Set a single config key. Value is JSON-stringified per daemon contract.
pub async fn set_config_value(key: &str, value: &serde_json::Value) -> Result<(), String> {
    let live = key == "audio" || key.starts_with("audio.");
    let body = serde_json::json!({
        "key": key,
        "value": serde_json::to_string(value).unwrap_or_default(),
        "live": live,
    });
    client::post_json_discard("/api/v1/config/set", &body)
        .await
        .map_err(Into::into)
}

/// Reset a config key or section to defaults.
pub async fn reset_config_key(key: &str) -> Result<(), String> {
    let body = serde_json::json!({
        "key": key,
        "live": key == "audio" || key.starts_with("audio."),
    });
    client::post_json_discard("/api/v1/config/reset", &body)
        .await
        .map_err(Into::into)
}

/// Enumerate available audio devices.
pub async fn fetch_audio_devices() -> Result<AudioDevicesData, String> {
    client::fetch_json("/api/v1/audio/devices")
        .await
        .map_err(Into::into)
}
