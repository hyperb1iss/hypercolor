//! Scene endpoints — `/api/v1/scenes/*`.

use std::collections::HashMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::response::Response;
use serde::{Deserialize, Serialize};

use hypercolor_types::scene::{
    ColorInterpolation, EasingFunction, Scene, SceneId, ScenePriority, SceneScope, TransitionSpec,
};
use hypercolor_core::scene::SceneManager;

use crate::api::AppState;
use crate::api::envelope::{ApiError, ApiResponse};

// ── Request / Response Types ─────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct CreateSceneRequest {
    pub name: String,
    pub description: Option<String>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateSceneRequest {
    pub name: String,
    pub description: Option<String>,
    pub enabled: Option<bool>,
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
}

// ── Handlers ─────────────────────────────────────────────────────────────

/// `GET /api/v1/scenes` — List all scenes.
pub async fn list_scenes(State(state): State<Arc<AppState>>) -> Response {
    let manager = state.scene_manager.read().await;
    let scenes = manager.list();

    let items: Vec<SceneSummary> = scenes
        .iter()
        .map(|s| SceneSummary {
            id: s.id.to_string(),
            name: s.name.clone(),
            description: s.description.clone(),
            enabled: s.enabled,
            priority: s.priority.0,
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
        transition: TransitionSpec {
            duration_ms: 1000,
            easing: EasingFunction::Linear,
            color_interpolation: ColorInterpolation::Oklab,
        },
        priority: ScenePriority::USER,
        enabled: body.enabled.unwrap_or(true),
        metadata: HashMap::new(),
    };

    let summary = SceneSummary {
        id: scene.id.to_string(),
        name: scene.name.clone(),
        description: scene.description.clone(),
        enabled: scene.enabled,
        priority: scene.priority.0,
    };

    if let Err(e) = manager.create(scene) {
        return ApiError::conflict(format!("Failed to create scene: {e}"));
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
        transition: existing.transition,
        priority: existing.priority,
        enabled: body.enabled.unwrap_or(existing.enabled),
        metadata: existing.metadata,
    };

    let summary = SceneSummary {
        id: updated.id.to_string(),
        name: updated.name.clone(),
        description: updated.description.clone(),
        enabled: updated.enabled,
        priority: updated.priority.0,
    };

    if let Err(e) = manager.update(updated) {
        return ApiError::internal(format!("Failed to update scene: {e}"));
    }

    ApiResponse::ok(summary)
}

/// `DELETE /api/v1/scenes/:id` — Delete a scene.
pub async fn delete_scene(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let mut manager = state.scene_manager.write().await;
    let Some(scene_id) = resolve_scene_id(&manager, &id) else {
        return ApiError::not_found(format!("Scene not found: {id}"));
    };

    if let Err(e) = manager.delete(&scene_id) {
        return ApiError::not_found(format!("Scene not found: {e}"));
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

    let scene_name = match manager.get(&scene_id) {
        Some(s) => s.name.clone(),
        None => return ApiError::not_found(format!("Scene not found: {id}")),
    };

    if let Err(e) = manager.activate(&scene_id, None) {
        return ApiError::internal(format!("Failed to activate scene: {e}"));
    }

    ApiResponse::ok(serde_json::json!({
        "scene": {
            "id": scene_id.to_string(),
            "name": scene_name,
        },
        "activated": true,
    }))
}

fn resolve_scene_id(manager: &SceneManager, id_or_name: &str) -> Option<SceneId> {
    if let Ok(uuid) = id_or_name.parse::<uuid::Uuid>() {
        return Some(SceneId(uuid));
    }

    manager
        .list()
        .iter()
        .find(|scene| scene.name.eq_ignore_ascii_case(id_or_name))
        .map(|scene| scene.id)
}
