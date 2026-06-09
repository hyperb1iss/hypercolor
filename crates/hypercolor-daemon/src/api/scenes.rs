//! Scene endpoints — `/api/v1/scenes/*`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::response::Response;
use serde::{Deserialize, Serialize};
use tracing::warn;

use hypercolor_core::scene::{SceneManager, default_primary_group};
use hypercolor_types::asset::AssetId;
use hypercolor_types::config::MediaConfig;
use hypercolor_types::event::{HypercolorEvent, SceneLibraryChangeKind};
use hypercolor_types::layer::{LayerSource, SceneLayer};
use hypercolor_types::scene::{
    ColorInterpolation, EasingFunction, Scene, SceneId, SceneKind, SceneMutationMode,
    ScenePriority, SceneScope, TransitionSpec, UnassignedBehavior, Zone,
};

use crate::api::AppState;
use crate::api::envelope::{ApiError, ApiResponse};
use crate::api::{
    persist_runtime_session, publish_active_scene_changed, save_scene_store_snapshot,
};

const MEDIA_SOFT_PRODUCER_COST_US: u64 = 60_000;
const LOTTIE_PRODUCER_COST_US: u64 = 8_000;
const VIDEO_PRODUCER_COST_US: u64 = 20_000;
const LIVESTREAM_PRODUCER_COST_US: u64 = 25_000;

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
    pub groups: Vec<Zone>,
    pub groups_revision: u64,
    pub unassigned_behavior: UnassignedBehavior,
}

/// Tell subscribers the saved-scene library changed so scene pickers can
/// refresh their lists without polling.
fn publish_scene_library_changed(
    state: &AppState,
    scene_id: SceneId,
    kind: SceneLibraryChangeKind,
    name: Option<String>,
) {
    state
        .event_bus
        .publish(HypercolorEvent::SceneLibraryChanged {
            scene_id,
            kind,
            name,
        });
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
    crate::api::displays::sync_active_display_surfaces(&state).await;

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
        groups_revision: scene.groups_revision,
        unassigned_behavior: scene.unassigned_behavior.clone(),
    })
}

/// `POST /api/v1/scenes` — Create a new scene.
pub async fn create_scene(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateSceneRequest>,
) -> Response {
    // Every scene is born with a Default zone holding the current device
    // output roster, so the Studio scene selector always has a zone to
    // select (§5.2). The zone is `Primary`; the user renames it freely.
    let default_layout = crate::api::effects::resolve_full_scope_layout(state.as_ref()).await;
    let default_zone = default_primary_group(default_layout);

    let scene = Scene {
        id: SceneId::new(),
        name: body.name,
        description: body.description,
        scope: SceneScope::Full,
        zone_assignments: Vec::new(),
        groups: vec![default_zone],
        groups_revision: 0,
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

    let scene_id = scene.id;
    {
        let mut manager = state.scene_manager.write().await;
        if let Err(e) = manager.create(scene) {
            return ApiError::conflict(format!("Failed to create scene: {e}"));
        }
    }

    if let Err(error) = save_scene_store_snapshot(state.as_ref()).await {
        return ApiError::internal(format!("Failed to persist scenes: {error}"));
    }

    publish_scene_library_changed(
        state.as_ref(),
        scene_id,
        SceneLibraryChangeKind::Created,
        Some(summary.name.clone()),
    );

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
        groups_revision: existing.groups_revision,
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

    publish_scene_library_changed(
        state.as_ref(),
        scene_id,
        SceneLibraryChangeKind::Updated,
        Some(summary.name.clone()),
    );

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
    let current_active_scene = manager.active_scene().cloned();
    drop(manager);

    if let Err(error) = save_scene_store_snapshot(state.as_ref()).await {
        return ApiError::internal(format!("Failed to persist scenes: {error}"));
    }
    persist_runtime_session(&state).await;
    if previous_active_scene != current_active_scene.as_ref().map(|scene| scene.id)
        && let Some(current_active_scene) = current_active_scene.as_ref()
    {
        publish_active_scene_changed(
            state.as_ref(),
            previous_active_scene,
            current_active_scene,
            hypercolor_types::event::SceneChangeReason::UserDeactivate,
        );
    }
    publish_scene_library_changed(
        state.as_ref(),
        scene_id,
        SceneLibraryChangeKind::Deleted,
        None,
    );

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
    let asset_mime_types = asset_mime_types(state.as_ref()).await;
    let media_config = current_media_config(state.as_ref());
    let mut manager = state.scene_manager.write().await;
    let Some(scene_id) = resolve_scene_id(&manager, &id) else {
        return ApiError::not_found(format!("Scene not found: {id}"));
    };
    let previous_active_scene = manager.active_scene_id().copied();

    let (scene_name, media_admission) = match manager.get(&scene_id) {
        Some(scene) => {
            let media_admission = scene_media_admission_counts(scene, &asset_mime_types);
            if let Some(response) = validate_scene_media_admission(&media_admission, &media_config)
            {
                return response;
            }
            (scene.name.clone(), media_admission)
        }
        None => return ApiError::not_found(format!("Scene not found: {id}")),
    };

    if let Err(e) = manager.activate(&scene_id, None) {
        return ApiError::internal(format!("Failed to activate scene: {e}"));
    }
    let current_active_scene = manager.active_scene().cloned();
    drop(manager);

    apply_scene_media_soft_admission(
        state.as_ref(),
        scene_id,
        &scene_name,
        media_admission.estimated_cost_us,
    )
    .await;

    persist_runtime_session(&state).await;
    if previous_active_scene != current_active_scene.as_ref().map(|scene| scene.id)
        && let Some(current_active_scene) = current_active_scene.as_ref()
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
    let current_active_scene = manager.active_scene().cloned();
    let current_active_scene_id = current_active_scene.as_ref().map(|scene| scene.id);
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
        && let Some(current_active_scene) = current_active_scene.as_ref()
    {
        publish_active_scene_changed(
            state.as_ref(),
            previous_active_scene_id,
            current_active_scene,
            hypercolor_types::event::SceneChangeReason::UserDeactivate,
        );
    }

    ApiResponse::ok(serde_json::json!({
        "deactivated": true,
        "previous_scene": previous_scene,
        "scene": current_scene,
    }))
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

pub(crate) async fn asset_mime_types(state: &AppState) -> HashMap<AssetId, String> {
    let library = state.asset_library.read().await;
    library
        .records()
        .iter()
        .map(|record| (record.id, record.mime_type.clone()))
        .collect()
}

pub(crate) fn current_media_config(state: &AppState) -> MediaConfig {
    state
        .config_manager
        .as_ref()
        .map_or_else(MediaConfig::default, |manager| manager.get().media.clone())
}

pub(crate) fn validate_scene_media_admission(
    counts: &MediaAdmissionCounts,
    media_config: &MediaConfig,
) -> Option<Response> {
    let Some(details) = scene_media_admission_violation_details(counts, media_config) else {
        return None;
    };

    Some(ApiError::validation_with_details(
        details.message,
        serde_json::json!({
            "caps": details.caps,
            "counts": details.counts,
            "layers": details.layers,
        }),
    ))
}

pub(crate) struct MediaAdmissionViolationDetails {
    pub message: String,
    pub caps: serde_json::Value,
    pub counts: serde_json::Value,
    pub layers: serde_json::Value,
}

pub(crate) fn scene_media_admission_violation_details(
    counts: &MediaAdmissionCounts,
    media_config: &MediaConfig,
) -> Option<MediaAdmissionViolationDetails> {
    let video_cap = usize::from(media_config.max_video_producers.clamp(1, 4));
    let livestream_cap = usize::from(media_config.max_livestream_producers.clamp(0, 2));
    let video_count = counts.video_asset_ids.len();
    let livestream_count = counts.livestream_asset_ids.len();

    if video_count <= video_cap && livestream_count <= livestream_cap {
        return None;
    }

    let mut violations = Vec::new();
    if video_count > video_cap {
        violations.push(format!("video producers {video_count}/{video_cap}"));
    }
    if livestream_count > livestream_cap {
        violations.push(format!(
            "livestream producers {livestream_count}/{livestream_cap}"
        ));
    }

    Some(MediaAdmissionViolationDetails {
        message: format!(
            "Scene exceeds media producer caps: {}",
            violations.join(", ")
        ),
        caps: serde_json::json!({
            "video": video_cap,
            "livestream": livestream_cap,
        }),
        counts: serde_json::json!({
            "video": video_count,
            "livestream": livestream_count,
        }),
        layers: serde_json::json!({
            "video": counts.video_layers,
            "livestream": counts.livestream_layers,
        }),
    })
}

#[derive(Debug, Default)]
pub(crate) struct MediaAdmissionCounts {
    video_asset_ids: HashSet<AssetId>,
    livestream_asset_ids: HashSet<AssetId>,
    lottie_asset_ids: HashSet<AssetId>,
    estimated_cost_us: u64,
    video_layers: Vec<serde_json::Value>,
    livestream_layers: Vec<serde_json::Value>,
}

pub(crate) fn scene_media_admission_counts(
    scene: &Scene,
    asset_mime_types: &HashMap<AssetId, String>,
) -> MediaAdmissionCounts {
    let mut counts = MediaAdmissionCounts::default();

    for group in scene.groups.iter().filter(|group| group.enabled) {
        for layer in group
            .effective_layers()
            .iter()
            .filter(|layer| layer.enabled)
        {
            let LayerSource::Media { asset_id, .. } = &layer.source else {
                continue;
            };
            let Some(mime_type) = asset_mime_types.get(asset_id) else {
                continue;
            };

            match mime_type.as_str() {
                "video/mp4" | "video/webm" => {
                    if counts.video_asset_ids.insert(*asset_id) {
                        counts.estimated_cost_us = counts
                            .estimated_cost_us
                            .saturating_add(VIDEO_PRODUCER_COST_US);
                    }
                    counts.video_layers.push(media_admission_layer_detail(
                        group, layer, *asset_id, mime_type,
                    ));
                }
                "application/vnd.hypercolor.stream-url" => {
                    if counts.livestream_asset_ids.insert(*asset_id) {
                        counts.estimated_cost_us = counts
                            .estimated_cost_us
                            .saturating_add(LIVESTREAM_PRODUCER_COST_US);
                    }
                    counts.livestream_layers.push(media_admission_layer_detail(
                        group, layer, *asset_id, mime_type,
                    ));
                }
                "application/json" if counts.lottie_asset_ids.insert(*asset_id) => {
                    counts.estimated_cost_us = counts
                        .estimated_cost_us
                        .saturating_add(LOTTIE_PRODUCER_COST_US);
                }
                _ => {}
            }
        }
    }

    counts
}

async fn apply_scene_media_soft_admission(
    state: &AppState,
    scene_id: SceneId,
    scene_name: &str,
    estimated_cost_us: u64,
) {
    if estimated_cost_us <= MEDIA_SOFT_PRODUCER_COST_US {
        return;
    }

    let mut render_loop = state.render_loop.write().await;
    let current_tier = render_loop.stats().tier;
    let Some(next_tier) = current_tier.downshift() else {
        warn!(
            %scene_id,
            scene_name,
            estimated_cost_us,
            soft_cap_us = MEDIA_SOFT_PRODUCER_COST_US,
            current_tier = %current_tier,
            "Scene media producer cost exceeds soft cap but render loop is already at minimum tier"
        );
        return;
    };

    warn!(
        %scene_id,
        scene_name,
        estimated_cost_us,
        soft_cap_us = MEDIA_SOFT_PRODUCER_COST_US,
        previous_tier = %current_tier,
        next_tier = %next_tier,
        "Scene media producer cost exceeds soft cap; preemptively downshifting render loop"
    );
    render_loop.set_tier(next_tier);
}

fn media_admission_layer_detail(
    group: &Zone,
    layer: &SceneLayer,
    asset_id: AssetId,
    mime_type: &str,
) -> serde_json::Value {
    serde_json::json!({
        "group_id": group.id.to_string(),
        "group_name": &group.name,
        "layer_id": layer.id.to_string(),
        "layer_name": &layer.name,
        "asset_id": asset_id.to_string(),
        "mime_type": mime_type,
    })
}
