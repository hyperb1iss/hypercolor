//! Effect endpoints — `/api/v1/effects/*`.

use std::collections::{HashMap, HashSet};
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;

use axum::Json;
use axum::extract::{Multipart, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use tokio::fs;
use tracing::{info, warn};

use hypercolor_core::bus::CanvasFrame;
use hypercolor_core::effect::{
    EffectRegistry, HtmlControlKind, ParsedHtmlEffectMetadata, load_html_effect_file,
    parse_html_effect_metadata,
};
use hypercolor_core::engine::RenderLoopState;
use hypercolor_types::api::output::OutputPowerMode;
use hypercolor_types::api::scene::{
    ApplyEffectRequest as SceneApplyEffectRequest, ApplyEffectResponse as SceneApplyEffectResponse,
    TransitionType,
};
use hypercolor_types::canvas::{Canvas, Rgba};
use hypercolor_types::device::{DriverModuleKind, DriverTransportKind};
use hypercolor_types::effect::{
    ControlValue, EffectCategory, EffectId, EffectMetadata, EffectSource,
};
use hypercolor_types::event::{EffectRef, FrameData, HypercolorEvent, ZoneColors};
use hypercolor_types::library::PresetId;
use hypercolor_types::scene::Zone;
use hypercolor_types::session::OffOutputBehavior;
use hypercolor_types::spatial::SpatialLayout;

use crate::api::AppState;
use crate::api::control_values::json_to_control_value;
use crate::api::envelope::ApiResponse;
use crate::discovery;
use crate::domain;
// One definition of the source spelling, shared with the `source`
// catalog filter: a second one here would let a listing narrow on values
// the payload never reports.
use crate::domain::effect::RequestedTransition;
use crate::domain::effect::effect_source_kind as source_kind;
use crate::domain::{DomainError, MutationContext, ResourceKind};
use crate::session::set_output_stopped;

// ── Request / Response Types ─────────────────────────────────────────────

const MAX_EFFECT_UPLOAD_BYTES: usize = 1024 * 1024;
const EFFECT_COVER_FILE_NAME: &str = "default.webp";
const EFFECT_COVER_CONTENT_TYPE: &str = "image/webp";

pub(crate) async fn invalidate_active_render_groups_after_effect_registry_update(state: &AppState) {
    if let Err(error) = domain::effect::invalidate_active_zones(state).await {
        warn!(%error, "Failed to refresh active zones after an effect registry update");
    }
}

// Wire contracts live in hypercolor-types::api::effects — shared with the
// web UI and the TUI.
pub use hypercolor_types::api::effects::{
    EffectCapabilitySet, EffectDetailResponse, EffectListResponse, EffectPresetListResponse,
    EffectPresetOrigin, EffectPresetSummary, EffectSummary, InstalledEffectResponse,
    RescanResponse,
};

struct ResolvedEffectPreset {
    id: PresetId,
    controls: HashMap<String, ControlValue>,
}

/// Bring output back to running so a freshly applied effect is visible.
///
/// Reports whether output is running once the attempt settles, which is
/// the post-commit outcome the apply response carries (Spec 78 §2.3).
pub(crate) async fn wake_output_for_effect_start(state: &AppState) -> bool {
    if output_is_running(state).await {
        return true;
    }
    crate::domain::output::set_power(state, OutputPowerMode::Running).await;
    output_is_running(state).await
}

async fn output_is_running(state: &AppState) -> bool {
    let sleeping = state.power_state.borrow().sleeping();
    !sleeping && state.render_loop.read().await.state() != RenderLoopState::Paused
}

pub(crate) fn schedule_network_output_reconnect(state: &AppState) {
    schedule_output_reconnect(state, true);
}

pub(crate) fn schedule_all_output_reconnect(state: &AppState) {
    schedule_output_reconnect(state, false);
}

fn schedule_output_reconnect(state: &AppState, network_only: bool) {
    let Some(config_manager) = state.config_manager.as_ref() else {
        return;
    };
    let config_guard = config_manager.get();
    let config = Arc::clone(&*config_guard);
    let target_ids = network_only.then(|| {
        state
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
            .collect::<Vec<_>>()
    });
    if target_ids.as_ref().is_some_and(Vec::is_empty) {
        return;
    }
    let targets = match discovery::resolve_targets(
        target_ids.as_deref(),
        &config,
        state.driver_registry.as_ref(),
    ) {
        Ok(targets) => targets,
        Err(error) => {
            warn!(%error, network_only, "Skipping reconnect scan after output release");
            return;
        }
    };
    if targets.is_empty() {
        return;
    }

    discovery::schedule_discovery_scan(
        super::discovery_runtime(state),
        Arc::clone(&state.driver_registry),
        Arc::clone(&state.driver_host),
        config,
        targets,
        discovery::default_timeout(),
    );
}

pub(crate) async fn quiesce_output_after_effect_stop(state: &AppState) -> usize {
    let _transition_guard = state.output_power_transition.lock().await;
    {
        let mut render_loop = state.render_loop.write().await;
        render_loop.pause();
    }

    set_output_stopped(&state.power_state, &state.event_bus);

    let runtime = super::discovery_runtime(state);
    let released_network_devices = discovery::release_renderable_network_devices(&runtime).await;

    publish_static_output_snapshot(state, [0, 0, 0]).await;
    state.performance.write().await.clear_frame_timings();
    released_network_devices
}

pub(crate) async fn publish_static_output_snapshot(state: &AppState, color: [u8; 3]) {
    let (layout, canvas, mut zones) = {
        let spatial = state.spatial_engine.read().await;
        let layout = spatial.layout();
        let Ok(mut canvas) = Canvas::try_new(layout.canvas_width, layout.canvas_height).inspect_err(
            |error| {
                warn!(%error, "Static output canvas allocation failed; preserving the last published output");
            },
        ) else {
            return;
        };
        canvas.fill(Rgba::new(color[0], color[1], color[2], 255));
        let Ok(zones) = spatial.try_sample(&canvas).inspect_err(|error| {
            warn!(%error, "Static output sampling failed; preserving the last published output");
        }) else {
            return;
        };
        (layout, canvas, zones)
    };
    let frame_number = next_black_frame_number(state);
    let elapsed_ms = elapsed_ms_u32(state);

    let write_stats = {
        let mut backend_manager = state.backend_manager.lock().await;
        let unassigned_outputs = backend_manager.unassigned_output_zones(layout.as_ref());
        if unassigned_outputs.is_empty() {
            backend_manager.write_frame(&zones, layout.as_ref()).await
        } else {
            zones.extend(unassigned_outputs.iter().map(|output| ZoneColors {
                zone_id: output.id.clone(),
                colors: vec![
                    color;
                    usize::try_from(output.topology.led_count()).unwrap_or_default()
                ],
            }));
            let mut static_layout = layout.as_ref().clone();
            static_layout.zones.extend(unassigned_outputs);
            backend_manager.write_frame(&zones, &static_layout).await
        }
    };
    if !write_stats.errors.is_empty() {
        warn!(
            error_count = write_stats.errors.len(),
            "One-shot static frame encountered output errors while quiescing effect output"
        );
    }

    let canvas_frame = CanvasFrame::from_canvas(&canvas, frame_number, elapsed_ms);
    let group_frame = hypercolor_core::bus::DisplayGroupFrame::Canvas(canvas_frame.clone());
    let (_, display_group_targets) = state.event_bus.display_group_targets_snapshot();
    for group_id in display_group_targets.keys().copied() {
        state
            .event_bus
            .group_canvas_sender(group_id)
            .send_replace(group_frame.clone());
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

pub(crate) async fn reconcile_static_output_hold(state: &AppState) -> bool {
    let _transition_guard = state.output_power_transition.lock().await;
    let output_power = *state.power_state.borrow();
    if !output_power.sleeping()
        || output_power.effective_off_output_behavior() != OffOutputBehavior::Static
    {
        return false;
    }

    publish_static_output_snapshot(state, output_power.effective_off_output_color()).await;
    true
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

/// Query string for `GET /api/v1/effects`.
///
/// Every field narrows the catalog; omitted fields do not. `include` is
/// a comma-separated list of expansions (`controls`, `presets`) that add
/// optional fields to each summary.
///
/// Parameters this type does not name stay ignored, which keeps the v1
/// contract for the paging arguments the fabricated pagination block
/// has always discarded.
#[derive(Debug, Clone, Default, serde::Deserialize, utoipa::IntoParams)]
pub struct EffectListQuery {
    /// Exact effect category.
    #[serde(default)]
    pub category: Option<String>,
    /// Declared audio reactivity.
    #[serde(default)]
    pub audio_reactive: Option<bool>,
    /// Declared screen reactivity.
    #[serde(default)]
    pub screen_reactive: Option<bool>,
    /// Declared input reactivity.
    #[serde(default)]
    pub input_reactive: Option<bool>,
    /// Rendering source: `native`, `html`, or `shader`.
    #[serde(default)]
    pub source: Option<String>,
    /// Case-insensitive substring over name, description, author, tags.
    #[serde(default)]
    pub q: Option<String>,
    /// Comma-separated expansions: `controls`, `presets`.
    #[serde(default)]
    pub include: Option<String>,
}

/// Which summary expansions a listing asked for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct EffectListIncludes {
    controls: bool,
    presets: bool,
}

impl EffectListIncludes {
    fn parse(raw: Option<&str>) -> Result<Self, DomainError> {
        let mut includes = Self::default();
        for token in raw.unwrap_or_default().split(',') {
            match token.trim() {
                "" => {}
                "controls" => includes.controls = true,
                "presets" => includes.presets = true,
                other => {
                    return Err(DomainError::validation_field(
                        "include",
                        format!("unknown expansion '{other}'; expected controls or presets"),
                    ));
                }
            }
        }
        Ok(includes)
    }
}

/// `GET /api/v1/effects` — the effect catalog, narrowed server-side.
#[utoipa::path(
    get,
    path = "/api/v1/effects",
    params(EffectListQuery),
    responses(
        (
            status = 200,
            description = "Effect catalog",
            body = crate::api::envelope::ApiResponse<EffectListResponse>
        ),
        (
            status = 422,
            description = "A filter or expansion named a value that does not exist",
            body = hypercolor_types::api::envelope::ApiErrorBody
        )
    ),
    tag = "effects"
)]
pub async fn list_effects(
    State(state): State<Arc<AppState>>,
    Query(query): Query<EffectListQuery>,
) -> Response {
    let includes = match EffectListIncludes::parse(query.include.as_deref()) {
        Ok(includes) => includes,
        Err(error) => return error.into_response(),
    };
    let catalog_query = match domain::effect::EffectCatalogQuery::parse(
        query.category.as_deref(),
        query.source.as_deref(),
        query.q.as_deref(),
    ) {
        Ok(parsed) => domain::effect::EffectCatalogQuery {
            audio_reactive: query.audio_reactive,
            screen_reactive: query.screen_reactive,
            input_reactive: query.input_reactive,
            ..parsed
        },
        Err(error) => return error.into_response(),
    };

    let items: Vec<EffectSummary> = domain::effect::list_catalog(state.as_ref(), &catalog_query)
        .await
        .into_iter()
        .map(|meta| effect_summary(&meta, includes))
        .collect();

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

fn effect_summary(meta: &EffectMetadata, includes: EffectListIncludes) -> EffectSummary {
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
        input_reactive: meta.input_reactive,
        capabilities: EffectCapabilitySet {
            audio_reactive: meta.audio_reactive,
            screen_reactive: meta.screen_reactive,
            input_reactive: meta.input_reactive,
        },
        cover_image_url: effect_cover_image_url(meta),
        controls: includes.controls.then(|| meta.controls.clone()),
        presets: includes.presets.then(|| meta.presets.clone()),
    }
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
            body = hypercolor_types::api::envelope::ApiErrorBody
        )
    ),
    tag = "effects"
)]
pub async fn get_effect(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let registry = state.effect_registry.read().await;

    let Some(meta) = resolve_effect_metadata(&registry, &id) else {
        return DomainError::not_found(ResourceKind::Effect, &id).into_response();
    };
    drop(registry);

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
        controls: meta.controls,
        presets: meta.presets,
        cover_image_url,
    })
}

/// `GET /api/v1/effects/:id/presets` lists bundled and saved presets.
#[utoipa::path(
    get,
    path = "/api/v1/effects/{id}/presets",
    params(("id" = String, Path, description = "Effect id or name")),
    responses(
        (
            status = 200,
            description = "Unified effect preset stack",
            body = crate::api::envelope::ApiResponse<EffectPresetListResponse>
        ),
        (
            status = 404,
            description = "Effect was not found",
            body = hypercolor_types::api::envelope::ApiErrorBody
        )
    ),
    tag = "effects"
)]
pub async fn list_effect_presets(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    let metadata = {
        let registry = state.effect_registry.read().await;
        let Some(metadata) = resolve_effect_metadata(&registry, &id) else {
            return DomainError::not_found(ResourceKind::Effect, &id).into_response();
        };
        metadata
    };
    let items = effect_preset_stack(state.as_ref(), &metadata).await;
    let total = items.len();

    ApiResponse::ok(EffectPresetListResponse {
        items,
        pagination: super::devices::Pagination {
            offset: 0,
            limit: total,
            total,
            has_more: false,
        },
    })
}

/// `POST /api/v1/effects/:id/presets/:preset_id/apply` applies one preset.
pub async fn apply_effect_preset(
    State(state): State<Arc<AppState>>,
    Path((id, preset_id)): Path<(String, String)>,
    headers: HeaderMap,
    body: Option<Json<SceneApplyEffectRequest>>,
) -> Response {
    let Ok(preset) = preset_id.parse::<PresetId>() else {
        return DomainError::not_found(ResourceKind::Preset, &preset_id).into_response();
    };
    let mut request = body.map(|Json(body)| body).unwrap_or_default();
    request.preset_id = Some(preset);

    apply_effect(State(state), Path(id), headers, Some(Json(request))).await
}

/// `POST /api/v1/effects/:id/apply` — Start rendering an effect.
pub async fn apply_effect(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Option<Json<SceneApplyEffectRequest>>,
) -> Response {
    let request = body.map(|Json(body)| body).unwrap_or_default();
    let transition = match request.transition.unwrap_or_default() {
        TransitionType::Cut => RequestedTransition::cut(),
    };
    let expected_revision = match crate::api::scene::parse_if_match(&headers) {
        Ok(revision) => revision,
        Err(error) => return error.into_response(),
    };

    // Validate before the scene commit or output wake so a refusal leaves
    // the rig unchanged.
    let metadata = {
        let registry = state.effect_registry.read().await;
        let Some(meta) = resolve_effect_metadata(&registry, &id) else {
            return DomainError::not_found(ResourceKind::Effect, &id).into_response();
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
        return DomainError::validation(format!(
            "Effect '{}' is a display face and must be assigned to a display device, not applied to the LED pipeline",
            metadata.name
        ))
        .into_response();
    }

    let resolved_preset = match request.preset_id.as_ref() {
        None => None,
        Some(preset_ref) => {
            let preset_ref = preset_ref.to_string();
            let Some(preset) = resolve_effect_preset(state.as_ref(), &metadata, &preset_ref).await
            else {
                if let Some(saved) =
                    state
                        .library_store
                        .list_presets()
                        .await
                        .into_iter()
                        .find(|saved| {
                            saved.id.to_string() == preset_ref
                                || saved.name.eq_ignore_ascii_case(&preset_ref)
                        })
                {
                    return DomainError::validation(format!(
                        "Preset '{}' targets effect '{}', not '{}'",
                        saved.name, saved.effect_id, metadata.id
                    ))
                    .into_response();
                }
                return DomainError::not_found(ResourceKind::Preset, &preset_ref).into_response();
            };
            Some(preset)
        }
    };

    // Explicit controls win; otherwise the preset seeds the layer.
    let requested_controls = request.controls.unwrap_or_default();
    let (normalized_controls, dropped_controls) = if requested_controls.is_empty()
        && let Some(preset) = resolved_preset.as_ref()
    {
        normalize_control_values(&metadata, &preset.controls)
    } else {
        let owned: HashMap<String, ControlValue> = requested_controls.into_iter().collect();
        normalize_control_values(&metadata, &owned)
    };
    if !dropped_controls.is_empty() {
        return DomainError::validation_details(
            "one or more control values were rejected",
            serde_json::json!({ "rejected": dropped_controls }),
        )
        .into_response();
    }
    let control_count = normalized_controls.len();

    // Commit before waking output. A wake failure rides in the 200 because
    // the scene mutation is already real.
    let applied = match domain::effect::apply_effect(
        state.as_ref(),
        domain::effect::ApplyEffect {
            effect: metadata.clone(),
            controls: normalized_controls,
            preset_id: resolved_preset.as_ref().map(|preset| preset.id),
            target_zone: request.zone,
            expected_revision,
            transition,
            wake_output: true,
        },
        MutationContext::api(),
    )
    .await
    {
        Ok(applied) => applied,
        Err(error) => return error.into_response(),
    };

    if let Some(error) = applied.commit.retry_error() {
        // Admitted and converging, not failed: the retry supervisor
        // owns the bytes. The caller gets its success.
        warn!(%error, "Scene write has not proven durable yet; retry remains active");
    }

    log_effect_apply_completion(
        applied
            .previous_effect
            .as_ref()
            .map(|effect| effect.name.as_str()),
        &applied.effect.name,
        control_count,
        &[],
    );

    let revision = applied.commit.revision();
    let mut response = ApiResponse::ok(SceneApplyEffectResponse {
        zone: crate::domain::scene_tree::zone_resource(&applied.zone),
        transition: TransitionType::Cut,
        output: applied.output,
    });
    if let Ok(value) = HeaderValue::from_str(&format!("\"{revision}\"")) {
        response.headers_mut().insert(header::ETAG, value);
    }
    response
}

/// `GET /api/v1/effects/:id/cover` — Get an effect cover image.
pub async fn get_effect_cover(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    let metadata = {
        let registry = state.effect_registry.read().await;
        let Some(meta) = resolve_effect_metadata(&registry, &id) else {
            return DomainError::not_found(ResourceKind::Effect, &id).into_response();
        };
        meta
    };

    effect_cover_image_response(&metadata, EffectCoverCache::Catalog).await
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
        Err(error) => return error.into_response(),
    };

    if file_bytes.len() > MAX_EFFECT_UPLOAD_BYTES {
        return DomainError::PayloadTooLarge {
            limit_bytes: MAX_EFFECT_UPLOAD_BYTES as u64,
        }
        .into_response();
    }

    let Ok(html) = String::from_utf8(file_bytes) else {
        return DomainError::malformed("Uploaded effect must be valid UTF-8 HTML.").into_response();
    };

    let validated = match validate_uploaded_html(&html) {
        Ok(validated) => validated,
        Err(errors) => {
            return DomainError::validation_details(
                "Uploaded effect failed validation.",
                serde_json::json!({ "errors": errors }),
            )
            .into_response();
        }
    };

    let install_dir = user_effects_install_dir(state.as_ref());
    if let Err(error) = fs::create_dir_all(&install_dir).await {
        return DomainError::Internal(anyhow::anyhow!(
            "Failed to create user effects directory '{}': {error}",
            install_dir.display()
        ))
        .into_response();
    }

    let preferred_stem = file_name
        .as_deref()
        .and_then(uploaded_file_stem)
        .map_or_else(
            || sanitize_effect_filename_stem(&validated.title),
            sanitize_effect_filename_stem,
        );
    // Same stem updates in place: the path-derived effect id stays stable,
    // so existing assignments follow the update instead of a `-2` clone
    // appearing beside the original.
    let installed_path = install_dir.join(format!("{preferred_stem}.html"));
    let replacing = installed_path.exists();

    if let Err(error) = fs::write(&installed_path, html.as_bytes()).await {
        return DomainError::Internal(anyhow::anyhow!(
            "Failed to write uploaded effect to '{}': {error}",
            installed_path.display()
        ))
        .into_response();
    }

    let entry = match load_html_effect_file(&installed_path) {
        Ok(Some(entry)) => entry,
        Ok(None) => {
            let _ = fs::remove_file(&installed_path).await;
            return DomainError::validation(
                "Uploaded effect is not supported by this daemon build.",
            )
            .into_response();
        }
        Err(error) => {
            let _ = fs::remove_file(&installed_path).await;
            return DomainError::Internal(anyhow::anyhow!(
                "Failed to register uploaded effect '{}': {}",
                error.path.display(),
                error.message
            ))
            .into_response();
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
        replaced_existing = replacing,
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

async fn effect_preset_stack(
    state: &AppState,
    metadata: &EffectMetadata,
) -> Vec<EffectPresetSummary> {
    let mut items = metadata
        .presets
        .iter()
        .map(|preset| EffectPresetSummary {
            id: preset.id.to_string(),
            name: preset.name.clone(),
            description: preset.description.clone(),
            effect_id: metadata.id.to_string(),
            controls: preset.controls.clone(),
            tags: Vec::new(),
            origin: EffectPresetOrigin::Bundled,
            editable: false,
        })
        .collect::<Vec<_>>();

    let mut saved = state
        .library_store
        .list_presets()
        .await
        .into_iter()
        .filter(|preset| preset.effect_id == metadata.id)
        .map(|preset| EffectPresetSummary {
            id: preset.id.to_string(),
            name: preset.name,
            description: preset.description,
            effect_id: preset.effect_id.to_string(),
            controls: preset.controls,
            tags: preset.tags,
            origin: EffectPresetOrigin::Saved,
            editable: true,
        })
        .collect::<Vec<_>>();
    saved.sort_by(|left, right| {
        left.name
            .to_ascii_lowercase()
            .cmp(&right.name.to_ascii_lowercase())
            .then_with(|| left.name.cmp(&right.name))
    });
    items.extend(saved);
    items
}

async fn resolve_effect_preset(
    state: &AppState,
    metadata: &EffectMetadata,
    id_or_name: &str,
) -> Option<ResolvedEffectPreset> {
    let saved = state.library_store.list_presets().await;
    if let Ok(id) = id_or_name.parse::<PresetId>() {
        if let Some(preset) = metadata.presets.iter().find(|preset| preset.id == id) {
            return Some(ResolvedEffectPreset {
                id: preset.id,
                controls: preset.controls.clone(),
            });
        }
        return saved
            .into_iter()
            .find(|preset| preset.id == id && preset.effect_id == metadata.id)
            .map(|preset| ResolvedEffectPreset {
                id: preset.id,
                controls: preset.controls,
            });
    }

    if let Some(preset) = saved.into_iter().find(|preset| {
        preset.effect_id == metadata.id && preset.name.eq_ignore_ascii_case(id_or_name)
    }) {
        return Some(ResolvedEffectPreset {
            id: preset.id,
            controls: preset.controls,
        });
    }
    metadata
        .presets
        .iter()
        .find(|preset| preset.name.eq_ignore_ascii_case(id_or_name))
        .map(|preset| ResolvedEffectPreset {
            id: preset.id,
            controls: preset.controls.clone(),
        })
}

pub(crate) async fn active_primary_group(state: &AppState) -> Option<Zone> {
    let scene_manager = state.scene_manager.read().await;
    scene_manager.active_scene()?.primary_group().cloned()
}

pub(crate) async fn active_primary_effect(state: &AppState) -> Option<(Zone, EffectMetadata)> {
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

/// Resolve the full device-output roster as a [`SpatialLayout`] — every
/// discovered device output with default placement. This is the canonical
/// source for a fresh `Primary` zone (§5.2): a new scene's Default zone,
/// the lazily created `effects/apply` `Primary`, and `Primary` recovery
/// all seed from it.
pub(crate) async fn resolve_full_scope_layout(state: &AppState) -> SpatialLayout {
    let spatial = state.spatial_engine.read().await;
    spatial.layout().as_ref().clone()
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
    Catalog,
}

async fn effect_cover_image_response(
    metadata: &EffectMetadata,
    cache: EffectCoverCache,
) -> Response {
    let Some(cover) = effect_cover_image_bytes(metadata).await else {
        return DomainError::not_found(ResourceKind::Asset, &metadata.name).into_response();
    };

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&cover.content_type)
            .unwrap_or_else(|_| HeaderValue::from_static(EFFECT_COVER_CONTENT_TYPE)),
    );
    headers.insert(
        header::CACHE_CONTROL,
        match cache {
            EffectCoverCache::Catalog => HeaderValue::from_static("public, max-age=86400"),
        },
    );
    (headers, cover.bytes).into_response()
}

fn effect_cover_image_url(metadata: &EffectMetadata) -> Option<String> {
    if effect_cover_image_path(metadata).is_none() && html_effect_source_path(metadata).is_none() {
        return None;
    }
    Some(format!("/api/v1/effects/{}/cover", metadata.id))
}

struct EffectCover {
    content_type: String,
    bytes: Vec<u8>,
}

/// Resolve cover bytes, preferring curated artwork over whatever the effect
/// ships inline so a locally curated image can override the bundled art.
async fn effect_cover_image_bytes(metadata: &EffectMetadata) -> Option<EffectCover> {
    if let Some(path) = effect_cover_image_path(metadata) {
        match fs::read(&path).await {
            Ok(bytes) => {
                return Some(EffectCover {
                    content_type: EFFECT_COVER_CONTENT_TYPE.to_owned(),
                    bytes,
                });
            }
            Err(error) => warn!(
                path = %path.display(),
                error = %error,
                "Failed to read curated effect cover; falling back to inline cover"
            ),
        }
    }

    effect_inline_cover(metadata).await
}

/// Decode the `data:image/<type>;base64,` cover an HTML effect declares in its
/// `<meta cover>` tag.
async fn effect_inline_cover(metadata: &EffectMetadata) -> Option<EffectCover> {
    let path = html_effect_source_path(metadata)?;
    let html = fs::read_to_string(path).await.ok()?;
    let cover = hypercolor_core::effect::parse_html_effect_metadata(&html).cover?;

    let (declaration, payload) = cover.split_once(";base64,")?;
    let content_type = declaration.strip_prefix("data:")?.to_owned();

    match BASE64_STANDARD.decode(payload) {
        Ok(bytes) => Some(EffectCover {
            content_type,
            bytes,
        }),
        Err(error) => {
            warn!(
                effect = %metadata.name,
                error = %error,
                "Effect declared an inline cover that is not valid base64"
            );
            None
        }
    }
}

fn html_effect_source_path(metadata: &EffectMetadata) -> Option<&PathBuf> {
    match &metadata.source {
        EffectSource::Html { path } => Some(path),
        EffectSource::Native { .. } | EffectSource::Shader { .. } => None,
    }
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

fn is_runnable_source(source: &EffectSource) -> bool {
    match source {
        EffectSource::Native { .. } => true,
        EffectSource::Html { .. } => cfg!(feature = "servo"),
        EffectSource::Shader { .. } => false,
    }
}

struct ValidatedUploadedHtml {
    title: String,
}

async fn next_uploaded_html_field(
    multipart: &mut Multipart,
) -> Result<(Option<String>, Vec<u8>), DomainError> {
    while let Some(field) = multipart.next_field().await.map_err(|error| {
        DomainError::malformed(format!("Failed to read multipart upload: {error}"))
    })? {
        let file_name = field.file_name().map(ToOwned::to_owned);
        let field_name = field.name().map(ToOwned::to_owned);
        if field_name.as_deref() != Some("file") || file_name.is_none() {
            continue;
        }

        let bytes = field.bytes().await.map_err(|error| {
            DomainError::malformed(format!("Failed to read uploaded file: {error}"))
        })?;
        return Ok((file_name, bytes.to_vec()));
    }

    Err(DomainError::malformed(
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

    let mut seen_preset_ids = HashSet::new();
    for preset in &parsed.presets {
        let key = preset.id.as_deref().unwrap_or(&preset.name);
        let preset_id = PresetId::stable(key);
        if !seen_preset_ids.insert(preset_id) {
            errors.push(format!("Duplicate bundled preset id \"{preset_id}\""));
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
