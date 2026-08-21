//! Scene-related MCP tools and live-tree mutations.

use serde_json::{Value, json};

use super::{ToolDefinition, ToolError, output_schema, serialize_result};
use crate::api::AppState;
use crate::domain::scene::{ActivateScene, CreateScene, activate_scene, create_scene};
use crate::domain::scene_tree::{ClearScene, PatchLayerControls};
use crate::domain::{DomainError, MutationContext};
use crate::mcp::results::{
    ActivateSceneResult, AdjustControlsResult, CreateSceneResult, SceneListItem, SceneListResult,
};
use crate::mcp::selector::SelectorCandidate;
use hypercolor_types::api::scene::{PatchControlsRequest, SceneDocument};
use hypercolor_types::api::scenes::ActivatedSceneRef;
use hypercolor_types::scene::TransitionSpec;
use hypercolor_types::scene::ZoneRole;
use hypercolor_types::scene::{SceneKind, SceneMutationMode};

// ── Tool Definitions ──────────────────────────────────────────────────────

pub(super) fn build_activate_scene() -> ToolDefinition {
    ToolDefinition {
        name: "activate_scene".into(),
        title: "Activate Scene".into(),
        description: "Activate a named lighting scene. Scenes combine effects, device assignments, brightness, and transitions into a single preset.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Scene ID, exact name, or unique name substring"
                },
                "transition_ms": {
                    "type": "integer",
                    "description": "Crossfade transition duration in milliseconds",
                    "default": 1000,
                    "minimum": 0,
                    "maximum": 10000
                }
            },
            "required": ["name"],
            "additionalProperties": false
        }),
        output_schema: output_schema::<ActivateSceneResult>(),
        read_only: false,
        destructive: true,
        idempotent: true,
    }
}

pub(super) fn build_list_scenes() -> ToolDefinition {
    ToolDefinition {
        name: "list_scenes".into(),
        title: "List Scenes".into(),
        description: "List all available lighting scenes with their names, descriptions, and activation state.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "enabled_only": {
                    "type": "boolean",
                    "description": "Only show enabled scenes",
                    "default": false
                }
            },
            "additionalProperties": false
        }),
        output_schema: output_schema::<SceneListResult>(),
        read_only: true,
        destructive: false,
        idempotent: true,
    }
}

pub(super) fn build_create_scene() -> ToolDefinition {
    ToolDefinition {
        name: "create_scene".into(),
        title: "Create Scene".into(),
        description: "Create a reusable lighting scene.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Human-readable scene name"
                },
                "description": {
                    "type": "string",
                    "description": "What this scene does"
                },
                "enabled": {
                    "type": "boolean",
                    "description": "Whether the scene may be activated",
                    "default": true
                },
                "mutation_mode": {
                    "type": "string",
                    "enum": ["live", "snapshot"],
                    "description": "Whether runtime effect and display-face actions are allowed to rewrite the scene",
                    "default": "live"
                }
            },
            "required": ["name"],
            "additionalProperties": false
        }),
        output_schema: output_schema::<CreateSceneResult>(),
        read_only: false,
        destructive: false,
        idempotent: false,
    }
}

pub(super) fn build_clear_zone() -> ToolDefinition {
    ToolDefinition {
        name: "clear_zone".into(),
        title: "Clear Scene Zone".into(),
        description: "Clear one non-display zone's layer stack by ID, exact name, or unique name substring. Omit zone to clear every non-display zone and quiesce output.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "zone": {
                    "type": "string",
                    "description": "Optional zone ID, exact name, or unique name substring"
                }
            },
            "additionalProperties": false
        }),
        output_schema: output_schema::<SceneDocument>(),
        read_only: false,
        destructive: true,
        idempotent: true,
    }
}

pub(super) fn build_adjust_controls() -> ToolDefinition {
    ToolDefinition {
        name: "adjust_controls".into(),
        title: "Adjust Layer Controls".into(),
        description: "Atomically patch typed control values and clear bindings on one live scene layer. Zones and named layers accept IDs, exact names, or unique name substrings; unnamed layers require their ID.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "zone": {
                    "type": "string",
                    "description": "Zone ID, exact name, or unique name substring"
                },
                "layer": {
                    "type": "string",
                    "description": "Layer ID, exact name, or unique name substring"
                },
                "values": {
                    "type": "object",
                    "description": "Canonical typed ControlValue entries keyed by control ID",
                    "default": {},
                    "additionalProperties": true
                },
                "clear_bindings": {
                    "type": "array",
                    "description": "Control bindings to remove in the same atomic commit",
                    "default": [],
                    "items": { "type": "string" }
                }
            },
            "required": ["zone", "layer"],
            "additionalProperties": false
        }),
        output_schema: output_schema::<AdjustControlsResult>(),
        read_only: false,
        destructive: false,
        idempotent: true,
    }
}

// ── Handlers ──────────────────────────────────────────────────────────────

pub(super) async fn handle_activate_scene_with_state(
    params: &Value,
    state: &AppState,
) -> Result<Value, ToolError> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::MissingParam("name".into()))?;

    let transition_ms = params
        .get("transition_ms")
        .and_then(Value::as_u64)
        .unwrap_or(1000);
    let scene = {
        let scene_manager = state.scene_manager.snapshot().await;
        let candidates = scene_manager
            .list()
            .into_iter()
            .map(|scene| {
                SelectorCandidate::named(scene.id.to_string(), scene.name.clone(), scene.clone())
            })
            .collect();
        crate::mcp::selector::resolve(name, candidates)
            .map_err(|error| ToolError::selector("name", error))?
    };

    let admission = state.scene.evaluate_media_admission(&scene).await;
    if let Some(details) = admission.violation.as_ref() {
        return Err(ToolError::Conflict(details.message.clone()));
    }

    let activated = activate_scene(
        state,
        ActivateScene {
            scene_id: scene.id,
            transition: Some(TransitionSpec {
                duration_ms: transition_ms,
                ..scene.transition.clone()
            }),
        },
        MutationContext::mcp(),
    )
    .await?;

    serialize_result(ActivateSceneResult {
        activated: true,
        scene: ActivatedSceneRef {
            id: activated.scene_id.to_string(),
            name: activated.scene_name,
        },
        transition_ms,
    })
}

pub(super) async fn handle_clear_zone_with_state(
    params: &Value,
    state: &AppState,
) -> Result<Value, ToolError> {
    let Some(zone) = params.get("zone") else {
        let written = crate::domain::scene_tree::clear_scene(
            &state.scene_tree,
            ClearScene {
                zone: None,
                expected_revision: None,
            },
        )
        .await?;
        return serialize_result(written.document);
    };
    let Value::String(query) = zone else {
        return Err(ToolError::InvalidParam {
            param: "zone".into(),
            reason: "must be a string".into(),
        });
    };

    loop {
        let document = crate::domain::scene_tree::read_document(&state.scene_tree).await?;
        let revision = document.revision;
        let candidates = document
            .zones
            .into_iter()
            .map(|zone| SelectorCandidate::named(zone.id.to_string(), zone.name.clone(), zone))
            .collect();
        let zone = crate::mcp::selector::resolve(query, candidates)
            .map_err(|error| ToolError::selector("zone", error))?;
        if zone.role == ZoneRole::Display {
            return Err(ToolError::InvalidParam {
                param: "zone".into(),
                reason: "display zones are cleared through the display-face tools".into(),
            });
        }

        match crate::domain::scene_tree::clear_scene(
            &state.scene_tree,
            ClearScene {
                zone: Some(zone.id),
                expected_revision: Some(revision),
            },
        )
        .await
        {
            Ok(written) => {
                return serialize_result(written.document);
            }
            Err(error) if scene_snapshot_was_superseded(&error) => {}
            Err(error) => return Err(error.into()),
        }
    }
}

pub(super) async fn handle_adjust_controls_with_state(
    params: &Value,
    state: &AppState,
) -> Result<Value, ToolError> {
    let zone_query = params
        .get("zone")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::MissingParam("zone".into()))?;
    let layer_query = params
        .get("layer")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::MissingParam("layer".into()))?;

    let patch: PatchControlsRequest = serde_json::from_value(json!({
        "values": params.get("values").cloned().unwrap_or_else(|| json!({})),
        "clear_bindings": params
            .get("clear_bindings")
            .cloned()
            .unwrap_or_else(|| json!([])),
    }))
    .map_err(|error| ToolError::InvalidParam {
        param: "values".into(),
        reason: error.to_string(),
    })?;

    loop {
        let document = crate::domain::scene_tree::read_document(&state.scene_tree).await?;
        let revision = document.revision;
        let zone_candidates = document
            .zones
            .into_iter()
            .map(|zone| SelectorCandidate::named(zone.id.to_string(), zone.name.clone(), zone))
            .collect();
        let zone = crate::mcp::selector::resolve(zone_query, zone_candidates)
            .map_err(|error| ToolError::selector("zone", error))?;
        if zone.role == ZoneRole::Display {
            return Err(ToolError::InvalidParam {
                param: "zone".into(),
                reason: "display-face controls are adjusted through the display tools".into(),
            });
        }

        let layer_candidates = zone
            .layers
            .iter()
            .cloned()
            .map(|layer| match layer.name.clone() {
                Some(name) => SelectorCandidate::named(layer.id.to_string(), name, layer),
                None => SelectorCandidate::unnamed(layer.id.to_string(), layer),
            })
            .collect();
        let layer = crate::mcp::selector::resolve(layer_query, layer_candidates)
            .map_err(|error| ToolError::selector("layer", error))?;

        let mut values = std::collections::HashMap::with_capacity(patch.values.len());
        for (name, value) in &patch.values {
            values.insert(
                name.clone(),
                value
                    .to_effect_wire()
                    .map_err(|error| ToolError::InvalidParam {
                        param: format!("values.{name}"),
                        reason: error.to_string(),
                    })?,
            );
        }

        match crate::domain::scene_tree::patch_layer_controls(
            &state.scene_tree,
            PatchLayerControls {
                zone_id: zone.id,
                layer_id: layer.id,
                values,
                clear_bindings: patch.clear_bindings.clone(),
                expected_revision: Some(revision),
            },
            MutationContext::mcp(),
        )
        .await
        {
            Ok(written) => {
                return serialize_result(AdjustControlsResult {
                    zone: written.zone,
                    revision: written.revision,
                });
            }
            Err(error) if scene_snapshot_was_superseded(&error) => {}
            Err(error) => return Err(error.into()),
        }
    }
}

fn scene_snapshot_was_superseded(error: &DomainError) -> bool {
    matches!(error, DomainError::PreconditionFailed { .. })
}

pub(super) async fn handle_list_scenes_with_state(
    params: &Value,
    state: &AppState,
) -> Result<Value, ToolError> {
    let enabled_only = params
        .get("enabled_only")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let scene_manager = state.scene_manager.snapshot().await;
    let active_scene_id = scene_manager.active_scene_id().copied();
    let scenes = scene_manager
        .list()
        .into_iter()
        .filter(|scene| scene.kind != SceneKind::Ephemeral)
        .filter(|scene| !enabled_only || scene.enabled)
        .map(|scene| SceneListItem {
            id: scene.id.to_string(),
            name: scene.name.clone(),
            description: scene.description.clone(),
            enabled: scene.enabled,
            mutation_mode: scene.mutation_mode,
            active: Some(scene.id) == active_scene_id,
        })
        .collect::<Vec<_>>();

    let total = scenes.len();
    serialize_result(SceneListResult { scenes, total })
}

pub(super) async fn handle_create_scene_with_state(
    params: &Value,
    state: &AppState,
) -> Result<Value, ToolError> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::MissingParam("name".into()))?;
    let enabled = params
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let mutation_mode = match params.get("mutation_mode").and_then(Value::as_str) {
        Some("snapshot") => SceneMutationMode::Snapshot,
        Some("live") | None => SceneMutationMode::Live,
        Some(other) => {
            return Err(ToolError::InvalidParam {
                param: "mutation_mode".into(),
                reason: format!("unsupported mutation mode: {other}"),
            });
        }
    };

    let created = create_scene(
        state,
        CreateScene {
            name: name.to_owned(),
            description: params
                .get("description")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            enabled: Some(enabled),
            mutation_mode: Some(mutation_mode),
            metadata: std::collections::HashMap::default(),
        },
        MutationContext::mcp(),
    )
    .await?;

    serialize_result(CreateSceneResult {
        scene_id: created.scene.id.to_string(),
        name: created.scene.name,
        enabled: created.scene.enabled,
        mutation_mode: created.scene.mutation_mode,
    })
}
