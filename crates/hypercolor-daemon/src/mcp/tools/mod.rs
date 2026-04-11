//! MCP tool definitions — the daemon tools exposed to AI assistants.
//!
//! Each tool is a `ToolDefinition` with a JSON Schema input spec. Tool execution
//! is handled by `execute_tool`, which dispatches to the appropriate handler in
//! a per-cluster submodule.

use serde_json::{Map, Value, json};

use crate::api::AppState;
use hypercolor_types::effect::ControlValue;

mod devices;
mod effects;
mod library;
mod overlays;
mod scenes;
mod system;

/// Definition of a single MCP tool.
#[derive(Debug, Clone)]
pub struct ToolDefinition {
    /// Tool name in `snake_case` per MCP convention.
    pub name: String,
    /// Human-readable title for display in tool lists.
    pub title: String,
    /// Detailed description of what the tool does and how to use it.
    pub description: String,
    /// JSON Schema for the tool's input parameters.
    pub input_schema: Value,
    /// JSON Schema for the tool's structured result payload.
    pub output_schema: Value,
    /// Whether this tool only reads state (never modifies).
    pub read_only: bool,
    /// Whether repeated calls with the same input produce the same result.
    pub idempotent: bool,
}

/// Build all MCP tool definitions.
pub fn build_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        effects::build_set_effect(),
        effects::build_list_effects(),
        effects::build_stop_effect(),
        effects::build_set_color(),
        devices::build_get_devices(),
        devices::build_set_brightness(),
        system::build_get_status(),
        scenes::build_activate_scene(),
        scenes::build_list_scenes(),
        scenes::build_create_scene(),
        system::build_get_audio_state(),
        system::build_get_sensor_data(),
        overlays::build_list_display_overlays(),
        overlays::build_set_display_overlay(),
        library::build_set_profile(),
        system::build_get_layout(),
        system::build_diagnose(),
    ]
}

pub(super) fn default_output_schema() -> Value {
    json!({
        "type": "object",
        "description": "Structured JSON result returned by this tool. Field-level schemas are intentionally broad for now and should be tightened as the MCP surface stabilizes."
    })
}

/// Execute a tool by name with the given arguments. Returns the result as JSON.
pub fn execute_tool(name: &str, params: &Value) -> Result<Value, ToolError> {
    match name {
        "set_effect" => effects::handle_set_effect(params),
        "list_effects" => effects::handle_list_effects(params),
        "stop_effect" => effects::handle_stop_effect(params),
        "set_color" => effects::handle_set_color(params),
        "get_devices" => devices::handle_get_devices(params),
        "set_brightness" => devices::handle_set_brightness(params),
        "get_status" => system::handle_get_status(params),
        "activate_scene" => scenes::handle_activate_scene(params),
        "list_scenes" => scenes::handle_list_scenes(params),
        "create_scene" => scenes::handle_create_scene(params),
        "get_audio_state" => system::handle_get_audio_state(params),
        "get_sensor_data" => system::handle_get_sensor_data(params),
        "list_display_overlays" => overlays::handle_list_display_overlays(params),
        "set_display_overlay" => overlays::handle_set_display_overlay(params),
        "set_profile" => library::handle_set_profile(params),
        "get_layout" => system::handle_get_layout(params),
        "diagnose" => system::handle_diagnose(params),
        _ => Err(ToolError::NotFound(name.to_owned())),
    }
}

/// Execute a tool with live daemon state access.
pub async fn execute_tool_with_state(
    name: &str,
    params: &Value,
    state: &AppState,
) -> Result<Value, ToolError> {
    match name {
        "set_effect" => effects::handle_set_effect_with_state(params, state).await,
        "list_effects" => effects::handle_list_effects_with_state(params, state).await,
        "stop_effect" => effects::handle_stop_effect_with_state(params, state).await,
        "set_color" => effects::handle_set_color_with_state(params, state).await,
        "get_devices" => devices::handle_get_devices_with_state(params, state).await,
        "set_brightness" => devices::handle_set_brightness_with_state(params, state).await,
        "get_status" => system::handle_get_status_with_state(state).await,
        "activate_scene" => scenes::handle_activate_scene_with_state(params, state).await,
        "list_scenes" => scenes::handle_list_scenes_with_state(params, state).await,
        "create_scene" => scenes::handle_create_scene_with_state(params, state).await,
        "get_audio_state" => Ok(system::handle_get_audio_state_with_state(state)),
        "get_sensor_data" => system::handle_get_sensor_data_with_state(params, state).await,
        "list_display_overlays" => {
            overlays::handle_list_display_overlays_with_state(params, state).await
        }
        "set_display_overlay" => {
            overlays::handle_set_display_overlay_with_state(params, state).await
        }
        "set_profile" => library::handle_set_profile_with_state(params, state).await,
        "get_layout" => system::handle_get_layout_with_state(state).await,
        "diagnose" => system::handle_diagnose_with_state(params, state).await,
        _ => Err(ToolError::NotFound(name.to_owned())),
    }
}

/// Errors that can occur during tool execution.
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    /// Tool name not recognized.
    #[error("tool not found: {0}")]
    NotFound(String),
    /// Required parameter missing.
    #[error("missing required parameter: {0}")]
    MissingParam(String),
    /// Parameter has wrong type or invalid value.
    #[error("invalid parameter '{param}': {reason}")]
    InvalidParam {
        /// Parameter name.
        param: String,
        /// What was wrong with it.
        reason: String,
    },
    /// Internal execution error.
    #[error("execution error: {0}")]
    Internal(String),
}

impl ToolError {
    /// JSON-RPC error code for this error type.
    pub const fn error_code(&self) -> i64 {
        match self {
            Self::NotFound(_) => -32601, // Method not found
            Self::MissingParam(_) | Self::InvalidParam { .. } => -32602, // Invalid params
            Self::Internal(_) => -32603, // Internal error
        }
    }
}

pub(super) async fn find_effect_metadata(
    state: &AppState,
    primary_name: &str,
    fallback_name: &str,
) -> Option<hypercolor_types::effect::EffectMetadata> {
    let registry = state.effect_registry.read().await;
    registry
        .iter()
        .map(|(_, entry)| entry.metadata.clone())
        .find(|metadata| {
            metadata.name.eq_ignore_ascii_case(primary_name)
                || metadata.name.eq_ignore_ascii_case(fallback_name)
        })
}

pub(super) fn apply_controls(
    engine: &mut hypercolor_core::effect::EffectEngine,
    controls: &Map<String, Value>,
) {
    for (name, value) in controls {
        if let Some(control_value) = control_value_from_json(value) {
            engine.set_control(name, &control_value);
        }
    }
}

/// Convert a 0.0–1.0 brightness float to a 0–100 percentage.
pub(crate) fn brightness_percent(brightness: f32) -> u8 {
    let scaled = (brightness.clamp(0.0, 1.0) * 100.0).round();
    if scaled <= 0.0 {
        0
    } else if scaled >= 100.0 {
        100
    } else {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::as_conversions
        )]
        let result = scaled as u8;
        result
    }
}

/// Compute actual delivery FPS, capped at the target tier rate.
///
/// The EWMA frame time measures render *work* only (excluding the sleep between
/// frames), so `1/avg_frame_time` gives theoretical throughput. The real delivery
/// rate is bounded by the FPS tier.
pub(crate) fn capped_fps(stats: &hypercolor_core::engine::RenderLoopStats) -> f32 {
    let avg_secs = stats.avg_frame_time.as_secs_f32();
    if avg_secs <= 0.0 {
        return 0.0;
    }
    let throughput = 1.0 / avg_secs;
    #[expect(clippy::cast_precision_loss, clippy::as_conversions)]
    let target = stats.tier.fps() as f32;
    throughput.min(target)
}

fn control_value_from_json(value: &Value) -> Option<ControlValue> {
    if let Some(flag) = value.as_bool() {
        return Some(ControlValue::Boolean(flag));
    }

    if let Some(integer_value) = value.as_i64() {
        let coerced = i32::try_from(integer_value).ok()?;
        return Some(ControlValue::Integer(coerced));
    }

    if let Some(float_value) = value.as_f64() {
        let finite = if float_value.is_finite() {
            float_value
        } else {
            return None;
        };
        #[expect(clippy::cast_possible_truncation, clippy::as_conversions)]
        let coerced = finite as f32;
        return Some(ControlValue::Float(coerced));
    }

    if let Some(text) = value.as_str() {
        return Some(ControlValue::Text(text.to_owned()));
    }

    if let Some(array) = value.as_array()
        && array.len() == 4
    {
        let mut rgba = [0.0_f32; 4];
        for (idx, component) in array.iter().enumerate() {
            let number = component.as_f64()?;
            #[expect(clippy::cast_possible_truncation, clippy::as_conversions)]
            let number = number as f32;
            rgba[idx] = number;
        }
        return Some(ControlValue::Color(rgba));
    }

    None
}
