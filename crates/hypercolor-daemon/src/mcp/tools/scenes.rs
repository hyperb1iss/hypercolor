//! Scene-related MCP tools and live-tree mutations.

use serde_json::{Value, json};

use super::{ToolDefinition, ToolError, output_schema, serialize_result};
use crate::api::AppState;
use crate::api::scenes::{asset_mime_types, current_media_config};
use crate::domain::scene::{
    ActivateScene, CreateScene, activate_scene, create_scene, evaluate_scene_media_admission,
};
use crate::domain::scene_tree::{ClearScene, PatchLayerControls};
use crate::domain::{DomainError, MutationContext};
use crate::mcp::results::{
    ActivateSceneResult, AdjustControlsResult, CreateSceneResult, SceneListItem, SceneListResult,
};
use crate::mcp::selector::SelectorCandidate;
use hypercolor_types::api::scene::{PatchControlsRequest, SceneDocument};
use hypercolor_types::api::scenes::ActivatedSceneRef;
use hypercolor_types::control::control_value_json_schema;
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
    let control_value_schema = control_value_json_schema();
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
                    "additionalProperties": { "$ref": "#/$defs/controlValue" }
                },
                "clear_bindings": {
                    "type": "array",
                    "description": "Control bindings to remove in the same atomic commit",
                    "default": [],
                    "items": { "type": "string" }
                }
            },
            "required": ["zone", "layer"],
            "additionalProperties": false,
            "$defs": {
                "controlValue": control_value_schema
            }
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
    let asset_mime_types = asset_mime_types(state).await;
    let media_config = current_media_config(state);

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

    let admission = evaluate_scene_media_admission(&scene, &asset_mime_types, &media_config);
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
            state,
            ClearScene {
                zone: None,
                expected_revision: None,
            },
            MutationContext::mcp(),
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

    let document = crate::domain::scene_tree::read_document(state).await?;
    let revision = document.revision;
    let zone = resolve_zone(query, &document)?;
    if zone.role == ZoneRole::Display {
        return Err(ToolError::InvalidParam {
            param: "zone".into(),
            reason: "display zones are cleared through the display-face tools".into(),
        });
    }
    let zone_id = zone.id;

    let written = retry_resolved_scene_target(
        revision,
        || async {
            crate::domain::scene_tree::read_document(state)
                .await
                .map(|document| document.revision)
        },
        |expected_revision| async move {
            crate::domain::scene_tree::clear_scene(
                state,
                ClearScene {
                    zone: Some(zone_id),
                    expected_revision: Some(expected_revision),
                },
                MutationContext::mcp(),
            )
            .await
        },
    )
    .await?;
    serialize_result(written.document)
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

    let document = crate::domain::scene_tree::read_document(state).await?;
    let revision = document.revision;
    let zone = resolve_zone(zone_query, &document)?;
    if zone.role == ZoneRole::Display {
        return Err(ToolError::InvalidParam {
            param: "zone".into(),
            reason: "display-face controls are adjusted through the display tools".into(),
        });
    }
    let layer = resolve_layer(layer_query, &zone)?;
    let zone_id = zone.id;
    let layer_id = layer.id;

    let written = retry_resolved_scene_target(
        revision,
        || async {
            crate::domain::scene_tree::read_document(state)
                .await
                .map(|document| document.revision)
        },
        |expected_revision| {
            let values = patch.values.clone().into_iter().collect();
            let clear_bindings = patch.clear_bindings.clone();
            async move {
                crate::domain::scene_tree::patch_layer_controls(
                    state,
                    PatchLayerControls {
                        zone_id,
                        layer_id,
                        values,
                        clear_bindings,
                        expected_revision: Some(expected_revision),
                    },
                    MutationContext::mcp(),
                )
                .await
            }
        },
    )
    .await?;
    serialize_result(AdjustControlsResult {
        zone: written.zone,
        revision: written.revision,
    })
}

fn resolve_zone(
    query: &str,
    document: &SceneDocument,
) -> Result<hypercolor_types::api::scene::ZoneResource, ToolError> {
    let candidates = document
        .zones
        .iter()
        .cloned()
        .map(|zone| SelectorCandidate::named(zone.id.to_string(), zone.name.clone(), zone))
        .collect();
    crate::mcp::selector::resolve(query, candidates)
        .map_err(|error| ToolError::selector("zone", error))
}

fn resolve_layer(
    query: &str,
    zone: &hypercolor_types::api::scene::ZoneResource,
) -> Result<hypercolor_types::layer::SceneLayer, ToolError> {
    let candidates = zone
        .layers
        .iter()
        .cloned()
        .map(|layer| match layer.name.clone() {
            Some(name) => SelectorCandidate::named(layer.id.to_string(), name, layer),
            None => SelectorCandidate::unnamed(layer.id.to_string(), layer),
        })
        .collect();
    crate::mcp::selector::resolve(query, candidates)
        .map_err(|error| ToolError::selector("layer", error))
}

async fn retry_resolved_scene_target<T, ReadRevision, ReadFuture, Mutate, MutateFuture>(
    mut revision: u64,
    mut read_revision: ReadRevision,
    mut mutate: Mutate,
) -> Result<T, DomainError>
where
    ReadRevision: FnMut() -> ReadFuture,
    ReadFuture: std::future::Future<Output = Result<u64, DomainError>>,
    Mutate: FnMut(u64) -> MutateFuture,
    MutateFuture: std::future::Future<Output = Result<T, DomainError>>,
{
    loop {
        match mutate(revision).await {
            Err(error) if scene_snapshot_was_superseded(&error) => {
                revision = read_revision().await?;
            }
            outcome => return outcome,
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

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::future::ready;
    use std::rc::Rc;

    use hypercolor_types::api::scene::SceneDocument;
    use serde_json::json;

    use super::{resolve_layer, resolve_zone, retry_resolved_scene_target};
    use crate::domain::{DomainError, ResourceKind};

    fn scene_document(
        revision: u64,
        zone_id: &str,
        zone_name: &str,
        layer_id: &str,
        layer_name: &str,
    ) -> SceneDocument {
        serde_json::from_value(json!({
            "id": "00000000-0000-0000-0000-000000000001",
            "name": "live",
            "kind": "ephemeral",
            "is_default": true,
            "revision": revision,
            "zones": [{
                "id": zone_id,
                "name": zone_name,
                "role": "primary",
                "enabled": true,
                "brightness": 1.0,
                "members": [],
                "layers": [{
                    "id": layer_id,
                    "name": layer_name,
                    "source": {
                        "type": "effect",
                        "effect_id": "00000000-0000-0000-0000-00000000000a",
                        "controls": {}
                    }
                }]
            }]
        }))
        .expect("scene fixture should deserialize")
    }

    #[tokio::test]
    async fn clear_retry_keeps_the_zone_id_resolved_before_same_name_replacement() {
        let old_zone = "00000000-0000-0000-0000-000000000010";
        let new_zone = "00000000-0000-0000-0000-000000000020";
        let initial = scene_document(
            1,
            old_zone,
            "Desk",
            "00000000-0000-0000-0000-000000000011",
            "Glow",
        );
        let replacement = Rc::new(scene_document(
            2,
            new_zone,
            "Desk",
            "00000000-0000-0000-0000-000000000021",
            "Glow",
        ));
        let target = resolve_zone("desk", &initial)
            .expect("initial zone selector should resolve")
            .id;
        let attempts = Rc::new(RefCell::new(Vec::new()));
        let mutation_count = Rc::new(Cell::new(0_u8));

        let error = retry_resolved_scene_target(
            initial.revision,
            {
                let replacement = Rc::clone(&replacement);
                move || {
                    assert_eq!(
                        resolve_zone("desk", &replacement)
                            .expect("replacement selector should resolve")
                            .id
                            .to_string(),
                        new_zone
                    );
                    ready(Ok(replacement.revision))
                }
            },
            {
                let attempts = Rc::clone(&attempts);
                let mutation_count = Rc::clone(&mutation_count);
                move |revision| {
                    attempts.borrow_mut().push((target, revision));
                    let result: Result<(), DomainError> =
                        if mutation_count.replace(mutation_count.get() + 1) == 0 {
                            Err(DomainError::PreconditionFailed {
                                resource: ResourceKind::Scene,
                                expected: 1,
                                current: 2,
                            })
                        } else {
                            Err(DomainError::not_found(ResourceKind::Zone, target))
                        };
                    ready(result)
                }
            },
        )
        .await
        .expect_err("the retired zone ID must fail instead of clearing its replacement");

        assert!(matches!(error, DomainError::NotFound { .. }));
        assert_eq!(
            attempts
                .borrow()
                .iter()
                .map(|(zone, revision)| (zone.to_string(), *revision))
                .collect::<Vec<_>>(),
            [(old_zone.to_owned(), 1), (old_zone.to_owned(), 2)]
        );
    }

    #[tokio::test]
    async fn control_retry_keeps_the_layer_id_resolved_before_same_name_replacement() {
        let zone_id = "00000000-0000-0000-0000-000000000010";
        let old_layer = "00000000-0000-0000-0000-000000000011";
        let new_layer = "00000000-0000-0000-0000-000000000012";
        let initial = scene_document(1, zone_id, "Desk", old_layer, "Glow");
        let replacement = Rc::new(scene_document(2, zone_id, "Desk", new_layer, "Glow"));
        let zone = resolve_zone("desk", &initial).expect("zone selector should resolve");
        let target = resolve_layer("glow", &zone)
            .expect("initial layer selector should resolve")
            .id;
        let attempts = Rc::new(RefCell::new(Vec::new()));
        let mutation_count = Rc::new(Cell::new(0_u8));

        let error = retry_resolved_scene_target(
            initial.revision,
            {
                let replacement = Rc::clone(&replacement);
                move || {
                    let zone = resolve_zone("desk", &replacement)
                        .expect("replacement zone selector should resolve");
                    assert_eq!(
                        resolve_layer("glow", &zone)
                            .expect("replacement layer selector should resolve")
                            .id
                            .to_string(),
                        new_layer
                    );
                    ready(Ok(replacement.revision))
                }
            },
            {
                let attempts = Rc::clone(&attempts);
                let mutation_count = Rc::clone(&mutation_count);
                move |revision| {
                    attempts.borrow_mut().push((target, revision));
                    let result: Result<(), DomainError> =
                        if mutation_count.replace(mutation_count.get() + 1) == 0 {
                            Err(DomainError::PreconditionFailed {
                                resource: ResourceKind::Scene,
                                expected: 1,
                                current: 2,
                            })
                        } else {
                            Err(DomainError::not_found(ResourceKind::Layer, target))
                        };
                    ready(result)
                }
            },
        )
        .await
        .expect_err("the retired layer ID must fail instead of patching its replacement");

        assert!(matches!(error, DomainError::NotFound { .. }));
        assert_eq!(
            attempts
                .borrow()
                .iter()
                .map(|(layer, revision)| (layer.to_string(), *revision))
                .collect::<Vec<_>>(),
            [(old_layer.to_owned(), 1), (old_layer.to_owned(), 2)]
        );
    }
}
