//! Display overlay MCP tools: `list_display_overlays`, `set_display_overlay`.

use std::collections::HashSet;

use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::{ToolDefinition, ToolError, default_output_schema};
use crate::api::AppState;
use crate::api::displays::{
    CreateOverlaySlotRequest, OverlayRuntimeResponse, UpdateOverlaySlotRequest,
    current_overlay_config, display_surface_info, persist_overlay_config, validate_overlay_config,
};
use crate::display_overlays::OverlaySlotRuntime;
use hypercolor_types::device::{DeviceId, DeviceInfo};
use hypercolor_types::overlay::{DisplayOverlayConfig, OverlaySlot, OverlaySlotId};

pub(super) fn build_list_display_overlays() -> ToolDefinition {
    ToolDefinition {
        name: "list_display_overlays".into(),
        title: "List Display Overlays".into(),
        description: "List overlay stacks for all display-capable devices, or inspect one display by ID or exact name. Returns each slot with runtime diagnostics so you can spot disabled or failed overlays.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "device": {
                    "type": "string",
                    "description": "Optional display device ID or exact display name."
                }
            },
            "additionalProperties": false
        }),
        output_schema: default_output_schema(),
        read_only: true,
        idempotent: true,
    }
}

pub(super) fn build_set_display_overlay() -> ToolDefinition {
    ToolDefinition {
        name: "set_display_overlay".into(),
        title: "Configure Display Overlays".into(),
        description: "Manage a display's overlay stack. Supports replace, append, update, delete, reorder, and clear operations using the same overlay JSON shapes as the REST API.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "device": {
                    "type": "string",
                    "description": "Display device ID or exact display name."
                },
                "operation": {
                    "type": "string",
                    "enum": ["replace", "append", "update", "delete", "reorder", "clear"],
                    "description": "Overlay operation to perform. Defaults are inferred from the supplied fields."
                },
                "config": {
                    "type": "object",
                    "description": "Full display overlay config used by the replace operation.",
                    "additionalProperties": true
                },
                "slot": {
                    "type": "object",
                    "description": "Overlay slot payload. For append, use the POST /displays/{id}/overlays body shape. For update, use the PATCH body shape.",
                    "additionalProperties": true
                },
                "slot_id": {
                    "type": "string",
                    "description": "Overlay slot ID used by update and delete."
                },
                "slot_ids": {
                    "type": "array",
                    "description": "Ordered overlay slot IDs used by reorder.",
                    "items": {
                        "type": "string"
                    }
                }
            },
            "required": ["device"],
            "additionalProperties": false
        }),
        output_schema: default_output_schema(),
        read_only: false,
        idempotent: true,
    }
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "stateless MCP mode returns a placeholder payload until daemon state is injected"
)]
pub(super) fn handle_list_display_overlays(_params: &Value) -> Result<Value, ToolError> {
    Ok(json!({
        "displays": [],
        "total": 0
    }))
}

pub(super) fn handle_set_display_overlay(params: &Value) -> Result<Value, ToolError> {
    let device = params
        .get("device")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::MissingParam("device".into()))?;

    Ok(json!({
        "device": device,
        "applied": false,
        "message": "Display overlay tools require live daemon state."
    }))
}

pub(super) async fn handle_list_display_overlays_with_state(
    params: &Value,
    state: &AppState,
) -> Result<Value, ToolError> {
    let displays = if let Some(raw) = params.get("device").and_then(Value::as_str) {
        let (device_id, info, surface) = resolve_display_device(state, raw).await?;
        vec![(device_id, info, surface)]
    } else {
        let mut displays = state
            .device_registry
            .list()
            .await
            .into_iter()
            .filter_map(|tracked| {
                let info = tracked.info;
                display_surface_info(&info).map(|surface| (info.id, info, surface))
            })
            .collect::<Vec<_>>();
        displays.sort_by(|left, right| {
            left.1
                .name
                .cmp(&right.1.name)
                .then_with(|| left.0.to_string().cmp(&right.0.to_string()))
        });
        displays
    };

    let mut payload = Vec::with_capacity(displays.len());
    for (device_id, info, surface) in displays {
        let config = current_overlay_config(state, device_id).await;
        let mut overlays = Vec::with_capacity(config.overlays.len());
        for slot in &config.overlays {
            overlays.push(overlay_slot_payload(state, device_id, slot).await);
        }
        payload.push(json!({
            "device": display_device_payload(&info, surface),
            "overlay_count": config.overlays.len(),
            "enabled_overlay_count": config.overlays.iter().filter(|slot| slot.enabled).count(),
            "overlays": overlays,
        }));
    }
    let total = payload.len();

    Ok(json!({
        "displays": payload,
        "total": total,
    }))
}

pub(super) async fn handle_set_display_overlay_with_state(
    params: &Value,
    state: &AppState,
) -> Result<Value, ToolError> {
    let raw_device = params
        .get("device")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::MissingParam("device".into()))?;
    let (device_id, info, surface) = resolve_display_device(state, raw_device).await?;

    let operation = inferred_operation(params)?;
    let mut config = current_overlay_config(state, device_id).await;
    let mut affected_slot: Option<OverlaySlot> = None;

    match operation {
        "replace" => {
            config = parse_json_param::<DisplayOverlayConfig>(params, "config")?.normalized();
        }
        "append" => {
            let request = parse_json_param::<CreateOverlaySlotRequest>(params, "slot")?;
            let slot = OverlaySlot {
                id: OverlaySlotId::generate(),
                name: request.name,
                source: request.source,
                position: request.position,
                blend_mode: request.blend_mode,
                opacity: request.opacity,
                enabled: request.enabled,
            }
            .normalized();
            config.overlays.push(slot.clone());
            affected_slot = Some(slot);
        }
        "update" => {
            let slot_id = required_slot_id(params, "slot_id")?;
            let request = parse_json_param::<UpdateOverlaySlotRequest>(params, "slot")?;
            let Some(slot_index) = config.overlays.iter().position(|slot| slot.id == slot_id)
            else {
                return Err(ToolError::InvalidParam {
                    param: "slot_id".into(),
                    reason: format!("overlay not found: {slot_id}"),
                });
            };

            let slot = &mut config.overlays[slot_index];
            if let Some(name) = request.name {
                slot.name = name;
            }
            if let Some(source) = request.source {
                slot.source = source;
            }
            if let Some(position) = request.position {
                slot.position = position;
            }
            if let Some(blend_mode) = request.blend_mode {
                slot.blend_mode = blend_mode;
            }
            if let Some(opacity) = request.opacity {
                slot.opacity = opacity;
            }
            if let Some(enabled) = request.enabled {
                slot.enabled = enabled;
            }

            let slot = slot.clone().normalized();
            config.overlays[slot_index] = slot.clone();
            affected_slot = Some(slot);
        }
        "delete" => {
            let slot_id = required_slot_id(params, "slot_id")?;
            let previous_len = config.overlays.len();
            config.overlays.retain(|slot| slot.id != slot_id);
            if config.overlays.len() == previous_len {
                return Err(ToolError::InvalidParam {
                    param: "slot_id".into(),
                    reason: format!("overlay not found: {slot_id}"),
                });
            }
        }
        "reorder" => {
            let slot_ids = required_slot_ids(params)?;
            if has_duplicate_slot_ids(&slot_ids) {
                return Err(ToolError::InvalidParam {
                    param: "slot_ids".into(),
                    reason: "slot_ids must not contain duplicates".into(),
                });
            }
            if slot_ids.len() != config.overlays.len() {
                return Err(ToolError::InvalidParam {
                    param: "slot_ids".into(),
                    reason: "slot_ids must include every configured overlay exactly once".into(),
                });
            }

            let mut reordered = Vec::with_capacity(config.overlays.len());
            for slot_id in &slot_ids {
                let Some(slot) = config.overlays.iter().find(|slot| &slot.id == slot_id) else {
                    return Err(ToolError::InvalidParam {
                        param: "slot_ids".into(),
                        reason: "slot_ids must match the configured overlay set".into(),
                    });
                };
                reordered.push(slot.clone());
            }
            config = DisplayOverlayConfig {
                overlays: reordered,
            }
            .normalized();
        }
        "clear" => {
            config = DisplayOverlayConfig::default();
        }
        _ => {
            return Err(ToolError::InvalidParam {
                param: "operation".into(),
                reason: format!("unsupported operation: {operation}"),
            });
        }
    }

    validate_overlay_config(state, &config)
        .await
        .map_err(|reason| ToolError::InvalidParam {
            param: operation.into(),
            reason,
        })?;
    persist_overlay_config(state, device_id, &config)
        .await
        .map_err(ToolError::Internal)?;

    let refreshed = current_overlay_config(state, device_id).await;
    let affected_runtime = match affected_slot.as_ref() {
        Some(slot) => Some(runtime_for_slot(state, device_id, slot).await),
        None => None,
    };

    Ok(json!({
        "device": display_device_payload(&info, surface),
        "operation": operation,
        "applied": true,
        "overlay_count": refreshed.overlays.len(),
        "enabled_overlay_count": refreshed.overlays.iter().filter(|slot| slot.enabled).count(),
        "config": refreshed,
        "affected_slot": affected_slot,
        "affected_runtime": affected_runtime,
    }))
}

fn inferred_operation(params: &Value) -> Result<&str, ToolError> {
    if let Some(operation) = params.get("operation").and_then(Value::as_str) {
        return Ok(operation);
    }

    if params.get("config").is_some() {
        return Ok("replace");
    }
    if params.get("slot_ids").is_some() {
        return Ok("reorder");
    }
    if params.get("slot_id").is_some() {
        return Ok("update");
    }
    if params.get("slot").is_some() {
        return Ok("append");
    }

    Err(ToolError::MissingParam("operation".into()))
}

fn parse_json_param<T: DeserializeOwned>(params: &Value, param: &str) -> Result<T, ToolError> {
    let value = params
        .get(param)
        .cloned()
        .ok_or_else(|| ToolError::MissingParam(param.into()))?;
    serde_json::from_value(value).map_err(|error| ToolError::InvalidParam {
        param: param.into(),
        reason: error.to_string(),
    })
}

fn required_slot_id(params: &Value, param: &str) -> Result<OverlaySlotId, ToolError> {
    let raw = params
        .get(param)
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::MissingParam(param.into()))?;
    raw.parse::<OverlaySlotId>()
        .map_err(|error| ToolError::InvalidParam {
            param: param.into(),
            reason: error.to_string(),
        })
}

fn required_slot_ids(params: &Value) -> Result<Vec<OverlaySlotId>, ToolError> {
    let slot_ids = params
        .get("slot_ids")
        .and_then(Value::as_array)
        .ok_or_else(|| ToolError::MissingParam("slot_ids".into()))?;
    slot_ids
        .iter()
        .map(|slot_id| {
            slot_id
                .as_str()
                .ok_or_else(|| ToolError::InvalidParam {
                    param: "slot_ids".into(),
                    reason: "slot_ids must be strings".into(),
                })
                .and_then(|slot_id| {
                    slot_id
                        .parse::<OverlaySlotId>()
                        .map_err(|error| ToolError::InvalidParam {
                            param: "slot_ids".into(),
                            reason: error.to_string(),
                        })
                })
        })
        .collect()
}

async fn resolve_display_device(
    state: &AppState,
    raw: &str,
) -> Result<
    (
        DeviceId,
        DeviceInfo,
        crate::api::displays::DisplaySurfaceInfo,
    ),
    ToolError,
> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(ToolError::InvalidParam {
            param: "device".into(),
            reason: "must not be empty".into(),
        });
    }

    if let Ok(device_id) = trimmed.parse::<DeviceId>() {
        let Some(device) = state.device_registry.get(&device_id).await else {
            return Err(ToolError::InvalidParam {
                param: "device".into(),
                reason: format!("display not found: {trimmed}"),
            });
        };
        let Some(surface) = display_surface_info(&device.info) else {
            return Err(ToolError::InvalidParam {
                param: "device".into(),
                reason: format!(
                    "device does not support display overlays: {}",
                    device.info.name
                ),
            });
        };
        return Ok((device_id, device.info, surface));
    }

    let matches = state
        .device_registry
        .list()
        .await
        .into_iter()
        .filter(|tracked| tracked.info.name.eq_ignore_ascii_case(trimmed))
        .collect::<Vec<_>>();
    if matches.is_empty() {
        return Err(ToolError::InvalidParam {
            param: "device".into(),
            reason: format!("display not found: {trimmed}"),
        });
    }
    if matches.len() > 1 {
        return Err(ToolError::InvalidParam {
            param: "device".into(),
            reason: format!("display name is ambiguous: {trimmed}"),
        });
    }

    let device = matches
        .into_iter()
        .next()
        .expect("match count already checked");
    let Some(surface) = display_surface_info(&device.info) else {
        return Err(ToolError::InvalidParam {
            param: "device".into(),
            reason: format!(
                "device does not support display overlays: {}",
                device.info.name
            ),
        });
    };
    Ok((device.info.id, device.info, surface))
}

async fn overlay_slot_payload(state: &AppState, device_id: DeviceId, slot: &OverlaySlot) -> Value {
    json!({
        "slot": slot,
        "runtime": runtime_for_slot(state, device_id, slot).await,
    })
}

fn display_device_payload(
    info: &DeviceInfo,
    surface: crate::api::displays::DisplaySurfaceInfo,
) -> Value {
    json!({
        "id": info.id.to_string(),
        "name": info.name.clone(),
        "vendor": info.vendor.clone(),
        "family": format!("{}", info.family),
        "width": surface.width,
        "height": surface.height,
        "circular": surface.circular,
    })
}

fn has_duplicate_slot_ids(slot_ids: &[OverlaySlotId]) -> bool {
    let mut seen = HashSet::with_capacity(slot_ids.len());
    slot_ids.iter().any(|slot_id| !seen.insert(*slot_id))
}

async fn runtime_for_slot(
    state: &AppState,
    device_id: DeviceId,
    slot: &OverlaySlot,
) -> OverlayRuntimeResponse {
    let runtime = state
        .display_overlay_runtime
        .get(device_id)
        .await
        .slot(slot.id)
        .cloned()
        .unwrap_or_else(|| OverlaySlotRuntime::from_slot(slot));
    OverlayRuntimeResponse::from(runtime)
}
