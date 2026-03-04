//! Saved effect library types: favorites, presets, and playlists.
//!
//! This module defines the durable, serializable shape of user-curated effect
//! data. The daemon can back these types with in-memory storage today and a
//! database adapter (e.g. Turso/libsql) later without changing API contracts.

use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::effect::{ControlValue, EffectId};

// ── Strong IDs ─────────────────────────────────────────────────────────────

/// Opaque identifier for a saved effect preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PresetId(pub Uuid);

impl PresetId {
    /// Create a fresh UUID v7 preset identifier.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FavoriteEffect {
    /// Canonical effect identifier.
    pub effect_id: EffectId,
    /// Unix epoch milliseconds when this favorite was added.
    pub added_at_ms: u64,
}

// ── Presets ───────────────────────────────────────────────────────────────

/// A saved parameter snapshot for one effect.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EffectPreset {
    pub id: PresetId,
    pub name: String,
    pub description: Option<String>,
    pub effect_id: EffectId,
    #[serde(default)]
    pub controls: HashMap<String, ControlValue>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

// ── Playlists ─────────────────────────────────────────────────────────────

/// Target entity for one playlist slot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PlaylistItemTarget {
    /// Run an effect directly.
    Effect { effect_id: EffectId },
    /// Run a saved preset (effect + controls).
    Preset { preset_id: PresetId },
}

/// One item in a playlist sequence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlaylistItem {
    pub id: PlaylistItemId,
    pub target: PlaylistItemTarget,
    pub duration_ms: Option<u64>,
    pub transition_ms: Option<u64>,
}

/// A user-defined effect sequence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
