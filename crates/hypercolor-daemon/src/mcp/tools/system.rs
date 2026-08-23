//! System-level MCP tools: `get_status`, `get_audio_state`, `get_layout`, `get_sensor_data`, `diagnose`.

use serde_json::{Value, json};

use super::{ToolDefinition, ToolError, output_schema, serialize_result};
use crate::app_state::AppState;
use crate::domain::output;
use crate::mcp::results::{
    AudioLevelsResult, AudioStateResult, BeatResult, LayoutResult, LayoutSummaryResult,
    LayoutZoneResult, OutputPowerResult, SensorDataResult, StatusResult,
};
use hypercolor_types::api::output::{OutputPatchRequest, OutputPowerMode};
use hypercolor_types::sensor::SystemSnapshot;
use std::sync::Arc;

// ── Tool Definitions ──────────────────────────────────────────────────────

pub(super) fn build_get_status() -> ToolDefinition {
    ToolDefinition {
        name: "get_status".into(),
        title: "Get System State".into(),
        description: "Get the current state of the Hypercolor daemon including: active effect, global brightness, connected device count, FPS metrics, audio/screen input status, and uptime. Call this first to understand the current setup before making changes.".into(),
        input_schema: json!({
            "type": "object",
            "additionalProperties": false
        }),
        output_schema: output_schema::<StatusResult>(),
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
        output_schema: output_schema::<OutputPowerResult>(),
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
        output_schema: output_schema::<AudioStateResult>(),
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
        output_schema: output_schema::<LayoutResult>(),
        read_only: true,
        destructive: false,
        idempotent: true,
    }
}

pub(super) fn build_diagnose() -> ToolDefinition {
    ToolDefinition {
        name: "diagnose".into(),
        title: "Diagnose Issues".into(),
        description: "Run the daemon's safe default diagnostics and return canonical checks, summary counts, and the captured status snapshot.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }),
        output_schema: output_schema::<hypercolor_types::api::diagnose::DiagnoseResponse>(),
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
        output_schema: output_schema::<SensorDataResult>(),
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
        &state.domains.output,
        OutputPatchRequest {
            power: Some(requested),
            brightness: None,
        },
    )
    .await?;
    serialize_result(OutputPowerResult {
        state: outcome.output.power,
    })
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
    serialize_result(crate::mcp::payload::build_status_payload(state).await)
}

pub(super) async fn handle_get_sensor_data_with_state(
    params: &Value,
    state: &AppState,
) -> Result<Value, ToolError> {
    let label = params.get("label").and_then(Value::as_str);
    let snapshot = latest_sensor_snapshot(state).await;
    let reading = label.and_then(|value| snapshot.reading(value));

    serialize_result(SensorDataResult {
        snapshot: snapshot.as_ref().clone(),
        reading,
    })
}

pub(super) fn handle_get_audio_state_with_state(state: &AppState) -> Result<Value, ToolError> {
    let spectrum = state.event_bus.spectrum_receiver().borrow().clone();
    let enabled = state
        .config_manager
        .as_ref()
        .is_some_and(|config_manager| config_manager.get().audio.enabled);

    serialize_result(AudioStateResult {
        enabled,
        levels: AudioLevelsResult {
            overall: spectrum.level,
            bass: spectrum.bass,
            mid: spectrum.mid,
            treble: spectrum.treble,
        },
        beat: BeatResult {
            detected: spectrum.beat,
            confidence: spectrum.beat_confidence,
            bpm_estimate: spectrum.bpm,
        },
        spectrum_bins: spectrum.bins.len(),
    })
}

async fn latest_sensor_snapshot(state: &AppState) -> Arc<SystemSnapshot> {
    let input_manager = state.input_manager().lock().await;
    input_manager
        .latest_sensor_snapshot()
        .unwrap_or_else(|| Arc::new(SystemSnapshot::empty()))
}

pub(super) async fn handle_get_layout_with_state(state: &AppState) -> Result<Value, ToolError> {
    let spatial = state.spatial_engine.snapshot();
    let layout = spatial.layout();
    let total_leds: u64 = layout
        .zones
        .iter()
        .map(|zone| u64::from(zone.topology.led_count()))
        .sum();

    serialize_result(LayoutResult {
        layout: LayoutSummaryResult {
            id: layout.id.clone(),
            name: layout.name.clone(),
            description: layout.description.clone(),
            canvas_width: layout.canvas_width,
            canvas_height: layout.canvas_height,
            zone_count: layout.zones.len(),
        },
        zones: layout
            .zones
            .iter()
            .map(|zone| LayoutZoneResult {
                id: zone.id.clone(),
                name: zone.name.clone(),
                device_id: zone.device_id.clone(),
                led_count: zone.topology.led_count(),
            })
            .collect(),
        total_devices: state.device_registry.len().await,
        total_leds,
    })
}

pub(super) async fn handle_diagnose_with_state(
    _params: &Value,
    state: &AppState,
) -> Result<Value, ToolError> {
    serialize_result(state.domains.diagnostics.collect_default().await)
}
