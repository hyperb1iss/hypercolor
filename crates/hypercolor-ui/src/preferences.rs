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

use hypercolor_color::LinearRgba;
use hypercolor_types::control::ControlValue;
use hypercolor_types::effect::GradientStop;
use hypercolor_types::spatial::NormalizedRect;
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
    /// Missing state starts empty. Preferences from before the canonical
    /// control wire are migrated in place. Malformed state remains untouched
    /// and blocks later writes so a valid subset never replaces data that
    /// failed canonical admission.
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

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum LegacyEffectControlValue {
    Float(f32),
    Integer(i32),
    Boolean(bool),
    Color([f32; 4]),
    Gradient(Vec<LegacyGradientStop>),
    Enum(String),
    Text(String),
    Rect(LegacyNormalizedRect),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyGradientStop {
    position: f32,
    color: [f32; 4],
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyNormalizedRect {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

impl From<LegacyEffectControlValue> for ControlValue {
    fn from(value: LegacyEffectControlValue) -> Self {
        match value {
            LegacyEffectControlValue::Float(value) => Self::Float(f64::from(value)),
            LegacyEffectControlValue::Integer(value) => Self::Int(i64::from(value)),
            LegacyEffectControlValue::Boolean(value) => Self::Bool(value),
            LegacyEffectControlValue::Color([r, g, b, a]) => {
                Self::ColorLinear(LinearRgba::new(r, g, b, a))
            }
            LegacyEffectControlValue::Gradient(value) => Self::Gradient(
                value
                    .into_iter()
                    .map(|stop| GradientStop {
                        position: stop.position,
                        color: stop.color,
                    })
                    .collect(),
            ),
            LegacyEffectControlValue::Enum(value) => Self::Enum(value),
            LegacyEffectControlValue::Text(value) => Self::Text(value),
            LegacyEffectControlValue::Rect(value) => Self::Rect(NormalizedRect {
                x: value.x,
                y: value.y,
                width: value.width,
                height: value.height,
            }),
        }
    }
}

struct DecodedPreferences {
    entries: HashMap<String, EffectPreferences>,
    migrated: bool,
}

fn load_from_storage() -> Result<HashMap<String, EffectPreferences>, String> {
    let Some(raw) = storage::get(STORAGE_KEY) else {
        return Ok(HashMap::new());
    };
    let decoded = decode_preferences_document(&raw)?;
    if decoded.migrated {
        let json = serde_json::to_string(&decoded.entries)
            .map_err(|error| format!("preferences migration serialization failed: {error}"))?;
        storage::try_set(STORAGE_KEY, &json)
            .map_err(|error| format!("preferences migration write failed: {error:?}"))?;
    }
    Ok(decoded.entries)
}

#[cfg(test)]
fn decode_preferences(raw: &str) -> Result<HashMap<String, EffectPreferences>, String> {
    decode_preferences_document(raw).map(|decoded| decoded.entries)
}

fn decode_preferences_document(raw: &str) -> Result<DecodedPreferences, String> {
    let document: serde_json::Value =
        serde_json::from_str(raw).map_err(|error| format!("preferences document: {error}"))?;
    let entries = document
        .as_object()
        .ok_or_else(|| "preferences document: expected an object".to_owned())?;

    let mut decoded_entries = HashMap::with_capacity(entries.len());
    let mut migrated = false;
    for (effect_id, value) in entries {
        let raw: RawEffectPreferences = serde_json::from_value(value.clone())
            .map_err(|error| format!("effect '{effect_id}': {error}"))?;
        let mut control_values = HashMap::with_capacity(raw.control_values.len());
        for (control_id, value) in raw.control_values {
            let (value, value_migrated) = decode_control_value(value)
                .map_err(|error| format!("effect '{effect_id}' control '{control_id}': {error}"))?;
            migrated |= value_migrated;
            control_values.insert(control_id, value);
        }
        decoded_entries.insert(
            effect_id.clone(),
            EffectPreferences {
                preset_id: raw.preset_id,
                control_values,
            },
        );
    }

    Ok(DecodedPreferences {
        entries: decoded_entries,
        migrated,
    })
}

fn decode_control_value(value: serde_json::Value) -> Result<(ControlValue, bool), String> {
    match serde_json::from_value(value.clone()) {
        Ok(value) => Ok((value, false)),
        Err(canonical_error) => {
            if !is_legacy_effect_value_candidate(&value) {
                return Err(canonical_error.to_string());
            }
            let legacy = serde_json::from_value::<LegacyEffectControlValue>(value)
                .map_err(|error| format!("invalid legacy value: {error}"))?;
            let value = ControlValue::from(legacy);
            value
                .validate()
                .map_err(|error| format!("invalid legacy value: {error}"))?;
            Ok((value, true))
        }
    }
}

fn is_legacy_effect_value_candidate(value: &serde_json::Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    object.len() == 1
        && object.keys().any(|key| {
            matches!(
                key.as_str(),
                "float" | "integer" | "boolean" | "color" | "gradient" | "enum" | "text" | "rect"
            )
        })
}

#[cfg(test)]
mod tests {
    use hypercolor_color::LinearRgba;
    use hypercolor_types::control::ControlValue;
    use hypercolor_types::effect::GradientStop;
    use hypercolor_types::spatial::NormalizedRect;
    use leptos::prelude::Owner;
    use serde_json::json;

    use super::{
        EffectPreferences, PreferencesStore, STORAGE_KEY, decode_control_value, decode_preferences,
    };
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
    fn persisted_preferences_restore_every_legacy_effect_value() {
        let preferences = decode_preferences(
            r#"{
                "rain": {
                    "control_values": {
                        "speed": {"float": 0.75},
                        "count": {"integer": 7},
                        "enabled": {"boolean": true},
                        "bgColor": {"color": [0.1, 0.2, 0.3, 1.0]},
                        "gradient": {"gradient": [
                            {"position": 0.0, "color": [0.0, 0.0, 0.0, 1.0]},
                            {"position": 1.0, "color": [1.0, 1.0, 1.0, 1.0]}
                        ]},
                        "mode": {"enum": "soft"},
                        "label": {"text": "Rain"},
                        "region": {"rect": {
                            "x": 0.1, "y": 0.2, "width": 0.3, "height": 0.4
                        }}
                    }
                }
            }"#,
        )
        .expect("legacy preferences should migrate");

        let values = &preferences["rain"].control_values;
        assert_eq!(values["speed"], ControlValue::Float(0.75));
        assert_eq!(values["count"], ControlValue::Int(7));
        assert_eq!(values["enabled"], ControlValue::Bool(true));
        assert_eq!(
            values["bgColor"],
            ControlValue::ColorLinear(LinearRgba::new(0.1, 0.2, 0.3, 1.0))
        );
        assert_eq!(
            values["gradient"],
            ControlValue::Gradient(vec![
                GradientStop {
                    position: 0.0,
                    color: [0.0, 0.0, 0.0, 1.0],
                },
                GradientStop {
                    position: 1.0,
                    color: [1.0, 1.0, 1.0, 1.0],
                },
            ])
        );
        assert_eq!(values["mode"], ControlValue::Enum("soft".to_owned()));
        assert_eq!(values["label"], ControlValue::Text("Rain".to_owned()));
        assert_eq!(
            values["region"],
            ControlValue::Rect(NormalizedRect {
                x: 0.1,
                y: 0.2,
                width: 0.3,
                height: 0.4,
            })
        );
    }

    #[test]
    fn legacy_preferences_rewrite_storage_to_the_canonical_wire() {
        let raw = r#"{
            "0351e268-515b-4859-91a6-1fe70c1a0089": {
                "control_values": {
                    "bgColor": {"color": [0.1, 0.2, 0.3, 1.0]}
                }
            }
        }"#;
        assert!(storage::set(STORAGE_KEY, raw));

        Owner::new().with(|| {
            let store = PreferencesStore::new();
            assert!(store.load_error().is_none());
            assert!(store.get("0351e268-515b-4859-91a6-1fe70c1a0089").is_some());

            let rewritten = storage::get(STORAGE_KEY).expect("preferences should remain stored");
            let rewritten: serde_json::Value =
                serde_json::from_str(&rewritten).expect("rewritten preferences should be JSON");
            assert_eq!(
                rewritten["0351e268-515b-4859-91a6-1fe70c1a0089"]["control_values"]["bgColor"],
                json!({
                    "kind": "color_linear",
                    "value": {"r": 0.1, "g": 0.2, "b": 0.3, "a": 1.0}
                })
            );
        });

        assert!(storage::remove(STORAGE_KEY));
    }

    #[test]
    fn unknown_nested_legacy_fields_block_writes_without_erasing_storage() {
        let raw = r#"{
            "rain": {
                "control_values": {
                    "speed": {"kind": "float", "value": 0.75},
                    "region": {"rect": {
                        "x": 0.1,
                        "y": 0.2,
                        "width": 0.3,
                        "height": 0.4,
                        "future": 42
                    }}
                }
            }
        }"#;
        assert!(storage::set(STORAGE_KEY, raw));

        Owner::new().with(|| {
            let store = PreferencesStore::new();
            let error = store
                .load_error()
                .expect("unknown nested fields must refuse migration");
            assert!(error.contains("effect 'rain' control 'region'"));
            assert!(error.contains("unknown field `future`"));
            assert!(
                store
                    .save("rain".to_owned(), EffectPreferences::default())
                    .is_err()
            );
            assert_eq!(storage::get(STORAGE_KEY).as_deref(), Some(raw));
        });

        assert!(storage::remove(STORAGE_KEY));
    }

    #[test]
    fn legacy_nested_values_reject_unknown_fields() {
        for (value, field) in [
            (
                json!({"rect": {
                    "x": 0.1,
                    "y": 0.2,
                    "width": 0.3,
                    "height": 0.4,
                    "future_rect": 42
                }}),
                "future_rect",
            ),
            (
                json!({"gradient": [
                    {
                        "position": 0.0,
                        "color": [0.0, 0.0, 0.0, 1.0],
                        "future_stop": 42
                    },
                    {"position": 1.0, "color": [1.0, 1.0, 1.0, 1.0]}
                ]}),
                "future_stop",
            ),
        ] {
            let error = decode_control_value(value)
                .expect_err("unknown nested fields must refuse legacy values");
            assert!(error.contains(field));
        }
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
