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
//! [`STORAGE_KEY`] on every mutation. Invalid persisted values block
//! writes and surface their effect and control identifiers so a later
//! save can never erase data that failed to load.

use std::collections::HashMap;

use hypercolor_types::control::ControlValue;
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
    load_error: RwSignal<Option<String>>,
}

impl PreferencesStore {
    /// Creates a new store seeded from `localStorage`.
    ///
    /// Missing state starts empty. Malformed state remains untouched and
    /// blocks later writes so a valid subset never replaces data that failed
    /// canonical admission.
    pub fn new() -> Self {
        let (initial, load_error) = match load_from_storage() {
            Ok(initial) => (initial, None),
            Err(error) => {
                log::error!("refusing malformed effect preferences: {error}");
                crate::toasts::toast_error(&format!(
                    "Saved effect preferences are invalid at {error}. Storage was left untouched."
                ));
                (HashMap::new(), Some(error))
            }
        };
        Self {
            entries: RwSignal::new(initial),
            load_error: RwSignal::new(load_error),
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
    ///
    /// # Errors
    ///
    /// Returns the original load failure while malformed persisted data is
    /// present, or the serialization/storage error for this write.
    pub fn save(&self, effect_id: String, prefs: EffectPreferences) -> Result<(), String> {
        if let Some(error) = self.load_error.get_untracked() {
            return Err(format!("persisted preferences are invalid at {error}"));
        }

        let mut next = self.entries.get_untracked();
        next.insert(effect_id, prefs);
        let json = serde_json::to_string(&next)
            .map_err(|error| format!("preferences serialization failed: {error}"))?;
        storage::try_set(STORAGE_KEY, &json)
            .map_err(|error| format!("localStorage write failed: {error:?}"))?;
        self.entries.set(next);
        Ok(())
    }

    /// Return the persisted-data error that disabled this store.
    #[must_use]
    pub fn load_error(&self) -> Option<String> {
        self.load_error.get_untracked()
    }
}

impl Default for PreferencesStore {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEffectPreferences {
    #[serde(default)]
    preset_id: Option<String>,
    #[serde(default)]
    control_values: serde_json::Map<String, serde_json::Value>,
}

fn load_from_storage() -> Result<HashMap<String, EffectPreferences>, String> {
    let Some(raw) = storage::get(STORAGE_KEY) else {
        return Ok(HashMap::new());
    };
    decode_preferences(&raw)
}

fn decode_preferences(raw: &str) -> Result<HashMap<String, EffectPreferences>, String> {
    let document: serde_json::Value =
        serde_json::from_str(raw).map_err(|error| format!("preferences document: {error}"))?;
    let entries = document
        .as_object()
        .ok_or_else(|| "preferences document: expected an object".to_owned())?;

    entries
        .iter()
        .map(|(effect_id, value)| {
            let raw: RawEffectPreferences = serde_json::from_value(value.clone())
                .map_err(|error| format!("effect '{effect_id}': {error}"))?;
            let control_values = raw
                .control_values
                .into_iter()
                .map(|(control_id, value)| {
                    serde_json::from_value(value)
                        .map_err(|error| {
                            format!("effect '{effect_id}' control '{control_id}': {error}")
                        })
                        .map(|value| (control_id, value))
                })
                .collect::<Result<_, _>>()?;
            Ok((
                effect_id.clone(),
                EffectPreferences {
                    preset_id: raw.preset_id,
                    control_values,
                },
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use hypercolor_types::control::ControlValue;
    use leptos::prelude::Owner;

    use super::{EffectPreferences, PreferencesStore, STORAGE_KEY, decode_preferences};
    use crate::storage;

    #[test]
    fn persisted_preferences_name_the_invalid_effect_and_control() {
        let error = decode_preferences(
            r#"{
                "rain": {
                    "control_values": {
                        "speed": {"kind": "null", "value": null}
                    }
                }
            }"#,
        )
        .expect_err("invalid canonical values must refuse the document");

        assert!(error.contains("effect 'rain' control 'speed'"));
        assert!(error.contains("null must not contain a value"));
    }

    #[test]
    fn persisted_preferences_restore_valid_canonical_values() {
        let preferences = decode_preferences(
            r#"{
                "rain": {
                    "preset_id": "preset",
                    "control_values": {
                        "speed": {"kind": "float", "value": 0.75}
                    }
                }
            }"#,
        )
        .expect("valid preferences should load");

        assert_eq!(preferences["rain"].preset_id.as_deref(), Some("preset"));
        assert_eq!(
            preferences["rain"].control_values["speed"],
            ControlValue::Float(0.75)
        );
    }

    #[test]
    fn malformed_preferences_block_writes_without_erasing_storage() {
        let raw = r#"{
            "rain": {
                "control_values": {
                    "speed": {"kind": "null", "value": null}
                }
            }
        }"#;
        assert!(storage::set(STORAGE_KEY, raw));

        Owner::new().with(|| {
            let store = PreferencesStore::new();
            assert!(store.load_error().is_some());
            assert!(
                store
                    .save("rain".to_owned(), EffectPreferences::default())
                    .is_err()
            );
            assert_eq!(storage::get(STORAGE_KEY).as_deref(), Some(raw));
        });

        assert!(storage::remove(STORAGE_KEY));
    }
}
