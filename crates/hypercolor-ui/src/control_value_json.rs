//! Conversion between raw JSON control payloads and the typed
//! [`ControlValue`] shape the UI keeps in state.
//!
//! `ControlPanel`'s `on_change` callback hands back a bare `serde_json`
//! scalar (a number, bool, string, or 4-element array). The effect-control
//! schema disambiguates the few cases a bare value cannot: a string is a
//! dropdown `Enum` or free `Text`, a hex string is a `Color`. Effects,
//! display faces, and the Studio layer inspector funnel control edits
//! through here so the conversion stays in one place.
//!
//! The reverse path also lives here: typed control values become the
//! daemon API payloads used by presets, app state, and live patches.

use std::collections::HashMap;

use hypercolor_types::canvas::LinearRgba;
use hypercolor_types::control::ControlValue;
use hypercolor_types::effect::ControlDefinition;

/// Convert a raw control-panel JSON value into a typed [`ControlValue`],
/// using the effect's control schema to disambiguate string and color
/// inputs. Returns `None` for a value that matches no known control shape.
#[must_use]
pub fn json_to_control_value(
    control_name: &str,
    controls: &[ControlDefinition],
    value: &serde_json::Value,
) -> Option<ControlValue> {
    let definition = controls
        .iter()
        .find(|definition| definition.control_id().eq_ignore_ascii_case(control_name));
    definition.map_or_else(
        || ControlValue::try_from_effect_json(value).ok(),
        |definition| definition.admit_effect_json(value).ok(),
    )
}

/// Fold one raw control edit into a control-value map, returning the
/// updated map. A value that converts to no known shape is dropped.
#[must_use]
pub fn apply_control_edit(
    mut values: HashMap<String, ControlValue>,
    control_name: &str,
    controls: &[ControlDefinition],
    raw: &serde_json::Value,
) -> HashMap<String, ControlValue> {
    if let Some(typed) = json_to_control_value(control_name, controls, raw) {
        values.insert(control_name.to_owned(), typed);
    }
    values
}

/// Convert typed control values into the JSON payload shape the daemon API
/// expects for live control patches and saved presets.
#[must_use]
pub fn controls_to_json(
    values: &HashMap<String, ControlValue>,
) -> serde_json::Map<String, serde_json::Value> {
    values
        .iter()
        .map(|(key, value)| (key.clone(), control_value_to_json(value)))
        .collect()
}

/// Convert a typed control value into its API JSON representation.
#[must_use]
pub fn control_value_to_json(value: &ControlValue) -> serde_json::Value {
    value
        .try_to_effect_json()
        .expect("UI effect state contains renderer-compatible control values")
}

/// Parse a hex color into linear RGB plus normalized alpha.
///
/// The kernel's grammar accepts the CSS shorthand forms as well, so
/// `#f80` and `#f80c` now parse where they used to be rejected. Callers
/// keep deciding what a failed parse means; this returns `None`.
#[must_use]
pub fn hex_to_rgba(hex: &str) -> Option<[f32; 4]> {
    let color = LinearRgba::from_hex_srgb(hex).ok()?;
    Some([color.r, color.g, color.b, color.a])
}

/// Convert a hex color string into a linear-RGB RGBA JSON array payload.
#[must_use]
pub fn hex_to_rgba_json(hex: &str) -> Option<serde_json::Value> {
    let [r, g, b, a] = hex_to_rgba(hex)?;
    Some(serde_json::json!([r, g, b, a]))
}

/// Convert a hex color string into a [`ControlValue::ColorLinear`].
#[must_use]
pub fn hex_to_control_value(hex: &str) -> Option<ControlValue> {
    Some(ControlValue::linear_color(hex_to_rgba(hex)?))
}
