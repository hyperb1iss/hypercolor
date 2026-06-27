//! Display-face MCP tool.

use serde_json::{Value, json};

use super::{ToolDefinition, ToolError, default_output_schema};
use crate::api::displays::{DisplaySurfaceInfo, display_face_layout, display_surface_info};
use crate::api::effects::resolve_effect_metadata;
use crate::api::{AppState, publish_render_group_changed, save_runtime_session_snapshot};
use hypercolor_types::device::{DeviceId, DeviceInfo};
use hypercolor_types::effect::{ControlValue, EffectCategory};
use hypercolor_types::event::ZoneChangeKind;

pub(super) fn build_set_display_face() -> ToolDefinition {
    ToolDefinition {
        name: "set_display_face".into(),
        title: "Assign Display Face".into(),
        description: "Assign or clear an HTML display-face effect on a display device. Scope 'default' (the default) persists across scenes; scope 'scene' writes the active scene's display zone, which always wins over the default while that scene is active.".into(),
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
                    "description": "When true, removes the face assignment on the chosen scope."
                },
                "scope": {
                    "type": "string",
                    "enum": ["default", "scene"],
                    "description": "Assignment layer: 'default' persists across scenes (the default); 'scene' targets only the active scene."
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
    let scope = match params.get("scope").and_then(Value::as_str) {
        None | Some("default") => crate::api::displays::DisplayFaceScope::Default,
        Some("scene") => crate::api::displays::DisplayFaceScope::Scene,
        Some(other) => {
            return Err(ToolError::InvalidParam {
                param: "scope".into(),
                reason: format!("must be 'default' or 'scene', got '{other}'"),
            });
        }
    };
    let (device_id, info, surface) = resolve_display_device(state, raw_device).await?;
    let controls = parse_controls_map(params.get("controls"))?;

    if scope == crate::api::displays::DisplayFaceScope::Default {
        return handle_default_scope(state, params, device_id, &info, surface, clear, controls)
            .await;
    }

    if clear {
        let (active_scene_id, previous_group, cleared_group) = {
            let mut scene_manager = state.scene_manager.write().await;
            let active_scene_id = crate::api::active_scene_id_for_runtime_mutation(&scene_manager)
                .map_err(|error| ToolError::Conflict(error.message("removing a display face")))?;
            let previous_group = scene_manager
                .active_scene()
                .and_then(|scene| scene.display_group_for(device_id))
                .cloned();
            let cleared_group = scene_manager
                .clear_display_group_assignment(
                    device_id,
                    info.name.as_str(),
                    display_face_layout(device_id, info.name.as_str(), surface),
                )
                .map_err(|error| ToolError::Internal(error.to_string()))?
                .clone();
            (active_scene_id, previous_group, cleared_group)
        };
        let change_kind = if previous_group.is_some() {
            ZoneChangeKind::Updated
        } else {
            ZoneChangeKind::Created
        };
        publish_render_group_changed(state, active_scene_id, &cleared_group, change_kind);
        save_runtime_session_snapshot(state).await;
        let live_scope = live_scope_payload(state, device_id).await;
        return Ok(json!({
            "device": display_device_payload(&info, surface),
            "scene_id": active_scene_id.to_string(),
            "group": cleared_group,
            "scope": "scene",
            "live_scope": live_scope,
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
            ZoneChangeKind::Updated
        } else {
            ZoneChangeKind::Created
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
        // upsert seeds the target as Replace; blend over the live effect by
        // default so the face layers on top instead of blacking it out,
        // mirroring the REST scene-scope contract.
        let group = scene_manager
            .patch_display_group_target(
                group.id,
                Some(hypercolor_types::scene::DisplayFaceBlendMode::Alpha),
                Some(1.0),
            )
            .ok_or_else(|| ToolError::Internal("failed to set display face composition".into()))?
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
        "scope": "scene",
        "live_scope": "scene",
        "cleared": false,
    }))
}

/// Which layer currently drives the display, mirroring the REST contract.
async fn live_scope_payload(state: &AppState, device_id: DeviceId) -> Value {
    let scene_assigned = {
        let scene_manager = state.scene_manager.read().await;
        scene_manager
            .active_scene()
            .and_then(|scene| scene.display_group_for(device_id))
            .is_some_and(|zone| zone.effect_id.is_some())
    };
    if scene_assigned {
        return json!("scene");
    }
    let default_assigned = {
        let store = state.display_preferences.read().await;
        store.get(device_id).is_some()
    };
    if default_assigned {
        json!("default")
    } else {
        Value::Null
    }
}

async fn handle_default_scope(
    state: &AppState,
    params: &Value,
    device_id: DeviceId,
    info: &DeviceInfo,
    surface: DisplaySurfaceInfo,
    clear: bool,
    controls: std::collections::HashMap<String, ControlValue>,
) -> Result<Value, ToolError> {
    if clear {
        let removed = {
            let mut store = state.display_preferences.write().await;
            let removed = store.remove(device_id).is_some();
            if removed && let Err(error) = store.save() {
                tracing::warn!(%error, "Failed to persist display preferences");
            }
            removed
        };
        let (was_live, scene_id, cleared_zone) = {
            let mut scene_manager = state.scene_manager.write().await;
            let scene_assigned = scene_manager
                .active_scene()
                .and_then(|scene| scene.display_group_for(device_id))
                .is_some_and(|zone| zone.effect_id.is_some());
            let cleared = scene_manager.default_display_group_for(device_id).cloned();
            scene_manager.remove_default_display_group(device_id);
            let scene_id = scene_manager
                .active_scene()
                .map(|scene| scene.id)
                .unwrap_or(hypercolor_types::scene::SceneId::DEFAULT);
            (!scene_assigned, scene_id, cleared)
        };
        if removed
            && was_live
            && let Some(mut zone) = cleared_zone
        {
            zone.effect_id = None;
            zone.layers.clear();
            publish_render_group_changed(state, scene_id, &zone, ZoneChangeKind::Updated);
        }
        let live_scope = live_scope_payload(state, device_id).await;
        return Ok(json!({
            "device": display_device_payload(info, surface),
            "scope": "default",
            "live_scope": live_scope,
            "cleared": removed,
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

    {
        let mut store = state.display_preferences.write().await;
        store.set(
            device_id,
            crate::display_preferences::DisplayPreference {
                // Blend over the live effect by default; Replace is opt-in
                // via the composition controls for face-only looks.
                blend_mode: hypercolor_types::scene::DisplayFaceBlendMode::Alpha,
                controls,
                effect_id: effect.id,
                opacity: 1.0,
            },
        );
        if let Err(error) = store.save() {
            tracing::warn!(%error, "Failed to persist display preferences");
        }
    }
    let Some(group) =
        crate::api::displays::apply_display_preference_overlay(state, device_id).await
    else {
        return Err(ToolError::Internal(
            "failed to install the default face overlay".into(),
        ));
    };
    let live_scope = live_scope_payload(state, device_id).await;
    if live_scope == json!("default") {
        let scene_id = {
            let scene_manager = state.scene_manager.read().await;
            scene_manager
                .active_scene()
                .map(|scene| scene.id)
                .unwrap_or(hypercolor_types::scene::SceneId::DEFAULT)
        };
        publish_render_group_changed(state, scene_id, &group, ZoneChangeKind::Updated);
    }

    Ok(json!({
        "device": display_device_payload(info, surface),
        "effect": effect,
        "group": group,
        "scope": "default",
        "live_scope": live_scope,
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
