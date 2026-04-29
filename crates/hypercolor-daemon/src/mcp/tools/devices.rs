//! Device-related MCP tools: `get_devices`, `set_brightness`.

use serde_json::{Value, json};

use super::{ToolDefinition, ToolError, brightness_percent, default_output_schema};
use crate::api::AppState;
use crate::session::{current_global_brightness, set_global_brightness};
use hypercolor_types::event::HypercolorEvent;

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
            }
        }),
        output_schema: default_output_schema(),
        read_only: true,
        idempotent: true,
    }
}

pub(super) fn build_set_brightness() -> ToolDefinition {
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

// ── Stateless Handlers ────────────────────────────────────────────────────

#[expect(
    clippy::unnecessary_wraps,
    reason = "will return errors when wired to device manager"
)]
pub(super) fn handle_get_devices(params: &Value) -> Result<Value, ToolError> {
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

pub(super) fn handle_set_brightness(params: &Value) -> Result<Value, ToolError> {
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

// ── Stateful Handlers ─────────────────────────────────────────────────────

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
