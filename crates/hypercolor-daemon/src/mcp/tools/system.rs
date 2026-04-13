//! System-level MCP tools: `get_status`, `get_audio_state`, `get_layout`, `get_sensor_data`, `diagnose`.

use serde_json::{Value, json};

use super::{ToolDefinition, ToolError, brightness_percent, capped_fps, default_output_schema};
use crate::api::AppState;
use crate::api::effects::active_effect_metadata;
use crate::session::current_global_brightness;
use hypercolor_types::sensor::SystemSnapshot;
use std::sync::Arc;

// ── Tool Definitions ──────────────────────────────────────────────────────

pub(super) fn build_get_status() -> ToolDefinition {
    ToolDefinition {
        name: "get_status".into(),
        title: "Get System State".into(),
        description: "Get the current state of the Hypercolor daemon including: active effect, global brightness, connected device count, active profile, FPS metrics, audio/screen input status, and uptime. Call this first to understand the current setup before making changes.".into(),
        input_schema: json!({
            "type": "object",
            "additionalProperties": false
        }),
        output_schema: default_output_schema(),
        read_only: true,
        idempotent: true,
    }
}

pub(super) fn build_get_audio_state() -> ToolDefinition {
    ToolDefinition {
        name: "get_audio_state".into(),
        title: "Get Audio State".into(),
        description: "Get the current audio analysis state including levels, beat detection, and spectrum data.".into(),
        input_schema: json!({
            "type": "object",
            "additionalProperties": false
        }),
        output_schema: default_output_schema(),
        read_only: true,
        idempotent: true,
    }
}

pub(super) fn build_get_layout() -> ToolDefinition {
    ToolDefinition {
        name: "get_layout".into(),
        title: "Get Spatial Layout".into(),
        description: "Get the current spatial layout information including device positions, zones, and topology.".into(),
        input_schema: json!({
            "type": "object",
            "additionalProperties": false
        }),
        output_schema: default_output_schema(),
        read_only: true,
        idempotent: true,
    }
}

pub(super) fn build_diagnose() -> ToolDefinition {
    ToolDefinition {
        name: "diagnose".into(),
        title: "Diagnose Issues".into(),
        description: "Run diagnostics on the Hypercolor system or a specific device. Checks connectivity, protocol health, frame delivery, latency, and error rates. Returns actionable findings with severity levels.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "device_id": {
                    "type": "string",
                    "description": "Specific device to diagnose. Omit for full system diagnostics."
                },
                "checks": {
                    "type": "array",
                    "items": {
                        "type": "string",
                        "enum": ["connectivity", "latency", "frame_delivery", "color_accuracy", "protocol", "all"]
                    },
                    "description": "Which diagnostic checks to run. Defaults to 'all'.",
                    "default": ["all"]
                }
            }
        }),
        output_schema: default_output_schema(),
        read_only: true,
        idempotent: true,
    }
}

pub(super) fn build_get_sensor_data() -> ToolDefinition {
    ToolDefinition {
        name: "get_sensor_data".into(),
        title: "Get Sensor Data".into(),
        description: "Get the latest system telemetry snapshot, or one named sensor reading, including CPU, GPU, memory, and raw component temperatures.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "label": {
                    "type": "string",
                    "description": "Optional sensor label like cpu_temp, gpu_load, ram_used, or a normalized raw component label."
                }
            },
            "additionalProperties": false
        }),
        output_schema: default_output_schema(),
        read_only: true,
        idempotent: true,
    }
}

// ── Stateless Handlers ────────────────────────────────────────────────────

#[expect(
    clippy::unnecessary_wraps,
    reason = "will return errors when wired to daemon state"
)]
pub(super) fn handle_get_status(_params: &Value) -> Result<Value, ToolError> {
    // Would read from DaemonState
    Ok(json!({
        "running": true,
        "paused": false,
        "brightness": 100,
        "fps": {
            "target": 60,
            "actual": 0.0
        },
        "effect": null,
        "profile": null,
        "devices": {
            "connected": 0,
            "total": 0,
            "total_leds": 0
        },
        "inputs": {
            "audio": "disabled",
            "screen": "disabled"
        },
        "uptime_seconds": 0,
        "version": env!("CARGO_PKG_VERSION")
    }))
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "will return errors when wired to audio state"
)]
pub(super) fn handle_get_audio_state(_params: &Value) -> Result<Value, ToolError> {
    // Would read from the spectrum watch channel
    Ok(json!({
        "enabled": false,
        "levels": {
            "overall": 0.0,
            "bass": 0.0,
            "mid": 0.0,
            "treble": 0.0
        },
        "beat": {
            "detected": false,
            "confidence": 0.0,
            "bpm_estimate": null
        }
    }))
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "will return errors when wired to layout state"
)]
pub(super) fn handle_get_layout(_params: &Value) -> Result<Value, ToolError> {
    // Would read from spatial layout state
    Ok(json!({
        "layout": null,
        "zones": [],
        "total_devices": 0,
        "total_leds": 0
    }))
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "will return errors when wired to diagnostics"
)]
pub(super) fn handle_diagnose(params: &Value) -> Result<Value, ToolError> {
    let _device_id = params.get("device_id").and_then(Value::as_str);
    let _checks = params.get("checks");

    // Would run actual diagnostic checks
    Ok(json!({
        "overall_status": "healthy",
        "findings": [],
        "metrics": {
            "fps": 0.0,
            "frame_drop_rate": 0.0,
            "avg_latency_ms": 0.0,
            "device_error_count": 0,
            "uptime_seconds": 0
        }
    }))
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "state-less handler is a placeholder until live daemon state is injected"
)]
pub(super) fn handle_get_sensor_data(_params: &Value) -> Result<Value, ToolError> {
    Ok(json!({
        "snapshot": SystemSnapshot::empty(),
        "reading": null,
    }))
}

// ── Stateful Handlers ─────────────────────────────────────────────────────

pub(super) async fn handle_get_status_with_state(state: &AppState) -> Result<Value, ToolError> {
    let render_stats = {
        let render_loop = state.render_loop.read().await;
        render_loop.stats()
    };
    let target_fps = render_stats.tier.fps();
    let actual_fps = capped_fps(&render_stats);

    let brightness = brightness_percent(current_global_brightness(&state.power_state));

    let active_effect = active_effect_metadata(state).await;

    let effect_count = state.effect_registry.read().await.len();
    let scene_count = state.scene_manager.read().await.scene_count();
    let devices = state.device_registry.list().await;
    let connected_devices = devices
        .iter()
        .filter(|device| device.state.is_renderable())
        .count();
    let total_leds: u64 = devices
        .iter()
        .map(|device| u64::from(device.info.total_led_count()))
        .sum();

    let (audio_status, screen_status) = if let Some(config_manager) = state.config_manager.as_ref()
    {
        let config = config_manager.get();
        (
            if config.audio.enabled {
                "enabled"
            } else {
                "disabled"
            },
            if config.capture.enabled {
                "enabled"
            } else {
                "disabled"
            },
        )
    } else {
        ("unknown", "unknown")
    };

    Ok(json!({
        "running": matches!(render_stats.state, hypercolor_core::engine::RenderLoopState::Running),
        "paused": matches!(render_stats.state, hypercolor_core::engine::RenderLoopState::Paused),
        "brightness": brightness,
        "fps": {
            "target": target_fps,
            "actual": actual_fps
        },
        "effect": active_effect.map(|metadata| json!({
            "id": metadata.id.to_string(),
            "name": metadata.name,
        })),
        "effect_count": effect_count,
        "scene_count": scene_count,
        "devices": {
            "connected": connected_devices,
            "total": devices.len(),
            "total_leds": total_leds
        },
        "inputs": {
            "audio": audio_status,
            "screen": screen_status
        },
        "uptime_seconds": state.start_time.elapsed().as_secs(),
        "version": env!("CARGO_PKG_VERSION")
    }))
}

pub(super) async fn handle_get_sensor_data_with_state(
    params: &Value,
    state: &AppState,
) -> Result<Value, ToolError> {
    let label = params.get("label").and_then(Value::as_str);
    let snapshot = latest_sensor_snapshot(state).await;
    let reading = label.and_then(|value| snapshot.reading(value));

    Ok(json!({
        "snapshot": snapshot.as_ref(),
        "reading": reading,
    }))
}

pub(super) fn handle_get_audio_state_with_state(state: &AppState) -> Value {
    let spectrum = state.event_bus.spectrum_receiver().borrow().clone();
    let enabled = state
        .config_manager
        .as_ref()
        .is_some_and(|config_manager| config_manager.get().audio.enabled);

    json!({
        "enabled": enabled,
        "levels": {
            "overall": spectrum.level,
            "bass": spectrum.bass,
            "mid": spectrum.mid,
            "treble": spectrum.treble
        },
        "beat": {
            "detected": spectrum.beat,
            "confidence": spectrum.beat_confidence,
            "bpm_estimate": spectrum.bpm
        },
        "spectrum_bins": spectrum.bins.len()
    })
}

async fn latest_sensor_snapshot(state: &AppState) -> Arc<SystemSnapshot> {
    let input_manager = state.input_manager.lock().await;
    input_manager
        .latest_sensor_snapshot()
        .unwrap_or_else(|| Arc::new(SystemSnapshot::empty()))
}

pub(super) async fn handle_get_layout_with_state(state: &AppState) -> Result<Value, ToolError> {
    let spatial = state.spatial_engine.read().await;
    let layout = spatial.layout();
    let total_leds: u64 = layout
        .zones
        .iter()
        .map(|zone| u64::from(zone.topology.led_count()))
        .sum();

    Ok(json!({
        "layout": {
            "id": layout.id,
            "name": layout.name,
            "description": layout.description,
            "canvas_width": layout.canvas_width,
            "canvas_height": layout.canvas_height,
            "zone_count": layout.zones.len()
        },
        "zones": layout.zones.iter().map(|zone| json!({
            "id": zone.id,
            "name": zone.name,
            "device_id": zone.device_id,
            "led_count": zone.topology.led_count()
        })).collect::<Vec<_>>(),
        "total_devices": state.device_registry.len().await,
        "total_leds": total_leds
    }))
}

pub(super) async fn handle_diagnose_with_state(
    _params: &Value,
    state: &AppState,
) -> Result<Value, ToolError> {
    let render_stats = state.render_loop.read().await.stats();
    let fps = capped_fps(&render_stats);
    let target_fps = render_stats.tier.fps();
    let consecutive_misses = render_stats.consecutive_misses;
    let render_time_ms = render_stats.avg_frame_time.as_secs_f64() * 1000.0;

    let devices = state.device_registry.list().await;
    let device_count = devices.len();
    let connected_count = devices.iter().filter(|d| d.state.is_renderable()).count();
    let disconnected_count = device_count - connected_count;

    let mut findings = Vec::new();

    if device_count == 0 {
        findings.push(json!({
            "severity": "warning",
            "message": "No devices discovered. Check backend configuration and network visibility."
        }));
    } else if disconnected_count > 0 {
        findings.push(json!({
            "severity": "info",
            "message": format!("{disconnected_count} of {device_count} devices are disconnected.")
        }));
    }

    if consecutive_misses > 5 {
        findings.push(json!({
            "severity": "warning",
            "message": format!("Render loop has {consecutive_misses} consecutive frame budget misses — effects may stutter.")
        }));
    }

    let is_running = matches!(
        render_stats.state,
        hypercolor_core::engine::RenderLoopState::Running
    );
    if !is_running {
        findings.push(json!({
            "severity": "warning",
            "message": format!("Render loop is {:?}, not running.", render_stats.state)
        }));
    }

    let status = if findings.iter().any(|f| f["severity"] == "warning") {
        "warning"
    } else {
        "healthy"
    };

    Ok(json!({
        "overall_status": status,
        "findings": findings,
        "metrics": {
            "fps": fps,
            "target_fps": target_fps,
            "consecutive_misses": consecutive_misses,
            "avg_render_time_ms": render_time_ms,
            "device_count": device_count,
            "connected_devices": connected_count,
            "uptime_seconds": state.start_time.elapsed().as_secs()
        }
    }))
}
