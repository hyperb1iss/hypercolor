//! Playlists CRUD endpoints — `/api/v1/library/playlists/*`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, State};
use axum::response::Response;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use tracing::warn;

use hypercolor_types::library::{
    EffectPlaylist, PlaylistId, PlaylistItem, PlaylistItemId, PlaylistItemTarget,
};

use crate::api::AppState;
use crate::api::effects::resolve_effect_metadata;
use crate::api::envelope::{ApiError, ApiResponse};
use crate::playlist_runtime::ActivePlaylistRuntime;

use super::{
    activate_effect_with_controls, metadata_for_effect_id, resolve_preset_id,
    store_error_to_response, unix_epoch_ms,
};

const DEFAULT_PLAYLIST_ITEM_DURATION_MS: u64 = 30_000;

// ── Request / Response Types ────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct PlaylistListResponse {
    pub items: Vec<EffectPlaylist>,
    pub pagination: crate::api::devices::Pagination,
}

#[derive(Debug, Clone, Serialize)]
pub struct ActivePlaylistResponse {
    pub id: String,
    pub name: String,
    pub loop_enabled: bool,
    pub item_count: usize,
    pub started_at_ms: u64,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PlaylistTargetRequest {
    Effect { effect: String },
    Preset { preset_id: String },
}

#[derive(Debug, Deserialize)]
pub struct PlaylistItemRequest {
    pub target: PlaylistTargetRequest,
    pub duration_ms: Option<u64>,
    pub transition_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct SavePlaylistRequest {
    pub name: String,
    pub description: Option<String>,
    pub loop_enabled: Option<bool>,
    pub items: Option<Vec<PlaylistItemRequest>>,
}

// ── Handlers ────────────────────────────────────────────────────────────

/// `GET /api/v1/library/playlists` — list all playlists.
pub async fn list_playlists(State(state): State<Arc<AppState>>) -> Response {
    let items = state.library_store.list_playlists().await;
    let total = items.len();

    ApiResponse::ok(PlaylistListResponse {
        items,
        pagination: crate::api::devices::Pagination {
            offset: 0,
            limit: 50,
            total,
            has_more: false,
        },
    })
}

/// `GET /api/v1/library/playlists/:id` — fetch one playlist.
pub async fn get_playlist(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let Some(playlist_id) = resolve_playlist_id(&state, &id).await else {
        return ApiError::not_found(format!("Playlist not found: {id}"));
    };

    let Some(playlist) = state.library_store.get_playlist(playlist_id).await else {
        return ApiError::not_found(format!("Playlist not found: {id}"));
    };

    ApiResponse::ok(playlist)
}

/// `POST /api/v1/library/playlists` — create a new playlist.
pub async fn create_playlist(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SavePlaylistRequest>,
) -> Response {
    if body.name.trim().is_empty() {
        return ApiError::validation("Playlist name must not be empty");
    }

    let items = match build_playlist_items(&state, body.items.as_deref()).await {
        Ok(items) => items,
        Err(error) => return ApiError::validation(error),
    };
    let now = unix_epoch_ms();
    let playlist = EffectPlaylist {
        id: PlaylistId::new(),
        name: body.name.trim().to_owned(),
        description: body.description,
        items,
        loop_enabled: body.loop_enabled.unwrap_or(true),
        created_at_ms: now,
        updated_at_ms: now,
    };

    if let Err(error) = state.library_store.insert_playlist(playlist.clone()).await {
        return store_error_to_response(&error);
    }

    ApiResponse::created(playlist)
}

/// `PUT /api/v1/library/playlists/:id` — update an existing playlist.
pub async fn update_playlist(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<SavePlaylistRequest>,
) -> Response {
    let Some(playlist_id) = resolve_playlist_id(&state, &id).await else {
        return ApiError::not_found(format!("Playlist not found: {id}"));
    };
    if body.name.trim().is_empty() {
        return ApiError::validation("Playlist name must not be empty");
    }

    let Some(existing) = state.library_store.get_playlist(playlist_id).await else {
        return ApiError::not_found(format!("Playlist not found: {id}"));
    };
    let items = match build_playlist_items(&state, body.items.as_deref()).await {
        Ok(items) => items,
        Err(error) => return ApiError::validation(error),
    };

    let playlist = EffectPlaylist {
        id: playlist_id,
        name: body.name.trim().to_owned(),
        description: body.description,
        items,
        loop_enabled: body.loop_enabled.unwrap_or(existing.loop_enabled),
        created_at_ms: existing.created_at_ms,
        updated_at_ms: unix_epoch_ms(),
    };

    if let Err(error) = state.library_store.update_playlist(playlist.clone()).await {
        return store_error_to_response(&error);
    }

    let active = {
        let mut runtime = state.playlist_runtime.lock().await;
        let should_stop = runtime
            .active
            .as_ref()
            .is_some_and(|active| active.playlist_id == playlist_id);
        if should_stop {
            runtime.active.take()
        } else {
            None
        }
    };
    stop_runtime(active);

    ApiResponse::ok(playlist)
}

/// `DELETE /api/v1/library/playlists/:id` — remove a playlist.
pub async fn delete_playlist(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    let Some(playlist_id) = resolve_playlist_id(&state, &id).await else {
        return ApiError::not_found(format!("Playlist not found: {id}"));
    };

    let active = {
        let mut runtime = state.playlist_runtime.lock().await;
        let should_stop = runtime
            .active
            .as_ref()
            .is_some_and(|active| active.playlist_id == playlist_id);
        if should_stop {
            runtime.active.take()
        } else {
            None
        }
    };
    stop_runtime(active);

    if !state.library_store.remove_playlist(playlist_id).await {
        return ApiError::not_found(format!("Playlist not found: {id}"));
    }

    ApiResponse::ok(serde_json::json!({
        "id": playlist_id.to_string(),
        "deleted": true,
    }))
}

/// `POST /api/v1/library/playlists/:id/activate` — start playlist playback.
pub async fn activate_playlist(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    let Some(playlist_id) = resolve_playlist_id(&state, &id).await else {
        return ApiError::not_found(format!("Playlist not found: {id}"));
    };
    let Some(playlist) = state.library_store.get_playlist(playlist_id).await else {
        return ApiError::not_found(format!("Playlist not found: {id}"));
    };
    if playlist.items.is_empty() {
        return ApiError::validation("Playlist must contain at least one item");
    }

    let previous = {
        let mut runtime = state.playlist_runtime.lock().await;
        runtime.active.take()
    };
    stop_runtime(previous);

    if let Some(first_item) = playlist.items.first()
        && let Err(error) = activate_playlist_item(&state, first_item).await
    {
        return ApiError::internal(format!(
            "Failed to activate first playlist item for '{}': {error}",
            playlist.name
        ));
    }

    let generation;
    let started_at_ms = unix_epoch_ms();
    let (stop_tx, stop_rx) = watch::channel(false);
    {
        let mut runtime = state.playlist_runtime.lock().await;
        generation = runtime.allocate_generation();
    }

    let state_for_task = Arc::clone(&state);
    let playlist_for_task = playlist.clone();
    let task = tokio::spawn(async move {
        run_playlist_task(state_for_task, playlist_for_task, generation, stop_rx, true).await;
    });

    let response_payload;
    {
        let mut runtime = state.playlist_runtime.lock().await;
        let active = ActivePlaylistRuntime {
            generation,
            playlist_id: playlist.id,
            playlist_name: playlist.name,
            loop_enabled: playlist.loop_enabled,
            item_count: playlist.items.len(),
            started_at_ms,
            stop_tx,
            task,
        };
        response_payload = active_playlist_payload(&active);
        runtime.active = Some(active);
    }

    ApiResponse::ok(serde_json::json!({
        "playlist": response_payload,
        "active": true,
    }))
}

/// `GET /api/v1/library/playlists/active` — inspect the active playlist runtime.
pub async fn get_active_playlist(State(state): State<Arc<AppState>>) -> Response {
    let runtime = state.playlist_runtime.lock().await;
    let Some(active) = runtime.active.as_ref() else {
        return ApiError::not_found("No playlist is currently active");
    };

    ApiResponse::ok(serde_json::json!({
        "playlist": active_playlist_payload(active),
        "state": "running",
    }))
}

/// `POST /api/v1/library/playlists/stop` — stop playlist playback if active.
pub async fn stop_playlist(State(state): State<Arc<AppState>>) -> Response {
    let active = {
        let mut runtime = state.playlist_runtime.lock().await;
        runtime.active.take()
    };
    let Some(active) = active else {
        return ApiError::not_found("No playlist is currently active");
    };

    let payload = active_playlist_payload(&active);
    stop_runtime(Some(active));

    ApiResponse::ok(serde_json::json!({
        "playlist": payload,
        "stopped": true,
    }))
}

// ── Helpers ─────────────────────────────────────────────────────────────

fn active_playlist_payload(active: &ActivePlaylistRuntime) -> ActivePlaylistResponse {
    ActivePlaylistResponse {
        id: active.playlist_id.to_string(),
        name: active.playlist_name.clone(),
        loop_enabled: active.loop_enabled,
        item_count: active.item_count,
        started_at_ms: active.started_at_ms,
    }
}

async fn resolve_playlist_id(state: &Arc<AppState>, id_or_name: &str) -> Option<PlaylistId> {
    if let Ok(id) = id_or_name.parse::<PlaylistId>() {
        return Some(id);
    }

    state
        .library_store
        .list_playlists()
        .await
        .iter()
        .find(|playlist| playlist.name.eq_ignore_ascii_case(id_or_name))
        .map(|playlist| playlist.id)
}

fn stop_runtime(active: Option<ActivePlaylistRuntime>) {
    let Some(active) = active else {
        return;
    };
    let _ = active.stop_tx.send(true);
    active.task.abort();
}

async fn run_playlist_task(
    state: Arc<AppState>,
    playlist: EffectPlaylist,
    generation: u64,
    mut stop_rx: watch::Receiver<bool>,
    first_item_already_applied: bool,
) {
    let mut index = 0usize;
    if first_item_already_applied {
        let first_duration = playlist
            .items
            .first()
            .and_then(|item| item.duration_ms)
            .unwrap_or(DEFAULT_PLAYLIST_ITEM_DURATION_MS)
            .max(1);
        if wait_for_item_window(first_duration, &mut stop_rx).await {
            clear_runtime_if_generation_matches(&state, generation).await;
            return;
        }
        index = 1;
        if index >= playlist.items.len() {
            if playlist.loop_enabled {
                index = 0;
            } else {
                clear_runtime_if_generation_matches(&state, generation).await;
                return;
            }
        }
    }

    while index < playlist.items.len() {
        if *stop_rx.borrow() {
            break;
        }

        let Some(item) = playlist.items.get(index) else {
            break;
        };
        if let Err(error) = activate_playlist_item(&state, item).await {
            warn!(
                playlist_id = %playlist.id,
                playlist = %playlist.name,
                item_index = index,
                %error,
                "Playlist item activation failed"
            );
        }

        let duration_ms = item
            .duration_ms
            .unwrap_or(DEFAULT_PLAYLIST_ITEM_DURATION_MS)
            .max(1);
        if wait_for_item_window(duration_ms, &mut stop_rx).await {
            break;
        }

        index += 1;
        if index >= playlist.items.len() {
            if playlist.loop_enabled {
                index = 0;
            } else {
                break;
            }
        }
    }

    clear_runtime_if_generation_matches(&state, generation).await;
}

async fn wait_for_item_window(duration_ms: u64, stop_rx: &mut watch::Receiver<bool>) -> bool {
    let sleep = tokio::time::sleep(Duration::from_millis(duration_ms));
    tokio::pin!(sleep);
    tokio::select! {
        () = &mut sleep => false,
        changed = stop_rx.changed() => changed.is_err() || *stop_rx.borrow(),
    }
}

async fn clear_runtime_if_generation_matches(state: &Arc<AppState>, generation: u64) {
    let mut runtime = state.playlist_runtime.lock().await;
    let should_clear = runtime
        .active
        .as_ref()
        .is_some_and(|active| active.generation == generation);
    if should_clear {
        runtime.active = None;
    }
}

async fn activate_playlist_item(state: &Arc<AppState>, item: &PlaylistItem) -> Result<(), String> {
    match &item.target {
        PlaylistItemTarget::Effect { effect_id } => {
            let metadata = metadata_for_effect_id(state, *effect_id).await?;
            let controls = HashMap::new();
            let activation = activate_effect_with_controls(state, &metadata, &controls)
                .await
                .map_err(|error| error.to_string())?;
            if !activation.rejected.is_empty() {
                warn!(
                    effect_id = %metadata.id,
                    effect = %metadata.name,
                    rejected = ?activation.rejected,
                    "Rejected controls while activating playlist effect item"
                );
            }
            if !activation.warnings.is_empty() {
                warn!(
                    effect_id = %metadata.id,
                    effect = %metadata.name,
                    warnings = ?activation.warnings,
                    "Auto-disabled HTML overlays while activating playlist effect item"
                );
            }
        }
        PlaylistItemTarget::Preset { preset_id } => {
            let Some(preset) = state.library_store.get_preset(*preset_id).await else {
                return Err(format!("playlist references missing preset: {preset_id}"));
            };
            let metadata = metadata_for_effect_id(state, preset.effect_id).await?;
            let activation = activate_effect_with_controls(state, &metadata, &preset.controls)
                .await
                .map_err(|error| error.to_string())?;
            if !activation.rejected.is_empty() {
                warn!(
                    preset_id = %preset.id,
                    preset = %preset.name,
                    rejected = ?activation.rejected,
                    "Rejected controls while activating playlist preset item"
                );
            }
            if !activation.warnings.is_empty() {
                warn!(
                    preset_id = %preset.id,
                    preset = %preset.name,
                    warnings = ?activation.warnings,
                    "Auto-disabled HTML overlays while activating playlist preset item"
                );
            }
        }
    }

    Ok(())
}

async fn build_playlist_items(
    state: &Arc<AppState>,
    items_payload: Option<&[PlaylistItemRequest]>,
) -> Result<Vec<PlaylistItem>, String> {
    let Some(items_payload) = items_payload else {
        return Ok(Vec::new());
    };

    let mut items = Vec::with_capacity(items_payload.len());
    for item in items_payload {
        let target = match &item.target {
            PlaylistTargetRequest::Effect { effect } => {
                let resolved = {
                    let registry = state.effect_registry.read().await;
                    resolve_effect_metadata(&registry, effect)
                };
                let Some(resolved) = resolved else {
                    return Err(format!("Playlist references unknown effect: {effect}"));
                };
                PlaylistItemTarget::Effect {
                    effect_id: resolved.id,
                }
            }
            PlaylistTargetRequest::Preset { preset_id } => {
                let Some(parsed) = resolve_preset_id(state, preset_id).await else {
                    return Err(format!("Playlist references unknown preset: {preset_id}"));
                };
                if state.library_store.get_preset(parsed).await.is_none() {
                    return Err(format!("Playlist references unknown preset: {preset_id}"));
                }
                PlaylistItemTarget::Preset { preset_id: parsed }
            }
        };

        items.push(PlaylistItem {
            id: PlaylistItemId::new(),
            target,
            duration_ms: item.duration_ms,
            transition_ms: item.transition_ms,
        });
    }

    Ok(items)
}
