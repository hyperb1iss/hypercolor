//! Display overlay MCP tools: `list_display_overlays`, `set_display_overlay`.

use std::collections::HashSet;

use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::{ToolDefinition, ToolError, default_output_schema};
use crate::api::displays::{
    CreateOverlaySlotRequest, OverlayRuntimeResponse, UpdateOverlaySlotRequest,
    current_overlay_config, display_surface_info, overlay_runtime_for_slot, persist_overlay_config,
    validate_overlay_config,
};
use crate::api::effects::resolve_effect_metadata;
use crate::api::{AppState, publish_render_group_changed, save_runtime_session_snapshot};
use hypercolor_types::device::{DeviceId, DeviceInfo};
use hypercolor_types::effect::{ControlValue, EffectCategory};
use hypercolor_types::event::RenderGroupChangeKind;
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

pub(super) fn build_set_display_face() -> ToolDefinition {
    ToolDefinition {
        name: "set_display_face".into(),
        title: "Assign Display Face".into(),
        description: "Assign or clear an HTML display-face effect on a display device by updating the active scene's display-target render group.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "device": {
                    "type": "string",
                    "description": "Display device ID or exact display name."
                },
                "effect_id": {
                    "type": "string",
                    "description": "Display-face effect UUID, exact name, or source stem. Omit when clearing."
                },
                "clear": {
                    "type": "boolean",
                    "description": "When true, removes the active scene's face assignment for the display."
                },
                "controls": {
                    "type": "object",
                    "description": "Optional control overrides to store on the display face group.",
                    "additionalProperties": true
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

pub(super) fn handle_set_display_face(params: &Value) -> Result<Value, ToolError> {
    let device = params
        .get("device")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::MissingParam("device".into()))?;

    Ok(json!({
        "device": device,
        "applied": false,
        "message": "Display face tools require live daemon state."
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

pub(super) async fn handle_set_display_face_with_state(
    params: &Value,
    state: &AppState,
) -> Result<Value, ToolError> {
    let raw_device = params
        .get("device")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::MissingParam("device".into()))?;
    let clear = params
        .get("clear")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let (device_id, info, surface) = resolve_display_device(state, raw_device).await?;
    let controls = parse_controls_map(params.get("controls"))?;

    if clear {
        let (active_scene_id, removed_group) = {
            let mut scene_manager = state.scene_manager.write().await;
            let active_scene_id = scene_manager
                .active_scene_id()
                .copied()
                .ok_or_else(|| ToolError::Internal("no active scene available".into()))?;
            let removed_group = scene_manager
                .active_scene()
                .and_then(|scene| scene.display_group_for(device_id))
                .cloned();
            let removed = scene_manager
                .remove_display_group(device_id)
                .map_err(|error| ToolError::Internal(error.to_string()))?;
            if !removed {
                return Err(ToolError::InvalidParam {
                    param: "device".into(),
                    reason: format!("no display face is assigned to {device_id}"),
                });
            }
            (active_scene_id, removed_group)
        };
        if let Some(removed_group) = removed_group.as_ref() {
            publish_render_group_changed(
                state,
                active_scene_id,
                removed_group,
                RenderGroupChangeKind::Removed,
            );
        }
        save_runtime_session_snapshot(state).await;
        return Ok(json!({
            "device": display_device_payload(&info, surface),
            "scene_id": active_scene_id.to_string(),
            "cleared": true,
        }));
    }

    let effect_lookup = params
        .get("effect_id")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::MissingParam("effect_id".into()))?;
    let effect = {
        let registry = state.effect_registry.read().await;
        let Some(effect) = resolve_effect_metadata(&registry, effect_lookup) else {
            return Err(ToolError::InvalidParam {
                param: "effect_id".into(),
                reason: format!("effect not found: {effect_lookup}"),
            });
        };
        if effect.category != EffectCategory::Display {
            return Err(ToolError::InvalidParam {
                param: "effect_id".into(),
                reason: format!("effect '{}' is not a display face", effect.name),
            });
        }
        if !matches!(
            effect.source,
            hypercolor_types::effect::EffectSource::Html { .. }
        ) {
            return Err(ToolError::InvalidParam {
                param: "effect_id".into(),
                reason: format!("effect '{}' is not an HTML display face", effect.name),
            });
        }
        effect
    };

    let (active_scene_id, group, change_kind) = {
        let mut scene_manager = state.scene_manager.write().await;
        let active_scene_id = scene_manager
            .active_scene_id()
            .copied()
            .ok_or_else(|| ToolError::Internal("no active scene available".into()))?;
        let change_kind = if scene_manager
            .active_scene()
            .and_then(|scene| scene.display_group_for(device_id))
            .is_some()
        {
            RenderGroupChangeKind::Updated
        } else {
            RenderGroupChangeKind::Created
        };
        let group = scene_manager
            .upsert_display_group(
                device_id,
                info.name.as_str(),
                &effect,
                controls,
                crate::api::displays::display_face_layout(device_id, info.name.as_str(), surface),
            )
            .map_err(|error| ToolError::Internal(error.to_string()))?
            .clone();
        (active_scene_id, group, change_kind)
    };
    publish_render_group_changed(state, active_scene_id, &group, change_kind);
    save_runtime_session_snapshot(state).await;

    Ok(json!({
        "device": display_device_payload(&info, surface),
        "scene_id": active_scene_id.to_string(),
        "effect": effect,
        "group": group,
        "cleared": false,
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

fn parse_controls_map(
    value: Option<&Value>,
) -> Result<std::collections::HashMap<String, ControlValue>, ToolError> {
    let Some(value) = value else {
        return Ok(std::collections::HashMap::new());
    };
    let Some(map) = value.as_object() else {
        return Err(ToolError::InvalidParam {
            param: "controls".into(),
            reason: "controls must be an object".into(),
        });
    };

    let mut controls = std::collections::HashMap::with_capacity(map.len());
    for (key, value) in map {
        let Some(control) = control_value_from_json(value) else {
            return Err(ToolError::InvalidParam {
                param: "controls".into(),
                reason: format!("unsupported control value for '{key}'"),
            });
        };
        controls.insert(key.clone(), control);
    }
    Ok(controls)
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
    OverlayRuntimeResponse::from(overlay_runtime_for_slot(state, device_id, slot).await)
}
