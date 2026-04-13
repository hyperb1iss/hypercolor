//! Scene endpoints — `/api/v1/scenes/*`.

use std::collections::HashMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::response::Response;
use serde::{Deserialize, Serialize};

use hypercolor_core::scene::SceneManager;
use hypercolor_types::scene::{
    ColorInterpolation, EasingFunction, RenderGroup, Scene, SceneId, SceneKind, SceneMutationMode,
    ScenePriority, SceneScope, TransitionSpec, UnassignedBehavior,
};

use crate::api::AppState;
use crate::api::envelope::{ApiError, ApiResponse};
use crate::api::{
    persist_runtime_session, publish_active_scene_changed, save_scene_store_snapshot,
};

// ── Request / Response Types ─────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct CreateSceneRequest {
    pub name: String,
    pub description: Option<String>,
    pub enabled: Option<bool>,
    pub mutation_mode: Option<SceneMutationMode>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateSceneRequest {
    pub name: String,
    pub description: Option<String>,
    pub enabled: Option<bool>,
    pub mutation_mode: Option<SceneMutationMode>,
}

#[derive(Debug, Serialize)]
pub struct SceneListResponse {
    pub items: Vec<SceneSummary>,
    pub pagination: super::devices::Pagination,
}

#[derive(Debug, Serialize)]
pub struct SceneSummary {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub enabled: bool,
    pub priority: u8,
    pub mutation_mode: SceneMutationMode,
}

#[derive(Debug, Serialize)]
pub struct ActiveSceneResponse {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub enabled: bool,
    pub priority: u8,
    pub kind: SceneKind,
    pub mutation_mode: SceneMutationMode,
    pub groups: Vec<RenderGroup>,
}

// ── Handlers ─────────────────────────────────────────────────────────────

/// `GET /api/v1/scenes` — List all scenes.
pub async fn list_scenes(State(state): State<Arc<AppState>>) -> Response {
    let manager = state.scene_manager.read().await;
    let scenes = manager.list();

    let items: Vec<SceneSummary> = scenes
        .iter()
        .filter(|scene| scene.kind != SceneKind::Ephemeral)
        .map(|s| SceneSummary {
            id: s.id.to_string(),
            name: s.name.clone(),
            description: s.description.clone(),
            enabled: s.enabled,
            priority: s.priority.0,
            mutation_mode: s.mutation_mode,
        })
        .collect();

    let total = items.len();
    ApiResponse::ok(SceneListResponse {
        items,
        pagination: super::devices::Pagination {
            offset: 0,
            limit: 50,
            total,
            has_more: false,
        },
    })
}

/// `GET /api/v1/scenes/:id` — Get a single scene.
pub async fn get_scene(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let manager = state.scene_manager.read().await;
    let Some(scene_id) = resolve_scene_id(&manager, &id) else {
        return ApiError::not_found(format!("Scene not found: {id}"));
    };

    let Some(scene) = manager.get(&scene_id) else {
        return ApiError::not_found(format!("Scene not found: {id}"));
    };

    ApiResponse::ok(SceneSummary {
        id: scene.id.to_string(),
        name: scene.name.clone(),
        description: scene.description.clone(),
        enabled: scene.enabled,
        priority: scene.priority.0,
        mutation_mode: scene.mutation_mode,
    })
}

/// `GET /api/v1/scenes/active` — Get the currently active scene, including Default.
pub async fn get_active_scene(State(state): State<Arc<AppState>>) -> Response {
    let manager = state.scene_manager.read().await;
    let Some(scene) = manager.active_scene() else {
        return ApiError::not_found("No active scene".to_owned());
    };

    ApiResponse::ok(ActiveSceneResponse {
        id: scene.id.to_string(),
        name: scene.name.clone(),
        description: scene.description.clone(),
        enabled: scene.enabled,
        priority: scene.priority.0,
        kind: scene.kind,
        mutation_mode: scene.mutation_mode,
        groups: scene.groups.clone(),
    })
}

/// `POST /api/v1/scenes` — Create a new scene.
pub async fn create_scene(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateSceneRequest>,
) -> Response {
    let mut manager = state.scene_manager.write().await;

    let scene = Scene {
        id: SceneId::new(),
        name: body.name,
        description: body.description,
        scope: SceneScope::Full,
        zone_assignments: Vec::new(),
        groups: Vec::new(),
        transition: TransitionSpec {
            duration_ms: 1000,
            easing: EasingFunction::Linear,
            color_interpolation: ColorInterpolation::Oklab,
        },
        priority: ScenePriority::USER,
        enabled: body.enabled.unwrap_or(true),
        metadata: HashMap::new(),
        unassigned_behavior: UnassignedBehavior::Off,
        kind: SceneKind::Named,
        mutation_mode: body.mutation_mode.unwrap_or(SceneMutationMode::Live),
    };

    let summary = SceneSummary {
        id: scene.id.to_string(),
        name: scene.name.clone(),
        description: scene.description.clone(),
        enabled: scene.enabled,
        priority: scene.priority.0,
        mutation_mode: scene.mutation_mode,
    };

    if let Err(e) = manager.create(scene) {
        return ApiError::conflict(format!("Failed to create scene: {e}"));
    }
    drop(manager);

    if let Err(error) = save_scene_store_snapshot(state.as_ref()).await {
        return ApiError::internal(format!("Failed to persist scenes: {error}"));
    }

    ApiResponse::created(summary)
}

/// `PUT /api/v1/scenes/:id` — Update a scene.
pub async fn update_scene(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateSceneRequest>,
) -> Response {
    let mut manager = state.scene_manager.write().await;
    let Some(scene_id) = resolve_scene_id(&manager, &id) else {
        return ApiError::not_found(format!("Scene not found: {id}"));
    };

    let Some(existing) = manager.get(&scene_id).cloned() else {
        return ApiError::not_found(format!("Scene not found: {id}"));
    };

    let updated = Scene {
        id: existing.id,
        name: body.name,
        description: body.description,
        scope: existing.scope,
        zone_assignments: existing.zone_assignments,
        groups: existing.groups,
        transition: existing.transition,
        priority: existing.priority,
        enabled: body.enabled.unwrap_or(existing.enabled),
        metadata: existing.metadata,
        unassigned_behavior: existing.unassigned_behavior,
        kind: existing.kind,
        mutation_mode: body.mutation_mode.unwrap_or(existing.mutation_mode),
    };

    let summary = SceneSummary {
        id: updated.id.to_string(),
        name: updated.name.clone(),
        description: updated.description.clone(),
        enabled: updated.enabled,
        priority: updated.priority.0,
        mutation_mode: updated.mutation_mode,
    };

    if let Err(e) = manager.update(updated) {
        return ApiError::internal(format!("Failed to update scene: {e}"));
    }
    drop(manager);

    if let Err(error) = save_scene_store_snapshot(state.as_ref()).await {
        return ApiError::internal(format!("Failed to persist scenes: {error}"));
    }

    ApiResponse::ok(summary)
}

/// `DELETE /api/v1/scenes/:id` — Delete a scene.
pub async fn delete_scene(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let mut manager = state.scene_manager.write().await;
    let Some(scene_id) = resolve_scene_id(&manager, &id) else {
        return ApiError::not_found(format!("Scene not found: {id}"));
    };
    if scene_id.is_default() {
        return ApiError::conflict("Default scene cannot be deleted".to_owned());
    }
    let previous_active_scene = manager.active_scene_id().copied();

    if let Err(e) = manager.delete(&scene_id) {
        return ApiError::not_found(format!("Scene not found: {e}"));
    }
    let current_active_scene = manager.active_scene_id().copied();
    drop(manager);

    if let Err(error) = save_scene_store_snapshot(state.as_ref()).await {
        return ApiError::internal(format!("Failed to persist scenes: {error}"));
    }
    persist_runtime_session(&state).await;
    if previous_active_scene != current_active_scene
        && let Some(current_active_scene) = current_active_scene
    {
        publish_active_scene_changed(
            state.as_ref(),
            previous_active_scene,
            current_active_scene,
            hypercolor_types::event::SceneChangeReason::UserDeactivate,
        );
    }

    ApiResponse::ok(serde_json::json!({
        "id": id,
        "deleted": true,
    }))
}

/// `POST /api/v1/scenes/:id/activate` — Manually activate a scene.
pub async fn activate_scene(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    let mut manager = state.scene_manager.write().await;
    let Some(scene_id) = resolve_scene_id(&manager, &id) else {
        return ApiError::not_found(format!("Scene not found: {id}"));
    };
    let previous_active_scene = manager.active_scene_id().copied();

    let scene_name = match manager.get(&scene_id) {
        Some(s) => s.name.clone(),
        None => return ApiError::not_found(format!("Scene not found: {id}")),
    };

    if let Err(e) = manager.activate(&scene_id, None) {
        return ApiError::internal(format!("Failed to activate scene: {e}"));
    }
    let current_active_scene = manager.active_scene_id().copied();
    drop(manager);

    persist_runtime_session(&state).await;
    if previous_active_scene != current_active_scene
        && let Some(current_active_scene) = current_active_scene
    {
        publish_active_scene_changed(
            state.as_ref(),
            previous_active_scene,
            current_active_scene,
            hypercolor_types::event::SceneChangeReason::UserActivate,
        );
    }

    ApiResponse::ok(serde_json::json!({
        "scene": {
            "id": scene_id.to_string(),
            "name": scene_name,
        },
        "activated": true,
    }))
}

/// `POST /api/v1/scenes/deactivate` — Return to the synthesized default scene.
pub async fn deactivate_scene(State(state): State<Arc<AppState>>) -> Response {
    let mut manager = state.scene_manager.write().await;
    let previous_active_scene_id = manager.active_scene_id().copied();
    let previous_scene = manager.active_scene().map(|scene| SceneSummary {
        id: scene.id.to_string(),
        name: scene.name.clone(),
        description: scene.description.clone(),
        enabled: scene.enabled,
        priority: scene.priority.0,
        mutation_mode: scene.mutation_mode,
    });
    manager.deactivate_current();
    let current_active_scene_id = manager.active_scene_id().copied();
    let current_scene = manager.active_scene().map(|scene| SceneSummary {
        id: scene.id.to_string(),
        name: scene.name.clone(),
        description: scene.description.clone(),
        enabled: scene.enabled,
        priority: scene.priority.0,
        mutation_mode: scene.mutation_mode,
    });
    drop(manager);

    persist_runtime_session(&state).await;
    if previous_active_scene_id != current_active_scene_id
        && let Some(current_active_scene_id) = current_active_scene_id
    {
        publish_active_scene_changed(
            state.as_ref(),
            previous_active_scene_id,
            current_active_scene_id,
            hypercolor_types::event::SceneChangeReason::UserDeactivate,
        );
    }

    ApiResponse::ok(serde_json::json!({
        "deactivated": true,
        "previous_scene": previous_scene,
        "scene": current_scene,
    }))
}

fn resolve_scene_id(manager: &SceneManager, id_or_name: &str) -> Option<SceneId> {
    if id_or_name.eq_ignore_ascii_case("default") {
        return Some(SceneId::DEFAULT);
    }

    if let Ok(uuid) = id_or_name.parse::<uuid::Uuid>() {
        return Some(SceneId(uuid));
    }

    manager
        .list()
        .iter()
        .find(|scene| scene.name.eq_ignore_ascii_case(id_or_name))
        .map(|scene| scene.id)
}
