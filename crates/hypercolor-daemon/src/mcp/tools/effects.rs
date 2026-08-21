//! Effect-related MCP tools: `set_effect`, `list_effects`, `set_color`.

use std::cmp::min;
use std::collections::HashMap;

use serde_json::{Value, json};

use super::{
    ToolDefinition, ToolError, find_effect_metadata, output_schema, resolve_effect_selector,
    serialize_result,
};
use crate::api::AppState;
use crate::api::effects::normalize_control_payload;
use crate::domain::MutationContext;
use crate::domain::effect::{
    ApplyEffect, EffectCatalogQuery, RequestedTransition, apply_effect, list_catalog,
};
use hypercolor_types::api::scene::{ApplyEffectResponse, TransitionType};
use hypercolor_types::effect::{ControlValue, EffectCategory};
use strum::VariantNames;

use crate::mcp::results::{EffectCatalogItem, EffectCatalogResult, EffectControlItem};

// ── Tool Definitions ──────────────────────────────────────────────────────

pub(super) fn build_set_effect() -> ToolDefinition {
    ToolDefinition {
        name: "set_effect".into(),
        title: "Set Lighting Effect".into(),
        description: "Replace the target zone's layer stack with one lighting effect. The selector accepts an exact effect ID, exact name, or unique name substring. Use list_effects first if unsure what's available.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Effect ID, exact name, or unique name substring"
                },
                "controls": {
                    "type": "object",
                    "description": "Optional effect parameter overrides as key-value pairs",
                    "additionalProperties": true
                },
                "transition": {
                    "type": "object",
                    "description": "Applied transition. Only an immediate cut is currently supported.",
                    "properties": {
                        "type": {
                            "type": "string",
                            "enum": ["cut"]
                        }
                    },
                    "required": ["type"],
                    "additionalProperties": false
                }
            },
            "required": ["query"],
            "additionalProperties": false
        }),
        output_schema: output_schema::<ApplyEffectResponse>(),
        read_only: false,
        destructive: true,
        idempotent: false,
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
        output_schema: output_schema::<EffectCatalogResult>(),
        read_only: true,
        destructive: false,
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
                }
            },
            "required": ["color"],
            "additionalProperties": false
        }),
        output_schema: output_schema::<ApplyEffectResponse>(),
        read_only: false,
        destructive: true,
        idempotent: false,
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

    let effect = resolve_effect_selector(state, "query", query).await?;
    if effect.category == EffectCategory::Display {
        return Err(ToolError::InvalidParam {
            param: "query".into(),
            reason: format!(
                "effect '{}' is a display face and must be assigned to a display device, not applied to the LED pipeline",
                effect.name
            ),
        });
    }
    parse_transition(params.get("transition"))?;

    let controls = params
        .get("controls")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let (normalized_controls, rejected_controls) = normalize_control_payload(&effect, &controls);
    if !rejected_controls.is_empty() {
        return Err(ToolError::InvalidParam {
            param: "controls".into(),
            reason: format!("rejected values: {}", rejected_controls.join(", ")),
        });
    }

    // The service enforces this too, but its DomainError::Internal for a
    // missing active scene would reach MCP as -32603 "internal error".
    // These tools have always answered -32000 with the specific reason,
    // so the reason is resolved here and rendered in the tool's own
    // frozen shape.
    {
        let scene_manager = state.scene_manager.snapshot().await;
        crate::domain::scene::active_scene_for_runtime_mutation(
            &scene_manager,
            "applying an effect",
        )
        .map_err(|error| ToolError::Conflict(error.to_string()))?;
    }

    let applied = apply_effect(
        &state.effects,
        ApplyEffect {
            effect,
            controls: normalized_controls,
            preset_id: None,
            target_zone: None,
            expected_revision: None,
            transition: RequestedTransition::cut(),
            wake_output: true,
        },
        MutationContext::mcp(),
    )
    .await?;

    serialize_apply_response(applied)
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
    let filtered = list_catalog(&state.effects, &query).await;

    let total = filtered.len();
    let limit = usize::try_from(limit_u64).unwrap_or(20);
    let offset = usize::try_from(offset_u64).unwrap_or_default();
    let start = min(offset, total);
    let end = min(start.saturating_add(limit), total);

    let effects = filtered[start..end]
        .iter()
        .map(|metadata| EffectCatalogItem {
            id: metadata.id.to_string(),
            name: metadata.name.clone(),
            description: metadata.description.clone(),
            category: metadata.category,
            audio_reactive: metadata.audio_reactive,
            tags: metadata.tags.clone(),
            controls: metadata
                .controls
                .iter()
                .map(|control| EffectControlItem {
                    id: control.control_id().to_owned(),
                    name: control.name.clone(),
                    kind: control.kind.clone(),
                    default: control.default_value.clone(),
                    min: control.min,
                    max: control.max,
                    step: control.step,
                    options: control.labels.clone(),
                    tooltip: control.tooltip.clone(),
                })
                .collect(),
        })
        .collect::<Vec<_>>();

    serialize_result(EffectCatalogResult {
        effects,
        total,
        has_more: end < total,
        limit: limit_u64,
        offset: offset_u64,
    })
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
        let scene_manager = state.scene_manager.snapshot().await;
        crate::domain::scene::active_scene_for_runtime_mutation(
            &scene_manager,
            "applying an effect",
        )
        .map_err(|error| ToolError::Conflict(error.to_string()))?;
    }

    let applied = apply_effect(
        &state.effects,
        ApplyEffect {
            effect: solid_effect.clone(),
            controls,
            preset_id: None,
            target_zone: None,
            expected_revision: None,
            transition: RequestedTransition::cut(),
            wake_output: true,
        },
        MutationContext::mcp(),
    )
    .await?;

    serialize_apply_response(applied)
}

fn parse_transition(value: Option<&Value>) -> Result<(), ToolError> {
    let Some(value) = value else {
        return Ok(());
    };
    let Some(object) = value.as_object() else {
        return Err(ToolError::InvalidParam {
            param: "transition".into(),
            reason: "must be an object with type 'cut'".into(),
        });
    };
    if object.len() != 1 || !object.contains_key("type") {
        return Err(ToolError::InvalidParam {
            param: "transition".into(),
            reason: "accepts exactly one field: type".into(),
        });
    }
    if object.get("type").and_then(Value::as_str) != Some("cut") {
        return Err(ToolError::InvalidParam {
            param: "transition".into(),
            reason: "type must be 'cut'".into(),
        });
    }
    Ok(())
}

fn serialize_apply_response(
    applied: crate::domain::effect::EffectApplied,
) -> Result<Value, ToolError> {
    serialize_result(ApplyEffectResponse {
        zone: crate::domain::scene_tree::zone_resource(&applied.zone),
        transition: TransitionType::Cut,
        output: applied.output,
    })
}
