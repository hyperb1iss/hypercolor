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

/// Fetch the full daemon config while preserving typed HTTP errors.
pub async fn fetch_config_typed()
-> Result<hypercolor_types::config::HypercolorConfig, client::ApiError> {
    client::fetch_json("/api/v1/config")
        .await
}

/// Set a single config key. Value is JSON-stringified per daemon contract.
pub async fn set_config_value(key: &str, value: &serde_json::Value) -> Result<(), String> {
    let body = serde_json::json!({
        "key": key,
        "value": serde_json::to_string(value).unwrap_or_default(),
        "live": applies_live(key),
    });
    client::post_json_discard("/api/v1/config/set", &body)
        .await
        .map_err(Into::into)
}

/// Reset a config key or section to defaults.
pub async fn reset_config_key(key: &str) -> Result<(), String> {
    let body = serde_json::json!({
        "key": key,
        "live": applies_live(key),
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

fn applies_live(key: &str) -> bool {
    key == "audio"
        || key.starts_with("audio.")
        || matches!(
            key,
            "daemon.target_fps" | "daemon.canvas_width" | "daemon.canvas_height"
        )
}

#[cfg(test)]
mod tests {
    use super::applies_live;

    #[test]
    fn render_timing_keys_apply_live() {
        assert!(applies_live("daemon.target_fps"));
        assert!(applies_live("daemon.canvas_width"));
        assert!(applies_live("daemon.canvas_height"));
    }

    #[test]
    fn restart_only_render_keys_do_not_apply_live() {
        assert!(!applies_live("effect_engine.render_acceleration_mode"));
    }
}
