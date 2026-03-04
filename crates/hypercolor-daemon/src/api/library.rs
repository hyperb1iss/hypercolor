//! Saved effect library endpoints — `/api/v1/library/*`.

use std::collections::HashMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::response::Response;
use serde::{Deserialize, Serialize};

use hypercolor_core::effect::create_renderer_for_metadata;
use hypercolor_types::effect::ControlValue;
use hypercolor_types::library::{
    EffectPlaylist, EffectPreset, PlaylistId, PlaylistItem, PlaylistItemId, PlaylistItemTarget,
    PresetId,
};

use crate::api::AppState;
use crate::api::control_values::json_to_control_value;
use crate::api::effects::resolve_effect_metadata;
use crate::api::envelope::{ApiError, ApiResponse};
use crate::library::LibraryStoreError;

// ── Request / Response Types ─────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct FavoriteSummary {
    pub effect_id: String,
    pub effect_name: String,
    pub added_at_ms: u64,
}

#[derive(Debug, Serialize)]
pub struct FavoriteListResponse {
    pub items: Vec<FavoriteSummary>,
    pub pagination: super::devices::Pagination,
}

#[derive(Debug, Deserialize)]
pub struct AddFavoriteRequest {
    pub effect: String,
}

#[derive(Debug, Serialize)]
pub struct PresetListResponse {
    pub items: Vec<EffectPreset>,
    pub pagination: super::devices::Pagination,
}

#[derive(Debug, Deserialize)]
pub struct SavePresetRequest {
    pub name: String,
    pub description: Option<String>,
    pub effect: String,
    pub controls: Option<serde_json::Value>,
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
pub struct PlaylistListResponse {
    pub items: Vec<EffectPlaylist>,
    pub pagination: super::devices::Pagination,
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

// ── Favorites ────────────────────────────────────────────────────────────

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
        pagination: super::devices::Pagination {
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

// ── Presets ──────────────────────────────────────────────────────────────

/// `GET /api/v1/library/presets` — list all saved presets.
pub async fn list_presets(State(state): State<Arc<AppState>>) -> Response {
    let items = state.library_store.list_presets().await;
    let total = items.len();

    ApiResponse::ok(PresetListResponse {
        items,
        pagination: super::devices::Pagination {
            offset: 0,
            limit: 50,
            total,
            has_more: false,
        },
    })
}

/// `GET /api/v1/library/presets/:id` — fetch one preset.
pub async fn get_preset(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let Ok(preset_id) = id.parse::<PresetId>() else {
        return ApiError::bad_request(format!("Invalid preset id: {id}"));
    };

    let Some(preset) = state.library_store.get_preset(preset_id).await else {
        return ApiError::not_found(format!("Preset not found: {id}"));
    };

    ApiResponse::ok(preset)
}

/// `POST /api/v1/library/presets` — create a new saved preset.
pub async fn create_preset(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SavePresetRequest>,
) -> Response {
    if body.name.trim().is_empty() {
        return ApiError::validation("Preset name must not be empty");
    }

    let effect = {
        let registry = state.effect_registry.read().await;
        let Some(effect) = resolve_effect_metadata(&registry, &body.effect) else {
            return ApiError::not_found(format!("Effect not found: {}", body.effect));
        };
        effect
    };

    let controls = match parse_preset_controls(&effect, body.controls.as_ref()) {
        Ok(controls) => controls,
        Err(rejected) => {
            return ApiError::validation(format!(
                "Invalid preset controls: {}",
                rejected.join(", ")
            ));
        }
    };

    let now = unix_epoch_ms();
    let preset = EffectPreset {
        id: PresetId::new(),
        name: body.name.trim().to_owned(),
        description: body.description,
        effect_id: effect.id,
        controls,
        tags: normalize_tags(body.tags),
        created_at_ms: now,
        updated_at_ms: now,
    };

    if let Err(error) = state.library_store.insert_preset(preset.clone()).await {
        return store_error_to_response(&error);
    }

    ApiResponse::created(preset)
}

/// `PUT /api/v1/library/presets/:id` — update an existing preset.
pub async fn update_preset(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<SavePresetRequest>,
) -> Response {
    let Ok(preset_id) = id.parse::<PresetId>() else {
        return ApiError::bad_request(format!("Invalid preset id: {id}"));
    };
    if body.name.trim().is_empty() {
        return ApiError::validation("Preset name must not be empty");
    }

    let Some(existing) = state.library_store.get_preset(preset_id).await else {
        return ApiError::not_found(format!("Preset not found: {id}"));
    };

    let effect = {
        let registry = state.effect_registry.read().await;
        let Some(effect) = resolve_effect_metadata(&registry, &body.effect) else {
            return ApiError::not_found(format!("Effect not found: {}", body.effect));
        };
        effect
    };

    let controls = match parse_preset_controls(&effect, body.controls.as_ref()) {
        Ok(controls) => controls,
        Err(rejected) => {
            return ApiError::validation(format!(
                "Invalid preset controls: {}",
                rejected.join(", ")
            ));
        }
    };

    let preset = EffectPreset {
        id: preset_id,
        name: body.name.trim().to_owned(),
        description: body.description,
        effect_id: effect.id,
        controls,
        tags: normalize_tags(body.tags),
        created_at_ms: existing.created_at_ms,
        updated_at_ms: unix_epoch_ms(),
    };

    if let Err(error) = state.library_store.update_preset(preset.clone()).await {
        return store_error_to_response(&error);
    }

    ApiResponse::ok(preset)
}

/// `DELETE /api/v1/library/presets/:id` — remove a preset.
pub async fn delete_preset(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let Ok(preset_id) = id.parse::<PresetId>() else {
        return ApiError::bad_request(format!("Invalid preset id: {id}"));
    };

    if !state.library_store.remove_preset(preset_id).await {
        return ApiError::not_found(format!("Preset not found: {id}"));
    }

    ApiResponse::ok(serde_json::json!({
        "id": preset_id.to_string(),
        "deleted": true,
    }))
}

/// `POST /api/v1/library/presets/:id/apply` — activate a preset immediately.
pub async fn apply_preset(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let Ok(preset_id) = id.parse::<PresetId>() else {
        return ApiError::bad_request(format!("Invalid preset id: {id}"));
    };
    let Some(preset) = state.library_store.get_preset(preset_id).await else {
        return ApiError::not_found(format!("Preset not found: {id}"));
    };

    let metadata = {
        let registry = state.effect_registry.read().await;
        let Some(entry) = registry.get(&preset.effect_id) else {
            return ApiError::not_found(format!(
                "Preset references missing effect: {}",
                preset.effect_id
            ));
        };
        entry.metadata.clone()
    };

    let renderer = match create_renderer_for_metadata(&metadata) {
        Ok(renderer) => renderer,
        Err(error) => {
            return ApiError::bad_request(format!(
                "Failed to prepare renderer for preset '{}': {error}",
                preset.name
            ));
        }
    };

    let mut applied: HashMap<String, ControlValue> = HashMap::new();
    let mut rejected: Vec<String> = Vec::new();
    {
        let mut engine = state.effect_engine.lock().await;
        if let Err(error) = engine.activate(renderer, metadata.clone()) {
            return ApiError::internal(format!(
                "Failed to activate effect '{}' from preset '{}': {error}",
                metadata.name, preset.name
            ));
        }

        for (name, value) in &preset.controls {
            match engine.set_control_checked(name, value) {
                Ok(normalized) => {
                    applied.insert(name.clone(), normalized);
                }
                Err(error) => rejected.push(format!("{name} ({error})")),
            }
        }
    }

    ApiResponse::ok(serde_json::json!({
        "preset": {
            "id": preset.id.to_string(),
            "name": preset.name,
        },
        "effect": {
            "id": metadata.id.to_string(),
            "name": metadata.name,
        },
        "applied_controls": applied,
        "rejected_controls": rejected,
    }))
}

// ── Playlists ────────────────────────────────────────────────────────────

/// `GET /api/v1/library/playlists` — list all playlists.
pub async fn list_playlists(State(state): State<Arc<AppState>>) -> Response {
    let items = state.library_store.list_playlists().await;
    let total = items.len();

    ApiResponse::ok(PlaylistListResponse {
        items,
        pagination: super::devices::Pagination {
            offset: 0,
            limit: 50,
            total,
            has_more: false,
        },
    })
}

/// `GET /api/v1/library/playlists/:id` — fetch one playlist.
pub async fn get_playlist(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let Ok(playlist_id) = id.parse::<PlaylistId>() else {
        return ApiError::bad_request(format!("Invalid playlist id: {id}"));
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
    let Ok(playlist_id) = id.parse::<PlaylistId>() else {
        return ApiError::bad_request(format!("Invalid playlist id: {id}"));
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

    ApiResponse::ok(playlist)
}

/// `DELETE /api/v1/library/playlists/:id` — remove a playlist.
pub async fn delete_playlist(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    let Ok(playlist_id) = id.parse::<PlaylistId>() else {
        return ApiError::bad_request(format!("Invalid playlist id: {id}"));
    };

    if !state.library_store.remove_playlist(playlist_id).await {
        return ApiError::not_found(format!("Playlist not found: {id}"));
    }

    ApiResponse::ok(serde_json::json!({
        "id": playlist_id.to_string(),
        "deleted": true,
    }))
}

// ── Helpers ──────────────────────────────────────────────────────────────

fn normalize_tags(tags: Option<Vec<String>>) -> Vec<String> {
    tags.unwrap_or_default()
        .into_iter()
        .map(|tag| tag.trim().to_owned())
        .filter(|tag| !tag.is_empty())
        .collect()
}

fn parse_preset_controls(
    effect: &hypercolor_types::effect::EffectMetadata,
    controls_payload: Option<&serde_json::Value>,
) -> Result<HashMap<String, ControlValue>, Vec<String>> {
    let Some(controls_json) = controls_payload else {
        return Ok(HashMap::new());
    };
    let Some(control_map) = controls_json.as_object() else {
        return Err(vec!["controls must be a JSON object".to_owned()]);
    };

    let mut normalized = HashMap::new();
    let mut rejected = Vec::new();
    for (name, raw_value) in control_map {
        let Some(parsed) = json_to_control_value(raw_value) else {
            rejected.push(format!("{name} (unsupported JSON shape)"));
            continue;
        };
        let Some(definition) = effect.control_by_id(name) else {
            rejected.push(format!("{name} (unknown control)"));
            continue;
        };
        match definition.validate_value(&parsed) {
            Ok(validated) => {
                normalized.insert(name.clone(), validated);
            }
            Err(error) => rejected.push(format!("{name} ({error})")),
        }
    }

    if rejected.is_empty() {
        Ok(normalized)
    } else {
        Err(rejected)
    }
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
                let parsed = preset_id.parse::<PresetId>().map_err(|_| {
                    format!("Playlist references invalid preset id: {preset_id}")
                })?;
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

fn store_error_to_response(error: &LibraryStoreError) -> Response {
    match error {
        LibraryStoreError::PresetNotFound(id) => {
            ApiError::not_found(format!("Preset not found: {id}"))
        }
        LibraryStoreError::PresetConflict(id) => {
            ApiError::conflict(format!("Preset already exists: {id}"))
        }
        LibraryStoreError::PlaylistNotFound(id) => {
            ApiError::not_found(format!("Playlist not found: {id}"))
        }
        LibraryStoreError::PlaylistConflict(id) => {
            ApiError::conflict(format!("Playlist already exists: {id}"))
        }
    }
}

fn unix_epoch_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}
