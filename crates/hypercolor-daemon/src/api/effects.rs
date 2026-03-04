//! Effect endpoints — `/api/v1/effects/*`.

use std::collections::HashMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::response::Response;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use hypercolor_core::effect::{EffectRegistry, create_renderer_for_metadata};
use hypercolor_types::effect::{
    ControlDefinition, ControlValue, EffectId, EffectMetadata, EffectSource,
};

use crate::api::AppState;
use crate::api::envelope::{ApiError, ApiResponse};

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
}

#[derive(Debug, Serialize)]
pub struct ActiveEffectResponse {
    pub id: String,
    pub name: String,
    pub state: String,
    pub controls: Vec<ControlDefinition>,
    pub control_values: HashMap<String, ControlValue>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateCurrentControlsRequest {
    pub controls: Option<serde_json::Value>,
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
    pub active_control_values: Option<HashMap<String, ControlValue>>,
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
        active_control_values,
    })
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

    let renderer = match create_renderer_for_metadata(&metadata) {
        Ok(renderer) => renderer,
        Err(error) => {
            warn!(
                effect_id = %metadata.id,
                effect = %metadata.name,
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
    let previous_effect: Option<String>;

    {
        let mut engine = state.effect_engine.lock().await;
        previous_effect = engine.active_metadata().map(|meta| meta.name.clone());
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

    log_effect_apply_completion(
        previous_effect.as_deref(),
        &metadata.name,
        controls.len(),
        &dropped_controls,
    );
    let (transition_type, duration_ms) = extract_transition_request(body.as_ref());

    ApiResponse::ok(serde_json::json!({
        "effect": {
            "id": metadata.id.to_string(),
            "name": metadata.name,
        },
        "applied_controls": controls,
        "transition": {
            "type": transition_type,
            "duration_ms": duration_ms,
        },
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
    })
}

/// `POST /api/v1/effects/stop` — Stop the currently active effect.
pub async fn stop_effect(State(state): State<Arc<AppState>>) -> Response {
    let mut engine = state.effect_engine.lock().await;

    if engine.active_metadata().is_none() {
        return ApiError::not_found("No effect is currently active");
    }

    engine.deactivate();

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

    ApiResponse::ok(serde_json::json!({
        "effect": effect_name,
        "applied": applied,
        "rejected": rejected,
    }))
}

fn resolve_effect_metadata(registry: &EffectRegistry, id_or_name: &str) -> Option<EffectMetadata> {
    if let Ok(uuid) = id_or_name.parse::<uuid::Uuid>() {
        let effect_id = EffectId::new(uuid);
        return registry.get(&effect_id).map(|entry| entry.metadata.clone());
    }

    registry
        .iter()
        .find(|(_, entry)| entry.metadata.name.eq_ignore_ascii_case(id_or_name))
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

fn extract_transition_request(body: Option<&Json<ApplyEffectRequest>>) -> (String, u64) {
    let transition_type = body
        .and_then(|payload| payload.transition.as_ref())
        .and_then(|transition| transition.transition_type.clone())
        .unwrap_or_else(|| "crossfade".to_owned());
    let duration_ms = body
        .and_then(|payload| payload.transition.as_ref())
        .and_then(|transition| transition.duration_ms)
        .unwrap_or(300);
    (transition_type, duration_ms)
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

fn json_to_control_value(value: &serde_json::Value) -> Option<ControlValue> {
    if let Some(v) = value.as_i64() {
        let int = i32::try_from(v).ok()?;
        return Some(ControlValue::Integer(int));
    }
    if let Some(v) = value.as_f64() {
        let float = parse_f32(v)?;
        return Some(ControlValue::Float(float));
    }
    if let Some(v) = value.as_bool() {
        return Some(ControlValue::Boolean(v));
    }
    if let Some(v) = value.as_str() {
        return Some(ControlValue::Text(v.to_owned()));
    }
    if let Some(array) = value.as_array() {
        if array.len() == 4 {
            let mut color = [0.0f32; 4];
            for (idx, component) in array.iter().enumerate() {
                let parsed = component.as_f64()?;
                color[idx] = parse_f32(parsed)?;
            }
            return Some(ControlValue::Color(color));
        }
    }
    None
}

fn parse_f32(value: f64) -> Option<f32> {
    if !value.is_finite() {
        return None;
    }
    value.to_string().parse::<f32>().ok()
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
