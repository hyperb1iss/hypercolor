//! System status API.

use hypercolor_types::sensor::SystemSnapshot;
use serde::Deserialize;

use super::client;

// ── Types ───────────────────────────────────────────────────────────────────

/// System status from `GET /api/v1/status`.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct SystemStatus {
    pub running: bool,
    pub version: String,
    #[serde(default)]
    pub config_path: String,
    pub uptime_seconds: u64,
    pub device_count: usize,
    pub effect_count: usize,
    pub active_effect: Option<String>,
    pub active_scene: Option<String>,
    #[serde(default)]
    pub active_scene_snapshot_locked: bool,
    pub global_brightness: u8,
}

// ── Fetch Functions ─────────────────────────────────────────────────────────

/// Fetch system status.
pub async fn fetch_status() -> Result<SystemStatus, String> {
    client::fetch_json("/api/v1/status")
        .await
        .map_err(Into::into)
}

/// Fetch the latest system sensor snapshot.
pub async fn fetch_system_sensors() -> Result<SystemSnapshot, String> {
    client::fetch_json("/api/v1/system/sensors")
        .await
        .map_err(Into::into)
}
