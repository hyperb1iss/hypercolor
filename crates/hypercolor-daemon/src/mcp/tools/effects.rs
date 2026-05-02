//! Effect-related MCP tools: `set_effect`, `list_effects`, `stop_effect`, `set_color`.

use std::cmp::min;
use std::collections::HashMap;

use serde_json::{Value, json};

use super::{ToolDefinition, ToolError, default_output_schema, find_effect_metadata};
use crate::api::effects::{
    StopActiveEffectError, active_primary_effect, apply_associated_layout, effect_ref,
    normalize_control_payload, stop_active_effect_and_quiesce_output, wake_output_for_effect_start,
};
use crate::api::{AppState, publish_render_group_changed, save_runtime_session_snapshot};
use hypercolor_types::effect::{ControlValue, EffectCategory};
use hypercolor_types::event::{ChangeTrigger, HypercolorEvent, RenderGroupChangeKind};

// ── Tool Definitions ──────────────────────────────────────────────────────

pub(super) fn build_set_effect() -> ToolDefinition {
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

pub(super) fn build_list_effects() -> ToolDefinition {
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

pub(super) fn build_stop_effect() -> ToolDefinition {
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

pub(super) fn build_set_color() -> ToolDefinition {
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

// ── Stateless Handlers ────────────────────────────────────────────────────

pub(super) fn handle_set_effect(params: &Value) -> Result<Value, ToolError> {
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
    let matches = crate::mcp::fuzzy::match_effect(query, &effects);

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
pub(super) fn handle_list_effects(params: &Value) -> Result<Value, ToolError> {
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
pub(super) fn handle_stop_effect(params: &Value) -> Result<Value, ToolError> {
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

pub(super) fn handle_set_color(params: &Value) -> Result<Value, ToolError> {
    let color_str = params
        .get("color")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::MissingParam("color".into()))?;

    let resolved =
        crate::mcp::fuzzy::resolve_color(color_str).ok_or_else(|| ToolError::InvalidParam {
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

// ── Stateful Handlers ─────────────────────────────────────────────────────

pub(super) async fn handle_set_effect_with_state(
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

    let matches = crate::mcp::fuzzy::match_effect(query, &effect_catalog);
    let Some(best_match) = matches.first() else {
        return Ok(json!({
            "matched_effect": null,
            "confidence": 0.0,
            "alternatives": [],
            "applied": false,
            "message": format!("No effects matching '{query}' found. Use list_effects to browse available effects.")
        }));
    };
    if best_match.effect.category == EffectCategory::Display {
        return Err(ToolError::InvalidParam {
            param: "query".into(),
            reason: format!(
                "effect '{}' is a display face and must be assigned to a display device, not applied to the LED pipeline",
                best_match.effect.name
            ),
        });
    }
    wake_output_for_effect_start(state).await;

    let controls = params
        .get("controls")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let (normalized_controls, rejected_controls) =
        normalize_control_payload(&best_match.effect, &controls);
    let previous_effect = active_primary_effect(state)
        .await
        .map(|(_, effect)| effect_ref(&effect));
    let full_scope_layout = {
        let spatial = state.spatial_engine.read().await;
        spatial.layout().as_ref().clone()
    };
    let (scene_id, group, change_kind) = {
        let mut scene_manager = state.scene_manager.write().await;
        let scene_id = crate::api::active_scene_id_for_runtime_mutation(&scene_manager)
            .map_err(|error| ToolError::Conflict(error.message("applying an effect")))?;
        let change_kind = if scene_manager
            .active_scene()
            .and_then(|scene| scene.primary_group())
            .is_some()
        {
            RenderGroupChangeKind::Updated
        } else {
            RenderGroupChangeKind::Created
        };
        let group = scene_manager
            .upsert_primary_group(
                &best_match.effect,
                normalized_controls.clone(),
                None,
                full_scope_layout,
            )
            .map_err(|error| {
                ToolError::Internal(format!(
                    "failed to update active scene primary group: {error}"
                ))
            })?
            .clone();
        (scene_id, group, change_kind)
    };
    state.event_bus.publish(HypercolorEvent::EffectStarted {
        effect: effect_ref(&best_match.effect),
        trigger: ChangeTrigger::Mcp,
        previous: previous_effect,
        transition: None,
    });
    publish_render_group_changed(state, scene_id, &group, change_kind);
    let applied_layout = apply_associated_layout(state, &best_match.effect.id.to_string()).await;
    save_runtime_session_snapshot(state).await;

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
        "applied_controls": normalized_controls,
        "rejected_controls": rejected_controls,
        "transition_ms": transition_ms,
        "warnings": [],
        "layout": applied_layout
    }))
}

pub(super) async fn handle_list_effects_with_state(
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

pub(super) async fn handle_stop_effect_with_state(
    params: &Value,
    state: &AppState,
) -> Result<Value, ToolError> {
    let transition_ms = params
        .get("transition_ms")
        .and_then(Value::as_u64)
        .unwrap_or(300);

    let stop_result = match stop_active_effect_and_quiesce_output(state).await {
        Ok(result) => result,
        Err(StopActiveEffectError::NoActiveEffect) => {
            return Ok(json!({
                "stopped": false,
                "transition_ms": transition_ms,
                "effect": null
            }));
        }
        Err(StopActiveEffectError::ActiveGroupMissing) => {
            return Err(ToolError::Internal(
                "active primary group disappeared".into(),
            ));
        }
        Err(StopActiveEffectError::ActiveScene(error)) => {
            return Err(ToolError::Conflict(
                error.message("stopping the active effect"),
            ));
        }
    };

    Ok(json!({
        "stopped": true,
        "transition_ms": transition_ms,
        "released_network_devices": stop_result.released_network_devices,
        "effect": {
            "id": stop_result.effect.id,
            "name": stop_result.effect.name
        }
    }))
}

pub(super) async fn handle_set_color_with_state(
    params: &Value,
    state: &AppState,
) -> Result<Value, ToolError> {
    let color_str = params
        .get("color")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::MissingParam("color".into()))?;

    let resolved =
        crate::mcp::fuzzy::resolve_color(color_str).ok_or_else(|| ToolError::InvalidParam {
            param: "color".into(),
            reason: format!("could not resolve color: '{color_str}'"),
        })?;

    let solid_effect = find_effect_metadata(state, "solid_color", "Solid Color")
        .await
        .ok_or_else(|| ToolError::Internal("solid color effect is not registered".into()))?;
    wake_output_for_effect_start(state).await;

    let brightness = if let Some(brightness_u64) = params.get("brightness").and_then(Value::as_u64)
    {
        if brightness_u64 > 100 {
            return Err(ToolError::InvalidParam {
                param: "brightness".into(),
                reason: "must be between 0 and 100".into(),
            });
        }
        let brightness_u16 = u16::try_from(brightness_u64).unwrap_or(100);
        Some(f32::from(brightness_u16) / 100.0)
    } else {
        None
    };
    let previous_effect = active_primary_effect(state)
        .await
        .map(|(_, effect)| effect_ref(&effect));
    let mut controls = HashMap::from([(
        "color".to_owned(),
        ControlValue::Color([
            f32::from(resolved.r) / 255.0,
            f32::from(resolved.g) / 255.0,
            f32::from(resolved.b) / 255.0,
            1.0,
        ]),
    )]);
    if let Some(brightness) = brightness {
        controls.insert("brightness".to_owned(), ControlValue::Float(brightness));
    }
    let full_scope_layout = {
        let spatial = state.spatial_engine.read().await;
        spatial.layout().as_ref().clone()
    };
    let (scene_id, group, change_kind) = {
        let mut scene_manager = state.scene_manager.write().await;
        let scene_id = crate::api::active_scene_id_for_runtime_mutation(&scene_manager)
            .map_err(|error| ToolError::Conflict(error.message("applying an effect")))?;
        let change_kind = if scene_manager
            .active_scene()
            .and_then(|scene| scene.primary_group())
            .is_some()
        {
            RenderGroupChangeKind::Updated
        } else {
            RenderGroupChangeKind::Created
        };
        let group = scene_manager
            .upsert_primary_group(&solid_effect, controls.clone(), None, full_scope_layout)
            .map_err(|error| {
                ToolError::Internal(format!(
                    "failed to update active scene primary group: {error}"
                ))
            })?
            .clone();
        (scene_id, group, change_kind)
    };
    state.event_bus.publish(HypercolorEvent::EffectStarted {
        effect: effect_ref(&solid_effect),
        trigger: ChangeTrigger::Mcp,
        previous: previous_effect,
        transition: None,
    });
    publish_render_group_changed(state, scene_id, &group, change_kind);
    let applied_layout = apply_associated_layout(state, &solid_effect.id.to_string()).await;
    save_runtime_session_snapshot(state).await;

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
        "applied_controls": controls,
        "device_count": device_count,
        "warnings": [],
        "layout": applied_layout
    }))
}
