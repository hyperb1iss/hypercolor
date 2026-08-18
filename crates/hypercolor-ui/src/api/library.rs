//! Library API — presets and favorites.

use super::client;

// Wire contracts are shared with the daemon
// (hypercolor-types::api::library) — drift is a compile error rather
// than a runtime parse failure. `EffectPreset` is the daemon's stored
// preset record, returned verbatim by the preset routes.
pub use hypercolor_types::api::library::{
    AddFavoriteRequest, FavoriteListResponse, FavoriteSummary, PresetListResponse,
    SavePresetRequest,
};
pub use hypercolor_types::library::EffectPreset;

// ── Preset Functions ────────────────────────────────────────────────────────

/// Fetch all saved presets.
pub async fn fetch_presets() -> Result<Vec<EffectPreset>, String> {
    let list: PresetListResponse = client::fetch_json("/api/v1/library/presets").await?;
    Ok(list.items)
}

/// Create a new preset from current control values.
pub async fn create_preset(req: &SavePresetRequest) -> Result<EffectPreset, String> {
    client::post_json("/api/v1/library/presets", req)
        .await
        .map_err(Into::into)
}

/// Update an existing preset (name, controls, etc.).
pub async fn update_preset(id: &str, req: &SavePresetRequest) -> Result<EffectPreset, String> {
    client::put_json(&format!("/api/v1/library/presets/{id}"), req)
        .await
        .map_err(Into::into)
}

/// Delete a preset by ID.
pub async fn delete_preset(id: &str) -> Result<(), String> {
    client::delete_empty(&format!("/api/v1/library/presets/{id}"))
        .await
        .map_err(Into::into)
}

// ── Favorite Functions ──────────────────────────────────────────────────────

/// Fetch all favorited effect IDs.
pub async fn fetch_favorites() -> Result<Vec<FavoriteSummary>, String> {
    let list: FavoriteListResponse = client::fetch_json("/api/v1/library/favorites").await?;
    Ok(list.items)
}

/// Add an effect to favorites.
pub async fn add_favorite(effect_id: &str) -> Result<(), String> {
    client::post_json_discard(
        "/api/v1/library/favorites",
        &AddFavoriteRequest {
            effect: effect_id.to_owned(),
        },
    )
    .await
    .map_err(Into::into)
}

/// Remove an effect from favorites.
pub async fn remove_favorite(effect_id: &str) -> Result<(), String> {
    client::delete_empty(&format!("/api/v1/library/favorites/{effect_id}"))
        .await
        .map_err(Into::into)
}
