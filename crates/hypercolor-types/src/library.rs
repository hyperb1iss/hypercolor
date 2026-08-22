//! Saved effect library types: favorites, presets, and playlists.
//!
//! This module defines the durable, serializable shape of user-curated effect
//! data. The daemon can back these types with in-memory storage today and a
//! database adapter (e.g. Turso/libsql) later without changing API contracts.

use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::effect::{ControlValue, EffectId};

// ── Strong IDs ─────────────────────────────────────────────────────────────

/// Opaque identifier for an effect preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
pub struct PresetId(pub Uuid);

impl PresetId {
    /// Create a fresh UUID v7 preset identifier.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    /// Derive a stable identifier for an effect-authored preset.
    #[must_use]
    pub fn stable(key: &str) -> Self {
        let key = Self::normalize_key(key);
        let mut hash: u128 = 0x52a2_4f6d_0959_4929_82b3_c28a_d44a_a910;
        for byte in b"hypercolor:preset:".iter().chain(key.as_bytes()) {
            hash ^= u128::from(*byte);
            hash = hash.wrapping_mul(0x1000_0000_01b3);
        }

        let mut bytes = hash.to_be_bytes();
        bytes[6] = (bytes[6] & 0x0f) | 0x80;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        Self(Uuid::from_bytes(bytes))
    }

    /// Normalize an authored preset key before identity derivation.
    #[must_use]
    pub fn normalize_key(key: &str) -> String {
        key.split(|character: char| {
            character.is_whitespace() || matches!(character, '\u{1c}'..='\u{1f}' | '\u{feff}')
        })
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
    }
}

impl Default for PresetId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for PresetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for PresetId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s).map(Self)
    }
}

/// Opaque identifier for a playlist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
pub struct PlaylistId(pub Uuid);

impl PlaylistId {
    /// Create a fresh UUID v7 playlist identifier.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Default for PlaylistId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for PlaylistId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for PlaylistId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s).map(Self)
    }
}

/// Opaque identifier for a playlist item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
pub struct PlaylistItemId(pub Uuid);

impl PlaylistItemId {
    /// Create a fresh UUID v7 playlist item identifier.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Default for PlaylistItemId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for PlaylistItemId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for PlaylistItemId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s).map(Self)
    }
}

// ── Favorites ─────────────────────────────────────────────────────────────

/// A single favorited effect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct FavoriteEffect {
    /// Canonical effect identifier.
    pub effect_id: EffectId,
    /// Unix epoch milliseconds when this favorite was added.
    pub added_at_ms: u64,
}

// ── Presets ───────────────────────────────────────────────────────────────

/// A saved parameter snapshot for one effect.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct EffectPreset {
    pub id: PresetId,
    pub name: String,
    pub description: Option<String>,
    pub effect_id: EffectId,
    #[serde(default)]
    pub controls: HashMap<String, ControlValue>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub created_at_ms: u64,
    #[serde(default)]
    pub updated_at_ms: u64,
}

// ── Playlists ─────────────────────────────────────────────────────────────

/// Target entity for one playlist slot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PlaylistItemTarget {
    /// Run an effect directly.
    Effect { effect_id: EffectId },
    /// Run a saved preset (effect + controls).
    Preset { preset_id: PresetId },
}

/// One item in a playlist sequence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct PlaylistItem {
    pub id: PlaylistItemId,
    pub target: PlaylistItemTarget,
    pub duration_ms: Option<u64>,
    pub transition_ms: Option<u64>,
}

/// A user-defined effect sequence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct EffectPlaylist {
    pub id: PlaylistId,
    pub name: String,
    pub description: Option<String>,
    #[serde(default)]
    pub items: Vec<PlaylistItem>,
    pub loop_enabled: bool,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}
