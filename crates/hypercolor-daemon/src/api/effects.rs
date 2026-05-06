//! Effect endpoints — `/api/v1/effects/*`.

use std::collections::{HashMap, HashSet};
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;

use axum::Json;
use axum::extract::{Multipart, Path, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use tokio::fs;
use tracing::{debug, info, warn};
use utoipa::ToSchema;

use hypercolor_core::bus::CanvasFrame;
use hypercolor_core::effect::{
    EffectRegistry, HtmlControlKind, ParsedHtmlEffectMetadata, load_html_effect_file,
    parse_html_effect_metadata,
};
use hypercolor_core::scene::ControlsVersionMismatch;
use hypercolor_types::canvas::Canvas;
use hypercolor_types::device::{DriverModuleKind, DriverTransportKind};
use hypercolor_types::effect::{
    ControlBinding, ControlDefinition, ControlValue, EffectCategory, EffectId, EffectMetadata,
    EffectSource, PresetTemplate,
};
use hypercolor_types::event::{
    ChangeTrigger, EffectRef, EffectStopReason, EventControlValue, FrameData, HypercolorEvent,
    RenderGroupChangeKind,
};
use hypercolor_types::scene::RenderGroup;
use hypercolor_types::session::OffOutputBehavior;
use hypercolor_types::spatial::SpatialLayout;

use crate::api::AppState;
use crate::api::control_values::json_to_control_value;
use crate::api::envelope::{ApiError, ApiResponse};
use crate::api::{
    ActiveSceneMutationError, active_scene_id_for_runtime_mutation, publish_render_group_changed,
};
use crate::discovery;
use crate::effect_layouts;
use crate::scene_transactions::apply_layout_update;
use crate::session::OutputPowerState;

// ── Request / Response Types ─────────────────────────────────────────────

const MAX_EFFECT_UPLOAD_BYTES: usize = 1024 * 1024;
const EFFECT_COVER_FILE_NAME: &str = "default.webp";
const EFFECT_COVER_CONTENT_TYPE: &str = "image/webp";

pub(crate) async fn invalidate_active_render_groups_after_effect_registry_update(state: &AppState) {
    let mut scene_manager = state.scene_manager.write().await;
    scene_manager.invalidate_active_render_groups();
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ApplyEffectRequest {
    #[schema(value_type = Object)]
    pub controls: Option<serde_json::Value>,
    pub transition: Option<TransitionRequest>,
    /// Optional preset ID to associate with the render group in the same
    /// transaction as the effect start — lets the UI pass a remembered
    /// preset selection without a follow-up round-trip. If `controls` is
    /// also provided, the explicit controls win (they're presumed to
    /// already carry the preset's values, possibly with user tweaks).
    #[serde(default)]
    pub preset_id: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct TransitionRequest {
    #[serde(rename = "type")]
    pub transition_type: Option<String>,
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy)]
struct AppliedTransition {
    transition_type: &'static str,
    duration_ms: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct EffectListResponse {
    pub items: Vec<EffectSummary>,
    pub pagination: super::devices::Pagination,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct EffectSummary {
    pub id: String,
    pub name: String,
    pub description: String,
    pub author: String,
    pub category: String,
    pub source: String,
    pub runnable: bool,
    pub tags: Vec<String>,
    pub version: String,
    pub audio_reactive: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cover_image_url: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ActiveEffectResponse {
    pub id: Option<String>,
    pub name: Option<String>,
    pub state: String,
    pub controls: Vec<ControlDefinition>,
    pub control_values: HashMap<String, ControlValue>,
    pub active_preset_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub render_group_id: Option<String>,
    /// Server-side version token for the group's controls. Clients
    /// that want to use optimistic concurrency on the effect-id PATCH
    /// endpoint echo this value back via `If-Match`. Idle responses
    /// omit it (there's nothing to version).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub controls_version: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cover_image_url: Option<String>,
}

impl ActiveEffectResponse {
    fn idle() -> Self {
        Self {
            id: None,
            name: None,
            state: "idle".to_owned(),
            controls: Vec::new(),
            control_values: HashMap::new(),
            active_preset_id: None,
            render_group_id: None,
            controls_version: None,
            cover_image_url: None,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateCurrentControlsRequest {
    #[schema(value_type = Object)]
    pub controls: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct SetEffectLayoutRequest {
    pub layout_id: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct EffectDetailResponse {
    pub id: String,
    pub name: String,
    pub description: String,
    pub author: String,
    pub category: String,
    pub source: String,
    pub runnable: bool,
    pub tags: Vec<String>,
    pub version: String,
    pub audio_reactive: bool,
    pub controls: Vec<ControlDefinition>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub presets: Vec<PresetTemplate>,
    pub active_control_values: Option<HashMap<String, ControlValue>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cover_image_url: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LayoutLinkSummary {
    pub id: String,
    pub name: String,
    pub canvas_width: u32,
    pub canvas_height: u32,
    pub zone_count: usize,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct EffectLayoutApplyResult {
    pub associated_layout_id: String,
    pub resolved: bool,
    pub applied: bool,
    pub layout: Option<LayoutLinkSummary>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct InstalledEffectResponse {
    pub id: String,
    pub name: String,
    pub source: String,
    pub path: String,
    pub controls: usize,
    pub presets: usize,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct EffectRefSummary {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ApplyTransitionResponse {
    #[serde(rename = "type")]
    pub transition_type: String,
    pub duration_ms: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ApplyEffectResponse {
    pub effect: EffectRefSummary,
    #[schema(value_type = Object)]
    pub applied_controls: serde_json::Value,
    pub layout: Option<EffectLayoutApplyResult>,
    pub transition: ApplyTransitionResponse,
    pub warnings: Vec<String>,
}

#[derive(Debug)]
enum ResolveLayoutLinkError {
    NotFound(String),
    AmbiguousName(String),
}

#[derive(Debug)]
pub(crate) struct StopActiveEffectResult {
    pub effect: EffectRef,
    pub released_network_devices: usize,
}

#[derive(Debug)]
pub(crate) enum StopActiveEffectError {
    NoActiveEffect,
    ActiveScene(ActiveSceneMutationError),
    ActiveGroupMissing,
}

impl From<ActiveSceneMutationError> for StopActiveEffectError {
    fn from(value: ActiveSceneMutationError) -> Self {
        Self::ActiveScene(value)
    }
}

pub(crate) async fn wake_output_for_effect_start(state: &AppState) {
    let current = *state.power_state.borrow();
    if !current.sleeping {
        resume_paused_render_loop(state).await;
        return;
    }

    state.power_state.send_replace(OutputPowerState {
        global_brightness: current.global_brightness,
        session_brightness: 1.0,
        sleeping: false,
        off_output_behavior: current.off_output_behavior,
        off_output_color: current.off_output_color,
    });
    resume_paused_render_loop(state).await;

    if current.off_output_behavior == OffOutputBehavior::Release {
        schedule_network_output_reconnect(state);
    }
}

pub(crate) async fn stop_active_effect_and_quiesce_output(
    state: &AppState,
) -> Result<StopActiveEffectResult, StopActiveEffectError> {
    let Some((group, previous_effect)) = active_primary_effect(state).await else {
        return Err(StopActiveEffectError::NoActiveEffect);
    };

    let (scene_id, cleared_group) = {
        let mut scene_manager = state.scene_manager.write().await;
        let scene_id = active_scene_id_for_runtime_mutation(&scene_manager)?;
        let Some(cleared_group) = scene_manager.clear_group_effect(group.id).cloned() else {
            return Err(StopActiveEffectError::ActiveGroupMissing);
        };
        (scene_id, cleared_group)
    };

    let effect = effect_ref(&previous_effect);
    state.event_bus.publish(HypercolorEvent::EffectStopped {
        effect: effect.clone(),
        reason: EffectStopReason::Stopped,
    });
    publish_render_group_changed(
        state,
        scene_id,
        &cleared_group,
        RenderGroupChangeKind::Updated,
    );

    let released_network_devices = quiesce_output_after_effect_stop(state).await;
    super::save_runtime_session_snapshot(state).await;

    Ok(StopActiveEffectResult {
        effect,
        released_network_devices,
    })
}

async fn resume_paused_render_loop(state: &AppState) {
    let mut render_loop = state.render_loop.write().await;
    render_loop.resume();
}

fn schedule_network_output_reconnect(state: &AppState) {
    let Some(config_manager) = state.config_manager.as_ref() else {
        return;
    };
    let config_guard = config_manager.get();
    let config = Arc::clone(&*config_guard);
    let target_ids = state
        .driver_registry
        .discovery_drivers()
        .into_iter()
        .filter_map(|driver| {
            let descriptor = driver.module_descriptor();
            let is_network_driver = descriptor.module_kind == DriverModuleKind::Network
                || descriptor
                    .transports
                    .contains(&DriverTransportKind::Network);
            is_network_driver.then_some(descriptor.id)
        })
        .collect::<Vec<_>>();
    if target_ids.is_empty() {
        return;
    }
    let targets = match discovery::resolve_targets(
        Some(&target_ids),
        &config,
        state.driver_registry.as_ref(),
    ) {
        Ok(targets) => targets,
        Err(error) => {
            warn!(%error, "Skipping network reconnect scan after output release");
            return;
        }
    };
    if targets.is_empty() {
        return;
    }

    let runtime = super::discovery_runtime(state);
    let task_spawner = runtime.task_spawner.clone();
    let driver_registry = Arc::clone(&state.driver_registry);
    let driver_host = Arc::clone(&state.driver_host);
    task_spawner.spawn(async move {
        let Some(result) = discovery::execute_discovery_scan_if_idle(
            runtime,
            driver_registry,
            driver_host,
            config,
            targets,
            discovery::default_timeout(),
        )
        .await
        else {
            debug!("Skipping network reconnect scan because discovery is already running");
            return;
        };

        debug!(
            found = result.new_devices.len() + result.reappeared_devices.len(),
            vanished = result.vanished_devices.len(),
            duration_ms = result.duration_ms,
            "Network reconnect scan finished"
        );
    });
}

async fn quiesce_output_after_effect_stop(state: &AppState) -> usize {
    {
        let mut render_loop = state.render_loop.write().await;
        render_loop.pause();
    }

    let current = *state.power_state.borrow();
    state.power_state.send_replace(OutputPowerState {
        global_brightness: current.global_brightness,
        session_brightness: 0.0,
        sleeping: true,
        off_output_behavior: OffOutputBehavior::Release,
        off_output_color: [0, 0, 0],
    });

    let runtime = super::discovery_runtime(state);
    let released_network_devices = discovery::release_renderable_network_devices(&runtime).await;

    publish_black_output_snapshot(state).await;
    state.performance.write().await.clear_frame_timings();
    released_network_devices
}

async fn publish_black_output_snapshot(state: &AppState) {
    let (layout, canvas, zones) = {
        let spatial = state.spatial_engine.read().await;
        let layout = spatial.layout();
        let canvas = Canvas::new(layout.canvas_width, layout.canvas_height);
        let zones = spatial.sample(&canvas);
        (layout, canvas, zones)
    };
    let frame_number = next_black_frame_number(state);
    let elapsed_ms = elapsed_ms_u32(state);

    let write_stats = {
        let mut backend_manager = state.backend_manager.lock().await;
        backend_manager.write_frame(&zones, layout.as_ref()).await
    };
    if !write_stats.errors.is_empty() {
        warn!(
            error_count = write_stats.errors.len(),
            "One-shot black frame encountered output errors while stopping effect"
        );
    }

    let canvas_frame = CanvasFrame::from_canvas(&canvas, frame_number, elapsed_ms);
    let (_, display_group_targets) = state.event_bus.display_group_targets_snapshot();
    for group_id in display_group_targets.keys().copied() {
        state
            .event_bus
            .group_canvas_sender(group_id)
            .send_replace(canvas_frame.clone());
    }
    state
        .event_bus
        .frame_sender()
        .send_replace(FrameData::new(zones, frame_number, elapsed_ms));
    state
        .event_bus
        .scene_canvas_sender()
        .send_replace(canvas_frame.clone());
    state.event_bus.canvas_sender().send_replace(canvas_frame);
    state
        .preview_runtime
        .record_canvas_publication(frame_number, elapsed_ms);
}

fn next_black_frame_number(state: &AppState) -> u32 {
    state
        .event_bus
        .frame_receiver()
        .borrow()
        .frame_number
        .saturating_add(1)
}

fn elapsed_ms_u32(state: &AppState) -> u32 {
    u32::try_from(state.start_time.elapsed().as_millis()).unwrap_or(u32::MAX)
}

// ── Handlers ─────────────────────────────────────────────────────────────

/// `GET /api/v1/effects` — List all registered effects.
#[utoipa::path(
    get,
    path = "/api/v1/effects",
    responses(
        (
            status = 200,
            description = "Effect catalog",
            body = crate::api::envelope::ApiResponse<EffectListResponse>
        )
    ),
    tag = "effects"
)]
pub async fn list_effects(State(state): State<Arc<AppState>>) -> Response {
    let registry = state.effect_registry.read().await;
    let mut items: Vec<EffectSummary> = registry
        .iter()
        .map(|(_, entry)| {
            let meta = &entry.metadata;
            EffectSummary {
                id: meta.id.to_string(),
                name: meta.name.clone(),
                description: meta.description.clone(),
                author: meta.author.clone(),
                category: format!("{}", meta.category),
                source: source_kind(&meta.source).to_owned(),
                runnable: is_runnable_source(&meta.source),
                tags: meta.tags.clone(),
                version: meta.version.clone(),
                audio_reactive: meta.audio_reactive,
                cover_image_url: effect_cover_image_url(meta),
            }
        })
        .collect();
    items.sort_by(|left, right| {
        let left_norm = left.name.to_ascii_lowercase();
        let right_norm = right.name.to_ascii_lowercase();
        left_norm
            .cmp(&right_norm)
            .then_with(|| left.name.cmp(&right.name))
    });

    let total = items.len();
    ApiResponse::ok(EffectListResponse {
        items,
        pagination: super::devices::Pagination {
            offset: 0,
            limit: 50,
            total,
            has_more: false,
        },
    })
}

/// `GET /api/v1/effects/:id` — Get a single effect's metadata.
#[utoipa::path(
    get,
    path = "/api/v1/effects/{id}",
    params(("id" = String, Path, description = "Effect id or name")),
    responses(
        (
            status = 200,
            description = "Effect detail",
            body = crate::api::envelope::ApiResponse<EffectDetailResponse>
        ),
        (
            status = 404,
            description = "Effect was not found",
            body = crate::api::envelope::ApiErrorResponse
        )
    ),
    tag = "effects"
)]
pub async fn get_effect(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let registry = state.effect_registry.read().await;

    let Some(meta) = resolve_effect_metadata(&registry, &id) else {
        return ApiError::not_found(format!("Effect not found: {id}"));
    };
    drop(registry);

    let (controls, active_control_values) = if let Some(group) =
        active_primary_group(state.as_ref())
            .await
            .filter(|group| group.effect_id == Some(meta.id))
    {
        (
            controls_with_group_bindings(&meta, &group),
            Some(resolved_control_values(&meta, &group)),
        )
    } else {
        (meta.controls.clone(), None)
    };

    let cover_image_url = effect_cover_image_url(&meta);

    ApiResponse::ok(EffectDetailResponse {
        id: meta.id.to_string(),
        name: meta.name,
        description: meta.description,
        author: meta.author,
        category: format!("{}", meta.category),
        source: source_kind(&meta.source).to_owned(),
        runnable: is_runnable_source(&meta.source),
        tags: meta.tags,
        version: meta.version,
        audio_reactive: meta.audio_reactive,
        controls,
        presets: meta.presets,
        active_control_values,
        cover_image_url,
    })
}

/// `GET /api/v1/effects/:id/layout` — Get the layout associated with an effect.
pub async fn get_effect_layout(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    let effect = {
        let registry = state.effect_registry.read().await;
        let Some(meta) = resolve_effect_metadata(&registry, &id) else {
            return ApiError::not_found(format!("Effect not found: {id}"));
        };
        meta
    };
    let effect_id = effect.id.to_string();

    let Some(layout_id) = ({
        let links = state.effect_layout_links.read().await;
        links.get(&effect_id).cloned()
    }) else {
        return ApiError::not_found(format!("No layout associated with effect: {id}"));
    };

    let layout = {
        let layouts = state.layouts.read().await;
        layouts.get(&layout_id).cloned()
    };

    let summary = layout.as_ref().map(layout_link_summary);
    ApiResponse::ok(serde_json::json!({
        "effect": {
            "id": effect_id,
            "name": effect.name,
        },
        "layout_id": layout_id,
        "resolved": summary.is_some(),
        "layout": summary,
    }))
}

/// `PUT /api/v1/effects/:id/layout` — Associate an effect with a layout.
pub async fn set_effect_layout(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<SetEffectLayoutRequest>,
) -> Response {
    let effect = {
        let registry = state.effect_registry.read().await;
        let Some(meta) = resolve_effect_metadata(&registry, &id) else {
            return ApiError::not_found(format!("Effect not found: {id}"));
        };
        meta
    };

    let requested_layout = body.layout_id.trim();
    if requested_layout.is_empty() {
        return ApiError::validation("layout_id must not be empty");
    }

    let layout = {
        let layouts = state.layouts.read().await;
        match resolve_layout_for_link(&layouts, requested_layout) {
            Ok(layout) => layout,
            Err(ResolveLayoutLinkError::NotFound(layout_id)) => {
                return ApiError::not_found(format!("Layout not found: {layout_id}"));
            }
            Err(ResolveLayoutLinkError::AmbiguousName(name)) => {
                return ApiError::conflict(format!("Layout name is ambiguous: {name}"));
            }
        }
    };

    let effect_id = effect.id.to_string();
    let snapshot = {
        let mut links = state.effect_layout_links.write().await;
        links.insert(effect_id.clone(), layout.id.clone());
        links.clone()
    };
    if let Err(error) = save_effect_layout_links(&state, &snapshot) {
        return ApiError::internal(error);
    }

    ApiResponse::ok(serde_json::json!({
        "effect": {
            "id": effect_id,
            "name": effect.name,
        },
        "layout": layout_link_summary(&layout),
        "linked": true,
    }))
}

/// `DELETE /api/v1/effects/:id/layout` — Remove an effect -> layout association.
pub async fn delete_effect_layout(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    let effect = {
        let registry = state.effect_registry.read().await;
        let Some(meta) = resolve_effect_metadata(&registry, &id) else {
            return ApiError::not_found(format!("Effect not found: {id}"));
        };
        meta
    };
    let effect_id = effect.id.to_string();

    let (removed_layout_id, snapshot) = {
        let mut links = state.effect_layout_links.write().await;
        let removed = links.remove(&effect_id);
        let snapshot = removed.as_ref().map(|_| links.clone());
        (removed, snapshot)
    };

    if let Some(store_snapshot) = snapshot.as_ref()
        && let Err(error) = save_effect_layout_links(&state, store_snapshot)
    {
        return ApiError::internal(error);
    }

    ApiResponse::ok(serde_json::json!({
        "effect": {
            "id": effect_id,
            "name": effect.name,
        },
        "layout_id": removed_layout_id,
        "deleted": removed_layout_id.is_some(),
    }))
}

/// `POST /api/v1/effects/:id/apply` — Start rendering an effect.
#[utoipa::path(
    post,
    path = "/api/v1/effects/{id}/apply",
    params(("id" = String, Path, description = "Effect id or name")),
    request_body = Option<ApplyEffectRequest>,
    responses(
        (
            status = 200,
            description = "Effect applied",
            body = crate::api::envelope::ApiResponse<ApplyEffectResponse>
        ),
        (
            status = 400,
            description = "Request was malformed",
            body = crate::api::envelope::ApiErrorResponse
        ),
        (
            status = 404,
            description = "Effect or preset was not found",
            body = crate::api::envelope::ApiErrorResponse
        ),
        (
            status = 422,
            description = "Request validation failed",
            body = crate::api::envelope::ApiErrorResponse
        ),
        (
            status = 500,
            description = "The effect could not be applied",
            body = crate::api::envelope::ApiErrorResponse
        )
    ),
    tag = "effects"
)]
pub async fn apply_effect(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    body: Option<Json<ApplyEffectRequest>>,
) -> Response {
    let metadata = {
        let registry = state.effect_registry.read().await;
        let Some(meta) = resolve_effect_metadata(&registry, &id) else {
            return ApiError::not_found(format!("Effect not found: {id}"));
        };
        meta
    };

    info!(
        requested = %id,
        effect_id = %metadata.id,
        effect = %metadata.name,
        source = source_kind(&metadata.source),
        "Applying effect via API"
    );
    if metadata.category == EffectCategory::Display {
        return ApiError::validation(format!(
            "Effect '{}' is a display face and must be assigned to a display device, not applied to the LED pipeline",
            metadata.name
        ));
    }
    wake_output_for_effect_start(state.as_ref()).await;

    let applied_transition = match validate_transition_request(body.as_ref()) {
        Ok(transition) => transition,
        Err(error) => return ApiError::bad_request(error),
    };

    // Resolve optional preset up front — both to validate before we touch
    // the scene, and because if the caller didn't supply explicit controls
    // we fall back to the preset's controls (matches `apply_preset`'s
    // same-effect branch).
    let resolved_preset = match body.as_ref().and_then(|body| body.preset_id.as_deref()) {
        None => None,
        Some(preset_ref) => {
            let Some(preset_id) = crate::api::library::resolve_preset_id(&state, preset_ref).await
            else {
                return ApiError::not_found(format!("Preset not found: {preset_ref}"));
            };
            let Some(preset) = state.library_store.get_preset(preset_id).await else {
                return ApiError::not_found(format!("Preset not found: {preset_ref}"));
            };
            if preset.effect_id != metadata.id {
                return ApiError::validation(format!(
                    "Preset '{}' targets effect '{}', not '{}'",
                    preset.name, preset.effect_id, metadata.id
                ));
            }
            Some(preset)
        }
    };

    let raw_controls = extract_request_controls(body.as_ref());
    let (controls, normalized_controls, dropped_controls) = if raw_controls.is_empty()
        && let Some(preset) = resolved_preset.as_ref()
    {
        let (normalized, _) = normalize_control_values(&metadata, &preset.controls);
        (serde_json::Map::new(), normalized, Vec::new())
    } else {
        let (normalized, dropped) = normalize_control_payload(&metadata, &raw_controls);
        (raw_controls, normalized, dropped)
    };

    let previous_effect = active_primary_effect(state.as_ref())
        .await
        .map(|(_, effect)| effect_ref(&effect));
    let layout = resolve_full_scope_layout(state.as_ref()).await;

    let (scene_id, group, change_kind) = {
        let mut scene_manager = state.scene_manager.write().await;
        let scene_id = match active_scene_id_for_runtime_mutation(&scene_manager) {
            Ok(scene_id) => scene_id,
            Err(error) => return error.api_response("applying an effect"),
        };
        let change_kind = if scene_manager
            .active_scene()
            .and_then(|scene| scene.primary_group())
            .is_some()
        {
            RenderGroupChangeKind::Updated
        } else {
            RenderGroupChangeKind::Created
        };
        let group = match scene_manager.upsert_primary_group(
            &metadata,
            normalized_controls,
            resolved_preset.as_ref().map(|preset| preset.id),
            layout,
        ) {
            Ok(group) => group.clone(),
            Err(error) => {
                return ApiError::internal(format!(
                    "Failed to update active scene primary group: {error}"
                ));
            }
        };
        (scene_id, group, change_kind)
    };
    log_effect_apply_completion(
        previous_effect.as_ref().map(|effect| effect.name.as_str()),
        &metadata.name,
        controls.len(),
        &dropped_controls,
    );
    state.event_bus.publish(HypercolorEvent::EffectStarted {
        effect: effect_ref(&metadata),
        trigger: ChangeTrigger::Api,
        previous: previous_effect,
        transition: None,
    });
    publish_render_group_changed(state.as_ref(), scene_id, &group, change_kind);
    let applied_layout = apply_associated_layout(state.as_ref(), &metadata.id.to_string()).await;
    super::persist_runtime_session(&state).await;

    ApiResponse::ok(ApplyEffectResponse {
        effect: EffectRefSummary {
            id: metadata.id.to_string(),
            name: metadata.name,
        },
        applied_controls: serde_json::Value::Object(controls),
        layout: applied_layout,
        transition: ApplyTransitionResponse {
            transition_type: applied_transition.transition_type.to_owned(),
            duration_ms: applied_transition.duration_ms,
        },
        warnings: Vec::new(),
    })
}

/// `GET /api/v1/effects/active` — Get the currently active effect.
#[utoipa::path(
    get,
    path = "/api/v1/effects/active",
    responses(
        (
            status = 200,
            description = "Current active effect, or an idle payload if none is running",
            body = crate::api::envelope::ApiResponse<ActiveEffectResponse>
        )
    ),
    tag = "effects"
)]
pub async fn get_active_effect(State(state): State<Arc<AppState>>) -> Response {
    let Some((group, meta)) = active_primary_effect(state.as_ref()).await else {
        return ApiResponse::ok(ActiveEffectResponse::idle());
    };

    let controls_version = group.controls_version;
    let response = ActiveEffectResponse {
        id: Some(meta.id.to_string()),
        name: Some(meta.name.clone()),
        state: "running".to_owned(),
        controls: controls_with_group_bindings(&meta, &group),
        control_values: resolved_control_values(&meta, &group),
        active_preset_id: group.preset_id.map(|preset| preset.to_string()),
        render_group_id: Some(group.id.to_string()),
        controls_version: Some(controls_version),
        cover_image_url: effect_cover_image_url(&meta),
    };
    let response = ApiResponse::ok(response).into_response();
    attach_controls_version_headers(response, controls_version)
}

/// `GET /api/v1/effects/active/cover` — Get the active effect cover image.
pub async fn get_active_effect_cover(State(state): State<Arc<AppState>>) -> Response {
    let Some((_, meta)) = active_primary_effect(state.as_ref()).await else {
        return ApiError::not_found("No effect is currently active");
    };

    effect_cover_image_response(&meta, EffectCoverCache::Active).await
}

/// `GET /api/v1/effects/:id/cover` — Get an effect cover image.
pub async fn get_effect_cover(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    let metadata = {
        let registry = state.effect_registry.read().await;
        let Some(meta) = resolve_effect_metadata(&registry, &id) else {
            return ApiError::not_found(format!("Effect not found: {id}"));
        };
        meta
    };

    effect_cover_image_response(&metadata, EffectCoverCache::Catalog).await
}

/// `POST /api/v1/effects/stop` — Stop the currently active effect.
pub async fn stop_effect(State(state): State<Arc<AppState>>) -> Response {
    let stop_result = match stop_active_effect_and_quiesce_output(state.as_ref()).await {
        Ok(result) => result,
        Err(StopActiveEffectError::NoActiveEffect | StopActiveEffectError::ActiveGroupMissing) => {
            return ApiError::not_found("No effect is currently active");
        }
        Err(StopActiveEffectError::ActiveScene(error)) => {
            return error.api_response("stopping the active effect");
        }
    };

    ApiResponse::ok(serde_json::json!({
        "stopped": true,
        "released_network_devices": stop_result.released_network_devices,
    }))
}

/// `PATCH /api/v1/effects/current/controls` — Update controls on active effect
/// without reloading/reinitializing the effect renderer.
pub async fn update_current_controls(
    State(state): State<Arc<AppState>>,
    body: Option<Json<UpdateCurrentControlsRequest>>,
) -> Response {
    let controls = body
        .as_ref()
        .and_then(|payload| payload.controls.as_ref())
        .and_then(serde_json::Value::as_object)
        .cloned()
        .unwrap_or_default();

    if controls.is_empty() {
        return ApiError::bad_request("controls payload must include at least one key");
    }

    let mut rejected: Vec<String> = Vec::new();
    let mut applied: HashMap<String, ControlValue> = HashMap::new();
    let Some((group, active_meta)) = active_primary_effect(state.as_ref()).await else {
        return ApiError::not_found("No effect is currently active");
    };
    let effect_name = active_meta.name.clone();
    let (normalized, invalid) = normalize_control_payload(&active_meta, &controls);
    rejected.extend(invalid);
    applied.extend(normalized.clone());
    let previous_values = resolved_control_values(&active_meta, &group);
    let (scene_id, updated_group) = {
        let mut scene_manager = state.scene_manager.write().await;
        let scene_id = match active_scene_id_for_runtime_mutation(&scene_manager) {
            Ok(scene_id) => scene_id,
            Err(error) => return error.api_response("updating active effect controls"),
        };
        let Some(updated_group) = scene_manager
            .patch_group_controls(group.id, normalized)
            .cloned()
        else {
            return ApiError::not_found("No effect is currently active");
        };
        (scene_id, updated_group)
    };

    if !rejected.is_empty() {
        warn!(
            effect = %effect_name,
            rejected_controls = ?rejected,
            "Rejected one or more control updates"
        );
    }
    publish_render_group_changed(
        state.as_ref(),
        scene_id,
        &updated_group,
        RenderGroupChangeKind::ControlsPatched,
    );
    publish_primary_control_changed_events(
        state.as_ref(),
        &active_meta,
        &previous_values,
        &resolved_control_values(&active_meta, &updated_group),
        applied.keys().map(String::as_str),
        ChangeTrigger::Api,
    );
    super::persist_runtime_session(&state).await;

    ApiResponse::ok(serde_json::json!({
        "effect": effect_name,
        "applied": applied,
        "rejected": rejected,
    }))
}

/// `PATCH /api/v1/effects/{effect_id}/controls` — Update controls on a
/// specific effect, scoped by the effect's metadata id rather than the
/// ambient "currently active" effect.
///
/// Supports optimistic concurrency via the standard `If-Match` header:
/// the value is the `controls_version` the client last observed (as an
/// unsigned integer; no quoting required). On a version match the
/// update is applied, the new version is echoed in the `ETag` response
/// header AND the JSON body's `controls_version` field. On mismatch
/// the response is `412 Precondition Failed` with a body containing
/// the current server version so the client can rebase.
///
/// Callers that don't care about concurrency control omit the header;
/// the endpoint then behaves like `update_current_controls`.
///
/// Implemented per Spec 46 § 9.1.
pub async fn update_effect_controls(
    State(state): State<Arc<AppState>>,
    Path(effect_id_raw): Path<String>,
    headers: HeaderMap,
    body: Option<Json<UpdateCurrentControlsRequest>>,
) -> Response {
    let Ok(effect_uuid) = effect_id_raw.parse::<uuid::Uuid>() else {
        return ApiError::bad_request("effect_id must be a valid UUID");
    };
    let effect_id = EffectId::from(effect_uuid);

    let expected_version = match parse_if_match_version(&headers) {
        Ok(version) => version,
        Err(message) => return ApiError::bad_request(message),
    };

    let controls = body
        .as_ref()
        .and_then(|payload| payload.controls.as_ref())
        .and_then(serde_json::Value::as_object)
        .cloned()
        .unwrap_or_default();
    if controls.is_empty() {
        return ApiError::bad_request("controls payload must include at least one key");
    }

    let Some((group, active_meta)) = primary_effect_by_id(state.as_ref(), effect_id).await else {
        return ApiError::not_found("No render group loads that effect");
    };
    let effect_name = active_meta.name.clone();
    let (normalized, invalid) = normalize_control_payload(&active_meta, &controls);
    let mut rejected: Vec<String> = Vec::new();
    rejected.extend(invalid);
    let applied = normalized.clone();
    let previous_values = resolved_control_values(&active_meta, &group);

    // Resolve -> verify -> patch is one write-lock section so the
    // TOCTOU window between "I looked up this effect" and "I'm
    // patching the group that used to load it" is closed. Passing
    // `expected_effect_id` to the scene manager turns a concurrent
    // effect-swap into a `GroupMissing` error instead of a silent
    // overwrite. See `SceneManager::patch_effect_controls_with_precondition`.
    let (scene_id, updated_group, new_version) = {
        let mut scene_manager = state.scene_manager.write().await;
        let scene_id = match active_scene_id_for_runtime_mutation(&scene_manager) {
            Ok(scene_id) => scene_id,
            Err(error) => return error.api_response("updating effect controls"),
        };
        match scene_manager.patch_effect_controls_with_precondition(
            group.id,
            Some(effect_id),
            normalized,
            expected_version,
        ) {
            Ok((updated, version)) => (scene_id, updated.clone(), version),
            Err(ControlsVersionMismatch::NoActiveScene | ControlsVersionMismatch::GroupMissing) => {
                return ApiError::not_found("render group no longer loads that effect");
            }
            Err(ControlsVersionMismatch::Stale { current }) => {
                return controls_version_mismatch_response(current);
            }
        }
    };

    if !rejected.is_empty() {
        warn!(
            effect = %effect_name,
            rejected_controls = ?rejected,
            "Rejected one or more control updates"
        );
    }
    publish_render_group_changed(
        state.as_ref(),
        scene_id,
        &updated_group,
        RenderGroupChangeKind::ControlsPatched,
    );
    publish_primary_control_changed_events(
        state.as_ref(),
        &active_meta,
        &previous_values,
        &resolved_control_values(&active_meta, &updated_group),
        applied.keys().map(String::as_str),
        ChangeTrigger::Api,
    );
    super::persist_runtime_session(&state).await;

    let body = ApiResponse::ok(serde_json::json!({
        "effect": effect_name,
        "applied": applied,
        "rejected": rejected,
        "controls_version": new_version,
    }))
    .into_response();
    attach_controls_version_headers(body, new_version)
}

/// Parse an `If-Match` header as an unsigned decimal `controls_version`.
///
/// Per RFC 7232 the canonical shape is a quoted ETag; we accept both
/// quoted and bare decimal forms because clients do not consistently
/// quote integer ETags and the parser is cheap. A malformed header
/// surfaces a static error message the caller wraps into a `400 Bad
/// Request` — silent fallback to "no precondition" would defeat the
/// whole point.
///
/// Returns `Ok(Some(v))` for a valid precondition, `Ok(None)` for no
/// header or `*`, and `Err(msg)` for malformed input.
fn parse_if_match_version(headers: &HeaderMap) -> Result<Option<u64>, &'static str> {
    let Some(value) = headers.get(header::IF_MATCH) else {
        return Ok(None);
    };
    let raw = value
        .to_str()
        .map_err(|_| "If-Match header must be ASCII")?;
    let trimmed = raw.trim().trim_matches('"');
    if trimmed == "*" {
        // `*` traditionally means "any existing resource" — we honor
        // it by skipping the precondition, matching the common HTTP
        // semantic. Not used by our modal but harmless to support.
        return Ok(None);
    }
    trimmed
        .parse::<u64>()
        .map(Some)
        .map_err(|_| "If-Match must be a non-negative integer controls_version")
}

fn controls_version_mismatch_response(current: u64) -> Response {
    let body = serde_json::json!({
        "error": "controls_version mismatch",
        "current": current,
    });
    let mut response = (
        axum::http::StatusCode::PRECONDITION_FAILED,
        axum::Json(body),
    )
        .into_response();
    if let Ok(etag) = HeaderValue::from_str(&format!("\"{current}\"")) {
        response.headers_mut().insert(header::ETAG, etag);
    }
    response
}

fn attach_controls_version_headers(mut response: Response, version: u64) -> Response {
    if let Ok(etag) = HeaderValue::from_str(&format!("\"{version}\"")) {
        response.headers_mut().insert(header::ETAG, etag);
    }
    response
}

/// Resolve the render group currently loaded with the given effect id.
///
/// Wave 1 policy: if the effect is attached to multiple groups (unusual
/// but not impossible in custom scenes), take the first — deterministic
/// enough for the UI's single-effect modal. A future "pick which group"
/// affordance can extend this.
async fn primary_effect_by_id(
    state: &AppState,
    effect_id: EffectId,
) -> Option<(RenderGroup, EffectMetadata)> {
    let scene_manager = state.scene_manager.read().await;
    let scene = scene_manager.active_scene()?;
    let group = scene
        .groups
        .iter()
        .find(|group| group.effect_id == Some(effect_id))
        .cloned()?;
    drop(scene_manager);
    let registry = state.effect_registry.read().await;
    let metadata = registry.get(&effect_id)?.metadata.clone();
    Some((group, metadata))
}

/// `PUT /api/v1/effects/current/controls/{name}/binding` — Attach a live sensor
/// binding to a control on the active effect.
pub async fn set_current_control_binding(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Json(binding): Json<ControlBinding>,
) -> Response {
    let Some((group, active_meta)) = active_primary_effect(state.as_ref()).await else {
        return ApiError::not_found("No effect is currently active");
    };
    let effect_id = active_meta.id.to_string();
    let effect_name = active_meta.name.clone();
    let Some(control) = active_meta.control_by_id(&name) else {
        return ApiError::not_found(format!("Control not found on active effect: {name}"));
    };
    let control_id = control.control_id().to_owned();
    let normalized = match validate_control_binding_request(&active_meta, &name, binding) {
        Ok(normalized) => normalized,
        Err(error) => return ApiError::validation(error),
    };
    let (scene_id, updated_group) = {
        let mut scene_manager = state.scene_manager.write().await;
        let scene_id = match active_scene_id_for_runtime_mutation(&scene_manager) {
            Ok(scene_id) => scene_id,
            Err(error) => {
                return error.api_response("updating an active effect control binding");
            }
        };
        let Some(updated_group) = scene_manager
            .set_group_control_binding(group.id, control_id.clone(), normalized.clone())
            .cloned()
        else {
            return ApiError::not_found("No effect is currently active");
        };
        (scene_id, updated_group)
    };

    publish_render_group_changed(
        state.as_ref(),
        scene_id,
        &updated_group,
        RenderGroupChangeKind::Updated,
    );
    super::persist_runtime_session(&state).await;

    ApiResponse::ok(serde_json::json!({
        "effect": {
            "id": effect_id,
            "name": effect_name,
        },
        "control": control_id,
        "binding": normalized,
    }))
}

/// `POST /api/v1/effects/current/reset` — Reset all controls on the active
/// effect back to their metadata-defined defaults.
pub async fn reset_controls(State(state): State<Arc<AppState>>) -> Response {
    let Some((group, meta)) = active_primary_effect(state.as_ref()).await else {
        return ApiError::not_found("No effect is currently active");
    };
    let previous_values = resolved_control_values(&meta, &group);
    let (scene_id, updated_group) = {
        let mut scene_manager = state.scene_manager.write().await;
        let scene_id = match active_scene_id_for_runtime_mutation(&scene_manager) {
            Ok(scene_id) => scene_id,
            Err(error) => return error.api_response("resetting active effect controls"),
        };
        let Some(updated_group) = scene_manager
            .reset_group_controls(group.id, default_control_values(&meta))
            .cloned()
        else {
            return ApiError::not_found("No effect is currently active");
        };
        (scene_id, updated_group)
    };
    publish_render_group_changed(
        state.as_ref(),
        scene_id,
        &updated_group,
        RenderGroupChangeKind::ControlsPatched,
    );
    let control_ids = meta
        .controls
        .iter()
        .map(|control| control.control_id().to_owned())
        .collect::<Vec<_>>();
    publish_primary_control_changed_events(
        state.as_ref(),
        &meta,
        &previous_values,
        &resolved_control_values(&meta, &updated_group),
        control_ids.iter().map(String::as_str),
        ChangeTrigger::Api,
    );
    super::persist_runtime_session(&state).await;

    info!(effect = %meta.name, "Controls reset to defaults");

    ApiResponse::ok(serde_json::json!({
        "effect": {
            "id": meta.id.to_string(),
            "name": meta.name,
        },
        "reset": true,
    }))
}

/// `POST /api/v1/effects/rescan` — Manually trigger an effect registry rescan.
pub async fn rescan_effects(State(state): State<Arc<AppState>>) -> Response {
    let report = {
        let mut registry = state.effect_registry.write().await;
        registry.rescan()
    };

    if report.added > 0 || report.removed > 0 || report.updated > 0 {
        invalidate_active_render_groups_after_effect_registry_update(state.as_ref()).await;
    }

    info!(
        added = report.added,
        removed = report.removed,
        updated = report.updated,
        "Manual effect rescan completed"
    );

    state.event_bus.publish(
        hypercolor_types::event::HypercolorEvent::EffectRegistryUpdated {
            added: report.added,
            removed: report.removed,
            updated: report.updated,
        },
    );

    ApiResponse::ok(RescanResponse {
        added: report.added,
        removed: report.removed,
        updated: report.updated,
    })
}

/// `POST /api/v1/effects/install` — Validate and install a user HTML effect.
pub async fn install_effect(
    State(state): State<Arc<AppState>>,
    mut multipart: Multipart,
) -> Response {
    let (file_name, file_bytes) = match next_uploaded_html_field(&mut multipart).await {
        Ok(upload) => upload,
        Err(response) => return response,
    };

    if file_bytes.len() > MAX_EFFECT_UPLOAD_BYTES {
        return ApiError::payload_too_large(format!(
            "Uploaded effect exceeds the 1 MB limit ({} bytes).",
            file_bytes.len()
        ));
    }

    let Ok(html) = String::from_utf8(file_bytes) else {
        return ApiError::bad_request("Uploaded effect must be valid UTF-8 HTML.");
    };

    let validated = match validate_uploaded_html(&html) {
        Ok(validated) => validated,
        Err(errors) => {
            return ApiError::bad_request_with_details(
                "Uploaded effect failed validation.",
                serde_json::json!({ "errors": errors }),
            );
        }
    };

    let install_dir = user_effects_install_dir(state.as_ref());
    if let Err(error) = fs::create_dir_all(&install_dir).await {
        return ApiError::internal(format!(
            "Failed to create user effects directory '{}': {error}",
            install_dir.display()
        ));
    }

    let preferred_stem = file_name
        .as_deref()
        .and_then(uploaded_file_stem)
        .map_or_else(
            || sanitize_effect_filename_stem(&validated.title),
            sanitize_effect_filename_stem,
        );
    let installed_path = dedupe_install_path(&install_dir, &preferred_stem);

    if let Err(error) = fs::write(&installed_path, html.as_bytes()).await {
        return ApiError::internal(format!(
            "Failed to write uploaded effect to '{}': {error}",
            installed_path.display()
        ));
    }

    let entry = match load_html_effect_file(&installed_path) {
        Ok(Some(entry)) => entry,
        Ok(None) => {
            let _ = fs::remove_file(&installed_path).await;
            return ApiError::bad_request("Uploaded effect is not supported by this daemon build.");
        }
        Err(error) => {
            let _ = fs::remove_file(&installed_path).await;
            return ApiError::internal(format!(
                "Failed to register uploaded effect '{}': {}",
                error.path.display(),
                error.message
            ));
        }
    };

    let (added, updated) = {
        let mut registry = state.effect_registry.write().await;
        let replaced = registry.register(entry.clone()).is_some();
        if replaced { (0, 1) } else { (1, 0) }
    };

    invalidate_active_render_groups_after_effect_registry_update(state.as_ref()).await;

    state
        .event_bus
        .publish(HypercolorEvent::EffectRegistryUpdated {
            added,
            removed: 0,
            updated,
        });

    info!(
        effect = %entry.metadata.name,
        path = %entry.source_path.display(),
        "Installed uploaded effect"
    );

    ApiResponse::created(InstalledEffectResponse {
        id: entry.metadata.id.to_string(),
        name: entry.metadata.name,
        source: "user".to_owned(),
        path: entry.source_path.display().to_string(),
        controls: entry.metadata.controls.len(),
        presets: entry.metadata.presets.len(),
    })
}

#[derive(Debug, Serialize)]
pub struct RescanResponse {
    pub added: usize,
    pub removed: usize,
    pub updated: usize,
}

pub(crate) fn resolve_effect_metadata(
    registry: &EffectRegistry,
    id_or_name: &str,
) -> Option<EffectMetadata> {
    if let Ok(uuid) = id_or_name.parse::<uuid::Uuid>() {
        let effect_id = EffectId::new(uuid);
        return registry.get(&effect_id).map(|entry| entry.metadata.clone());
    }

    registry
        .iter()
        .find(|(_, entry)| entry.metadata.matches_lookup(id_or_name))
        .map(|(_, entry)| entry.metadata.clone())
}

fn publish_primary_control_changed_events<'a>(
    state: &AppState,
    metadata: &EffectMetadata,
    previous_values: &HashMap<String, ControlValue>,
    next_values: &HashMap<String, ControlValue>,
    changed_control_ids: impl IntoIterator<Item = &'a str>,
    trigger: ChangeTrigger,
) {
    for control_id in changed_control_ids {
        let Some(previous) = previous_values.get(control_id) else {
            continue;
        };
        let Some(next) = next_values.get(control_id) else {
            continue;
        };
        if previous == next {
            continue;
        }
        let (Some(old_value), Some(new_value)) =
            (event_control_value(previous), event_control_value(next))
        else {
            continue;
        };
        state
            .event_bus
            .publish(HypercolorEvent::EffectControlChanged {
                effect_id: metadata.id.to_string(),
                control_id: control_id.to_owned(),
                old_value,
                new_value,
                trigger: trigger.clone(),
            });
    }
}

fn event_control_value(value: &ControlValue) -> Option<EventControlValue> {
    match value {
        ControlValue::Float(_) | ControlValue::Integer(_) => {
            value.as_f32().map(EventControlValue::Number)
        }
        ControlValue::Boolean(value) => Some(EventControlValue::Boolean(*value)),
        ControlValue::Enum(value) | ControlValue::Text(value) => {
            Some(EventControlValue::String(value.clone()))
        }
        ControlValue::Color(_) | ControlValue::Rect(_) | ControlValue::Gradient(_) => None,
    }
}

fn extract_request_controls(
    body: Option<&Json<ApplyEffectRequest>>,
) -> serde_json::Map<String, serde_json::Value> {
    body.and_then(|payload| payload.controls.as_ref())
        .and_then(serde_json::Value::as_object)
        .cloned()
        .unwrap_or_default()
}

pub(crate) async fn active_primary_group(state: &AppState) -> Option<RenderGroup> {
    let scene_manager = state.scene_manager.read().await;
    scene_manager.active_scene()?.primary_group().cloned()
}

pub(crate) async fn active_primary_effect(
    state: &AppState,
) -> Option<(RenderGroup, EffectMetadata)> {
    let group = active_primary_group(state).await?;
    let effect_id = group.effect_id?;
    let registry = state.effect_registry.read().await;
    let metadata = registry.get(&effect_id)?.metadata.clone();
    Some((group, metadata))
}

pub(crate) async fn active_effect_metadata(state: &AppState) -> Option<EffectMetadata> {
    active_primary_effect(state)
        .await
        .map(|(_, metadata)| metadata)
}

fn controls_with_group_bindings(
    metadata: &EffectMetadata,
    group: &RenderGroup,
) -> Vec<ControlDefinition> {
    metadata
        .controls
        .iter()
        .cloned()
        .map(|mut control| {
            control.binding = group.control_bindings.get(control.control_id()).cloned();
            control
        })
        .collect()
}

pub(crate) fn normalize_control_payload(
    metadata: &EffectMetadata,
    raw_controls: &serde_json::Map<String, serde_json::Value>,
) -> (HashMap<String, ControlValue>, Vec<String>) {
    let mut normalized = HashMap::new();
    let mut rejected = Vec::new();

    for (name, value) in raw_controls {
        let Some(parsed) = json_to_control_value(value) else {
            rejected.push(format!("{name} (unsupported JSON shape)"));
            continue;
        };

        let result = metadata.control_by_id(name).map_or_else(
            || Ok(parsed.clone()),
            |control| control.validate_value(&parsed),
        );
        match result {
            Ok(control_value) => {
                normalized.insert(name.clone(), control_value);
            }
            Err(error) => rejected.push(format!("{name} ({error})")),
        }
    }

    (normalized, rejected)
}

pub(crate) fn normalize_control_values(
    metadata: &EffectMetadata,
    control_values: &HashMap<String, ControlValue>,
) -> (HashMap<String, ControlValue>, Vec<String>) {
    let mut normalized = HashMap::new();
    let mut rejected = Vec::new();

    for (name, value) in control_values {
        let result = metadata.control_by_id(name).map_or_else(
            || Ok(value.clone()),
            |control| control.validate_value(value),
        );
        match result {
            Ok(control_value) => {
                normalized.insert(name.clone(), control_value);
            }
            Err(error) => rejected.push(format!("{name} ({error})")),
        }
    }

    (normalized, rejected)
}

pub(crate) fn default_control_values(metadata: &EffectMetadata) -> HashMap<String, ControlValue> {
    metadata
        .controls
        .iter()
        .map(|control| {
            (
                control.control_id().to_owned(),
                control.default_value.clone(),
            )
        })
        .collect()
}

pub(crate) fn resolved_control_values(
    metadata: &EffectMetadata,
    group: &RenderGroup,
) -> HashMap<String, ControlValue> {
    let mut resolved = default_control_values(metadata);
    resolved.extend(group.controls.clone());
    resolved
}

fn validate_control_binding_request(
    metadata: &EffectMetadata,
    name: &str,
    binding: ControlBinding,
) -> Result<ControlBinding, String> {
    let normalized = binding.normalized();
    let Some(control) = metadata.control_by_id(name) else {
        return Err(format!("Control not found on active effect: {name}"));
    };

    if normalized.sensor.is_empty() {
        return Err(format!(
            "Control '{}' requires a non-empty sensor label",
            control.control_id()
        ));
    }

    if !matches!(
        control.kind,
        hypercolor_types::effect::ControlKind::Number
            | hypercolor_types::effect::ControlKind::Boolean
            | hypercolor_types::effect::ControlKind::Hue
            | hypercolor_types::effect::ControlKind::Area
    ) {
        return Err(format!(
            "Control '{}' does not support sensor bindings",
            control.control_id()
        ));
    }

    if !normalized.sensor_min.is_finite()
        || !normalized.sensor_max.is_finite()
        || !normalized.target_min.is_finite()
        || !normalized.target_max.is_finite()
    {
        return Err(format!(
            "Control '{}' binding range values must be finite",
            control.control_id()
        ));
    }

    if (normalized.sensor_max - normalized.sensor_min).abs() < f32::EPSILON {
        return Err(format!(
            "Control '{}' binding sensor range must not be zero",
            control.control_id()
        ));
    }

    Ok(normalized)
}

async fn resolve_full_scope_layout(state: &AppState) -> SpatialLayout {
    let spatial = state.spatial_engine.read().await;
    spatial.layout().as_ref().clone()
}

fn validate_transition_request(
    body: Option<&Json<ApplyEffectRequest>>,
) -> Result<AppliedTransition, String> {
    let Some(transition) = body.and_then(|payload| payload.transition.as_ref()) else {
        return Ok(AppliedTransition {
            transition_type: "cut",
            duration_ms: 0,
        });
    };

    let transition_type = transition
        .transition_type
        .as_deref()
        .unwrap_or("cut")
        .trim()
        .to_ascii_lowercase();
    let duration_ms = transition.duration_ms.unwrap_or(0);

    if (transition_type.is_empty() || transition_type == "cut") && duration_ms == 0 {
        return Ok(AppliedTransition {
            transition_type: "cut",
            duration_ms: 0,
        });
    }

    if transition_type.is_empty() || transition_type == "cut" {
        return Err(
            "Effect transitions are not implemented yet; only immediate cut applies today."
                .to_owned(),
        );
    }

    Err(format!(
        "Effect transition '{transition_type}' is not implemented yet; only immediate cut applies today."
    ))
}

fn log_effect_apply_completion(
    previous_effect: Option<&str>,
    effect_name: &str,
    control_count: usize,
    dropped_controls: &[String],
) {
    if let Some(previous) = previous_effect {
        info!(
            from_effect = %previous,
            to_effect = %effect_name,
            control_count,
            "Effect switch completed"
        );
    } else {
        info!(effect = %effect_name, control_count, "Effect activation completed");
    }

    if !dropped_controls.is_empty() {
        warn!(
            effect = %effect_name,
            dropped_controls = ?dropped_controls,
            "Ignored unsupported control value payloads"
        );
    }
}

pub(crate) fn effect_ref(metadata: &EffectMetadata) -> EffectRef {
    EffectRef {
        id: metadata.id.to_string(),
        name: metadata.name.clone(),
        engine: "servo".to_owned(),
    }
}

#[derive(Debug, Clone, Copy)]
enum EffectCoverCache {
    Active,
    Catalog,
}

async fn effect_cover_image_response(
    metadata: &EffectMetadata,
    cache: EffectCoverCache,
) -> Response {
    let Some(path) = effect_cover_image_path(metadata) else {
        return ApiError::not_found(format!(
            "Cover image not found for effect: {}",
            metadata.name
        ));
    };

    let bytes = match fs::read(&path).await {
        Ok(bytes) => bytes,
        Err(error) => {
            if error.kind() == std::io::ErrorKind::NotFound {
                return ApiError::not_found(format!(
                    "Cover image not found for effect: {}",
                    metadata.name
                ));
            }
            warn!(
                path = %path.display(),
                error = %error,
                "Failed to read effect cover image"
            );
            return ApiError::internal("Failed to read effect cover image");
        }
    };

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(EFFECT_COVER_CONTENT_TYPE),
    );
    headers.insert(
        header::CACHE_CONTROL,
        match cache {
            EffectCoverCache::Active => HeaderValue::from_static("no-store"),
            EffectCoverCache::Catalog => HeaderValue::from_static("public, max-age=86400"),
        },
    );
    (headers, bytes).into_response()
}

fn effect_cover_image_url(metadata: &EffectMetadata) -> Option<String> {
    effect_cover_image_path(metadata)?;
    Some(format!("/api/v1/effects/{}/cover", metadata.id))
}

fn effect_cover_image_path(metadata: &EffectMetadata) -> Option<PathBuf> {
    let root = hypercolor_core::effect::bundled_screenshots_root();
    effect_cover_slugs(metadata)
        .into_iter()
        .map(|slug| root.join(slug).join(EFFECT_COVER_FILE_NAME))
        .find(|path| path.is_file())
}

fn effect_cover_slugs(metadata: &EffectMetadata) -> Vec<String> {
    let mut slugs = Vec::new();
    if let Some(stem) = metadata.source.source_stem() {
        push_cover_slug(&mut slugs, stem);
    }
    push_cover_slug(&mut slugs, &metadata.name);
    slugs
}

fn push_cover_slug(slugs: &mut Vec<String>, value: &str) {
    let slug = cover_slug(value);
    if !slug.is_empty() && !slugs.iter().any(|existing| existing == &slug) {
        slugs.push(slug);
    }
}

fn cover_slug(value: &str) -> String {
    let mut slug = String::new();
    let mut last_was_separator = false;

    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
            last_was_separator = false;
        } else if !slug.is_empty() && !last_was_separator {
            slug.push('-');
            last_was_separator = true;
        }
    }

    if last_was_separator {
        let _ = slug.pop();
    }
    slug
}

fn source_kind(source: &EffectSource) -> &'static str {
    match source {
        EffectSource::Native { .. } => "native",
        EffectSource::Html { .. } => "html",
        EffectSource::Shader { .. } => "shader",
    }
}

fn is_runnable_source(source: &EffectSource) -> bool {
    match source {
        EffectSource::Native { .. } => true,
        EffectSource::Html { .. } => cfg!(feature = "servo"),
        EffectSource::Shader { .. } => false,
    }
}

fn layout_link_summary(layout: &SpatialLayout) -> LayoutLinkSummary {
    LayoutLinkSummary {
        id: layout.id.clone(),
        name: layout.name.clone(),
        canvas_width: layout.canvas_width,
        canvas_height: layout.canvas_height,
        zone_count: layout.zones.len(),
    }
}

fn resolve_layout_for_link(
    layouts: &HashMap<String, SpatialLayout>,
    id_or_name: &str,
) -> Result<SpatialLayout, ResolveLayoutLinkError> {
    if let Some(layout) = layouts.get(id_or_name) {
        return Ok(layout.clone());
    }

    let matches: Vec<SpatialLayout> = layouts
        .values()
        .filter(|layout| layout.name.eq_ignore_ascii_case(id_or_name))
        .cloned()
        .collect();
    if matches.is_empty() {
        return Err(ResolveLayoutLinkError::NotFound(id_or_name.to_owned()));
    }
    if matches.len() > 1 {
        return Err(ResolveLayoutLinkError::AmbiguousName(id_or_name.to_owned()));
    }

    Ok(matches
        .into_iter()
        .next()
        .expect("matches len checked above"))
}

fn save_effect_layout_links(
    state: &AppState,
    snapshot: &HashMap<String, String>,
) -> Result<(), String> {
    effect_layouts::save(&state.effect_layout_links_path, snapshot)
        .map_err(|error| format!("{} ({})", error, state.effect_layout_links_path.display()))
}

pub(crate) async fn apply_associated_layout(
    state: &AppState,
    effect_id: &str,
) -> Option<EffectLayoutApplyResult> {
    let associated_layout_id = {
        let links = state.effect_layout_links.read().await;
        links.get(effect_id).cloned()
    }?;

    let layout = {
        let layouts = state.layouts.read().await;
        layouts.get(&associated_layout_id).cloned()
    };

    if let Some(layout) = layout {
        apply_layout_update(
            &state.spatial_engine,
            &state.scene_manager,
            &state.scene_transactions,
            layout.clone(),
        )
        .await;
        return Some(EffectLayoutApplyResult {
            associated_layout_id,
            resolved: true,
            applied: true,
            layout: Some(layout_link_summary(&layout)),
        });
    }

    warn!(
        effect_id,
        associated_layout_id, "Effect has associated layout that no longer exists in layout store"
    );
    Some(EffectLayoutApplyResult {
        associated_layout_id,
        resolved: false,
        applied: false,
        layout: None,
    })
}

struct ValidatedUploadedHtml {
    title: String,
}

async fn next_uploaded_html_field(
    multipart: &mut Multipart,
) -> Result<(Option<String>, Vec<u8>), Response> {
    while let Some(field) = multipart.next_field().await.map_err(|error| {
        ApiError::bad_request(format!("Failed to read multipart upload: {error}"))
    })? {
        let file_name = field.file_name().map(ToOwned::to_owned);
        let field_name = field.name().map(ToOwned::to_owned);
        if file_name.is_none() && field_name.as_deref() != Some("file") {
            continue;
        }

        let bytes = field.bytes().await.map_err(|error| {
            ApiError::bad_request(format!("Failed to read uploaded file: {error}"))
        })?;
        return Ok((file_name, bytes.to_vec()));
    }

    Err(ApiError::bad_request(
        "Missing multipart file field named \"file\".",
    ))
}

fn validate_uploaded_html(html: &str) -> Result<ValidatedUploadedHtml, Vec<String>> {
    let sanitized = strip_html_comments(html);
    let parsed = parse_html_effect_metadata(&sanitized);
    let mut errors = Vec::new();

    if extract_html_title(&sanitized).is_none() {
        errors.push("Missing <title> tag".to_owned());
    }
    if !has_render_surface(&sanitized) {
        errors.push("Missing required render surface".to_owned());
    }
    if extract_start_tags(&sanitized, "script").is_empty() {
        errors.push("Missing <script> tag".to_owned());
    }

    let mut seen_controls = HashSet::new();
    for control in &parsed.controls {
        if !seen_controls.insert(control.property.clone()) {
            errors.push(format!(
                "Duplicate control property \"{}\"",
                control.property
            ));
        }

        if let HtmlControlKind::Other(kind) = &control.kind {
            errors.push(format!(
                "Control \"{}\" uses unknown type \"{}\"",
                control.property, kind
            ));
        }

        if matches!(control.kind, HtmlControlKind::Combobox) && control.values.is_empty() {
            errors.push(format!(
                "Control \"{}\" is a combobox without values",
                control.property
            ));
        }

        if let (Some(min), Some(max)) = (control.min, control.max)
            && min >= max
        {
            errors.push(format!("Control \"{}\" has min >= max", control.property));
        }
    }

    validate_preset_json(&sanitized, &parsed, &mut errors);

    if errors.is_empty() {
        Ok(ValidatedUploadedHtml {
            title: parsed.title,
        })
    } else {
        Err(errors)
    }
}

fn validate_preset_json(html: &str, parsed: &ParsedHtmlEffectMetadata, errors: &mut Vec<String>) {
    let known_controls = parsed
        .controls
        .iter()
        .map(|control| control.property.as_str())
        .collect::<HashSet<_>>();

    for tag in extract_start_tags(html, "meta") {
        let attrs = parse_tag_attributes(&tag);
        let Some(preset_name) = attr_value(&attrs, "preset") else {
            continue;
        };

        let Some(raw_controls) = attr_value(&attrs, "preset-controls") else {
            errors.push(format!(
                "Preset \"{}\" is missing preset-controls JSON",
                normalize_whitespace(preset_name)
            ));
            continue;
        };

        let parsed_json =
            serde_json::from_str::<serde_json::Value>(raw_controls).map_err(|error| {
                format!(
                    "Preset \"{}\" has invalid preset-controls JSON: {error}",
                    normalize_whitespace(preset_name)
                )
            });
        let value = match parsed_json {
            Ok(value) => value,
            Err(error) => {
                errors.push(error);
                continue;
            }
        };

        let Some(object) = value.as_object() else {
            errors.push(format!(
                "Preset \"{}\" preset-controls must be a JSON object",
                normalize_whitespace(preset_name)
            ));
            continue;
        };

        for key in object.keys() {
            if !known_controls.contains(key.as_str()) {
                warn!(
                    preset = %preset_name,
                    control = %key,
                    "Uploaded preset references unknown control"
                );
            }
        }
    }
}

fn user_effects_install_dir(state: &AppState) -> PathBuf {
    state
        .runtime_state_path
        .parent()
        .map(|dir| dir.join("effects").join("user"))
        .unwrap_or_else(|| {
            hypercolor_core::config::ConfigManager::data_dir()
                .join("effects")
                .join("user")
        })
}

fn uploaded_file_stem(file_name: &str) -> Option<&str> {
    FsPath::new(file_name)
        .file_stem()
        .and_then(|stem| stem.to_str())
}

fn sanitize_effect_filename_stem(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut prev_was_dash = false;

    for ch in input.trim().chars() {
        let mapped = if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            ch.to_ascii_lowercase()
        } else {
            '-'
        };

        if mapped == '-' {
            if prev_was_dash {
                continue;
            }
            prev_was_dash = true;
            out.push(mapped);
        } else {
            prev_was_dash = false;
            out.push(mapped);
        }
    }

    while out.ends_with('-') {
        out.pop();
    }

    if out.is_empty() {
        "effect".to_owned()
    } else {
        out
    }
}

fn dedupe_install_path(directory: &FsPath, preferred_stem: &str) -> PathBuf {
    let mut attempt = 1usize;
    loop {
        let name = if attempt == 1 {
            format!("{preferred_stem}.html")
        } else {
            format!("{preferred_stem}-{attempt}.html")
        };
        let candidate = directory.join(name);
        if !candidate.exists() {
            return candidate;
        }
        attempt += 1;
    }
}

fn strip_html_comments(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut cursor = 0usize;

    while let Some(start_rel) = input[cursor..].find("<!--") {
        let start = cursor + start_rel;
        out.push_str(&input[cursor..start]);

        let body_start = start + 4;
        if let Some(end_rel) = input[body_start..].find("-->") {
            cursor = body_start + end_rel + 3;
        } else {
            cursor = input.len();
            break;
        }
    }

    out.push_str(&input[cursor..]);
    out
}

fn extract_start_tags(input: &str, tag_name: &str) -> Vec<String> {
    let mut tags = Vec::new();
    let bytes = input.as_bytes();
    let tag_bytes = tag_name.as_bytes();

    let mut idx = 0usize;
    while idx < bytes.len() {
        if bytes[idx] != b'<' {
            idx += 1;
            continue;
        }

        let mut cursor = idx + 1;
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }

        if cursor >= bytes.len() || matches!(bytes[cursor], b'/' | b'!' | b'?') {
            idx += 1;
            continue;
        }

        let name_start = cursor;
        while cursor < bytes.len()
            && (bytes[cursor].is_ascii_alphanumeric() || bytes[cursor] == b'-')
        {
            cursor += 1;
        }

        if !eq_ignore_ascii_case_bytes(&bytes[name_start..cursor], tag_bytes) {
            idx += 1;
            continue;
        }

        let mut end = cursor;
        let mut in_single = false;
        let mut in_double = false;
        while end < bytes.len() {
            match bytes[end] {
                b'\'' if !in_double => in_single = !in_single,
                b'"' if !in_single => in_double = !in_double,
                b'>' if !in_single && !in_double => {
                    end += 1;
                    break;
                }
                _ => {}
            }
            end += 1;
        }

        let clamped_end = end.min(input.len());
        tags.push(input[idx..clamped_end].to_owned());
        idx = clamped_end;
    }

    tags
}

fn parse_tag_attributes(tag: &str) -> HashMap<String, String> {
    let mut attrs = HashMap::new();
    let trimmed = tag
        .trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .trim_end_matches('/')
        .trim();
    let body = trimmed
        .find(char::is_whitespace)
        .map_or("", |index| &trimmed[index..])
        .trim();
    let bytes = body.as_bytes();
    let mut idx = 0usize;

    while idx < bytes.len() {
        while idx < bytes.len() && bytes[idx].is_ascii_whitespace() {
            idx += 1;
        }
        if idx >= bytes.len() {
            break;
        }

        let key_start = idx;
        while idx < bytes.len() {
            let byte = bytes[idx];
            if byte.is_ascii_whitespace() || byte == b'=' || byte == b'/' {
                break;
            }
            idx += 1;
        }
        if idx == key_start {
            idx += 1;
            continue;
        }

        let key = body[key_start..idx].to_ascii_lowercase();
        while idx < bytes.len() && bytes[idx].is_ascii_whitespace() {
            idx += 1;
        }

        let mut value = String::new();
        if idx < bytes.len() && bytes[idx] == b'=' {
            idx += 1;
            while idx < bytes.len() && bytes[idx].is_ascii_whitespace() {
                idx += 1;
            }

            if idx < bytes.len() {
                if matches!(bytes[idx], b'"' | b'\'') {
                    let quote = bytes[idx];
                    idx += 1;
                    let value_start = idx;
                    while idx < bytes.len() && bytes[idx] != quote {
                        idx += 1;
                    }
                    value.push_str(&body[value_start..idx]);
                    if idx < bytes.len() {
                        idx += 1;
                    }
                } else {
                    let value_start = idx;
                    while idx < bytes.len() {
                        let byte = bytes[idx];
                        if byte.is_ascii_whitespace() || byte == b'/' {
                            break;
                        }
                        idx += 1;
                    }
                    value.push_str(&body[value_start..idx]);
                }
            }
        }

        attrs.insert(key, value);
    }

    attrs
}

fn attr_value<'a>(attrs: &'a HashMap<String, String>, key: &str) -> Option<&'a str> {
    attrs
        .get(key)
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn has_render_surface(html: &str) -> bool {
    has_tag_with_id(html, "canvas", "exCanvas") || has_tag_with_id(html, "div", "faceContainer")
}

fn has_tag_with_id(html: &str, tag_name: &str, expected_id: &str) -> bool {
    extract_start_tags(html, tag_name).into_iter().any(|tag| {
        parse_tag_attributes(&tag)
            .get("id")
            .is_some_and(|value| value.eq_ignore_ascii_case(expected_id))
    })
}

fn extract_html_title(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let start = find_ascii_case_insensitive(bytes, b"<title", 0)?;
    let mut open_end = start;
    while open_end < bytes.len() && bytes[open_end] != b'>' {
        open_end += 1;
    }
    if open_end >= bytes.len() {
        return None;
    }
    open_end += 1;

    let close_start = find_ascii_case_insensitive(bytes, b"</title>", open_end)?;
    let raw = &input[open_end..close_start];
    let normalized = normalize_whitespace(raw);
    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

fn normalize_whitespace(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn find_ascii_case_insensitive(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || from >= haystack.len() {
        return None;
    }

    let max_start = haystack.len().checked_sub(needle.len())?;
    let mut idx = from;
    while idx <= max_start {
        if eq_ignore_ascii_case_bytes(&haystack[idx..idx + needle.len()], needle) {
            return Some(idx);
        }
        idx += 1;
    }

    None
}

fn eq_ignore_ascii_case_bytes(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }

    a.iter()
        .zip(b.iter())
        .all(|(left, right)| left.eq_ignore_ascii_case(right))
}
