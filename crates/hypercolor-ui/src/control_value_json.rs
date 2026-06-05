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

use hypercolor_types::canvas::{linear_to_srgb, srgb_to_linear};
use hypercolor_types::effect::{ControlDefinition, ControlType, ControlValue};

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
        return Some(ControlValue::Boolean(boolean));
    }
    if let Some(integer) = value.as_i64() {
        return Some(ControlValue::Integer(i32::try_from(integer).ok()?));
    }
    if let Some(float) = value.as_f64() {
        return Some(ControlValue::Float(parse_f32(float)?));
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
        return Some(ControlValue::Color(color));
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
        ControlValue::Float(number) => serde_json::json!(number),
        ControlValue::Integer(number) => serde_json::json!(number),
        ControlValue::Boolean(boolean) => serde_json::json!(boolean),
        ControlValue::Text(text) | ControlValue::Enum(text) => serde_json::json!(text),
        ControlValue::Color(rgba) => serde_json::json!(format!(
            "#{:02x}{:02x}{:02x}",
            color_channel_to_byte(rgba[0]),
            color_channel_to_byte(rgba[1]),
            color_channel_to_byte(rgba[2]),
        )),
        ControlValue::Gradient(stops) => serde_json::json!(stops),
        ControlValue::Rect(rect) => serde_json::json!({
            "x": rect.x,
            "y": rect.y,
            "width": rect.width,
            "height": rect.height,
        }),
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

/// Parse `#rrggbb` or `#rrggbbaa` into linear RGB plus normalized alpha.
#[must_use]
pub fn hex_to_rgba(hex: &str) -> Option<[f32; 4]> {
    let hex = hex.strip_prefix('#').unwrap_or(hex);
    if hex.len() != 6 && hex.len() != 8 {
        return None;
    }
    let parse_byte = |slice: &str| u8::from_str_radix(slice, 16).ok();
    let r = parse_byte(&hex[0..2])?;
    let g = parse_byte(&hex[2..4])?;
    let b = parse_byte(&hex[4..6])?;
    let a = if hex.len() == 8 {
        parse_byte(&hex[6..8])?
    } else {
        255
    };
    Some([
        srgb_to_linear(f32::from(r) / 255.0),
        srgb_to_linear(f32::from(g) / 255.0),
        srgb_to_linear(f32::from(b) / 255.0),
        f32::from(a) / 255.0,
    ])
}

/// Convert a hex color string into a linear-RGB RGBA JSON array payload.
#[must_use]
pub fn hex_to_rgba_json(hex: &str) -> Option<serde_json::Value> {
    let [r, g, b, a] = hex_to_rgba(hex)?;
    Some(serde_json::json!([r, g, b, a]))
}

/// Convert a hex color string into a [`ControlValue::Color`].
#[must_use]
pub fn hex_to_control_value(hex: &str) -> Option<ControlValue> {
    Some(ControlValue::Color(hex_to_rgba(hex)?))
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::as_conversions
)]
fn color_channel_to_byte(channel: f32) -> u8 {
    (linear_to_srgb(channel.clamp(0.0, 1.0)) * 255.0).round() as u8
}
