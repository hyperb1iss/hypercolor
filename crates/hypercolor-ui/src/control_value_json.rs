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

use hypercolor_types::canvas::{LinearRgba, linear_to_srgb};
use hypercolor_types::control::ControlValue;
use hypercolor_types::effect::{ControlDefinition, ControlType};

/// Convert a raw control-panel JSON value into a typed [`ControlValue`],
/// using the effect's control schema to disambiguate string and color
/// inputs. Returns `None` for a value that matches no known control shape.
#[must_use]
pub fn json_to_control_value(
    control_name: &str,
    controls: &[ControlDefinition],
    value: &serde_json::Value,
) -> Option<ControlValue> {
    if let Some(boolean) = value.as_bool() {
        return Some(ControlValue::Bool(boolean));
    }
    if let Some(integer) = value.as_i64() {
        return Some(ControlValue::Int(integer));
    }
    if let Some(float) = value.as_f64() {
        parse_f32(float)?;
        return Some(ControlValue::Float(float));
    }
    if let Some(text) = value.as_str() {
        let (is_dropdown, is_color_picker) = controls
            .iter()
            .find(|def| def.control_id().eq_ignore_ascii_case(control_name))
            .map(|def| {
                (
                    matches!(def.control_type, ControlType::Dropdown),
                    matches!(def.control_type, ControlType::ColorPicker),
                )
            })
            .unwrap_or((false, false));
        if is_dropdown {
            return Some(ControlValue::Enum(text.to_owned()));
        }
        if is_color_picker && let Some(color) = hex_to_control_value(text) {
            return Some(color);
        }
        return Some(ControlValue::Text(text.to_owned()));
    }
    if let Some(array) = value.as_array()
        && array.len() == 4
    {
        let mut color = [0.0_f32; 4];
        for (idx, component) in array.iter().enumerate() {
            color[idx] = parse_f32(component.as_f64()?)?;
        }
        return Some(ControlValue::linear_color(color));
    }
    None
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
    match value {
        ControlValue::Null | ControlValue::Unknown => serde_json::Value::Null,
        ControlValue::Float(number) => serde_json::json!(number),
        ControlValue::Int(number) => serde_json::json!(number),
        ControlValue::Bool(boolean) => serde_json::json!(boolean),
        ControlValue::Text(text) | ControlValue::Enum(text) => serde_json::json!(text),
        ControlValue::SecretRef(reference) => serde_json::json!(reference.as_str()),
        ControlValue::Ip(value) => serde_json::json!(value.as_str()),
        ControlValue::Mac(value) => serde_json::json!(value.as_str()),
        ControlValue::Duration(value) => serde_json::json!(value.as_millis()),
        ControlValue::ColorRgb(color) => serde_json::json!(color.to_hex()),
        ControlValue::ColorRgba(color) => serde_json::json!(format!(
            "#{:02x}{:02x}{:02x}{:02x}",
            color.r, color.g, color.b, color.a
        )),
        ControlValue::ColorLinear(rgba) => serde_json::json!(format!(
            "#{:02x}{:02x}{:02x}",
            color_channel_to_byte(rgba.r),
            color_channel_to_byte(rgba.g),
            color_channel_to_byte(rgba.b),
        )),
        ControlValue::Gradient(stops) => serde_json::json!(stops),
        ControlValue::Rect(rect) => serde_json::json!({
            "x": rect.x,
            "y": rect.y,
            "width": rect.width,
            "height": rect.height,
        }),
        ControlValue::Flags(values) => serde_json::json!(values),
        ControlValue::List(values) => {
            serde_json::Value::Array(values.iter().map(control_value_to_json).collect())
        }
        ControlValue::Map(values) => serde_json::Value::Object(
            values
                .iter()
                .map(|(key, value)| (key.clone(), control_value_to_json(value)))
                .collect(),
        ),
    }
}

/// Narrow an `f64` to a finite `f32`, rejecting non-finite or out-of-range
/// inputs.
#[must_use]
pub fn parse_f32(value: f64) -> Option<f32> {
    if !value.is_finite() || value < f64::from(f32::MIN) || value > f64::from(f32::MAX) {
        return None;
    }
    #[allow(clippy::cast_possible_truncation)]
    Some(value as f32)
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

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::as_conversions
)]
fn color_channel_to_byte(channel: f32) -> u8 {
    (linear_to_srgb(channel.clamp(0.0, 1.0)) * 255.0).round() as u8
}
