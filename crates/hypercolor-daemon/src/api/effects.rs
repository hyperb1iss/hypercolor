//! Effect endpoints — `/api/v1/effects/*`.

use std::collections::HashMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::response::Response;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use hypercolor_core::effect::EffectRegistry;
use hypercolor_types::effect::{
    ControlBinding, ControlDefinition, ControlValue, EffectCategory, EffectId, EffectMetadata,
    EffectSource, PresetTemplate,
};
use hypercolor_types::event::{
    ChangeTrigger, EffectRef, EffectStopReason, EventControlValue, HypercolorEvent,
    RenderGroupChangeKind,
};
use hypercolor_types::scene::RenderGroup;
use hypercolor_types::spatial::SpatialLayout;

use crate::api::AppState;
use crate::api::control_values::json_to_control_value;
use crate::api::envelope::{ApiError, ApiResponse};
use crate::api::publish_render_group_changed;
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub render_group_id: Option<String>,
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
    if metadata.category == EffectCategory::Display {
        return ApiError::validation(format!(
            "Effect '{}' is a display face and must be assigned to a display device, not applied to the LED pipeline",
            metadata.name
        ));
    }
    let applied_transition = match validate_transition_request(body.as_ref()) {
        Ok(transition) => transition,
        Err(error) => return ApiError::bad_request(error),
    };

    let controls = extract_request_controls(body.as_ref());
    let (normalized_controls, dropped_controls) = normalize_control_payload(&metadata, &controls);
    let previous_effect = active_primary_effect(state.as_ref())
        .await
        .map(|(_, effect)| effect_ref(&effect));
    let layout = resolve_full_scope_layout(state.as_ref()).await;

    let (scene_id, group, change_kind) = {
        let mut scene_manager = state.scene_manager.write().await;
        let scene_id = match scene_manager.active_scene_id().copied() {
            Some(scene_id) => scene_id,
            None => return ApiError::internal("No active scene available"),
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
            None,
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
    publish_render_group_changed(state.as_ref(), scene_id, &group, change_kind);
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
    let Some((group, meta)) = active_primary_effect(state.as_ref()).await else {
        return ApiError::not_found("No effect is currently active");
    };

    ApiResponse::ok(ActiveEffectResponse {
        id: meta.id.to_string(),
        name: meta.name.clone(),
        state: "running".to_owned(),
        controls: controls_with_group_bindings(&meta, &group),
        control_values: resolved_control_values(&meta, &group),
        active_preset_id: group.preset_id.map(|preset| preset.to_string()),
        render_group_id: Some(group.id.to_string()),
    })
}

/// `POST /api/v1/effects/stop` — Stop the currently active effect.
pub async fn stop_effect(State(state): State<Arc<AppState>>) -> Response {
    let Some((group, previous_effect)) = active_primary_effect(state.as_ref()).await else {
        return ApiError::not_found("No effect is currently active");
    };

    let (scene_id, cleared_group) = {
        let mut scene_manager = state.scene_manager.write().await;
        let scene_id = match scene_manager.active_scene_id().copied() {
            Some(scene_id) => scene_id,
            None => return ApiError::internal("No active scene available"),
        };
        let Some(cleared_group) = scene_manager.clear_group_effect(group.id).cloned() else {
            return ApiError::not_found("No effect is currently active");
        };
        (scene_id, cleared_group)
    };
    state.event_bus.publish(HypercolorEvent::EffectStopped {
        effect: effect_ref(&previous_effect),
        reason: EffectStopReason::Stopped,
    });
    publish_render_group_changed(
        state.as_ref(),
        scene_id,
        &cleared_group,
        RenderGroupChangeKind::Updated,
    );
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
        let scene_id = match scene_manager.active_scene_id().copied() {
            Some(scene_id) => scene_id,
            None => return ApiError::internal("No active scene available"),
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
        let scene_id = match scene_manager.active_scene_id().copied() {
            Some(scene_id) => scene_id,
            None => return ApiError::internal("No active scene available"),
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
        let scene_id = match scene_manager.active_scene_id().copied() {
            Some(scene_id) => scene_id,
            None => return ApiError::internal("No active scene available"),
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
