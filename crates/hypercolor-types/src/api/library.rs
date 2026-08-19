//! Library API contracts — `/api/v1/library/*`.

use serde::{Deserialize, Serialize};

use crate::api::common::Pagination;
use crate::library::{EffectPlaylist, EffectPreset};

/// Request body for `POST /api/v1/library/favorites`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddFavoriteRequest {
    /// Effect id to favorite.
    pub effect: String,
}

/// One favorited effect.
///
/// `effect_name` is resolved from the registry at request time and falls
/// back to the id when the effect is no longer installed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FavoriteSummary {
    pub effect_id: String,
    #[serde(default)]
    pub effect_name: String,
    #[serde(default)]
    pub added_at_ms: u64,
}

/// Response for `GET /api/v1/library/favorites`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FavoriteListResponse {
    #[serde(default)]
    pub items: Vec<FavoriteSummary>,
    #[serde(default)]
    pub pagination: Pagination,
}

/// Response for `POST /api/v1/library/favorites`.
///
/// `created` is false when the effect was already favorited, which
/// re-stamps `added_at_ms` rather than erroring.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddFavoriteResponse {
    pub favorite: FavoriteSummary,
    pub created: bool,
}

/// Response for `DELETE /api/v1/library/favorites/{effect}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteFavoriteResponse {
    pub effect_id: String,
    pub deleted: bool,
}

/// What one playlist item plays.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PlaylistTargetRequest {
    Effect { effect: String },
    Preset { preset_id: String },
}

/// One item in a saved playlist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaylistItemRequest {
    pub target: PlaylistTargetRequest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transition_ms: Option<u64>,
}

/// Request body for `POST /api/v1/library/playlists` and
/// `PUT /api/v1/library/playlists/{id}`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavePlaylistRequest {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub items: Option<Vec<PlaylistItemRequest>>,
}

/// Request body for `POST /api/v1/library/presets` and
/// `PUT /api/v1/library/presets/{id}`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SavePresetRequest {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Effect the preset's control values belong to.
    pub effect: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub controls: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
}

/// Response for `GET /api/v1/library/presets`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PresetListResponse {
    #[serde(default)]
    pub items: Vec<EffectPreset>,
    #[serde(default)]
    pub pagination: Pagination,
}

/// Response for `DELETE /api/v1/library/presets/{id}`.
///
/// `id` is the resolved preset id, which differs from the path segment
/// when the caller addressed the preset by name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeletePresetResponse {
    pub id: String,
    pub deleted: bool,
}

// The preset apply response is deliberately NOT defined here: its
// `applied_controls` map carries f32 control values, and the daemon builds
// the body with `serde_json::json!`, which widens f32 to f64 and prints the
// widened digits. Naming the shape would change the bytes on the wire.

/// Response for `GET /api/v1/library/playlists`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlaylistListResponse {
    #[serde(default)]
    pub items: Vec<EffectPlaylist>,
    #[serde(default)]
    pub pagination: Pagination,
}

/// Response for `DELETE /api/v1/library/playlists/{id}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeletePlaylistResponse {
    pub id: String,
    pub deleted: bool,
}

/// The playlist the daemon is currently cycling through.
///
/// This is the live runtime's view, not the stored playlist: the item
/// list is reduced to `item_count`, and `started_at_ms` is when playback
/// began rather than when the playlist was saved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivePlaylistResponse {
    pub id: String,
    pub name: String,
    pub loop_enabled: bool,
    pub item_count: usize,
    pub started_at_ms: u64,
}

/// Response for `POST /api/v1/library/playlists/{id}/activate`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivatePlaylistResponse {
    pub playlist: ActivePlaylistResponse,
    pub active: bool,
}

/// Response for `GET /api/v1/library/playlists/active`.
///
/// The route answers 404 when nothing is playing, so `state` is always
/// `"running"` on a success.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivePlaylistStateResponse {
    pub playlist: ActivePlaylistResponse,
    #[serde(default)]
    pub state: String,
}

/// Response for `POST /api/v1/library/playlists/stop`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StopPlaylistResponse {
    /// The playlist as it stood when playback stopped.
    pub playlist: ActivePlaylistResponse,
    pub stopped: bool,
}
