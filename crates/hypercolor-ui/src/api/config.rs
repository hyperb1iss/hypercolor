//! Config and audio device API functions.

use serde::Deserialize;

use hypercolor_types::config_registry::ConfigKeySchemaEntry;

use super::client;
use crate::control_surface_api::path_segment;

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
    client::fetch_json("/api/v1/config").await
}

fn config_key_url(key: &str) -> String {
    format!("/api/v1/config/keys/{}", path_segment(key))
}

/// Write one config key. The body is the value itself, and the daemon
/// decides from its key registry which subsystem to re-apply.
pub async fn set_config_value(key: &str, value: &serde_json::Value) -> Result<(), String> {
    client::put_json_discard(&config_key_url(key), value)
        .await
        .map_err(Into::into)
}

/// Restore a config key or section to its default.
pub async fn reset_config_key(key: &str) -> Result<(), String> {
    client::delete_empty(&config_key_url(key))
        .await
        .map_err(Into::into)
}

/// The daemon's config key registry: how every key applies and renders.
pub async fn fetch_config_schema() -> Result<Vec<ConfigKeySchemaEntry>, String> {
    client::fetch_json("/api/v1/config/schema")
        .await
        .map_err(Into::into)
}

/// One display output capture can address, from `/api/v1/capture/monitors`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct CaptureMonitor {
    pub index: usize,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub primary: bool,
    /// Ready-to-store `capture.source` value selecting this output.
    pub value: String,
}

/// Display outputs the capture backend can address. Empty on portal
/// platforms, which is the UI's cue to show the picker button instead.
pub async fn fetch_capture_monitors() -> Result<Vec<CaptureMonitor>, String> {
    client::fetch_json("/api/v1/capture/monitors")
        .await
        .map_err(Into::into)
}

/// Enumerate available audio devices.
pub async fn fetch_audio_devices() -> Result<AudioDevicesData, String> {
    client::fetch_json("/api/v1/audio/devices")
        .await
        .map_err(Into::into)
}

/// Re-open the desktop portal source picker for screen capture.
pub async fn pick_capture_source() -> Result<(), String> {
    client::post_empty("/api/v1/capture/source/pick")
        .await
        .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::config_key_url;

    #[test]
    fn config_keys_address_one_path_segment() {
        assert_eq!(
            config_key_url("daemon.target_fps"),
            "/api/v1/config/keys/daemon.target_fps"
        );
        assert_eq!(
            config_key_url("drivers.wled/../hue"),
            "/api/v1/config/keys/drivers.wled%2F..%2Fhue"
        );
    }
}
