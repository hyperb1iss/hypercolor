//! Effect endpoints — `/api/v1/effects/*`.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::response::Response;
use serde::{Deserialize, Serialize};

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

    let Ok(uuid) = id.parse::<uuid::Uuid>() else {
        return ApiError::bad_request(format!("Invalid effect ID: {id}"));
    };
    let effect_id = hypercolor_types::effect::EffectId::new(uuid);

    let Some(entry) = registry.get(&effect_id) else {
        return ApiError::not_found(format!("Effect not found: {id}"));
    };

    let meta = &entry.metadata;
    ApiResponse::ok(EffectSummary {
        id: meta.id.to_string(),
        name: meta.name.clone(),
        description: meta.description.clone(),
        author: meta.author.clone(),
        category: format!("{}", meta.category),
        tags: meta.tags.clone(),
        version: meta.version.clone(),
    })
}

/// `POST /api/v1/effects/:id/apply` — Start rendering an effect.
pub async fn apply_effect(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    body: Option<Json<ApplyEffectRequest>>,
) -> Response {
    let registry = state.effect_registry.read().await;

    let Ok(uuid) = id.parse::<uuid::Uuid>() else {
        return ApiError::bad_request(format!("Invalid effect ID: {id}"));
    };
    let effect_id = hypercolor_types::effect::EffectId::new(uuid);

    let Some(entry) = registry.get(&effect_id) else {
        return ApiError::not_found(format!("Effect not found: {id}"));
    };

    let meta = &entry.metadata;
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
            "id": meta.id.to_string(),
            "name": meta.name,
        },
        "applied_controls": body.as_ref().and_then(|b| b.controls.clone()).unwrap_or(serde_json::json!({})),
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
