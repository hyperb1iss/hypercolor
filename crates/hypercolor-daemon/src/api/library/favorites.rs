//! Favorites CRUD endpoints — `/api/v1/library/favorites/*`.

use std::collections::HashMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::response::Response;
use serde::{Deserialize, Serialize};

use crate::api::AppState;
use crate::api::effects::resolve_effect_metadata;
use crate::api::envelope::{ApiError, ApiResponse};

use super::unix_epoch_ms;

// ── Request / Response Types ────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct FavoriteSummary {
    pub effect_id: String,
    pub effect_name: String,
    pub added_at_ms: u64,
}

#[derive(Debug, Serialize)]
pub struct FavoriteListResponse {
    pub items: Vec<FavoriteSummary>,
    pub pagination: crate::api::devices::Pagination,
}

#[derive(Debug, Deserialize)]
pub struct AddFavoriteRequest {
    pub effect: String,
}

// ── Handlers ────────────────────────────────────────────────────────────

/// `GET /api/v1/library/favorites` — list favorited effects.
pub async fn list_favorites(State(state): State<Arc<AppState>>) -> Response {
    let favorites = state.library_store.list_favorites().await;

    let registry = state.effect_registry.read().await;
    let effect_names: HashMap<_, _> = registry
        .iter()
        .map(|(_, entry)| (entry.metadata.id, entry.metadata.name.clone()))
        .collect();
    drop(registry);

    let items: Vec<FavoriteSummary> = favorites
        .iter()
        .map(|favorite| FavoriteSummary {
            effect_id: favorite.effect_id.to_string(),
            effect_name: effect_names
                .get(&favorite.effect_id)
                .cloned()
                .unwrap_or_else(|| favorite.effect_id.to_string()),
            added_at_ms: favorite.added_at_ms,
        })
        .collect();

    let total = items.len();
    ApiResponse::ok(FavoriteListResponse {
        items,
        pagination: crate::api::devices::Pagination {
            offset: 0,
            limit: 50,
            total,
            has_more: false,
        },
    })
}

/// `POST /api/v1/library/favorites` — add/update a favorite entry.
pub async fn add_favorite(
    State(state): State<Arc<AppState>>,
    Json(body): Json<AddFavoriteRequest>,
) -> Response {
    let effect = {
        let registry = state.effect_registry.read().await;
        let Some(effect) = resolve_effect_metadata(&registry, &body.effect) else {
            return ApiError::not_found(format!("Effect not found: {}", body.effect));
        };
        effect
    };

    let existing = state
        .library_store
        .list_favorites()
        .await
        .iter()
        .any(|favorite| favorite.effect_id == effect.id);
    let favorite = state
        .library_store
        .upsert_favorite(effect.id, unix_epoch_ms())
        .await;

    ApiResponse::ok(serde_json::json!({
        "favorite": FavoriteSummary {
            effect_id: favorite.effect_id.to_string(),
            effect_name: effect.name,
            added_at_ms: favorite.added_at_ms,
        },
        "created": !existing,
    }))
}

/// `DELETE /api/v1/library/favorites/:effect` — remove a favorite by effect id/name.
pub async fn remove_favorite(
    State(state): State<Arc<AppState>>,
    Path(effect): Path<String>,
) -> Response {
    let effect = {
        let registry = state.effect_registry.read().await;
        let Some(effect) = resolve_effect_metadata(&registry, &effect) else {
            return ApiError::not_found("Favorite effect not found");
        };
        effect
    };

    if !state.library_store.remove_favorite(effect.id).await {
        return ApiError::not_found("Favorite effect not found");
    }

    ApiResponse::ok(serde_json::json!({
        "effect_id": effect.id.to_string(),
        "deleted": true,
    }))
}
