//! Scene-related MCP tools: `activate_scene`, `list_scenes`, `create_scene`.

use serde_json::{Value, json};

use super::{ToolDefinition, ToolError, default_output_schema};
use crate::api::{
    AppState, publish_active_scene_changed, save_runtime_session_snapshot,
    save_scene_store_snapshot,
};
use hypercolor_core::scene::make_scene;
use hypercolor_types::event::SceneChangeReason;
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

// ── Stateless Handlers ────────────────────────────────────────────────────

pub(super) fn handle_activate_scene(params: &Value) -> Result<Value, ToolError> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::MissingParam("name".into()))?;

    let _transition_ms = params
        .get("transition_ms")
        .and_then(Value::as_u64)
        .unwrap_or(1000);

    // Would query scene manager with fuzzy matching
    Ok(json!({
        "activated": false,
        "message": format!("No scene matching '{name}' found. Use list_scenes to browse available scenes.")
    }))
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "will return errors when wired to scene manager"
)]
pub(super) fn handle_list_scenes(params: &Value) -> Result<Value, ToolError> {
    let _enabled_only = params
        .get("enabled_only")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    // Would query scene manager
    Ok(json!({
        "scenes": [],
        "total": 0
    }))
}

pub(super) fn handle_create_scene(params: &Value) -> Result<Value, ToolError> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::MissingParam("name".into()))?;

    let _profile_id = params
        .get("profile_id")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::MissingParam("profile_id".into()))?;

    let trigger = params
        .get("trigger")
        .ok_or_else(|| ToolError::MissingParam("trigger".into()))?;

    let _trigger_type = trigger
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::MissingParam("trigger.type".into()))?;

    let enabled = params
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let mutation_mode = params
        .get("mutation_mode")
        .and_then(Value::as_str)
        .unwrap_or("live");

    let scene_id = uuid::Uuid::now_v7().to_string();

    Ok(json!({
        "scene_id": scene_id,
        "name": name,
        "enabled": enabled,
        "mutation_mode": mutation_mode
    }))
}

// ── Stateful Handlers ─────────────────────────────────────────────────────

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

    let mut scene_manager = state.scene_manager.write().await;
    let previous_active_scene = scene_manager.active_scene_id().copied();
    let matched_scene = scene_manager
        .list()
        .into_iter()
        .find(|scene| {
            scene.name.eq_ignore_ascii_case(name)
                || scene.name.to_lowercase().contains(&name.to_lowercase())
        })
        .cloned();

    let Some(scene) = matched_scene else {
        return Ok(json!({
            "activated": false,
            "message": format!("No scene matching '{name}' found. Use list_scenes to browse available scenes.")
        }));
    };

    let transition_override = Some(TransitionSpec {
        duration_ms: transition_ms,
        ..scene.transition.clone()
    });
    scene_manager
        .activate(&scene.id, transition_override)
        .map_err(|error| ToolError::Internal(format!("failed to activate scene: {error}")))?;
    let current_active_scene = scene_manager.active_scene_id().copied();
    drop(scene_manager);
    save_runtime_session_snapshot(state).await;
    if previous_active_scene != current_active_scene {
        publish_active_scene_changed(
            state,
            previous_active_scene,
            scene.id,
            SceneChangeReason::UserActivate,
        );
    }

    Ok(json!({
        "activated": true,
        "scene": {
            "id": scene.id.to_string(),
            "name": scene.name
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
    {
        let mut scene_manager = state.scene_manager.write().await;
        scene_manager
            .create(scene)
            .map_err(|error| ToolError::Internal(format!("failed to create scene: {error}")))?;
    }
    save_scene_store_snapshot(state)
        .await
        .map_err(|error| ToolError::Internal(format!("failed to persist scenes: {error}")))?;

    Ok(json!({
        "scene_id": scene_id,
        "name": name,
        "enabled": enabled,
        "mutation_mode": mutation_mode
    }))
}
