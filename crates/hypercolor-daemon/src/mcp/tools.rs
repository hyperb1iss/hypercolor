//! MCP tool definitions — the 14 tools exposed to AI assistants.
//!
//! Each tool is a `ToolDefinition` with a JSON Schema input spec. Tool execution
//! is handled by `execute_tool`, which dispatches to the appropriate handler.

use std::cmp::min;

use serde_json::{Map, Value, json};

use crate::api::AppState;
use crate::session::{current_global_brightness, set_global_brightness};
use hypercolor_core::effect::create_renderer_for_metadata_with_mode;
use hypercolor_core::scene::make_scene;
use hypercolor_types::effect::ControlValue;
use hypercolor_types::event::{ChangeTrigger, EffectRef, EffectStopReason, HypercolorEvent};
use hypercolor_types::scene::TransitionSpec;

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

/// Build all 14 MCP tool definitions.
pub fn build_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        build_set_effect(),
        build_list_effects(),
        build_stop_effect(),
        build_set_color(),
        build_get_devices(),
        build_set_brightness(),
        build_get_status(),
        build_activate_scene(),
        build_list_scenes(),
        build_create_scene(),
        build_get_audio_state(),
        build_set_profile(),
        build_get_layout(),
        build_diagnose(),
    ]
}

fn default_output_schema() -> Value {
    json!({
        "type": "object",
        "description": "Structured JSON result returned by this tool. Field-level schemas are intentionally broad for now and should be tightened as the MCP surface stabilizes."
    })
}

/// Execute a tool by name with the given arguments. Returns the result as JSON.
pub fn execute_tool(name: &str, params: &Value) -> Result<Value, ToolError> {
    match name {
        "set_effect" => handle_set_effect(params),
        "list_effects" => handle_list_effects(params),
        "stop_effect" => handle_stop_effect(params),
        "set_color" => handle_set_color(params),
        "get_devices" => handle_get_devices(params),
        "set_brightness" => handle_set_brightness(params),
        "get_status" => handle_get_status(params),
        "activate_scene" => handle_activate_scene(params),
        "list_scenes" => handle_list_scenes(params),
        "create_scene" => handle_create_scene(params),
        "get_audio_state" => handle_get_audio_state(params),
        "set_profile" => handle_set_profile(params),
        "get_layout" => handle_get_layout(params),
        "diagnose" => handle_diagnose(params),
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
        "set_effect" => handle_set_effect_with_state(params, state).await,
        "list_effects" => handle_list_effects_with_state(params, state).await,
        "stop_effect" => handle_stop_effect_with_state(params, state).await,
        "set_color" => handle_set_color_with_state(params, state).await,
        "get_devices" => handle_get_devices_with_state(params, state).await,
        "set_brightness" => handle_set_brightness_with_state(params, state).await,
        "get_status" => handle_get_status_with_state(state).await,
        "activate_scene" => handle_activate_scene_with_state(params, state).await,
        "list_scenes" => handle_list_scenes_with_state(params, state).await,
        "create_scene" => handle_create_scene_with_state(params, state).await,
        "get_audio_state" => Ok(handle_get_audio_state_with_state(state)),
        "set_profile" => handle_set_profile_with_state(params, state).await,
        "get_layout" => handle_get_layout_with_state(state).await,
        "diagnose" => handle_diagnose_with_state(params, state).await,
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

// ── Tool Definitions ──────────────────────────────────────────────────────

fn build_set_effect() -> ToolDefinition {
    ToolDefinition {
        name: "set_effect".into(),
        title: "Set Lighting Effect".into(),
        description: "Apply a lighting effect to the RGB setup. Accepts exact effect names, partial matches, or natural language descriptions of the desired visual (e.g., 'aurora', 'something with northern lights', 'calm blue waves'). Returns the matched effect and confidence score. Use list_effects first if unsure what's available.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Effect name or natural language description of the desired lighting"
                },
                "controls": {
                    "type": "object",
                    "description": "Optional effect parameter overrides as key-value pairs",
                    "additionalProperties": true
                },
                "transition_ms": {
                    "type": "integer",
                    "description": "Crossfade transition duration in milliseconds (0 = instant)",
                    "default": 500,
                    "minimum": 0,
                    "maximum": 10000
                },
                "devices": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional list of device IDs to target. Omit to apply to all devices."
                }
            },
            "required": ["query"]
        }),
        output_schema: default_output_schema(),
        read_only: false,
        idempotent: true,
    }
}

fn build_list_effects() -> ToolDefinition {
    ToolDefinition {
        name: "list_effects".into(),
        title: "List Available Effects".into(),
        description: "Browse the lighting effect library. Returns effect names, descriptions, categories, and available control parameters. Use category and audio_reactive filters to narrow results.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "category": {
                    "type": "string",
                    "enum": ["ambient", "reactive", "audio", "gaming", "productivity", "utility", "interactive", "generative"],
                    "description": "Filter by effect category"
                },
                "audio_reactive": {
                    "type": "boolean",
                    "description": "Filter to only audio-reactive effects"
                },
                "query": {
                    "type": "string",
                    "description": "Full-text search across effect names, descriptions, and tags"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results to return",
                    "default": 20,
                    "minimum": 1,
                    "maximum": 100
                },
                "offset": {
                    "type": "integer",
                    "description": "Pagination offset",
                    "default": 0,
                    "minimum": 0
                }
            }
        }),
        output_schema: default_output_schema(),
        read_only: true,
        idempotent: true,
    }
}

fn build_stop_effect() -> ToolDefinition {
    ToolDefinition {
        name: "stop_effect".into(),
        title: "Stop Current Effect".into(),
        description: "Stop the currently running lighting effect. All LEDs will go dark unless a fallback is configured.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "transition_ms": {
                    "type": "integer",
                    "description": "Fade-out duration in milliseconds",
                    "default": 300,
                    "minimum": 0,
                    "maximum": 5000
                }
            }
        }),
        output_schema: default_output_schema(),
        read_only: false,
        idempotent: true,
    }
}

fn build_set_color() -> ToolDefinition {
    ToolDefinition {
        name: "set_color".into(),
        title: "Set Solid Color".into(),
        description: "Set a solid color on all or specific RGB devices. Accepts CSS color names ('coral', 'dodgerblue'), hex codes ('#ff6ac1'), RGB values ('rgb(255, 106, 193)'), HSL values ('hsl(330, 100%, 71%)'), or natural language descriptions ('warm sunset orange', 'deep ocean blue').".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "color": {
                    "type": "string",
                    "description": "Color specification: name, hex, rgb(), hsl(), or natural language description"
                },
                "brightness": {
                    "type": "integer",
                    "description": "Optional brightness override (0-100)",
                    "minimum": 0,
                    "maximum": 100
                },
                "transition_ms": {
                    "type": "integer",
                    "description": "Crossfade transition duration in milliseconds",
                    "default": 500,
                    "minimum": 0,
                    "maximum": 10000
                },
                "devices": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional list of device IDs. Omit to apply to all devices."
                }
            },
            "required": ["color"]
        }),
        output_schema: default_output_schema(),
        read_only: false,
        idempotent: true,
    }
}

fn build_get_devices() -> ToolDefinition {
    ToolDefinition {
        name: "get_devices".into(),
        title: "List RGB Devices".into(),
        description: "Enumerate all known RGB devices with their connection status, backend type, LED count, and zone configuration.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "status": {
                    "type": "string",
                    "enum": ["all", "connected", "disconnected"],
                    "default": "all",
                    "description": "Filter by connection status"
                }
            }
        }),
        output_schema: default_output_schema(),
        read_only: true,
        idempotent: true,
    }
}

fn build_set_brightness() -> ToolDefinition {
    ToolDefinition {
        name: "set_brightness".into(),
        title: "Set Brightness".into(),
        description: "Set the brightness level globally or for specific devices. Brightness is a percentage from 0 (off/dark) to 100 (maximum).".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "brightness": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 100,
                    "description": "Brightness percentage (0 = off, 100 = full brightness)"
                },
                "device_id": {
                    "type": "string",
                    "description": "Optional device ID for per-device brightness"
                },
                "transition_ms": {
                    "type": "integer",
                    "description": "Fade transition duration in milliseconds",
                    "default": 300,
                    "minimum": 0,
                    "maximum": 5000
                }
            },
            "required": ["brightness"]
        }),
        output_schema: default_output_schema(),
        read_only: false,
        idempotent: true,
    }
}

fn build_get_status() -> ToolDefinition {
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

fn build_activate_scene() -> ToolDefinition {
    ToolDefinition {
        name: "activate_scene".into(),
        title: "Activate Scene".into(),
        description: "Activate a named lighting scene. Scenes combine effects, device assignments, brightness, and transitions into a single preset.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Scene name or fuzzy query to match against"
                },
                "transition_ms": {
                    "type": "integer",
                    "description": "Crossfade transition duration in milliseconds",
                    "default": 1000,
                    "minimum": 0,
                    "maximum": 10000
                }
            },
            "required": ["name"]
        }),
        output_schema: default_output_schema(),
        read_only: false,
        idempotent: true,
    }
}

fn build_list_scenes() -> ToolDefinition {
    ToolDefinition {
        name: "list_scenes".into(),
        title: "List Scenes".into(),
        description: "List all available lighting scenes with their names, descriptions, and trigger configurations.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "enabled_only": {
                    "type": "boolean",
                    "description": "Only show enabled scenes",
                    "default": false
                }
            }
        }),
        output_schema: default_output_schema(),
        read_only: true,
        idempotent: true,
    }
}

fn build_create_scene() -> ToolDefinition {
    ToolDefinition {
        name: "create_scene".into(),
        title: "Create Scene".into(),
        description:
            "Create a new lighting scene from the current state or a specified configuration."
                .into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Human-readable scene name"
                },
                "description": {
                    "type": "string",
                    "description": "What this scene does"
                },
                "profile_id": {
                    "type": "string",
                    "description": "Profile ID to associate with this scene"
                },
                "trigger": {
                    "type": "object",
                    "description": "Trigger configuration",
                    "properties": {
                        "type": {
                            "type": "string",
                            "enum": ["schedule", "sunset", "sunrise", "device_connect", "device_disconnect", "audio_beat", "webhook"],
                            "description": "Trigger type"
                        },
                        "cron": {
                            "type": "string",
                            "description": "Cron expression for schedule triggers"
                        }
                    },
                    "required": ["type"]
                },
                "transition_ms": {
                    "type": "integer",
                    "description": "Crossfade duration when activated",
                    "default": 1000,
                    "minimum": 0,
                    "maximum": 30000
                },
                "enabled": {
                    "type": "boolean",
                    "description": "Whether the scene is active immediately",
                    "default": true
                }
            },
            "required": ["name", "profile_id", "trigger"]
        }),
        output_schema: default_output_schema(),
        read_only: false,
        idempotent: false,
    }
}

fn build_get_audio_state() -> ToolDefinition {
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

fn build_set_profile() -> ToolDefinition {
    ToolDefinition {
        name: "set_profile".into(),
        title: "Apply Lighting Profile".into(),
        description: "Activate a saved lighting profile by name or fuzzy query. Profiles capture the complete lighting state: effect, control parameters, device selection, and brightness.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Profile name or description to search for"
                },
                "transition_ms": {
                    "type": "integer",
                    "description": "Crossfade transition duration in milliseconds",
                    "default": 1000,
                    "minimum": 0,
                    "maximum": 10000
                }
            },
            "required": ["query"]
        }),
        output_schema: default_output_schema(),
        read_only: false,
        idempotent: true,
    }
}

fn build_get_layout() -> ToolDefinition {
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

fn build_diagnose() -> ToolDefinition {
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

// ── Tool Handlers ─────────────────────────────────────────────────────────

fn handle_set_effect(params: &Value) -> Result<Value, ToolError> {
    let query = params
        .get("query")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::MissingParam("query".into()))?;

    let transition_ms = params
        .get("transition_ms")
        .and_then(Value::as_u64)
        .unwrap_or(500);

    // In a full implementation this would query the effect registry and apply.
    // For now, we use the fuzzy matcher to demonstrate the pipeline.
    let effects = Vec::new(); // Would come from DaemonState
    let matches = super::fuzzy::match_effect(query, &effects);

    if let Some(best) = matches.first() {
        Ok(json!({
            "matched_effect": {
                "id": best.effect.id.to_string(),
                "name": best.effect.name,
                "description": best.effect.description,
                "category": format!("{}", best.effect.category)
            },
            "confidence": best.score,
            "alternatives": matches.iter().skip(1).take(5).map(|m| json!({
                "id": m.effect.id.to_string(),
                "name": m.effect.name,
                "score": m.score
            })).collect::<Vec<_>>(),
            "applied": true,
            "transition_ms": transition_ms
        }))
    } else {
        Ok(json!({
            "matched_effect": null,
            "confidence": 0.0,
            "alternatives": [],
            "applied": false,
            "message": format!("No effects matching '{query}' found. Use list_effects to browse available effects.")
        }))
    }
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "will return errors when wired to effect registry"
)]
fn handle_list_effects(params: &Value) -> Result<Value, ToolError> {
    let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(20);
    let offset = params.get("offset").and_then(Value::as_u64).unwrap_or(0);
    let _category = params.get("category").and_then(Value::as_str);
    let _query = params.get("query").and_then(Value::as_str);

    // Would query the effect registry with filters applied
    Ok(json!({
        "effects": [],
        "total": 0,
        "has_more": false,
        "limit": limit,
        "offset": offset
    }))
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "will return errors when wired to engine"
)]
fn handle_stop_effect(params: &Value) -> Result<Value, ToolError> {
    let transition_ms = params
        .get("transition_ms")
        .and_then(Value::as_u64)
        .unwrap_or(300);

    // Would send stop command via the event bus
    Ok(json!({
        "stopped": true,
        "transition_ms": transition_ms
    }))
}

fn handle_set_color(params: &Value) -> Result<Value, ToolError> {
    let color_str = params
        .get("color")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::MissingParam("color".into()))?;

    let resolved =
        super::fuzzy::resolve_color(color_str).ok_or_else(|| ToolError::InvalidParam {
            param: "color".into(),
            reason: format!("could not resolve color: '{color_str}'"),
        })?;

    Ok(json!({
        "resolved_color": {
            "hex": resolved.hex,
            "name": resolved.name,
            "rgb": {
                "r": resolved.r,
                "g": resolved.g,
                "b": resolved.b
            }
        },
        "applied": true,
        "device_count": 0
    }))
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "will return errors when wired to device manager"
)]
fn handle_get_devices(params: &Value) -> Result<Value, ToolError> {
    let _status = params
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("all");

    // Would query device manager
    Ok(json!({
        "devices": [],
        "summary": {
            "total": 0,
            "connected": 0,
            "total_leds": 0
        }
    }))
}

fn handle_set_brightness(params: &Value) -> Result<Value, ToolError> {
    let brightness = params
        .get("brightness")
        .and_then(Value::as_u64)
        .ok_or_else(|| ToolError::MissingParam("brightness".into()))?;

    if brightness > 100 {
        return Err(ToolError::InvalidParam {
            param: "brightness".into(),
            reason: "must be between 0 and 100".into(),
        });
    }

    let device_id = params.get("device_id").and_then(Value::as_str);
    let scope = if device_id.is_some() {
        "device"
    } else {
        "global"
    };

    Ok(json!({
        "brightness": brightness,
        "scope": scope,
        "device_id": device_id,
        "previous_brightness": 100
    }))
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "will return errors when wired to daemon state"
)]
fn handle_get_status(_params: &Value) -> Result<Value, ToolError> {
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

fn handle_activate_scene(params: &Value) -> Result<Value, ToolError> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::MissingParam("name".into()))?;

    let _transition_ms = params
        .get("transition_ms")
        .and_then(Value::as_u64)
        .unwrap_or(1000);

    // Would query scene manager with fuzzy matching
    Ok(json!({
        "activated": false,
        "message": format!("No scene matching '{name}' found. Use list_scenes to browse available scenes.")
    }))
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "will return errors when wired to scene manager"
)]
fn handle_list_scenes(params: &Value) -> Result<Value, ToolError> {
    let _enabled_only = params
        .get("enabled_only")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    // Would query scene manager
    Ok(json!({
        "scenes": [],
        "total": 0
    }))
}

fn handle_create_scene(params: &Value) -> Result<Value, ToolError> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::MissingParam("name".into()))?;

    let _profile_id = params
        .get("profile_id")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::MissingParam("profile_id".into()))?;

    let trigger = params
        .get("trigger")
        .ok_or_else(|| ToolError::MissingParam("trigger".into()))?;

    let _trigger_type = trigger
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::MissingParam("trigger.type".into()))?;

    let enabled = params
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(true);

    let scene_id = uuid::Uuid::now_v7().to_string();

    Ok(json!({
        "scene_id": scene_id,
        "name": name,
        "enabled": enabled
    }))
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "will return errors when wired to audio state"
)]
fn handle_get_audio_state(_params: &Value) -> Result<Value, ToolError> {
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

fn handle_set_profile(params: &Value) -> Result<Value, ToolError> {
    let query = params
        .get("query")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::MissingParam("query".into()))?;

    let _transition_ms = params
        .get("transition_ms")
        .and_then(Value::as_u64)
        .unwrap_or(1000);

    // Would query profile manager with fuzzy matching
    Ok(json!({
        "profile": null,
        "applied": false,
        "message": format!("No profile matching '{query}' found.")
    }))
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "will return errors when wired to layout state"
)]
fn handle_get_layout(_params: &Value) -> Result<Value, ToolError> {
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
fn handle_diagnose(params: &Value) -> Result<Value, ToolError> {
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

async fn handle_set_effect_with_state(
    params: &Value,
    state: &AppState,
) -> Result<Value, ToolError> {
    let query = params
        .get("query")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::MissingParam("query".into()))?;

    let transition_ms = params
        .get("transition_ms")
        .and_then(Value::as_u64)
        .unwrap_or(500);

    let effect_catalog = {
        let registry = state.effect_registry.read().await;
        registry
            .iter()
            .map(|(_, entry)| entry.metadata.clone())
            .collect::<Vec<_>>()
    };

    let matches = super::fuzzy::match_effect(query, &effect_catalog);
    let Some(best_match) = matches.first() else {
        return Ok(json!({
            "matched_effect": null,
            "confidence": 0.0,
            "alternatives": [],
            "applied": false,
            "message": format!("No effects matching '{query}' found. Use list_effects to browse available effects.")
        }));
    };

    let previous_effect = {
        let requested_mode =
            crate::api::configured_render_acceleration_mode(state.config_manager.as_ref());
        let renderer = create_renderer_for_metadata_with_mode(&best_match.effect, requested_mode)
            .map_err(|error| {
            ToolError::Internal(format!("failed to prepare effect: {error}"))
        })?;
        let mut engine = state.effect_engine.lock().await;
        let previous = engine.active_metadata().map(|m| EffectRef {
            id: m.id.to_string(),
            name: m.name.clone(),
            engine: "servo".into(),
        });
        engine
            .activate(renderer, best_match.effect.clone())
            .map_err(|error| ToolError::Internal(format!("failed to activate effect: {error}")))?;

        if let Some(controls) = params.get("controls").and_then(Value::as_object) {
            apply_controls(&mut engine, controls);
        }
        previous
    };

    state.event_bus.publish(HypercolorEvent::EffectStarted {
        effect: EffectRef {
            id: best_match.effect.id.to_string(),
            name: best_match.effect.name.clone(),
            engine: "servo".into(),
        },
        trigger: ChangeTrigger::Api,
        previous: previous_effect,
        transition: None,
    });

    Ok(json!({
        "matched_effect": {
            "id": best_match.effect.id.to_string(),
            "name": best_match.effect.name,
            "description": best_match.effect.description,
            "category": format!("{}", best_match.effect.category)
        },
        "confidence": best_match.score,
        "alternatives": matches.iter().skip(1).take(5).map(|candidate| json!({
            "id": candidate.effect.id.to_string(),
            "name": candidate.effect.name,
            "score": candidate.score
        })).collect::<Vec<_>>(),
        "applied": true,
        "transition_ms": transition_ms
    }))
}

async fn handle_list_effects_with_state(
    params: &Value,
    state: &AppState,
) -> Result<Value, ToolError> {
    let limit_u64 = params.get("limit").and_then(Value::as_u64).unwrap_or(20);
    let offset_u64 = params.get("offset").and_then(Value::as_u64).unwrap_or(0);
    let category_filter = params.get("category").and_then(Value::as_str);
    let query_filter = params.get("query").and_then(Value::as_str);
    let audio_reactive_filter = params.get("audio_reactive").and_then(Value::as_bool);

    let effect_catalog = {
        let registry = state.effect_registry.read().await;
        registry
            .iter()
            .map(|(_, entry)| entry.metadata.clone())
            .collect::<Vec<_>>()
    };

    let mut filtered = effect_catalog
        .into_iter()
        .filter(|metadata| {
            let category_ok = category_filter.is_none_or(|category| {
                format!("{}", metadata.category).eq_ignore_ascii_case(category)
            });
            let query_ok = query_filter.is_none_or(|query| {
                metadata.name.to_lowercase().contains(&query.to_lowercase())
                    || metadata
                        .description
                        .to_lowercase()
                        .contains(&query.to_lowercase())
                    || metadata
                        .tags
                        .iter()
                        .any(|tag| tag.to_lowercase().contains(&query.to_lowercase()))
            });
            let is_audio_reactive = metadata.audio_reactive
                || metadata
                    .tags
                    .iter()
                    .any(|tag| tag.eq_ignore_ascii_case("audio-reactive"))
                || matches!(
                    metadata.category,
                    hypercolor_types::effect::EffectCategory::Audio
                );
            let audio_ok =
                audio_reactive_filter.is_none_or(|required| required == is_audio_reactive);
            category_ok && query_ok && audio_ok
        })
        .collect::<Vec<_>>();

    filtered.sort_by(|left, right| left.name.to_lowercase().cmp(&right.name.to_lowercase()));

    let total = filtered.len();
    let limit = usize::try_from(limit_u64).unwrap_or(20);
    let offset = usize::try_from(offset_u64).unwrap_or_default();
    let start = min(offset, total);
    let end = min(start.saturating_add(limit), total);

    let effects = filtered[start..end]
        .iter()
        .map(|metadata| {
            let audio_reactive = metadata.audio_reactive
                || metadata
                    .tags
                    .iter()
                    .any(|tag| tag.eq_ignore_ascii_case("audio-reactive"))
                || matches!(
                    metadata.category,
                    hypercolor_types::effect::EffectCategory::Audio
                );
            json!({
                "id": metadata.id.to_string(),
                "name": metadata.name,
                "description": metadata.description,
                "category": format!("{}", metadata.category),
                "audio_reactive": audio_reactive,
                "tags": metadata.tags,
                "controls": metadata.controls.iter().map(|control| json!({
                    "id": control.control_id(),
                    "name": control.name,
                    "kind": control.kind,
                    "default": control.default_value,
                    "min": control.min,
                    "max": control.max,
                    "step": control.step,
                    "options": control.labels,
                    "tooltip": control.tooltip,
                })).collect::<Vec<_>>()
            })
        })
        .collect::<Vec<_>>();

    Ok(json!({
        "effects": effects,
        "total": total,
        "has_more": end < total,
        "limit": limit_u64,
        "offset": offset_u64
    }))
}

async fn handle_stop_effect_with_state(
    params: &Value,
    state: &AppState,
) -> Result<Value, ToolError> {
    let transition_ms = params
        .get("transition_ms")
        .and_then(Value::as_u64)
        .unwrap_or(300);

    let stopped_effect = {
        let mut engine = state.effect_engine.lock().await;
        let previous = engine.active_metadata().cloned();
        engine.deactivate();
        previous
    };

    if let Some(ref metadata) = stopped_effect {
        state.event_bus.publish(HypercolorEvent::EffectStopped {
            effect: EffectRef {
                id: metadata.id.to_string(),
                name: metadata.name.clone(),
                engine: "servo".into(),
            },
            reason: EffectStopReason::Stopped,
        });
    }

    Ok(json!({
        "stopped": stopped_effect.is_some(),
        "transition_ms": transition_ms,
        "effect": stopped_effect.map(|metadata| json!({
            "id": metadata.id.to_string(),
            "name": metadata.name
        }))
    }))
}

async fn handle_set_color_with_state(params: &Value, state: &AppState) -> Result<Value, ToolError> {
    let color_str = params
        .get("color")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::MissingParam("color".into()))?;

    let resolved =
        super::fuzzy::resolve_color(color_str).ok_or_else(|| ToolError::InvalidParam {
            param: "color".into(),
            reason: format!("could not resolve color: '{color_str}'"),
        })?;

    let solid_effect = find_effect_metadata(state, "solid_color", "Solid Color")
        .await
        .ok_or_else(|| ToolError::Internal("solid color effect is not registered".into()))?;

    let previous_effect = {
        let requested_mode =
            crate::api::configured_render_acceleration_mode(state.config_manager.as_ref());
        let renderer = create_renderer_for_metadata_with_mode(&solid_effect, requested_mode)
            .map_err(|error| {
                ToolError::Internal(format!("failed to prepare solid color: {error}"))
            })?;
        let mut engine = state.effect_engine.lock().await;
        let previous = engine.active_metadata().map(|m| EffectRef {
            id: m.id.to_string(),
            name: m.name.clone(),
            engine: "servo".into(),
        });
        engine
            .activate(renderer, solid_effect.clone())
            .map_err(|error| {
                ToolError::Internal(format!("failed to activate solid color: {error}"))
            })?;
        engine.set_control(
            "color",
            &ControlValue::Color([
                f32::from(resolved.r) / 255.0,
                f32::from(resolved.g) / 255.0,
                f32::from(resolved.b) / 255.0,
                1.0,
            ]),
        );

        if let Some(brightness_u64) = params.get("brightness").and_then(Value::as_u64) {
            if brightness_u64 > 100 {
                return Err(ToolError::InvalidParam {
                    param: "brightness".into(),
                    reason: "must be between 0 and 100".into(),
                });
            }
            let brightness_u16 = u16::try_from(brightness_u64).unwrap_or(100);
            let brightness = f32::from(brightness_u16) / 100.0;
            engine.set_control("brightness", &ControlValue::Float(brightness));
        }
        previous
    };

    state.event_bus.publish(HypercolorEvent::EffectStarted {
        effect: EffectRef {
            id: solid_effect.id.to_string(),
            name: solid_effect.name.clone(),
            engine: "servo".into(),
        },
        trigger: ChangeTrigger::Api,
        previous: previous_effect,
        transition: None,
    });

    let device_count = state.device_registry.len().await;
    Ok(json!({
        "resolved_color": {
            "hex": resolved.hex,
            "name": resolved.name,
            "rgb": {
                "r": resolved.r,
                "g": resolved.g,
                "b": resolved.b
            }
        },
        "applied": true,
        "device_count": device_count
    }))
}

async fn handle_get_devices_with_state(
    params: &Value,
    state: &AppState,
) -> Result<Value, ToolError> {
    let status_filter = params
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("all");

    let devices = state.device_registry.list().await;
    let filtered = devices
        .into_iter()
        .filter(|device| match status_filter {
            "connected" => device.state.is_renderable(),
            "disconnected" => !device.state.is_renderable(),
            _ => true,
        })
        .collect::<Vec<_>>();

    let connected = filtered
        .iter()
        .filter(|device| device.state.is_renderable())
        .count();
    let total_leds: u64 = filtered
        .iter()
        .map(|device| u64::from(device.info.total_led_count()))
        .sum();

    let payload = filtered
        .iter()
        .map(|device| {
            json!({
                "id": device.info.id.to_string(),
                "name": device.info.name,
                "vendor": device.info.vendor,
                "family": format!("{}", device.info.family),
                "connection_type": format!("{:?}", device.info.connection_type),
                "state": device.state.variant_name(),
                "led_count": device.info.total_led_count(),
                "zones": device.info.zones.len()
            })
        })
        .collect::<Vec<_>>();

    Ok(json!({
        "devices": payload,
        "summary": {
            "total": filtered.len(),
            "connected": connected,
            "total_leds": total_leds
        }
    }))
}

async fn handle_set_brightness_with_state(
    params: &Value,
    state: &AppState,
) -> Result<Value, ToolError> {
    let brightness = params
        .get("brightness")
        .and_then(Value::as_u64)
        .ok_or_else(|| ToolError::MissingParam("brightness".into()))?;

    if brightness > 100 {
        return Err(ToolError::InvalidParam {
            param: "brightness".into(),
            reason: "must be between 0 and 100".into(),
        });
    }

    let previous = brightness_percent(current_global_brightness(&state.power_state));

    let brightness_u16 = u16::try_from(brightness).unwrap_or(100);
    let normalized = f32::from(brightness_u16) / 100.0;

    set_global_brightness(&state.power_state, normalized);
    {
        let mut settings = state.device_settings.write().await;
        settings.set_global_brightness(normalized);
        if let Err(error) = settings.save() {
            tracing::warn!(%error, "Failed to persist global brightness");
        }
    }

    state.event_bus.publish(HypercolorEvent::BrightnessChanged {
        old: previous,
        new_value: brightness_percent(normalized),
    });

    let device_id = params.get("device_id").and_then(Value::as_str);
    let scope = if device_id.is_some() {
        "device"
    } else {
        "global"
    };

    Ok(json!({
        "brightness": brightness,
        "scope": scope,
        "device_id": device_id,
        "previous_brightness": previous
    }))
}

async fn handle_get_status_with_state(state: &AppState) -> Result<Value, ToolError> {
    let render_stats = {
        let render_loop = state.render_loop.read().await;
        render_loop.stats()
    };
    let target_fps = render_stats.tier.fps();
    let actual_fps = capped_fps(&render_stats);

    let brightness = brightness_percent(current_global_brightness(&state.power_state));

    let active_effect = {
        let engine = state.effect_engine.lock().await;
        engine.active_metadata().cloned()
    };

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

async fn handle_activate_scene_with_state(
    params: &Value,
    state: &AppState,
) -> Result<Value, ToolError> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::MissingParam("name".into()))?;

    let transition_ms = params
        .get("transition_ms")
        .and_then(Value::as_u64)
        .unwrap_or(1000);

    let mut scene_manager = state.scene_manager.write().await;
    let matched_scene = scene_manager
        .list()
        .into_iter()
        .find(|scene| {
            scene.name.eq_ignore_ascii_case(name)
                || scene.name.to_lowercase().contains(&name.to_lowercase())
        })
        .cloned();

    let Some(scene) = matched_scene else {
        return Ok(json!({
            "activated": false,
            "message": format!("No scene matching '{name}' found. Use list_scenes to browse available scenes.")
        }));
    };

    let transition_override = Some(TransitionSpec {
        duration_ms: transition_ms,
        ..scene.transition.clone()
    });
    scene_manager
        .activate(&scene.id, transition_override)
        .map_err(|error| ToolError::Internal(format!("failed to activate scene: {error}")))?;

    Ok(json!({
        "activated": true,
        "scene": {
            "id": scene.id.to_string(),
            "name": scene.name
        },
        "transition_ms": transition_ms
    }))
}

async fn handle_list_scenes_with_state(
    params: &Value,
    state: &AppState,
) -> Result<Value, ToolError> {
    let enabled_only = params
        .get("enabled_only")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let scene_manager = state.scene_manager.read().await;
    let active_scene_id = scene_manager.active_scene_id().copied();
    let scenes = scene_manager
        .list()
        .into_iter()
        .filter(|scene| !enabled_only || scene.enabled)
        .map(|scene| {
            json!({
                "id": scene.id.to_string(),
                "name": scene.name,
                "description": scene.description,
                "enabled": scene.enabled,
                "active": Some(scene.id) == active_scene_id
            })
        })
        .collect::<Vec<_>>();

    Ok(json!({
        "scenes": scenes,
        "total": scenes.len()
    }))
}

async fn handle_create_scene_with_state(
    params: &Value,
    state: &AppState,
) -> Result<Value, ToolError> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::MissingParam("name".into()))?;
    let profile_id = params
        .get("profile_id")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::MissingParam("profile_id".into()))?;

    {
        let profiles = state.profiles.read().await;
        if profiles.get(profile_id).is_none() {
            return Err(ToolError::InvalidParam {
                param: "profile_id".into(),
                reason: format!("profile '{profile_id}' not found"),
            });
        }
    }

    let trigger = params
        .get("trigger")
        .ok_or_else(|| ToolError::MissingParam("trigger".into()))?;
    let trigger_type = trigger
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::MissingParam("trigger.type".into()))?;

    let enabled = params
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(true);

    let mut scene = make_scene(name);
    scene.description = params
        .get("description")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    scene.enabled = enabled;
    scene
        .metadata
        .insert("profile_id".to_owned(), profile_id.to_owned());
    scene
        .metadata
        .insert("trigger_type".to_owned(), trigger_type.to_owned());

    let scene_id = scene.id.to_string();
    {
        let mut scene_manager = state.scene_manager.write().await;
        scene_manager
            .create(scene)
            .map_err(|error| ToolError::Internal(format!("failed to create scene: {error}")))?;
    }

    Ok(json!({
        "scene_id": scene_id,
        "name": name,
        "enabled": enabled
    }))
}

fn handle_get_audio_state_with_state(state: &AppState) -> Value {
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

async fn handle_set_profile_with_state(
    params: &Value,
    state: &AppState,
) -> Result<Value, ToolError> {
    let query = params
        .get("query")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::MissingParam("query".into()))?;

    let profiles = state.profiles.read().await;
    let matched = profiles
        .values()
        .find(|profile| {
            profile.name.eq_ignore_ascii_case(query)
                || profile.name.to_lowercase().contains(&query.to_lowercase())
        })
        .cloned();

    let Some(profile) = matched else {
        return Ok(json!({
            "profile": null,
            "applied": false,
            "message": format!("No profile matching '{query}' found.")
        }));
    };

    crate::api::profiles::apply_profile_snapshot(state, &profile)
        .await
        .map_err(ToolError::Internal)?;
    state.event_bus.publish(HypercolorEvent::ProfileLoaded {
        profile_id: profile.id.clone(),
        profile_name: profile.name.clone(),
        trigger: ChangeTrigger::Api,
    });

    Ok(json!({
        "profile": {
            "id": profile.id,
            "name": profile.name,
            "description": profile.description,
            "effect_id": profile.effect_id,
            "layout_id": profile.layout_id
        },
        "applied": true
    }))
}

async fn handle_get_layout_with_state(state: &AppState) -> Result<Value, ToolError> {
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

async fn handle_diagnose_with_state(_params: &Value, state: &AppState) -> Result<Value, ToolError> {
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

async fn find_effect_metadata(
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

fn apply_controls(
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
