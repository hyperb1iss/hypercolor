//! Config and audio device API functions.

pub use hypercolor_types::api::capture::CaptureMonitor;
use hypercolor_types::api::system::AudioDevicesResponse;
use hypercolor_types::config_registry::ConfigKeySchemaEntry;

use super::{ApiResult, client};
use crate::control_surface_api::path_segment;

// ── Fetch Functions ─────────────────────────────────────────────────────────

/// Fetch the full daemon config while preserving typed HTTP errors.
pub async fn fetch_config_typed() -> ApiResult<hypercolor_types::config::HypercolorConfig> {
    client::fetch_json("/api/v1/config").await
}

fn config_key_url(key: &str) -> String {
    format!("/api/v1/config/keys/{}", path_segment(key))
}

/// Write one config key. The body is the value itself, and the daemon
/// decides from its key registry which subsystem to re-apply.
pub async fn set_config_value(key: &str, value: &serde_json::Value) -> ApiResult<()> {
    client::put_json_discard(&config_key_url(key), value).await
}

/// Restore a config key or section to its default.
pub async fn reset_config_key(key: &str) -> ApiResult<()> {
    client::delete_empty(&config_key_url(key)).await
}

/// The daemon's config key registry: how every key applies and renders.
pub async fn fetch_config_schema() -> ApiResult<Vec<ConfigKeySchemaEntry>> {
    client::fetch_json("/api/v1/config/schema").await
}

/// Display outputs the capture backend can address. Empty on portal
/// platforms, which is the UI's cue to show the picker button instead.
pub async fn fetch_capture_monitors() -> ApiResult<Vec<CaptureMonitor>> {
    client::fetch_json("/api/v1/capture/monitors").await
}

/// Enumerate available audio devices.
pub async fn fetch_audio_devices() -> ApiResult<AudioDevicesResponse> {
    client::fetch_json("/api/v1/system/audio-devices").await
}

/// Re-open the desktop portal source picker for screen capture.
pub async fn pick_capture_source() -> ApiResult<()> {
    client::put_json_discard("/api/v1/capture/source", &serde_json::json!({})).await
}

/// Explicitly request Input Monitoring from the active macOS owner.
pub async fn authorize_input_monitoring() -> ApiResult<()> {
    client::post_empty("/api/v1/input/authorize").await
}

/// Explicitly request Screen Recording from the active macOS owner.
pub async fn authorize_screen_recording() -> ApiResult<()> {
    client::post_empty("/api/v1/capture/authorize").await
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
