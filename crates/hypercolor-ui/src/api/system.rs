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
    #[serde(default)]
    pub compositor_acceleration: RenderAccelerationStatus,
    #[serde(default)]
    pub render_loop: RenderLoopStatus,
    /// Named daemon capabilities (Spec 65 §9.6). Multi-zone Studio
    /// affordances gate on the presence of their backing capability —
    /// `zone-crud`, `multi-zone-sampling`, `zone-device-assignment`,
    /// `scene-unassigned-behavior-write`.
    #[serde(default)]
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RenderLoopStatus {
    pub state: String,
    pub fps_tier: String,
    pub target_fps: u32,
    pub ceiling_fps: u32,
    pub consecutive_misses: u32,
    pub total_frames: u64,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RenderAccelerationStatus {
    pub requested_mode: String,
    pub effective_mode: String,
    pub fallback_reason: Option<String>,
    pub servo_gpu_import_mode: String,
    pub servo_gpu_import_attempting: bool,
    pub gpu_probe: Option<GpuCompositorProbeStatus>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct GpuCompositorProbeStatus {
    pub adapter_name: String,
    pub backend: String,
    pub texture_format: String,
    pub max_texture_dimension_2d: u32,
    pub max_storage_textures_per_shader_stage: u32,
    pub linux_servo_gpu_import_backend_compatible: bool,
    pub linux_servo_gpu_import_backend_reason: Option<String>,
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
