//! Effect endpoints — `/api/v1/effects/*`.

use std::collections::HashMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::response::Response;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use hypercolor_core::effect::{EffectRegistry, create_renderer_for_metadata_with_mode};
use hypercolor_types::effect::{
    ControlDefinition, ControlValue, EffectId, EffectMetadata, EffectSource, PresetTemplate,
};
use hypercolor_types::event::{ChangeTrigger, EffectRef, EffectStopReason, HypercolorEvent};
use hypercolor_types::spatial::SpatialLayout;

use crate::api::AppState;
use crate::api::control_values::json_to_control_value;
use crate::api::envelope::{ApiError, ApiResponse};
use crate::effect_layouts;
use crate::scene_transactions::apply_layout_update;

// ── Request / Response Types ─────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ApplyEffectRequest {
    pub controls: Option<serde_json::Value>,
    pub transition: Option<TransitionRequest>,
}

#[derive(Debug, Deserialize)]
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

#[derive(Debug, Serialize)]
pub struct EffectListResponse {
    pub items: Vec<EffectSummary>,
    pub pagination: super::devices::Pagination,
}

#[derive(Debug, Serialize)]
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
}

#[derive(Debug, Serialize)]
pub struct ActiveEffectResponse {
    pub id: String,
    pub name: String,
    pub state: String,
    pub controls: Vec<ControlDefinition>,
    pub control_values: HashMap<String, ControlValue>,
    pub active_preset_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateCurrentControlsRequest {
    pub controls: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct SetEffectLayoutRequest {
    pub layout_id: String,
}

#[derive(Debug, Serialize)]
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
}

#[derive(Debug, Serialize)]
pub struct LayoutLinkSummary {
    pub id: String,
    pub name: String,
    pub canvas_width: u32,
    pub canvas_height: u32,
    pub zone_count: usize,
}

#[derive(Debug, Serialize)]
pub struct EffectLayoutApplyResult {
    pub associated_layout_id: String,
    pub resolved: bool,
    pub applied: bool,
    pub layout: Option<LayoutLinkSummary>,
}

#[derive(Debug)]
enum ResolveLayoutLinkError {
    NotFound(String),
    AmbiguousName(String),
}

// ── Handlers ─────────────────────────────────────────────────────────────

/// `GET /api/v1/effects` — List all registered effects.
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
pub async fn get_effect(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let registry = state.effect_registry.read().await;

    let Some(meta) = resolve_effect_metadata(&registry, &id) else {
        return ApiError::not_found(format!("Effect not found: {id}"));
    };
    drop(registry);

    let active_control_values = {
        let engine = state.effect_engine.lock().await;
        engine
            .active_metadata()
            .filter(|active| active.id == meta.id)
            .map(|_| engine.active_controls().clone())
    };

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
        active_control_values,
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
    let applied_transition = match validate_transition_request(body.as_ref()) {
        Ok(transition) => transition,
        Err(error) => return ApiError::bad_request(error),
    };

    let render_acceleration_mode =
        crate::api::configured_render_acceleration_mode(state.config_manager.as_ref());
    let renderer = match create_renderer_for_metadata_with_mode(&metadata, render_acceleration_mode)
    {
        Ok(renderer) => renderer,
        Err(error) => {
            warn!(
                effect_id = %metadata.id,
                effect = %metadata.name,
                requested_mode = ?render_acceleration_mode,
                %error,
                "Failed to prepare effect renderer"
            );
            return ApiError::bad_request(format!(
                "Failed to prepare renderer for effect '{}': {error}",
                metadata.name
            ));
        }
    };

    let controls = extract_request_controls(body.as_ref());
    let mut dropped_controls = Vec::new();
    let previous_effect: Option<EffectRef>;

    {
        let mut engine = state.effect_engine.lock().await;
        previous_effect = engine.active_metadata().map(effect_ref);
        if let Err(e) = engine.activate(renderer, metadata.clone()) {
            warn!(
                effect_id = %metadata.id,
                effect = %metadata.name,
                %e,
                "Effect activation failed"
            );
            return ApiError::internal(format!(
                "Failed to activate effect '{}': {e}",
                metadata.name
            ));
        }

        for (name, value) in &controls {
            if let Some(control_value) = json_to_control_value(value) {
                if let Err(error) = engine.set_control_checked(name, &control_value) {
                    dropped_controls.push(format!("{name} ({error})"));
                }
            } else {
                dropped_controls.push(name.clone());
            }
        }
    }
    let warnings =
        super::displays::auto_disable_html_overlays_for_effect(state.as_ref(), &metadata).await;

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
    let applied_layout = apply_associated_layout(&state, &metadata.id.to_string()).await;
    super::persist_runtime_session(&state).await;

    ApiResponse::ok(serde_json::json!({
        "effect": {
            "id": metadata.id.to_string(),
            "name": metadata.name,
        },
        "applied_controls": controls,
        "layout": applied_layout,
        "transition": {
            "type": applied_transition.transition_type,
            "duration_ms": applied_transition.duration_ms,
        },
        "warnings": warnings,
    }))
}

/// `GET /api/v1/effects/active` — Get the currently active effect.
pub async fn get_active_effect(State(state): State<Arc<AppState>>) -> Response {
    let engine = state.effect_engine.lock().await;

    let Some(meta) = engine.active_metadata() else {
        return ApiError::not_found("No effect is currently active");
    };

    ApiResponse::ok(ActiveEffectResponse {
        id: meta.id.to_string(),
        name: meta.name.clone(),
        state: format!("{:?}", engine.state()).to_lowercase(),
        controls: meta.controls.clone(),
        control_values: engine.active_controls().clone(),
        active_preset_id: engine.active_preset_id().map(String::from),
    })
}

/// `POST /api/v1/effects/stop` — Stop the currently active effect.
pub async fn stop_effect(State(state): State<Arc<AppState>>) -> Response {
    let mut engine = state.effect_engine.lock().await;

    let Some(previous_effect) = engine.active_metadata().cloned() else {
        return ApiError::not_found("No effect is currently active");
    };

    engine.deactivate();
    drop(engine);
    state.event_bus.publish(HypercolorEvent::EffectStopped {
        effect: effect_ref(&previous_effect),
        reason: EffectStopReason::Stopped,
    });
    super::persist_runtime_session(&state).await;

    ApiResponse::ok(serde_json::json!({
        "stopped": true,
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
    let effect_name: String;
    {
        let mut engine = state.effect_engine.lock().await;
        let Some(active_meta) = engine.active_metadata() else {
            return ApiError::not_found("No effect is currently active");
        };
        effect_name = active_meta.name.clone();

        for (name, value) in &controls {
            let Some(control_value) = json_to_control_value(value) else {
                rejected.push(format!("{name} (unsupported JSON shape)"));
                continue;
            };

            match engine.set_control_checked(name, &control_value) {
                Ok(normalized) => {
                    applied.insert(name.clone(), normalized);
                }
                Err(error) => rejected.push(format!("{name} ({error})")),
            }
        }
    }

    if !rejected.is_empty() {
        warn!(
            effect = %effect_name,
            rejected_controls = ?rejected,
            "Rejected one or more control updates"
        );
    }
    super::persist_runtime_session(&state).await;

    ApiResponse::ok(serde_json::json!({
        "effect": effect_name,
        "applied": applied,
        "rejected": rejected,
    }))
}

/// `POST /api/v1/effects/current/reset` — Reset all controls on the active
/// effect back to their metadata-defined defaults.
pub async fn reset_controls(State(state): State<Arc<AppState>>) -> Response {
    let mut engine = state.effect_engine.lock().await;

    let Some(meta) = engine.active_metadata().cloned() else {
        return ApiError::not_found("No effect is currently active");
    };

    if let Err(e) = engine.reset_to_defaults() {
        return ApiError::internal(format!("Failed to reset controls: {e}"));
    }
    drop(engine);
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

#[derive(Debug, Serialize)]
pub struct RescanResponse {
    pub added: usize,
    pub removed: usize,
    pub updated: usize,
}

pub(super) fn resolve_effect_metadata(
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

fn extract_request_controls(
    body: Option<&Json<ApplyEffectRequest>>,
) -> serde_json::Map<String, serde_json::Value> {
    body.and_then(|payload| payload.controls.as_ref())
        .and_then(serde_json::Value::as_object)
        .cloned()
        .unwrap_or_default()
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

fn effect_ref(metadata: &EffectMetadata) -> EffectRef {
    EffectRef {
        id: metadata.id.to_string(),
        name: metadata.name.clone(),
        engine: "servo".to_owned(),
    }
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

async fn apply_associated_layout(
    state: &Arc<AppState>,
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
