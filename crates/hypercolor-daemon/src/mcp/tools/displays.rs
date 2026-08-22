//! Display-face MCP tool.

use serde_json::{Value, json};

use super::{ToolDefinition, ToolError, output_schema, resolve_effect_selector, serialize_result};
use crate::api::displays::{DisplaySurfaceInfo, display_face_layout, display_surface_info};
use crate::api::{AppState, publish_render_group_changed};
use crate::domain::MutationContext;
use crate::domain::display::{
    ClearDisplayFace, SetDisplayFace, clear_display_face, remove_default_display_overlay,
    set_display_face,
};
use crate::mcp::results::{DisplayDeviceResult, DisplayFaceResult};
use crate::mcp::selector::SelectorCandidate;
use hypercolor_types::control::ControlValue;
use hypercolor_types::device::{DeviceId, DeviceInfo};
use hypercolor_types::effect::{EffectCategory, EffectMetadata};
use hypercolor_types::event::ZoneChangeKind;
use hypercolor_types::scene::{DisplayFaceBlendMode, DisplayFaceTarget};

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
                    "description": "Display device ID, exact name, or unique name substring."
                },
                "effect_id": {
                    "type": "string",
                    "description": "Display-face effect ID, exact name, or unique name substring. Omit when clearing."
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
                    "description": "Optional control overrides to store on the display face zone.",
                    "additionalProperties": true
                }
            },
            "required": ["device"],
            "additionalProperties": false
        }),
        output_schema: output_schema::<DisplayFaceResult>(),
        read_only: false,
        destructive: true,
        idempotent: false,
    }
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

    if scope == crate::api::displays::DisplayFaceScope::Default {
        return handle_default_scope(state, params, device_id, &info, surface, clear).await;
    }

    if clear {
        let cleared = clear_display_face(
            state,
            ClearDisplayFace {
                device_id,
                device_name: info.name.clone(),
                layout: display_face_layout(device_id, info.name.as_str(), surface),
            },
            MutationContext::mcp(),
        )
        .await?;
        let live_scope = live_scope_payload(state, device_id).await;
        return serialize_result(DisplayFaceResult {
            device: display_device_payload(&info, surface),
            scope,
            live_scope,
            cleared: true,
            scene_id: Some(cleared.scene_id.to_string()),
            effect: None,
            zone: Some(crate::domain::scene_tree::zone_resource(&cleared.zone)),
        });
    }

    let effect_lookup = params
        .get("effect_id")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::MissingParam("effect_id".into()))?;
    let effect = {
        let effect = resolve_effect_selector(state, "effect_id", effect_lookup).await?;
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
    let controls = parse_controls_map(params.get("controls"), &effect)?;

    // The face blends over the live effect by default; Replace is opt-in
    // through the REST composition endpoint for face-only looks.
    let written = set_display_face(
        state,
        SetDisplayFace {
            device_id,
            device_name: info.name.clone(),
            effect: effect.clone(),
            controls,
            layout: display_face_layout(device_id, info.name.as_str(), surface),
            target: DisplayFaceTarget {
                blend_mode: DisplayFaceBlendMode::Alpha,
                device_id,
                opacity: 1.0,
            },
        },
        MutationContext::mcp(),
    )
    .await?;

    serialize_result(DisplayFaceResult {
        device: display_device_payload(&info, surface),
        scope,
        live_scope: Some(crate::api::displays::DisplayFaceScope::Scene),
        cleared: false,
        scene_id: Some(written.scene_id.to_string()),
        effect: Some(crate::api::effects::effect_summary_with_details(&effect)),
        zone: Some(crate::domain::scene_tree::zone_resource(&written.zone)),
    })
}

/// Which layer currently drives the display, mirroring the REST contract.
async fn live_scope_payload(
    state: &AppState,
    device_id: DeviceId,
) -> Option<crate::api::displays::DisplayFaceScope> {
    let scene_assigned = {
        let scene_manager = state.scene_manager.snapshot().await;
        scene_manager
            .active_scene()
            .and_then(|scene| scene.display_zone_for(device_id))
            .is_some_and(|zone| zone.effect_ids().next().is_some())
    };
    if scene_assigned {
        return Some(crate::api::displays::DisplayFaceScope::Scene);
    }
    let default_assigned = {
        let store = state.display_preferences.read().await;
        store.get(device_id).is_some()
    };
    if default_assigned {
        Some(crate::api::displays::DisplayFaceScope::Default)
    } else {
        None
    }
}

async fn handle_default_scope(
    state: &AppState,
    params: &Value,
    device_id: DeviceId,
    info: &DeviceInfo,
    surface: DisplaySurfaceInfo,
    clear: bool,
) -> Result<Value, ToolError> {
    if clear {
        let removed = {
            let mut store = state.display_preferences.write().await;
            store
                .remove(device_id)
                .map_err(|error| ToolError::Internal(error.to_string()))?
                .is_some()
        };
        let scene_assigned = {
            let scene_manager = state.scene_manager.snapshot().await;
            scene_manager
                .active_scene()
                .and_then(|scene| scene.display_zone_for(device_id))
                .is_some_and(|zone| zone.effect_ids().next().is_some())
        };
        let cleared_zone = remove_default_display_overlay(state, device_id).await?;
        if removed
            && !scene_assigned
            && let Some(mut zone) = cleared_zone
        {
            zone.layers.clear();
            let scene_id = {
                let scene_manager = state.scene_manager.snapshot().await;
                scene_manager
                    .active_scene()
                    .map_or(hypercolor_types::scene::SceneId::DEFAULT, |scene| scene.id)
            };
            publish_render_group_changed(state, scene_id, &zone, ZoneChangeKind::Updated);
        }
        let live_scope = live_scope_payload(state, device_id).await;
        return serialize_result(DisplayFaceResult {
            device: display_device_payload(info, surface),
            scope: crate::api::displays::DisplayFaceScope::Default,
            live_scope,
            cleared: removed,
            scene_id: None,
            effect: None,
            zone: None,
        });
    }

    let effect_lookup = params
        .get("effect_id")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::MissingParam("effect_id".into()))?;
    let effect = {
        let effect = resolve_effect_selector(state, "effect_id", effect_lookup).await?;
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
    let controls = parse_controls_map(params.get("controls"), &effect)?;

    {
        let mut store = state.display_preferences.write().await;
        store
            .set(
                device_id,
                crate::display_preferences::DisplayPreference {
                    // Blend over the live effect by default; Replace is opt-in
                    // via the composition controls for face-only looks.
                    blend_mode: hypercolor_types::scene::DisplayFaceBlendMode::Alpha,
                    controls,
                    effect_id: effect.id,
                    opacity: 1.0,
                },
            )
            .map_err(|error| ToolError::Internal(error.to_string()))?;
    }
    let Some(group) =
        crate::api::displays::apply_display_preference_overlay(state, device_id).await
    else {
        return Err(ToolError::Internal(
            "failed to install the default face overlay".into(),
        ));
    };
    let live_scope = live_scope_payload(state, device_id).await;
    if live_scope == Some(crate::api::displays::DisplayFaceScope::Default) {
        let scene_id = {
            let scene_manager = state.scene_manager.snapshot().await;
            scene_manager
                .active_scene()
                .map(|scene| scene.id)
                .unwrap_or(hypercolor_types::scene::SceneId::DEFAULT)
        };
        publish_render_group_changed(state, scene_id, &group, ZoneChangeKind::Updated);
    }

    serialize_result(DisplayFaceResult {
        device: display_device_payload(info, surface),
        scope: crate::api::displays::DisplayFaceScope::Default,
        live_scope,
        cleared: false,
        scene_id: None,
        effect: Some(crate::api::effects::effect_summary_with_details(&effect)),
        zone: Some(crate::domain::scene_tree::zone_resource(&group)),
    })
}

fn parse_controls_map(
    value: Option<&Value>,
    effect: &EffectMetadata,
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
        let definition = effect
            .control_by_id(key)
            .ok_or_else(|| ToolError::InvalidParam {
                param: "controls".into(),
                reason: format!("unknown control '{key}'"),
            })?;
        let control =
            definition
                .admit_effect_json(value)
                .map_err(|error| ToolError::InvalidParam {
                    param: "controls".into(),
                    reason: format!("invalid control value for '{key}': {error}"),
                })?;
        controls.insert(key.clone(), control);
    }
    Ok(controls)
}

async fn resolve_display_device(
    state: &AppState,
    raw: &str,
) -> Result<(DeviceId, DeviceInfo, DisplaySurfaceInfo), ToolError> {
    let candidates = state
        .device_registry
        .list()
        .await
        .into_iter()
        .map(|tracked| {
            SelectorCandidate::named(
                tracked.info.id.to_string(),
                tracked.info.name.clone(),
                tracked,
            )
        })
        .collect();
    let device = crate::mcp::selector::resolve(raw, candidates)
        .map_err(|error| ToolError::selector("device", error))?;
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

fn display_device_payload(info: &DeviceInfo, surface: DisplaySurfaceInfo) -> DisplayDeviceResult {
    DisplayDeviceResult {
        id: info.id.to_string(),
        name: info.name.clone(),
        vendor: info.vendor.clone(),
        family: info.family.to_string(),
        width: surface.width,
        height: surface.height,
        circular: surface.circular,
    }
}

#[cfg(test)]
mod tests {
    use hypercolor_types::control::ControlValue;
    use hypercolor_types::effect::{
        ControlDefinition, ControlKind, ControlType, EffectCategory, EffectId, EffectMetadata,
        EffectSource,
    };

    use super::parse_controls_map;

    fn metadata() -> EffectMetadata {
        EffectMetadata {
            id: EffectId::new(uuid::Uuid::now_v7()),
            name: "display fixture".to_owned(),
            author: "test".to_owned(),
            version: "1".to_owned(),
            description: String::new(),
            category: EffectCategory::Display,
            tags: Vec::new(),
            controls: vec![
                ControlDefinition {
                    id: "accent".to_owned(),
                    name: "Accent".to_owned(),
                    kind: ControlKind::Color,
                    control_type: ControlType::ColorPicker,
                    default_value: ControlValue::linear_color([1.0, 1.0, 1.0, 1.0]),
                    min: None,
                    max: None,
                    step: None,
                    labels: Vec::new(),
                    group: None,
                    tooltip: None,
                    aspect_lock: None,
                    preview_source: None,
                    binding: None,
                },
                ControlDefinition {
                    id: "gain".to_owned(),
                    name: "Gain".to_owned(),
                    kind: ControlKind::Number,
                    control_type: ControlType::Slider,
                    default_value: ControlValue::Float(0.5),
                    min: Some(0.0),
                    max: Some(1.0),
                    step: None,
                    labels: Vec::new(),
                    group: None,
                    tooltip: None,
                    aspect_lock: None,
                    preview_source: None,
                    binding: None,
                },
            ],
            presets: Vec::new(),
            audio_reactive: false,
            screen_reactive: false,
            input_reactive: false,
            source: EffectSource::Html {
                path: "fixture.html".into(),
            },
            license: None,
        }
    }

    #[test]
    fn display_controls_reject_f64_values_outside_the_effect_abi() {
        let payload = serde_json::json!({
            "gain": f64::from(f32::MAX) * 2.0,
        });

        let error = parse_controls_map(Some(&payload), &metadata())
            .expect_err("f64 values beyond f32 must be rejected");
        assert!(error.to_string().contains("within the f32 range"));
    }

    #[test]
    fn display_controls_admit_checked_rgba_arrays() {
        let payload = serde_json::json!({
            "accent": [0.1, 0.2, 0.3, 0.4],
        });

        let controls =
            parse_controls_map(Some(&payload), &metadata()).expect("valid RGBA should parse");
        let ControlValue::ColorLinear(color) = controls["accent"] else {
            panic!("expected linear color");
        };
        assert!((color.a - 0.4).abs() < f32::EPSILON);
    }

    #[test]
    fn display_controls_reject_unknown_control_ids() {
        let payload = serde_json::json!({
            "missing": 0.5,
        });

        let error = parse_controls_map(Some(&payload), &metadata())
            .expect_err("undeclared controls must be rejected");
        assert!(error.to_string().contains("unknown control 'missing'"));
    }
}
