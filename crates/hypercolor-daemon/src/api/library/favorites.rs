//! Favorites CRUD endpoints — `/api/v1/library/favorites/*`.

use std::collections::HashMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use hypercolor_types::event::{HypercolorEvent, LibraryChangeKind, LibraryCollection};

use crate::api::envelope;
use crate::app_state::AppState;
use crate::domain::{DomainError, ResourceKind};

use super::unix_epoch_ms;

// Wire contracts live in hypercolor-types::api::library — shared with
// the web UI and the TUI.
pub use hypercolor_types::api::library::{
    AddFavoriteRequest, AddFavoriteResponse, DeleteFavoriteResponse, FavoriteListResponse,
    FavoriteSummary,
};

// ── Handlers ────────────────────────────────────────────────────────────

/// `GET /api/v1/library/favorites` — list favorited effects.
pub async fn list_favorites(State(state): State<Arc<AppState>>) -> Response {
    let favorites = state.library_store.list_favorites().await;

    let effect_names: HashMap<_, _> = state
        .domains
        .effects
        .all_metadata()
        .await
        .into_iter()
        .map(|metadata| (metadata.id, metadata.name))
        .collect();

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
    envelope::ok(FavoriteListResponse {
        items,
        total: u64::try_from(total).expect("favorite count fits in u64"),
        page: None,
    })
}

/// `POST /api/v1/library/favorites` — add/update a favorite entry.
pub async fn add_favorite(
    State(state): State<Arc<AppState>>,
    Json(body): Json<AddFavoriteRequest>,
) -> Response {
    let Some(effect) = state.domains.effects.resolve_metadata(&body.effect).await else {
        return DomainError::not_found(ResourceKind::Effect, &body.effect).into_response();
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
    let favorite = match favorite {
        Ok(favorite) => favorite,
        Err(error) => return super::store_error(&error).into_response(),
    };
    state
        .event_bus
        .publish(HypercolorEvent::LibraryStoreChanged {
            collection: LibraryCollection::Favorites,
            entry_id: favorite.effect_id.to_string(),
            kind: LibraryChangeKind::Upserted,
        });

    envelope::ok(AddFavoriteResponse {
        favorite: FavoriteSummary {
            effect_id: favorite.effect_id.to_string(),
            effect_name: effect.name,
            added_at_ms: favorite.added_at_ms,
        },
        created: !existing,
    })
}

/// `DELETE /api/v1/library/favorites/{effect}` — remove a favorite by effect id/name.
pub async fn remove_favorite(
    State(state): State<Arc<AppState>>,
    Path(effect): Path<String>,
) -> Response {
    let Some(effect) = state.domains.effects.resolve_metadata(&effect).await else {
        return DomainError::not_found(ResourceKind::Favorite, &effect).into_response();
    };

    let removed = match state.library_store.remove_favorite(effect.id).await {
        Ok(removed) => removed,
        Err(error) => return super::store_error(&error).into_response(),
    };
    if !removed {
        return DomainError::not_found(ResourceKind::Favorite, effect.id).into_response();
    }
    state
        .event_bus
        .publish(HypercolorEvent::LibraryStoreChanged {
            collection: LibraryCollection::Favorites,
            entry_id: effect.id.to_string(),
            kind: LibraryChangeKind::Removed,
        });

    envelope::ok(DeleteFavoriteResponse {
        effect_id: effect.id.to_string(),
        deleted: true,
    })
}
