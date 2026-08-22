//! Scene endpoints — `/api/v1/scenes/*`.

use std::collections::HashMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};

use hypercolor_core::scene::SceneManager;
use hypercolor_types::scene::{Scene, SceneId, SceneKind};

use crate::api::envelope;
use crate::app_state::AppState;
use crate::domain::{DomainError, ResourceKind};

// ── Request / Response Types ─────────────────────────────────────────────

// Wire contracts live in hypercolor-types::api::scenes — shared with the
// web UI and the TUI.
pub use hypercolor_types::api::scenes::{
    ActivateSceneResponse, ActivatedSceneRef, CreateSceneRequest, DeleteSceneResponse,
    ReplaceSceneRequest, SceneListResponse, SceneSummary, SnapshotSceneRequest,
};

// ── Handlers ─────────────────────────────────────────────────────────────

/// `GET /api/v1/scenes` — List all scenes.
pub async fn list_scenes(State(state): State<Arc<AppState>>) -> Response {
    let manager = state.scene_manager.snapshot().await;
    let scenes = manager.list();

    let items: Vec<SceneSummary> = scenes
        .iter()
        .filter(|scene| scene.kind != SceneKind::Ephemeral)
        .map(|scene| scene_summary(scene))
        .collect();

    let total = items.len();
    envelope::ok(SceneListResponse {
        items,
        total: u64::try_from(total).expect("scene count fits in u64"),
        page: None,
    })
}

/// `GET /api/v1/scenes/{id}` — Get a single scene.
pub async fn get_scene(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let manager = state.scene_manager.snapshot().await;
    let Some(scene_id) = resolve_stored_scene_id(&manager, &id) else {
        return DomainError::not_found(ResourceKind::Scene, &id).into_response();
    };

    let Some(scene) = manager.get(&scene_id) else {
        return DomainError::not_found(ResourceKind::Scene, &id).into_response();
    };

    let revision = state.scene_manager.revision();
    crate::api::scene::with_revision(
        envelope::ok(crate::domain::scene_tree::scene_document(scene, revision)),
        revision,
    )
}

/// `POST /api/v1/scenes` — Create a new scene.
pub async fn create_scene(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateSceneRequest>,
) -> Response {
    let created = match crate::domain::scene::create_scene(
        &state.domains.scene_library,
        crate::domain::scene::CreateScene {
            name: body.name,
            description: body.description,
            enabled: body.enabled,
            mutation_mode: body.mutation_mode,
            metadata: HashMap::new(),
        },
    )
    .await
    {
        Ok(created) => created,
        Err(error) => return error.into_response(),
    };

    envelope::created(scene_summary(&created.scene))
}

/// `POST /api/v1/scenes/snapshot` — Save the current runtime scene.
pub async fn snapshot_scene(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SnapshotSceneRequest>,
) -> Response {
    let created = match crate::domain::scene::snapshot_scene(
        &state.domains.scene_library,
        crate::domain::scene::SnapshotScene {
            name: body.name,
            description: body.description,
        },
    )
    .await
    {
        Ok(created) => created,
        Err(error) => return error.into_response(),
    };

    envelope::created(scene_summary(&created.scene))
}

/// `PUT /api/v1/scenes/{id}` — Replace a complete stored scene document.
pub async fn update_scene(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<ReplaceSceneRequest>,
) -> Response {
    let expected_revision = match crate::api::scene::parse_if_match(&headers) {
        Ok(revision) => revision,
        Err(error) => return error.into_response(),
    };
    let manager = state.scene_manager.snapshot().await;
    let Some(scene_id) = resolve_stored_scene_id(&manager, &id) else {
        return DomainError::not_found(ResourceKind::Scene, &id).into_response();
    };

    let updated = match crate::domain::scene::replace_scene(
        &state.domains.scene_library,
        crate::domain::scene::ReplaceScene {
            scene_id,
            document: body,
            expected_revision,
        },
    )
    .await
    {
        Ok(updated) => updated,
        // The service reports the resolved id; the caller gets back the
        // id it actually sent.
        Err(DomainError::NotFound { .. }) => {
            return DomainError::not_found(ResourceKind::Scene, &id).into_response();
        }
        Err(error) => return error.into_response(),
    };

    let revision = updated.commit.revision();
    crate::api::scene::with_revision(
        envelope::ok(crate::domain::scene_tree::scene_document(
            &updated.scene,
            revision,
        )),
        revision,
    )
}

/// `DELETE /api/v1/scenes/{id}` — Delete a scene.
pub async fn delete_scene(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let manager = state.scene_manager.snapshot().await;
    let Some(scene_id) = resolve_scene_id(&manager, &id) else {
        return DomainError::not_found(ResourceKind::Scene, &id).into_response();
    };

    if let Err(error) =
        crate::domain::scene::delete_scene(&state.domains.scene_library, scene_id).await
    {
        return match error {
            // The service reports the resolved id; the caller gets back
            // the id it actually sent.
            DomainError::NotFound { .. } => {
                DomainError::not_found(ResourceKind::Scene, &id).into_response()
            }
            other => other.into_response(),
        };
    }

    envelope::ok(DeleteSceneResponse { id, deleted: true })
}

/// `POST /api/v1/scenes/{id}/activate` — Manually activate a scene.
pub async fn activate_scene(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    // The media-cap violation body is a frozen v1 shape, so the adapter
    // renders it from the shared evaluation rather than from the service's
    // error text. The service enforces the same rule regardless.
    let (scene_id, admission) = {
        let manager = state.scene_manager.snapshot().await;
        let Some(scene_id) = resolve_scene_id(&manager, &id) else {
            return DomainError::not_found(ResourceKind::Scene, &id).into_response();
        };
        let Some(scene) = manager.get(&scene_id) else {
            return DomainError::not_found(ResourceKind::Scene, &id).into_response();
        };
        let admission = state.domains.scene.evaluate_media_admission(scene).await;
        (scene_id, admission)
    };
    if let Some(violation) = admission.violation.as_ref() {
        return DomainError::validation_details(
            violation.message.clone(),
            serde_json::json!({
                "caps": violation.caps,
                "counts": violation.counts,
                "layers": violation.layers,
            }),
        )
        .into_response();
    }

    let activated = match crate::domain::scene::activate_scene(
        &state.domains.scene_library,
        crate::domain::scene::ActivateScene {
            scene_id,
            transition: None,
        },
    )
    .await
    {
        Ok(activated) => activated,
        Err(error) => return error.into_response(),
    };

    envelope::ok(ActivateSceneResponse {
        scene: ActivatedSceneRef {
            id: activated.scene_id.to_string(),
            name: activated.scene_name,
        },
        activated: true,
        layout: activated.layout,
        brightness: activated.brightness,
    })
}

/// The scene summary every scene-library response carries.
fn scene_summary(scene: &Scene) -> SceneSummary {
    SceneSummary {
        id: scene.id.to_string(),
        name: scene.name.clone(),
        description: scene.description.clone(),
        enabled: scene.enabled,
        priority: scene.priority.0,
        mutation_mode: scene.mutation_mode,
    }
}

pub(crate) fn resolve_scene_id(manager: &SceneManager, id_or_name: &str) -> Option<SceneId> {
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

fn resolve_stored_scene_id(manager: &SceneManager, id_or_name: &str) -> Option<SceneId> {
    resolve_scene_id(manager, id_or_name).filter(|scene_id| {
        manager
            .get(scene_id)
            .is_some_and(|scene| scene.kind == SceneKind::Named)
    })
}
