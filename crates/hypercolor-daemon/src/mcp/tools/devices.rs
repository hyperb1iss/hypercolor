//! Device-related MCP tools: `get_devices`, `set_brightness`.

use serde_json::{Value, json};

use super::{ToolDefinition, ToolError, brightness_percent, default_output_schema};
use crate::api::AppState;
use crate::domain::output;
use crate::session::current_global_brightness;
use hypercolor_types::api::output::OutputPatchRequest;

// ── Tool Definitions ──────────────────────────────────────────────────────

pub(super) fn build_get_devices() -> ToolDefinition {
    ToolDefinition {
        name: "get_devices".into(),
        title: "List RGB Devices".into(),
        description: "Enumerate all known RGB devices with their connection status, driver origin, output backend, LED count, and zone configuration.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "status": {
                    "type": "string",
                    "enum": ["all", "connected", "disconnected"],
                    "default": "all",
                    "description": "Filter by connection status"
                },
                "driver_id": {
                    "type": "string",
                    "description": "Optional driver module id filter. Use ids reported by device origin metadata."
                },
                "backend_id": {
                    "type": "string",
                    "description": "Optional output backend id filter. Use ids reported by device origin metadata."
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

pub(super) fn build_set_brightness() -> ToolDefinition {
    ToolDefinition {
        name: "set_brightness".into(),
        title: "Set Brightness".into(),
        description: "Set the global brightness level. Brightness is a percentage from 0 (off/dark) to 100 (maximum), and the change is immediate.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "brightness": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 100,
                    "description": "Brightness percentage (0 = off, 100 = full brightness)"
                }
            },
            "required": ["brightness"],
            "additionalProperties": false
        }),
        output_schema: default_output_schema(),
        read_only: false,
        destructive: false,
        idempotent: true,
    }
}

// ── Handlers ──────────────────────────────────────────────────────────────

pub(super) async fn handle_get_devices_with_state(
    params: &Value,
    state: &AppState,
) -> Result<Value, ToolError> {
    let filter = crate::mcp::payload::DeviceInventoryFilter::from_params(params);
    Ok(crate::mcp::payload::build_device_inventory_payload(state, filter).await)
}

pub(super) async fn handle_set_brightness_with_state(
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

    let outcome = output::patch_output(
        state,
        OutputPatchRequest {
            power: None,
            brightness: Some(normalized),
        },
    )
    .await?;

    Ok(json!({
        "brightness": brightness_percent(outcome.brightness),
        "scope": "global",
        "previous_brightness": previous
    }))
}
