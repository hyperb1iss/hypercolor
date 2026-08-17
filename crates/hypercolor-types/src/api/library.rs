//! Library API contracts — `/api/v1/library/*`.

use serde::{Deserialize, Serialize};

/// Request body for `POST /api/v1/library/favorites`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddFavoriteRequest {
    /// Effect id to favorite.
    pub effect: String,
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

/// Optional body for `POST /api/v1/library/presets/{id}/apply` — scopes
/// the apply to one zone.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplyPresetRequest {
    /// Target zone id. Omitted targets the primary zone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zone_id: Option<String>,
}
