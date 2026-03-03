//! Effect endpoints — `/api/v1/effects/*`.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::response::Response;
use serde::{Deserialize, Serialize};

use hypercolor_core::effect::EffectRegistry;
use hypercolor_core::effect::builtin::create_builtin_renderer;
use hypercolor_types::effect::{ControlValue, EffectId, EffectMetadata};

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
    pub tags: Vec<String>,
    pub version: String,
}

#[derive(Debug, Serialize)]
pub struct ActiveEffectResponse {
    pub id: String,
    pub name: String,
    pub state: String,
}

// ── Handlers ─────────────────────────────────────────────────────────────

/// `GET /api/v1/effects` — List all registered effects.
pub async fn list_effects(State(state): State<Arc<AppState>>) -> Response {
    let registry = state.effect_registry.read().await;
    let items: Vec<EffectSummary> = registry
        .iter()
        .map(|(_, entry)| {
            let meta = &entry.metadata;
            EffectSummary {
                id: meta.id.to_string(),
                name: meta.name.clone(),
                description: meta.description.clone(),
                author: meta.author.clone(),
                category: format!("{}", meta.category),
                tags: meta.tags.clone(),
                version: meta.version.clone(),
            }
        })
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

/// `GET /api/v1/effects/:id` — Get a single effect's metadata.
pub async fn get_effect(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let registry = state.effect_registry.read().await;

    let Some(meta) = resolve_effect_metadata(&registry, &id) else {
        return ApiError::not_found(format!("Effect not found: {id}"));
    };

    ApiResponse::ok(EffectSummary {
        id: meta.id.to_string(),
        name: meta.name,
        description: meta.description,
        author: meta.author,
        category: format!("{}", meta.category),
        tags: meta.tags,
        version: meta.version,
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

    let Some(renderer) = create_builtin_renderer(&metadata.name) else {
        return ApiError::bad_request(format!(
            "Effect '{}' is registered but has no runnable built-in renderer",
            metadata.name
        ));
    };

    let controls = body
        .as_ref()
        .and_then(|b| b.controls.as_ref())
        .and_then(serde_json::Value::as_object)
        .cloned()
        .unwrap_or_default();

    {
        let mut engine = state.effect_engine.lock().await;
        if let Err(e) = engine.activate(renderer, metadata.clone()) {
            return ApiError::internal(format!(
                "Failed to activate effect '{}': {e}",
                metadata.name
            ));
        }

        for (name, value) in &controls {
            if let Some(control_value) = json_to_control_value(value) {
                engine.set_control(name, &control_value);
            }
        }
    }

    let transition_type = body
        .as_ref()
        .and_then(|b| b.transition.as_ref())
        .and_then(|t| t.transition_type.clone())
        .unwrap_or_else(|| "crossfade".to_owned());
    let duration_ms = body
        .as_ref()
        .and_then(|b| b.transition.as_ref())
        .and_then(|t| t.duration_ms)
        .unwrap_or(300);

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
