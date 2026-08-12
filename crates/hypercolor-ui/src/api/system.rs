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
    /// Host input capture health — consent gate plus device-node
    /// open/denied counts. Defaults tolerate daemons predating the field.
    #[serde(default)]
    pub input: InputStatus,
}

/// Host keyboard/mouse capture health from the daemon status payload.
///
/// `enabled` is the consent config gate (`input.enabled`). `devices_denied`
/// counts input nodes that exist but are unreadable (udev rules missing) —
/// the signal that separates "input is off" from "input is on but blocked".
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct InputStatus {
    pub enabled: bool,
    pub host_capture_registered: bool,
    pub host_capturing: bool,
    pub devices_opened: usize,
    pub devices_denied: usize,
    /// Session-level failure code the counters cannot express, e.g. a Windows
    /// daemon running without a visible window station.
    pub degraded: Option<String>,
    pub backends: Vec<String>,
    pub source_graph_generation: u64,
    pub sources: Vec<InputSourceStatus>,
}

/// Structured source issue from the daemon's operational status snapshot.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct InputSourceIssueStatus {
    pub code: String,
    pub message: String,
    pub remediation: Option<String>,
    pub retryable: bool,
}

/// Process topologies competing to own a protected macOS capability.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct MacosDaemonOwnerConflictStatus {
    pub active: Option<String>,
    pub contender: Option<String>,
    pub observed_at_ms: Option<u64>,
}

/// Persistability and redacted content style of a macOS screen selection.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MacosSelectionStatus {
    None,
    Display {
        #[serde(default)]
        source_id: Option<String>,
    },
    SessionScoped {
        #[serde(default)]
        content_style: Option<String>,
    },
    #[serde(other)]
    Unknown,
}

/// Tahoe capabilities proven for one selected capture incarnation.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct MacosTahoeSelectionStatus {
    pub source_id: Option<String>,
    pub capture_session_generation: Option<u64>,
    pub hdr_capture: Option<bool>,
    pub dual_range_screenshots: Option<bool>,
}

/// Platform-specific source state carried by the daemon status endpoint.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InputSourcePlatformStatus {
    MacosInput {
        #[serde(default)]
        keyboard: Option<String>,
        #[serde(default)]
        pointer: Option<String>,
        #[serde(default)]
        keyboard_tcc: Option<String>,
        #[serde(default)]
        keyboard_owner: Option<String>,
        #[serde(default)]
        pointer_owner: Option<String>,
        #[serde(default)]
        owner_conflict: Option<MacosDaemonOwnerConflictStatus>,
    },
    MacosScreen {
        #[serde(default)]
        state: Option<String>,
        #[serde(default)]
        tcc: Option<String>,
        #[serde(default)]
        owner: Option<String>,
        #[serde(default)]
        selection: Option<MacosSelectionStatus>,
        #[serde(default)]
        tahoe_selection: Option<MacosTahoeSelectionStatus>,
        #[serde(default)]
        owner_conflict: Option<MacosDaemonOwnerConflictStatus>,
    },
    #[serde(other)]
    Unknown,
}

/// Lock-free lifecycle and freshness status for one input source.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct InputSourceStatus {
    pub source_id: String,
    pub kind: String,
    pub backend: String,
    pub configured: bool,
    pub consented: bool,
    pub demanded: bool,
    pub state: String,
    pub freshness: String,
    pub source_graph_generation: u64,
    pub session_generation: u64,
    pub last_sample_age_ms: Option<u64>,
    pub freshness_remaining_ms: Option<u64>,
    pub resource_count: usize,
    pub denied_resource_count: usize,
    pub issue: Option<InputSourceIssueStatus>,
    pub lifecycle_issue: Option<InputSourceIssueStatus>,
    pub freshness_issue: Option<InputSourceIssueStatus>,
    pub platform: Option<InputSourcePlatformStatus>,
    pub retired: bool,
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
    pub servo_gpu_import_backend_compatible: bool,
    pub servo_gpu_import_backend_reason: Option<String>,
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        InputSourcePlatformStatus, InputSourceStatus, MacosDaemonOwnerConflictStatus,
        MacosSelectionStatus, MacosTahoeSelectionStatus,
    };

    #[test]
    fn input_source_status_decodes_macos_input_platform_tolerantly() {
        let status: InputSourceStatus = serde_json::from_value(json!({
            "platform": {
                "type": "macos_input",
                "keyboard": "needs_process_restart",
                "pointer": "live",
                "keyboard_tcc": "authorized",
                "keyboard_owner": "app_sidecar",
                "pointer_owner": "broker",
                "owner_conflict": {
                    "active": "launchd_service",
                    "contender": "homebrew_service",
                    "observed_at_ms": 1_725_000_000_123_u64,
                    "future_conflict_field": true
                },
                "future_probe": { "available": true }
            },
            "future_source_field": 42
        }))
        .expect("macOS input status should decode");

        let Some(InputSourcePlatformStatus::MacosInput {
            keyboard,
            pointer,
            keyboard_tcc,
            keyboard_owner,
            pointer_owner,
            owner_conflict,
        }) = status.platform
        else {
            panic!("fixture should decode the macOS input variant");
        };

        assert_eq!(keyboard.as_deref(), Some("needs_process_restart"));
        assert_eq!(pointer.as_deref(), Some("live"));
        assert_eq!(keyboard_tcc.as_deref(), Some("authorized"));
        assert_eq!(keyboard_owner.as_deref(), Some("app_sidecar"));
        assert_eq!(pointer_owner.as_deref(), Some("broker"));
        assert_eq!(
            owner_conflict,
            Some(MacosDaemonOwnerConflictStatus {
                active: Some("launchd_service".to_owned()),
                contender: Some("homebrew_service".to_owned()),
                observed_at_ms: Some(1_725_000_000_123),
            })
        );

        let partial: InputSourceStatus = serde_json::from_value(json!({
            "platform": { "type": "macos_input" }
        }))
        .expect("partial macOS input status should decode");
        assert!(matches!(
            partial.platform,
            Some(InputSourcePlatformStatus::MacosInput {
                keyboard: None,
                owner_conflict: None,
                ..
            })
        ));
    }

    #[test]
    fn input_source_status_decodes_macos_screen_platform_tolerantly() {
        let status: InputSourceStatus = serde_json::from_value(json!({
            "platform": {
                "type": "macos_screen",
                "state": "interrupted",
                "tcc": "denied",
                "owner": "standalone",
                "selection": {
                    "type": "session_scoped",
                    "content_style": "multiple_windows",
                    "future_selection_field": "ignored"
                },
                "tahoe_selection": {
                    "source_id": "session:23",
                    "capture_session_generation": 29,
                    "hdr_capture": true,
                    "dual_range_screenshots": true,
                    "future_tahoe_field": 4
                },
                "owner_conflict": {
                    "active": "standalone",
                    "contender": "app",
                    "observed_at_ms": 1_725_000_000_456_u64
                },
                "future_probe": { "available": true }
            }
        }))
        .expect("macOS screen status should decode");

        let Some(InputSourcePlatformStatus::MacosScreen {
            state,
            tcc,
            owner,
            selection,
            tahoe_selection,
            owner_conflict,
        }) = status.platform
        else {
            panic!("fixture should decode the macOS screen variant");
        };

        assert_eq!(state.as_deref(), Some("interrupted"));
        assert_eq!(tcc.as_deref(), Some("denied"));
        assert_eq!(owner.as_deref(), Some("standalone"));
        assert_eq!(
            selection,
            Some(MacosSelectionStatus::SessionScoped {
                content_style: Some("multiple_windows".to_owned()),
            })
        );
        assert_eq!(
            tahoe_selection,
            Some(MacosTahoeSelectionStatus {
                source_id: Some("session:23".to_owned()),
                capture_session_generation: Some(29),
                hdr_capture: Some(true),
                dual_range_screenshots: Some(true),
            })
        );
        assert_eq!(
            owner_conflict,
            Some(MacosDaemonOwnerConflictStatus {
                active: Some("standalone".to_owned()),
                contender: Some("app".to_owned()),
                observed_at_ms: Some(1_725_000_000_456),
            })
        );
    }

    #[test]
    fn input_source_status_decodes_absent_platform() {
        let status: InputSourceStatus = serde_json::from_value(json!({
            "source_id": "linux:host-input",
            "future_source_field": true
        }))
        .expect("status without platform should decode");

        assert_eq!(status.source_id, "linux:host-input");
        assert_eq!(status.platform, None);
    }

    #[test]
    fn input_source_status_decodes_future_platform_variant() {
        let status: InputSourceStatus = serde_json::from_value(json!({
            "platform": {
                "type": "future_platform",
                "future_state": "live"
            }
        }))
        .expect("future platform status should decode");

        assert_eq!(status.platform, Some(InputSourcePlatformStatus::Unknown));
    }
}
