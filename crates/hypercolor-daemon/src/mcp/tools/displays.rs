//! Display-face MCP tool.

use serde_json::{Value, json};

use super::{ToolDefinition, ToolError, default_output_schema};
use crate::api::displays::{DisplaySurfaceInfo, display_face_layout, display_surface_info};
use crate::api::effects::resolve_effect_metadata;
use crate::api::{AppState, publish_render_group_changed, save_runtime_session_snapshot};
use hypercolor_types::device::{DeviceId, DeviceInfo};
use hypercolor_types::effect::{ControlValue, EffectCategory};
use hypercolor_types::event::RenderGroupChangeKind;

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
            let active_scene_id = crate::api::active_scene_id_for_runtime_mutation(&scene_manager)
                .map_err(|error| ToolError::Conflict(error.message("removing a display face")))?;
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
        let active_scene_id = crate::api::active_scene_id_for_runtime_mutation(&scene_manager)
            .map_err(|error| ToolError::Conflict(error.message("assigning a display face")))?;
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
                display_face_layout(device_id, info.name.as_str(), surface),
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

async fn resolve_display_device(
    state: &AppState,
    raw: &str,
) -> Result<(DeviceId, DeviceInfo, DisplaySurfaceInfo), ToolError> {
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
                    "device does not support display faces: {}",
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
                "device does not support display faces: {}",
                device.info.name
            ),
        });
    };
    Ok((device.info.id, device.info, surface))
}

fn display_device_payload(info: &DeviceInfo, surface: DisplaySurfaceInfo) -> Value {
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
