//! Scene endpoints — `/api/v1/scenes/*`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use tracing::warn;

use hypercolor_core::scene::SceneManager;
use hypercolor_types::asset::AssetId;
use hypercolor_types::config::MediaConfig;
use hypercolor_types::layer::{LayerSource, SceneLayer};
use hypercolor_types::scene::{Scene, SceneId, SceneKind, Zone};

use crate::api::AppState;
use crate::api::envelope::ApiResponse;
use crate::domain::{DomainError, ResourceKind};

const MEDIA_SOFT_PRODUCER_COST_US: u64 = 60_000;
const LOTTIE_PRODUCER_COST_US: u64 = 8_000;
const VIDEO_PRODUCER_COST_US: u64 = 20_000;
const LIVESTREAM_PRODUCER_COST_US: u64 = 25_000;

// ── Request / Response Types ─────────────────────────────────────────────

// Wire contracts live in hypercolor-types::api::scenes — shared with the
// web UI and the TUI.
pub use hypercolor_types::api::scenes::{
    ActivateSceneResponse, ActivatedSceneRef, ActiveSceneResponse, CreateSceneRequest,
    DeactivateSceneResponse, DeleteSceneResponse, ReplaceSceneRequest, SceneListResponse,
    SceneSummary,
};

// ── Handlers ─────────────────────────────────────────────────────────────

/// `GET /api/v1/scenes` — List all scenes.
pub async fn list_scenes(State(state): State<Arc<AppState>>) -> Response {
    let manager = state.scene_manager.read().await;
    let scenes = manager.list();

    let items: Vec<SceneSummary> = scenes
        .iter()
        .filter(|scene| scene.kind != SceneKind::Ephemeral)
        .map(|scene| scene_summary(scene))
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
        return DomainError::not_found(ResourceKind::Scene, &id).into_response();
    };

    let Some(scene) = manager.get(&scene_id) else {
        return DomainError::not_found(ResourceKind::Scene, &id).into_response();
    };

    let revision = state.scene_commits.revision();
    crate::api::scene::with_revision(
        ApiResponse::ok(crate::domain::scene_tree::scene_document(scene, revision)),
        revision,
    )
}

/// `GET /api/v1/scenes/active` — Get the currently active scene, including Default.
pub async fn get_active_scene(State(state): State<Arc<AppState>>) -> Response {
    crate::api::displays::sync_active_display_surfaces(&state).await;

    let manager = state.scene_manager.read().await;
    let Some(scene) = manager.active_scene() else {
        return DomainError::not_found(ResourceKind::Scene, "active").into_response();
    };

    ApiResponse::ok(ActiveSceneResponse {
        id: scene.id.to_string(),
        name: scene.name.clone(),
        description: scene.description.clone(),
        enabled: scene.enabled,
        priority: scene.priority.0,
        kind: scene.kind,
        mutation_mode: scene.mutation_mode,
        zones: scene.groups.clone(),
        zones_revision: scene.groups_revision,
        unassigned_behavior: scene.unassigned_behavior.clone(),
    })
}

/// `POST /api/v1/scenes` — Create a new scene.
pub async fn create_scene(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateSceneRequest>,
) -> Response {
    let created = match crate::domain::scene::create_scene(
        state.as_ref(),
        crate::domain::scene::CreateScene {
            name: body.name,
            description: body.description,
            enabled: body.enabled,
            mutation_mode: body.mutation_mode,
            metadata: HashMap::new(),
        },
        crate::domain::MutationContext::api(),
    )
    .await
    {
        Ok(created) => created,
        Err(error) => return error.into_response(),
    };

    ApiResponse::created(scene_summary(&created.scene))
}

/// `PUT /api/v1/scenes/:id` — Replace a complete stored scene document.
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
    let Some(scene_id) = resolve_scene_id(&*state.scene_manager.read().await, &id) else {
        return DomainError::not_found(ResourceKind::Scene, &id).into_response();
    };

    let updated = match crate::domain::scene::replace_scene(
        state.as_ref(),
        crate::domain::scene::ReplaceScene {
            scene_id,
            document: body,
            expected_revision,
        },
        crate::domain::MutationContext::api(),
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
        ApiResponse::ok(crate::domain::scene_tree::scene_document(
            &updated.scene,
            revision,
        )),
        revision,
    )
}

/// `DELETE /api/v1/scenes/:id` — Delete a scene.
pub async fn delete_scene(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let Some(scene_id) = resolve_scene_id(&*state.scene_manager.read().await, &id) else {
        return DomainError::not_found(ResourceKind::Scene, &id).into_response();
    };

    if let Err(error) = crate::domain::scene::delete_scene(
        state.as_ref(),
        scene_id,
        crate::domain::MutationContext::api(),
    )
    .await
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

    ApiResponse::ok(DeleteSceneResponse { id, deleted: true })
}

/// `POST /api/v1/scenes/:id/activate` — Manually activate a scene.
pub async fn activate_scene(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    // The media-cap violation body is a frozen v1 shape, so the adapter
    // renders it from the shared evaluation rather than from the service's
    // error text. The service enforces the same rule regardless.
    let (scene_id, admission) = {
        let asset_mime_types = asset_mime_types(state.as_ref()).await;
        let media_config = current_media_config(state.as_ref());
        let manager = state.scene_manager.read().await;
        let Some(scene_id) = resolve_scene_id(&manager, &id) else {
            return DomainError::not_found(ResourceKind::Scene, &id).into_response();
        };
        let Some(scene) = manager.get(&scene_id) else {
            return DomainError::not_found(ResourceKind::Scene, &id).into_response();
        };
        let admission = crate::domain::scene::evaluate_scene_media_admission(
            scene,
            &asset_mime_types,
            &media_config,
        );
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
        state.as_ref(),
        crate::domain::scene::ActivateScene {
            scene_id,
            transition: None,
        },
        crate::domain::MutationContext::api(),
    )
    .await
    {
        Ok(activated) => activated,
        Err(error) => return error.into_response(),
    };

    ApiResponse::ok(ActivateSceneResponse {
        scene: ActivatedSceneRef {
            id: activated.scene_id.to_string(),
            name: activated.scene_name,
        },
        activated: true,
    })
}

/// `POST /api/v1/scenes/deactivate` — Return to the synthesized default scene.
pub async fn deactivate_scene(State(state): State<Arc<AppState>>) -> Response {
    let deactivated = match crate::domain::scene::deactivate_scene(
        state.as_ref(),
        crate::domain::MutationContext::api(),
    )
    .await
    {
        Ok(deactivated) => deactivated,
        Err(error) => return error.into_response(),
    };

    ApiResponse::ok(DeactivateSceneResponse {
        deactivated: true,
        previous_scene: deactivated.previous_scene.as_ref().map(scene_summary),
        scene: deactivated.current_scene.as_ref().map(scene_summary),
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

#[derive(Debug)]
pub struct MediaAdmissionViolationDetails {
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

impl MediaAdmissionCounts {
    /// Estimated per-frame producer cost in microseconds.
    pub(crate) const fn estimated_cost_us(&self) -> u64 {
        self.estimated_cost_us
    }
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

pub(crate) async fn apply_scene_media_soft_admission(
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
    zone: &Zone,
    layer: &SceneLayer,
    asset_id: AssetId,
    mime_type: &str,
) -> serde_json::Value {
    serde_json::json!({
        "zone_id": zone.id.to_string(),
        "zone_name": &zone.name,
        "layer_id": layer.id.to_string(),
        "layer_name": &layer.name,
        "asset_id": asset_id.to_string(),
        "mime_type": mime_type,
    })
}
