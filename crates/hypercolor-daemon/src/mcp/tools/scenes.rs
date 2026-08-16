//! Scene-related MCP tools: `activate_scene`, `list_scenes`, `create_scene`.

use serde_json::{Value, json};

use super::{ToolDefinition, ToolError, default_output_schema};
use crate::api::scenes::{asset_mime_types, current_media_config};
use crate::api::{
    AppState, admit_scene_store_snapshot, save_admitted_scene_store_snapshot,
    scene_store_coordinator,
};
use crate::domain::MutationContext;
use crate::domain::scene::{ActivateScene, activate_scene, evaluate_scene_media_admission};
use hypercolor_core::scene::make_scene;
use hypercolor_types::scene::TransitionSpec;
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
                    "description": "Scene name or fuzzy query to match against"
                },
                "transition_ms": {
                    "type": "integer",
                    "description": "Crossfade transition duration in milliseconds",
                    "default": 1000,
                    "minimum": 0,
                    "maximum": 10000
                }
            },
            "required": ["name"]
        }),
        output_schema: default_output_schema(),
        read_only: false,
        idempotent: true,
    }
}

pub(super) fn build_list_scenes() -> ToolDefinition {
    ToolDefinition {
        name: "list_scenes".into(),
        title: "List Scenes".into(),
        description: "List all available lighting scenes with their names, descriptions, and trigger configurations.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "enabled_only": {
                    "type": "boolean",
                    "description": "Only show enabled scenes",
                    "default": false
                }
            }
        }),
        output_schema: default_output_schema(),
        read_only: true,
        idempotent: true,
    }
}

pub(super) fn build_create_scene() -> ToolDefinition {
    ToolDefinition {
        name: "create_scene".into(),
        title: "Create Scene".into(),
        description:
            "Create a new lighting scene from the current state or a specified configuration."
                .into(),
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
                "profile_id": {
                    "type": "string",
                    "description": "Profile ID to associate with this scene"
                },
                "trigger": {
                    "type": "object",
                    "description": "Trigger configuration",
                    "properties": {
                        "type": {
                            "type": "string",
                            "enum": ["schedule", "sunset", "sunrise", "device_connect", "device_disconnect", "audio_beat", "webhook"],
                            "description": "Trigger type"
                        },
                        "cron": {
                            "type": "string",
                            "description": "Cron expression for schedule triggers"
                        }
                    },
                    "required": ["type"]
                },
                "transition_ms": {
                    "type": "integer",
                    "description": "Crossfade duration when activated",
                    "default": 1000,
                    "minimum": 0,
                    "maximum": 30000
                },
                "enabled": {
                    "type": "boolean",
                    "description": "Whether the scene is active immediately",
                    "default": true
                },
                "mutation_mode": {
                    "type": "string",
                    "enum": ["live", "snapshot"],
                    "description": "Whether runtime effect and display-face actions are allowed to rewrite the scene",
                    "default": "live"
                }
            },
            "required": ["name", "profile_id", "trigger"]
        }),
        output_schema: default_output_schema(),
        read_only: false,
        idempotent: false,
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

    // Fuzzy name matching is an adapter concern, and a miss stays a
    // structured success payload rather than a JSON-RPC error.
    let matched_scene = {
        let scene_manager = state.scene_manager.read().await;
        scene_manager
            .list()
            .into_iter()
            .find(|scene| {
                scene.name.eq_ignore_ascii_case(name)
                    || scene.name.to_lowercase().contains(&name.to_lowercase())
            })
            .cloned()
    };

    let Some(scene) = matched_scene else {
        return Ok(json!({
            "activated": false,
            "message": format!("No scene matching '{name}' found. Use list_scenes to browse available scenes.")
        }));
    };

    let admission = evaluate_scene_media_admission(&scene, &asset_mime_types, &media_config);
    if let Some(details) = admission.violation.as_ref() {
        return Ok(json!({
            "activated": false,
            "message": details.message,
            "details": {
                "caps": details.caps,
                "counts": details.counts,
                "layers": details.layers,
            }
        }));
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

    Ok(json!({
        "activated": true,
        "scene": {
            "id": activated.scene_id.to_string(),
            "name": activated.scene_name
        },
        "transition_ms": transition_ms
    }))
}

pub(super) async fn handle_list_scenes_with_state(
    params: &Value,
    state: &AppState,
) -> Result<Value, ToolError> {
    let enabled_only = params
        .get("enabled_only")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let scene_manager = state.scene_manager.read().await;
    let active_scene_id = scene_manager.active_scene_id().copied();
    let scenes = scene_manager
        .list()
        .into_iter()
        .filter(|scene| scene.kind != SceneKind::Ephemeral)
        .filter(|scene| !enabled_only || scene.enabled)
        .map(|scene| {
            json!({
                "id": scene.id.to_string(),
                "name": scene.name,
                "description": scene.description,
                "enabled": scene.enabled,
                "mutation_mode": scene.mutation_mode,
                "active": Some(scene.id) == active_scene_id
            })
        })
        .collect::<Vec<_>>();

    Ok(json!({
        "scenes": scenes,
        "total": scenes.len()
    }))
}

pub(super) async fn handle_create_scene_with_state(
    params: &Value,
    state: &AppState,
) -> Result<Value, ToolError> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::MissingParam("name".into()))?;
    let profile_id = params
        .get("profile_id")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::MissingParam("profile_id".into()))?;

    {
        let profiles = state.profiles.read().await;
        if profiles.get(profile_id).is_none() {
            return Err(ToolError::InvalidParam {
                param: "profile_id".into(),
                reason: format!("profile '{profile_id}' not found"),
            });
        }
    }

    let trigger = params
        .get("trigger")
        .ok_or_else(|| ToolError::MissingParam("trigger".into()))?;
    let trigger_type = trigger
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::MissingParam("trigger.type".into()))?;

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

    let mut scene = make_scene(name);
    scene.description = params
        .get("description")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    scene.enabled = enabled;
    scene.mutation_mode = mutation_mode;
    scene
        .metadata
        .insert("profile_id".to_owned(), profile_id.to_owned());
    scene
        .metadata
        .insert("trigger_type".to_owned(), trigger_type.to_owned());

    let scene_id = scene.id.to_string();
    let coordinator = scene_store_coordinator(state).await;
    let pending = {
        let mut scene_manager = state.scene_manager.write().await;
        let rollback = scene_manager.clone();
        scene_manager
            .create(scene)
            .map_err(|error| ToolError::Internal(format!("failed to create scene: {error}")))?;
        admit_scene_store_snapshot(&coordinator, &mut scene_manager, rollback)
            .map_err(|error| ToolError::Internal(format!("failed to persist scenes: {error}")))?
    };
    save_admitted_scene_store_snapshot(state, pending)
        .await
        .map_err(|error| ToolError::Internal(format!("failed to persist scenes: {error}")))?;

    Ok(json!({
        "scene_id": scene_id,
        "name": name,
        "enabled": enabled,
        "mutation_mode": mutation_mode
    }))
}
