//! Effect-related MCP tools: `set_effect`, `add_layer`, `list_effects`,
//! `set_color`.

use std::cmp::min;
use std::collections::{BTreeMap, HashMap};

use serde_json::{Value, json};

use super::scenes::{resolve_zone, retry_resolved_scene_target};
use super::{
    ToolDefinition, ToolError, find_effect_metadata, output_schema, resolve_effect_selector,
    serialize_result,
};
use crate::app_state::AppState;
use crate::domain::MutationContext;
use crate::domain::effect::{
    ApplyEffect, EffectCatalogQuery, RequestedTransition, apply_effect, list_catalog,
};
use hypercolor_types::api::scene::{ApplyEffectResponse, SceneFence, TransitionType, ZoneResource};
use hypercolor_types::control::ControlValue;
use hypercolor_types::effect::{EffectCategory, EffectMetadata};
use hypercolor_types::layer::{BlendMode, LayerScope, SceneLayer, SceneLayerId};
use hypercolor_types::scene::ZoneRole;
use strum::VariantNames;

use crate::mcp::control_payload::admit_raw_controls;
use crate::mcp::results::{EffectCatalogItem, EffectCatalogResult};
use crate::mcp::selector::{SelectorCandidate, SelectorError};
use crate::resource_summary::effect_summary_with_details;

// ── Tool Definitions ──────────────────────────────────────────────────────

pub(super) fn build_set_effect() -> ToolDefinition {
    ToolDefinition {
        name: "set_effect".into(),
        title: "Set Lighting Effect".into(),
        description: "Replace the primary zone's whole-zone layers with one lighting effect. Layers limited to some of the zone's outputs stay on top. The selector accepts an exact effect ID, exact name, or unique name substring. Use list_effects first if unsure what's available.".into(),
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

pub(super) fn build_add_layer() -> ToolDefinition {
    // Read from the type so the advertised vocabulary cannot drift from
    // the one the daemon decodes.
    let blend_modes = output_schema::<BlendMode>()["enum"].clone();
    ToolDefinition {
        name: "add_layer".into(),
        title: "Add Effect Layer".into(),
        description: "Stack one effect on top of a zone's layers without replacing anything. Name outputs to limit the layer to part of the zone, such as one keyboard, and omit them to cover the whole zone. The effect, zone, and output selectors accept an ID, an exact name, or a unique name substring.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "effect": {
                    "type": "string",
                    "description": "Effect ID, exact name, or unique name substring"
                },
                "zone": {
                    "type": "string",
                    "description": "Zone ID, exact name, or unique name substring. Defaults to the primary zone."
                },
                "outputs": {
                    "type": "array",
                    "description": "Limit the layer to these outputs of the zone. Each entry is an output's ID or name, or a device's ID or name for every output of that device the zone holds. Omit to apply the layer to the whole zone.",
                    "items": { "type": "string" },
                    "minItems": 1
                },
                "blend": {
                    "type": "string",
                    "enum": blend_modes,
                    "description": "How the layer composes with the layers beneath it",
                    "default": "alpha"
                },
                "opacity": {
                    "type": "number",
                    "description": "Layer opacity",
                    "default": 1.0,
                    "minimum": 0.0,
                    "maximum": 1.0
                },
                "controls": {
                    "type": "object",
                    "description": "Optional effect parameter overrides as key-value pairs",
                    "additionalProperties": true
                }
            },
            "required": ["effect"],
            "additionalProperties": false
        }),
        output_schema: output_schema::<ZoneResource>(),
        read_only: false,
        destructive: false,
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
    let normalized_controls = admitted_controls(params, &effect)?;

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
        &state.domains.effects,
        ApplyEffect {
            effect,
            controls: normalized_controls,
            preset_id: None,
            target_zone: None,
            fence: None,
            transition: RequestedTransition::cut(),
            wake_output: true,
        },
        MutationContext::mcp(),
    )
    .await?;

    serialize_apply_response(applied)
}

pub(super) async fn handle_add_layer_with_state(
    params: &Value,
    state: &AppState,
) -> Result<Value, ToolError> {
    let query = params
        .get("effect")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::MissingParam("effect".into()))?;
    let effect = resolve_effect_selector(state, "effect", query).await?;
    if effect.category == EffectCategory::Display {
        return Err(ToolError::InvalidParam {
            param: "effect".into(),
            reason: format!(
                "effect '{}' is a display face and must be assigned to a display device, not stacked on a lighting zone",
                effect.name
            ),
        });
    }
    let controls = admitted_controls(params, &effect)?;
    let blend = match params.get("blend") {
        Some(value) => decode_argument::<BlendMode>("blend", value)?,
        None => BlendMode::default(),
    };
    let opacity = match params.get("opacity") {
        Some(value) => decode_argument::<f32>("opacity", value)?,
        None => 1.0,
    };

    // As in `set_effect`: a scene that refuses runtime mutation answers
    // -32000 with its reason rather than the service's internal error.
    {
        let scene_manager = state.scene_manager.snapshot().await;
        crate::domain::scene::active_scene_for_runtime_mutation(&scene_manager, "adding a layer")
            .map_err(|error| ToolError::Conflict(error.to_string()))?;
    }

    let document = crate::domain::scene_tree::read_document(&state.domains.scene_tree).await?;
    let zone = match params.get("zone").and_then(Value::as_str) {
        Some(query) => resolve_zone(query, &document)?,
        None => document
            .zones
            .iter()
            .find(|zone| zone.role == ZoneRole::Primary)
            .cloned()
            .ok_or_else(|| ToolError::InvalidParam {
                param: "zone".into(),
                reason: "the active scene has no primary zone; name the zone to add the layer to"
                    .into(),
            })?,
    };
    if zone.role == ZoneRole::Display {
        return Err(ToolError::InvalidParam {
            param: "zone".into(),
            reason: "display zones take their layers through the display-face tools".into(),
        });
    }
    let scope = match params.get("outputs").and_then(Value::as_array) {
        Some(queries) => Some(resolve_scope(state, &zone, queries).await?),
        None => None,
    };

    let mut layer = SceneLayer::from_effect(
        SceneLayerId::new(),
        effect.id,
        controls,
        HashMap::new(),
        None,
    );
    layer.blend = blend;
    layer.opacity = opacity;
    layer.scope = scope;
    let zone_id = zone.id;

    // The zone and outputs were resolved against one read of the scene, so
    // the write is fenced to that revision. A fence that went stale before
    // the write began is refreshed and the same ids tried again.
    let written = retry_resolved_scene_target(
        document.revision,
        || async {
            crate::domain::scene_tree::read_document(&state.domains.scene_tree)
                .await
                .map(|document| document.revision)
        },
        |expected_revision| {
            let layer = layer.clone();
            async move {
                crate::domain::layer::insert_layer(
                    &state.domains.effects,
                    zone_id,
                    layer,
                    None,
                    Some(SceneFence::revision_only(expected_revision)),
                )
                .await
            }
        },
    )
    .await?
    .map_err(|refusal| crate::domain::scene_tree::layer_error(refusal, zone_id, None))?;

    serialize_result(crate::domain::scene_tree::zone_resource(&written.zone))
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
    let filtered = list_catalog(&state.domains.effects, &query).await;

    let total = filtered.len();
    let limit = usize::try_from(limit_u64).unwrap_or(20);
    let offset = usize::try_from(offset_u64).unwrap_or_default();
    let start = min(offset, total);
    let end = min(start.saturating_add(limit), total);

    let page = filtered[start..end].to_vec();
    // Naming covers may read cover files after a change; keep it off the
    // async workers.
    let effects = tokio::task::spawn_blocking(move || {
        page.iter()
            .map(|metadata| EffectCatalogItem {
                summary: effect_summary_with_details(metadata),
            })
            .collect::<Vec<_>>()
    })
    .await
    .unwrap_or_default();

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
        ControlValue::linear_color([
            f32::from(resolved.r) / 255.0,
            f32::from(resolved.g) / 255.0,
            f32::from(resolved.b) / 255.0,
            1.0,
        ]),
    )]);
    if let Some(brightness) = brightness {
        controls.insert(
            "brightness".to_owned(),
            ControlValue::Float(f64::from(brightness)),
        );
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
        &state.domains.effects,
        ApplyEffect {
            effect: solid_effect.clone(),
            controls,
            preset_id: None,
            target_zone: None,
            fence: None,
            transition: RequestedTransition::cut(),
            wake_output: true,
        },
        MutationContext::mcp(),
    )
    .await?;

    serialize_apply_response(applied)
}

/// Decode the `controls` argument through the effect's control schema.
fn admitted_controls(
    params: &Value,
    effect: &EffectMetadata,
) -> Result<HashMap<String, ControlValue>, ToolError> {
    let controls = params
        .get("controls")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let (normalized, rejected) = admit_raw_controls(effect, &controls);
    if rejected.is_empty() {
        return Ok(normalized);
    }
    let rejected = rejected
        .iter()
        .map(|change| format!("{} ({})", change.field_id, change.error))
        .collect::<Vec<_>>();
    Err(ToolError::InvalidParam {
        param: "controls".into(),
        reason: format!("rejected values: {}", rejected.join(", ")),
    })
}

fn decode_argument<T: serde::de::DeserializeOwned>(
    param: &str,
    value: &Value,
) -> Result<T, ToolError> {
    serde_json::from_value(value.clone()).map_err(|error| ToolError::InvalidParam {
        param: param.to_owned(),
        reason: error.to_string(),
    })
}

/// One thing an `outputs` entry can name: an output of the zone, or a
/// device standing for every output of it the zone holds.
struct ScopeTarget {
    id: String,
    name: Option<String>,
    /// Member ids, sorted.
    outputs: Vec<String>,
}

/// Resolve the `outputs` argument to the scope it names within `zone`.
async fn resolve_scope(
    state: &AppState,
    zone: &ZoneResource,
    queries: &[Value],
) -> Result<LayerScope, ToolError> {
    let targets = scope_targets(state, zone).await;
    let mut outputs = Vec::new();
    for (index, query) in queries.iter().enumerate() {
        let Some(query) = query.as_str() else {
            return Err(ToolError::InvalidParam {
                param: format!("outputs.{index}"),
                reason: "must be a string".into(),
            });
        };
        outputs.extend(
            resolve_scope_target(query, &targets)
                .map_err(|error| ToolError::selector("outputs", error))?,
        );
    }
    Ok(LayerScope { outputs }.normalized())
}

/// Everything an `outputs` entry can select in `zone`.
///
/// Members carry a layout device id, which is what a zone knows a device
/// by. The registry maps it back to the device an agent reads from
/// `get_devices`, so that device's own id and name select its outputs too.
async fn scope_targets(state: &AppState, zone: &ZoneResource) -> Vec<ScopeTarget> {
    let mut targets = Vec::new();
    let mut by_layout_device: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for member in &zone.members {
        targets.push(ScopeTarget {
            id: member.id.0.clone(),
            name: Some(member.name.clone()),
            outputs: vec![member.id.0.clone()],
        });
        by_layout_device
            .entry(member.device_id.as_str())
            .or_default()
            .push(member.id.0.clone());
    }

    let tracked = state.device_registry.list().await;
    let runtime = state.domains.devices.layout_runtime();
    let mut registry_device: HashMap<String, usize> = HashMap::new();
    for (index, device) in tracked.iter().enumerate() {
        let layout_device_id = state
            .domains
            .layout
            .resolved_layout_device_id(&runtime, &device.info)
            .await;
        registry_device.insert(layout_device_id, index);
    }
    for (layout_device_id, entry) in state.logical_devices.read().await.iter() {
        if let Some(index) = tracked
            .iter()
            .position(|device| device.info.id == entry.physical_device_id)
        {
            registry_device.insert(layout_device_id.clone(), index);
        }
    }

    let mut by_registry_device: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    for (layout_device_id, mut outputs) in by_layout_device {
        outputs.sort_unstable();
        if let Some(index) = registry_device.get(layout_device_id) {
            by_registry_device
                .entry(*index)
                .or_default()
                .extend(outputs.iter().cloned());
        }
        targets.push(ScopeTarget {
            id: layout_device_id.to_owned(),
            name: None,
            outputs,
        });
    }
    for (index, mut outputs) in by_registry_device {
        outputs.sort_unstable();
        let info = &tracked[index].info;
        targets.push(ScopeTarget {
            id: info.id.to_string(),
            name: Some(info.name.clone()),
            outputs,
        });
    }
    targets
}

/// Resolve one `outputs` entry with the shared selector policy.
///
/// An output and the device that holds only it are one target under two
/// names, so matches that all select the same outputs are not ambiguous.
fn resolve_scope_target(
    query: &str,
    targets: &[ScopeTarget],
) -> Result<Vec<String>, SelectorError> {
    let candidates = targets
        .iter()
        .map(|target| match &target.name {
            Some(name) => {
                SelectorCandidate::named(target.id.clone(), name.clone(), target.outputs.clone())
            }
            None => SelectorCandidate::unnamed(target.id.clone(), target.outputs.clone()),
        })
        .collect();
    match crate::mcp::selector::resolve(query, candidates) {
        Err(SelectorError::Ambiguous { query, candidates }) => {
            let mut matched = targets
                .iter()
                .filter(|target| {
                    candidates
                        .iter()
                        .any(|candidate| candidate.id == target.id && candidate.name == target.name)
                })
                .map(|target| &target.outputs);
            match matched.next() {
                Some(first) if matched.all(|outputs| outputs == first) => Ok(first.clone()),
                _ => Err(SelectorError::Ambiguous { query, candidates }),
            }
        }
        resolved => resolved,
    }
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
