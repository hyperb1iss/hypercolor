//! Effect-related MCP tools: `set_effect`, `list_effects`, `stop_effect`, `set_color`.

use std::cmp::min;
use std::collections::HashMap;

use serde_json::{Value, json};

use super::{ToolDefinition, ToolError, default_output_schema, find_effect_metadata};
use crate::api::AppState;
use crate::api::effects::normalize_control_payload;
use crate::domain::MutationContext;
use crate::domain::effect::{
    ApplyEffect, EffectCatalogQuery, RequestedTransition, apply_effect, list_catalog, stop_effect,
};
use hypercolor_types::effect::{ControlValue, EffectCategory};
use strum::VariantNames;

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
                }
            },
            "required": ["query"],
            "additionalProperties": false
        }),
        output_schema: default_output_schema(),
        read_only: false,
        destructive: true,
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
                    // Generated from EffectCategory so the advertised
                    // vocabulary cannot drift from the one the daemon
                    // matches against.
                    "enum": EffectCategory::VARIANTS,
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
            },
            "additionalProperties": false
        }),
        output_schema: default_output_schema(),
        read_only: true,
        destructive: false,
        idempotent: true,
    }
}

pub(super) fn build_stop_effect() -> ToolDefinition {
    ToolDefinition {
        name: "stop_effect".into(),
        title: "Stop Current Effect".into(),
        description: "Destructively stop the current effect, clear its live controls and preset provenance, and release network-device ownership. Use set_output_power with state 'paused' for a reversible blackout.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }),
        output_schema: default_output_schema(),
        read_only: false,
        destructive: true,
        idempotent: true,
    }
}

pub(super) fn build_set_color() -> ToolDefinition {
    ToolDefinition {
        name: "set_color".into(),
        title: "Set Solid Color".into(),
        description: "Set a solid color across the LED pipeline. Accepts CSS color names ('coral', 'dodgerblue'), hex codes ('#ff6ac1'), RGB values ('rgb(255, 106, 193)'), HSL values ('hsl(330, 100%, 71%)'), or natural language descriptions ('warm sunset orange', 'deep ocean blue').".into(),
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
                    "description": "Crossfade duration in milliseconds. Effect transitions are not implemented yet, so only 0 (immediate cut) is accepted.",
                    "default": 0,
                    "minimum": 0,
                    "maximum": 0
                }
            },
            "required": ["color"],
            "additionalProperties": false
        }),
        output_schema: default_output_schema(),
        read_only: false,
        destructive: true,
        idempotent: true,
    }
}

// ── Handlers ──────────────────────────────────────────────────────────────

pub(super) async fn handle_set_effect_with_state(
    params: &Value,
    state: &AppState,
) -> Result<Value, ToolError> {
    let query = params
        .get("query")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::MissingParam("query".into()))?;

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

    let controls = params
        .get("controls")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let (normalized_controls, rejected_controls) =
        normalize_control_payload(&best_match.effect, &controls);

    // The service enforces this too, but its DomainError::Internal for a
    // missing active scene would reach MCP as -32603 "internal error".
    // These tools have always answered -32000 with the specific reason,
    // so the reason is resolved here and rendered in the tool's own
    // frozen shape.
    {
        let scene_manager = state.scene_manager.read().await;
        crate::domain::scene::active_scene_for_runtime_mutation(
            &scene_manager,
            "applying an effect",
        )
        .map_err(|error| ToolError::Conflict(error.to_string()))?;
    }

    let applied = apply_effect(
        state,
        ApplyEffect {
            effect: best_match.effect.clone(),
            controls: normalized_controls.clone(),
            preset_id: None,
            target_zone: None,
            expected_revision: None,
            transition: RequestedTransition::cut(),
            wake_output: true,
        },
        MutationContext::mcp(),
    )
    .await?;

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
        "transition_ms": applied.transition.duration_ms,
        "warnings": []
    }))
}

pub(super) async fn handle_list_effects_with_state(
    params: &Value,
    state: &AppState,
) -> Result<Value, ToolError> {
    let limit_u64 = params.get("limit").and_then(Value::as_u64).unwrap_or(20);
    let offset_u64 = params.get("offset").and_then(Value::as_u64).unwrap_or(0);

    let query = EffectCatalogQuery {
        audio_reactive: params.get("audio_reactive").and_then(Value::as_bool),
        ..EffectCatalogQuery::parse(
            params.get("category").and_then(Value::as_str),
            None,
            params.get("query").and_then(Value::as_str),
        )?
    };
    let filtered = list_catalog(state, &query).await;

    let total = filtered.len();
    let limit = usize::try_from(limit_u64).unwrap_or(20);
    let offset = usize::try_from(offset_u64).unwrap_or_default();
    let start = min(offset, total);
    let end = min(start.saturating_add(limit), total);

    let effects = filtered[start..end]
        .iter()
        .map(|metadata| {
            json!({
                "id": metadata.id.to_string(),
                "name": metadata.name,
                "description": metadata.description,
                "category": format!("{}", metadata.category),
                "audio_reactive": metadata.audio_reactive,
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
    _params: &Value,
    state: &AppState,
) -> Result<Value, ToolError> {
    let Some(stopped) = stop_effect(state, MutationContext::mcp()).await? else {
        return Ok(json!({
            "stopped": false,
            "effect": null
        }));
    };

    Ok(json!({
        "stopped": true,
        "released_network_devices": stopped.released_network_devices,
        "effect": {
            "id": stopped.effect.id,
            "name": stopped.effect.name
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
    // The schema has always advertised transition_ms here. It now runs
    // the same rule set_effect does instead of being dropped on the
    // floor, so a caller asking for a crossfade learns it cannot have
    // one.
    let transition_ms = params
        .get("transition_ms")
        .and_then(Value::as_u64)
        .unwrap_or(0);

    let resolved =
        crate::mcp::fuzzy::resolve_color(color_str).ok_or_else(|| ToolError::InvalidParam {
            param: "color".into(),
            reason: format!("could not resolve color: '{color_str}'"),
        })?;

    let solid_effect = find_effect_metadata(state, "solid_color", "Solid Color")
        .await
        .ok_or_else(|| ToolError::Internal("solid color effect is not registered".into()))?;

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

    // The service enforces this too, but its DomainError::Internal for a
    // missing active scene would reach MCP as -32603 "internal error".
    // These tools have always answered -32000 with the specific reason,
    // so the reason is resolved here and rendered in the tool's own
    // frozen shape.
    {
        let scene_manager = state.scene_manager.read().await;
        crate::domain::scene::active_scene_for_runtime_mutation(
            &scene_manager,
            "applying an effect",
        )
        .map_err(|error| ToolError::Conflict(error.to_string()))?;
    }

    apply_effect(
        state,
        ApplyEffect {
            effect: solid_effect.clone(),
            controls: controls.clone(),
            preset_id: None,
            target_zone: None,
            expected_revision: None,
            transition: RequestedTransition::of_duration(transition_ms),
            wake_output: true,
        },
        MutationContext::mcp(),
    )
    .await?;

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
        "warnings": []
    }))
}
