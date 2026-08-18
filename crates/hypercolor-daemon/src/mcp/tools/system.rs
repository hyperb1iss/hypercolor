//! System-level MCP tools: `get_status`, `get_audio_state`, `get_layout`, `get_sensor_data`, `diagnose`.

use serde_json::{Value, json};

use super::{
    ToolDefinition, ToolError, brightness_percent, default_output_schema, render_capacity_fps,
};
use crate::api::AppState;
use crate::api::effects::active_effect_metadata;
use crate::api::system::{actionable_input_diagnostics, input_status_snapshot};
use crate::domain::output;
use crate::session::current_global_brightness;
use hypercolor_core::input::InteractionDegradation;
use hypercolor_types::api::output::{OutputPatchRequest, OutputPowerMode};
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
        destructive: false,
        idempotent: true,
    }
}

pub(super) fn build_set_output_power() -> ToolDefinition {
    ToolDefinition {
        name: "set_output_power".into(),
        title: "Set Output Power".into(),
        description: "Pause or resume all output without discarding the active effect, controls, preset provenance, or scene state.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "state": {
                    "type": "string",
                    "enum": ["running", "paused"],
                    "description": "Desired global output power state"
                }
            },
            "required": ["state"],
            "additionalProperties": false
        }),
        output_schema: json!({
            "type": "object",
            "properties": {
                "state": {
                    "type": "string",
                    "enum": ["running", "paused"]
                }
            },
            "required": ["state"],
            "additionalProperties": false
        }),
        read_only: false,
        destructive: false,
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
        destructive: false,
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
        destructive: false,
        idempotent: true,
    }
}

pub(super) fn build_diagnose() -> ToolDefinition {
    ToolDefinition {
        name: "diagnose".into(),
        title: "Diagnose Issues".into(),
        description: "Run full-system diagnostics. Checks connectivity, protocol health, frame delivery, latency, and error rates across every device. Returns actionable findings with severity levels.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }),
        output_schema: json!({
            "type": "object",
            "properties": {
                "overall_status": {
                    "type": "string",
                    "enum": ["healthy", "warning", "unhealthy"]
                },
                "findings": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "severity": {
                                "type": "string",
                                "enum": ["info", "warning", "error", "critical"]
                            },
                            "source_id": { "type": "string" },
                            "message": { "type": "string" }
                        },
                        "required": ["severity", "message"]
                    }
                },
                "metrics": { "type": "object" }
            },
            "required": ["overall_status", "findings", "metrics"],
            "additionalProperties": false
        }),
        read_only: true,
        destructive: false,
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
        destructive: false,
        idempotent: true,
    }
}

// ── Handlers ──────────────────────────────────────────────────────────────

pub(super) async fn handle_set_output_power_with_state(
    params: &Value,
    state: &AppState,
) -> Result<Value, ToolError> {
    let requested = parse_output_power_mode(params)?;
    let outcome = output::patch_output(
        state,
        OutputPatchRequest {
            power: Some(requested),
            brightness: None,
        },
    )
    .await?;
    Ok(json!({ "state": outcome.power }))
}

fn parse_output_power_mode(params: &Value) -> Result<OutputPowerMode, ToolError> {
    match params.get("state").and_then(Value::as_str) {
        Some("running") => Ok(OutputPowerMode::Running),
        Some("paused") => Ok(OutputPowerMode::Paused),
        Some(value) => Err(ToolError::InvalidParam {
            param: "state".into(),
            reason: format!("expected 'running' or 'paused', got '{value}'"),
        }),
        None => Err(ToolError::MissingParam("state".into())),
    }
}

pub(super) async fn handle_get_status_with_state(state: &AppState) -> Result<Value, ToolError> {
    let render_stats = {
        let render_loop = state.render_loop.read().await;
        render_loop.stats()
    };
    let target_fps = render_stats.tier.fps();
    let capacity_fps = render_capacity_fps(&render_stats);
    let delivered_fps = if matches!(
        render_stats.state,
        hypercolor_core::engine::RenderLoopState::Running
    ) {
        state.performance.read().await.snapshot().delivered_fps
    } else {
        0.0
    };

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
    let input = input_status_snapshot(state);
    let input_state = if input.enabled {
        match input.degraded.as_deref() {
            Some(code) if code == InteractionDegradation::AccessDenied.code() => {
                "blocked_permissions"
            }
            Some(code) if code == InteractionDegradation::NoInteractiveSession.code() => {
                "no_interactive_session"
            }
            Some(_) => "unavailable",
            None => "enabled",
        }
    } else {
        "disabled"
    };

    let power = *state.power_state.borrow();
    let paused = power.reported_paused();

    Ok(json!({
        "running": !power.sleeping(),
        "paused": paused,
        "brightness": brightness,
        "fps": {
            "target": target_fps,
            "capacity": capacity_fps,
            "delivered": delivered_fps,
            "actual": capacity_fps
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
            "screen": screen_status,
            "input": input_state,
            "input_devices_opened": input.devices_opened,
            "input_devices_denied": input.devices_denied,
            "input_degraded": input.degraded,
            "source_graph_generation": input.source_graph_generation,
            "sources": input.sources
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

#[expect(
    clippy::too_many_lines,
    reason = "MCP diagnose returns a compact but broad operational snapshot"
)]
pub(super) async fn handle_diagnose_with_state(
    _params: &Value,
    state: &AppState,
) -> Result<Value, ToolError> {
    let render_stats = state.render_loop.read().await.stats();
    let performance = state.performance.read().await.snapshot();
    let device_metrics = state.device_metrics.load_full();
    let usb_actor_metrics = hypercolor_core::device::usb_actor_metrics_snapshot();
    let capacity_fps = render_capacity_fps(&render_stats);
    let is_running = matches!(
        render_stats.state,
        hypercolor_core::engine::RenderLoopState::Running
    );
    let delivered_fps = if is_running {
        performance.delivered_fps
    } else {
        0.0
    };
    let target_fps = render_stats.tier.fps();
    let consecutive_misses = render_stats.consecutive_misses;
    let render_time_ms = render_stats.avg_frame_time.as_secs_f64() * 1000.0;
    let latest_frame = performance.latest_frame.as_ref().map(|frame| {
        json!({
            "frame_token": frame.timeline.frame_token,
            "compositor_backend": frame.compositor_backend.as_str(),
            "output_frame_source": frame.output_frame_source.as_str(),
            "output_reuses_published_frame": frame.output_reuses_published_frame,
            "gpu_zone_sampling": frame.gpu_zone_sampling,
            "gpu_sample_deferred": frame.gpu_sample_deferred,
            "gpu_sample_stale": frame.gpu_sample_stale,
            "gpu_sample_retry_hit": frame.gpu_sample_retry_hit,
            "gpu_sample_queue_saturated": frame.gpu_sample_queue_saturated,
            "gpu_sample_wait_blocked": frame.gpu_sample_wait_blocked,
            "cpu_readback_skipped": frame.cpu_readback_skipped,
            "gpu_readback_failed": frame.gpu_readback_failed,
            "devices_written": frame.devices_written,
            "total_leds": frame.total_leds,
            "sample_us": frame.sample_us,
            "push_us": frame.push_us,
            "publish_us": frame.publish_us,
            "total_us": frame.total_us,
            "output_routing_signature": frame.output_routing_signature,
            "output_zone_shape_signature": frame.output_zone_shape_signature,
            "output_unassigned_behavior_generation": frame.output_unassigned_behavior_generation,
            "output_errors": frame.output_errors
        })
    });
    let lagging_queues = device_metrics
        .items
        .iter()
        .filter(|item| item.fps_queued > 1.0 && item.fps_sent + 1.0 < item.fps_queued * 0.75)
        .count();
    let dropped_frames_total = device_metrics
        .items
        .iter()
        .fold(0_u64, |acc, item| acc.saturating_add(item.frames_dropped));
    let output_errors_total = device_metrics
        .items
        .iter()
        .fold(0_u64, |acc, item| acc.saturating_add(item.errors_total));
    let input = input_status_snapshot(state);
    let input_diagnostics = actionable_input_diagnostics(&input);

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

    if performance
        .latest_frame
        .as_ref()
        .is_some_and(|frame| frame.gpu_sample_stale)
    {
        findings.push(json!({
            "severity": "warning",
            "message": "Latest LED frame used a stale GPU sample; inspect metrics.latest_frame.output_frame_source and metrics.render_window."
        }));
    }

    if lagging_queues > 0 || dropped_frames_total > 0 {
        findings.push(json!({
            "severity": "warning",
            "message": format!("Device output queues show lag/drops: lagging={lagging_queues}, dropped_total={dropped_frames_total}.")
        }));
    }

    if !is_running {
        findings.push(json!({
            "severity": "warning",
            "message": format!("Render loop is {:?}, not running.", render_stats.state)
        }));
    }

    for diagnostic in &input_diagnostics {
        findings.push(json!({
            "severity": if diagnostic.status == "fail" { "error" } else { "warning" },
            "source_id": diagnostic.source_id,
            "message": diagnostic.detail,
        }));
    }

    let status = if findings.iter().any(|f| f["severity"] == "error") {
        "unhealthy"
    } else if findings.iter().any(|f| f["severity"] == "warning") {
        "warning"
    } else {
        "healthy"
    };

    Ok(json!({
        "overall_status": status,
        "findings": findings,
        "metrics": {
            "fps": capacity_fps,
            "capacity_fps": capacity_fps,
            "delivered_fps": delivered_fps,
            "target_fps": target_fps,
            "consecutive_misses": consecutive_misses,
            "avg_render_time_ms": render_time_ms,
            "device_count": device_count,
            "connected_devices": connected_count,
            "uptime_seconds": state.start_time.elapsed().as_secs(),
            "inputs": input,
            "latest_frame": latest_frame,
            "render_window": {
                "frames": performance.frame_count,
                "gpu_sample_deferred": performance.pacing.gpu_sample_deferred,
                "gpu_sample_stale": performance.pacing.gpu_sample_stale,
                "gpu_sample_retry_hit": performance.pacing.gpu_sample_retry_hit,
                "gpu_sample_queue_saturated": performance.pacing.gpu_sample_queue_saturated,
                "gpu_sample_wait_blocked": performance.pacing.gpu_sample_wait_blocked,
                "output_current_frame": performance.pacing.output_current_frame,
                "output_published_frame": performance.pacing.output_published_frame,
                "output_routed_reuse": performance.pacing.output_routed_reuse,
                "output_reused_published_frame": performance.pacing.output_reused_published_frame,
                "output_error_frames": performance.pacing.output_error_frames
            },
            "device_output": {
                "queues": device_metrics.items.len(),
                "lagging_queues": lagging_queues,
                "dropped_frames_total": dropped_frames_total,
                "errors_total": output_errors_total,
                "items": device_metrics.items.iter().map(|item| {
                    serde_json::to_value(item).unwrap_or(serde_json::Value::Null)
                }).collect::<Vec<_>>()
            },
            "usb_actor": {
                "display_frames_total": usb_actor_metrics.display_frames_total,
                "display_frames_delayed_for_led_total": usb_actor_metrics.display_frames_delayed_for_led_total,
                "display_led_priority_wait_total_us": usb_actor_metrics.display_led_priority_wait_total_us,
                "display_led_priority_wait_max_us": usb_actor_metrics.display_led_priority_wait_max_us
            }
        }
    }))
}
