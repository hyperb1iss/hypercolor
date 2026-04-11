//! Per-effect preference persistence.
//!
//! Switching effects feels broken if the daemon resets every control
//! value and discards the preset you picked. This store lives in the
//! browser and remembers the last preset + control-value snapshot for
//! every effect the user has customised, keyed by effect ID, so the
//! restore path in `app.rs` can re-apply the saved state on top of the
//! daemon's defaults whenever the user comes back to an effect.
//!
//! The store is provided as a Leptos context in [`crate::app`] and
//! persisted to `localStorage` as a single JSON blob under
//! [`STORAGE_KEY`] on every mutation. We pay a whole-map serialize on
//! each write to keep the moving parts minimal — presets are rarely
//! flipped and control values are already debounced in
//! `effects::flush_control_updates`, so the write rate is not a concern.

use std::collections::HashMap;

use hypercolor_types::effect::ControlValue;
use leptos::prelude::*;
use serde::{Deserialize, Serialize};

use crate::storage;

const STORAGE_KEY: &str = "hc-effect-preferences";

/// Remembered state for a single effect. Written whenever the user
/// changes a preset or tweaks a control, read when the effect becomes
/// active again so we can re-apply the saved state on top of whatever
/// fresh defaults the daemon loaded.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct EffectPreferences {
    #[serde(default)]
    pub preset_id: Option<String>,
    #[serde(default)]
    pub control_values: HashMap<String, ControlValue>,
}

/// Reactive per-effect preferences store keyed by effect ID.
///
/// `Copy` so it can be cheaply captured into closures and passed as a
/// Leptos context — the inner `RwSignal` is the only real state.
#[derive(Clone, Copy)]
pub struct PreferencesStore {
    entries: RwSignal<HashMap<String, EffectPreferences>>,
}

impl PreferencesStore {
    /// Creates a new store seeded from `localStorage`. Corrupt or missing
    /// data is silently treated as "no prior preferences" — we'd rather
    /// the UI start clean than crash on malformed state from an older
    /// build.
    pub fn new() -> Self {
        let initial = load_from_storage().unwrap_or_default();
        Self {
            entries: RwSignal::new(initial),
        }
    }

    /// Untracked lookup — the restore path reads prefs inside spawned
    /// tasks where reactive subscription would be meaningless and
    /// potentially dangerous.
    pub fn get(&self, effect_id: &str) -> Option<EffectPreferences> {
        self.entries
            .with_untracked(|map| map.get(effect_id).cloned())
    }

    /// Overwrite the stored preferences for an effect. Used by the
    /// snapshot save path after the daemon confirms either a preset
    /// apply or a control-value change.
    pub fn save(&self, effect_id: String, prefs: EffectPreferences) {
        self.entries.update(|map| {
            map.insert(effect_id, prefs);
        });
        self.persist();
    }

    fn persist(&self) {
        let json = self
            .entries
            .with_untracked(|map| serde_json::to_string(map).ok());
        if let Some(json) = json {
            storage::set(STORAGE_KEY, &json);
        }
    }
}

impl Default for PreferencesStore {
    fn default() -> Self {
        Self::new()
    }
}

fn load_from_storage() -> Option<HashMap<String, EffectPreferences>> {
    let raw = storage::get(STORAGE_KEY)?;
    serde_json::from_str(&raw).ok()
}
