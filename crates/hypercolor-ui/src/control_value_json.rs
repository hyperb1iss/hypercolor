//! Conversion between the raw JSON a `ControlPanel` emits and the typed
//! [`ControlValue`] the daemon stores.
//!
//! `ControlPanel`'s `on_change` callback hands back a bare `serde_json`
//! scalar (a number, bool, string, or 4-element array). The effect-control
//! schema disambiguates the few cases a bare value cannot — a string is a
//! dropdown `Enum` or free `Text`, a hex string is a `Color`. Both the
//! Effects page and the Studio layer inspector funnel control edits
//! through here so the conversion stays in one place.

use std::collections::HashMap;

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
        if is_color_picker && let Some(color) = hex_to_color_value(text) {
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

/// Parse a `#rrggbb` (or `rrggbb`) hex string into 8-bit RGB components.
#[must_use]
pub fn parse_hex_rgb(hex: &str) -> Option<(u8, u8, u8)> {
    let hex = hex.strip_prefix('#').unwrap_or(hex);
    if hex.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some((r, g, b))
}

/// Convert a hex color string into a linear-RGBA [`ControlValue::Color`].
#[must_use]
pub fn hex_to_color_value(hex: &str) -> Option<ControlValue> {
    let (r, g, b) = parse_hex_rgb(hex)?;
    Some(ControlValue::Color([
        f32::from(r) / 255.0,
        f32::from(g) / 255.0,
        f32::from(b) / 255.0,
        1.0,
    ]))
}
