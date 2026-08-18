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
    let status_filter = params
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("all");
    let driver_filter = params
        .get("driver_id")
        .and_then(Value::as_str)
        .map(str::to_ascii_lowercase);
    let backend_filter = params
        .get("backend_id")
        .and_then(Value::as_str)
        .map(str::to_ascii_lowercase);

    let devices = state.device_registry.list().await;
    let filtered = devices
        .into_iter()
        .filter(|device| match status_filter {
            "connected" => device.state.is_renderable(),
            "disconnected" => !device.state.is_renderable(),
            _ => true,
        })
        .filter(|device| {
            driver_filter
                .as_deref()
                .is_none_or(|expected| device.info.driver_id().to_ascii_lowercase() == expected)
        })
        .filter(|device| {
            backend_filter.as_deref().is_none_or(|expected| {
                device.info.output_backend_id().to_ascii_lowercase() == expected
            })
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
            crate::mcp::device_payload::inventory_device_payload(state, &device.info, &device.state)
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
