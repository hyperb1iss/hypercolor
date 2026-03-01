//! MCP tool definitions — the 14 tools exposed to AI assistants.
//!
//! Each tool is a `ToolDefinition` with a JSON Schema input spec. Tool execution
//! is handled by `execute_tool`, which dispatches to the appropriate handler.

use serde_json::{Value, json};

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
