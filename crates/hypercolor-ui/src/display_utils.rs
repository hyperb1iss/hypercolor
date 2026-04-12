//! Pure helpers used by the displays page that are worth unit-testing in
//! isolation. Keeping them out of `pages/displays.rs` avoids dragging the
//! full Leptos component tree into test builds.

use hypercolor_types::effect::{ControlDefinition, ControlType, ControlValue};

use crate::api;

/// Returns `true` when a display summary came from the virtual simulator
/// backend (distinguished by its `family` field). Used by the displays
/// page to show the "Simulator" badge and to gate the edit/delete UI.
#[must_use]
pub fn is_simulator_display(display: &api::DisplaySummary) -> bool {
    display.family.eq_ignore_ascii_case("simulator")
}

/// Parse a user-supplied simulator dimension string into a positive u32.
///
/// Trims whitespace, rejects zero and non-numeric input, and formats a
/// friendly error message citing the field label so validation feedback
/// can flow directly into the modal form.
pub fn parse_simulator_dimension(raw: &str, label: &str) -> Result<u32, String> {
    raw.trim()
        .parse::<u32>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| format!("{label} must be a positive number."))
}

/// Build the URL of the full-screen preview shell for a display. Opened
/// in a new tab via "Open preview" so users can cast the live face to a
/// secondary monitor or project it alongside the control column.
#[must_use]
pub fn display_preview_shell_url(display_id: &str) -> String {
    format!("/preview?display={display_id}")
}

/// JSON → `ControlValue` conversion used for optimistic local state when
/// a face control changes. Awareness of the control definition list lets
/// string inputs pick the right variant: `Enum` for dropdowns, `Color`
/// for hex-coded color pickers, `Text` otherwise.
#[must_use]
pub fn json_to_face_control_value(
    controls: &[ControlDefinition],
    name: &str,
    value: &serde_json::Value,
) -> Option<ControlValue> {
    if let Some(v) = value.as_bool() {
        return Some(ControlValue::Boolean(v));
    }
    if let Some(v) = value.as_i64() {
        return i32::try_from(v).ok().map(ControlValue::Integer);
    }
    if let Some(v) = value.as_f64() {
        return json_f32(v).map(ControlValue::Float);
    }
    if let Some(v) = value.as_str() {
        let def = controls
            .iter()
            .find(|def| def.control_id().eq_ignore_ascii_case(name));
        let (is_dropdown, is_color) = def
            .map(|d| {
                (
                    matches!(d.control_type, ControlType::Dropdown),
                    matches!(d.control_type, ControlType::ColorPicker),
                )
            })
            .unwrap_or((false, false));
        if is_dropdown {
            return Some(ControlValue::Enum(v.to_owned()));
        }
        if is_color && let Some(color) = hex_to_rgba(v) {
            return Some(ControlValue::Color(color));
        }
        return Some(ControlValue::Text(v.to_owned()));
    }
    if let Some(array) = value.as_array()
        && array.len() == 4
    {
        let mut color = [0.0f32; 4];
        for (idx, component) in array.iter().enumerate() {
            color[idx] = json_f32(component.as_f64()?)?;
        }
        return Some(ControlValue::Color(color));
    }
    None
}

/// Convert a clamped, finite `f64` into `f32` — returns `None` on
/// infinities, NaN, or values outside the `f32` range. Used by the JSON
/// conversion helpers so malformed numeric input doesn't corrupt the
/// optimistic control state.
#[expect(
    clippy::cast_possible_truncation,
    clippy::as_conversions,
    reason = "f64 → f32 coercion bounded by prior is_finite + range guards"
)]
#[must_use]
pub fn json_f32(value: f64) -> Option<f32> {
    if !value.is_finite() || value < f64::from(f32::MIN) || value > f64::from(f32::MAX) {
        return None;
    }
    Some(value as f32)
}

/// Parse `#RRGGBB` or `#RRGGBBAA` hex strings into a normalized `[0.0, 1.0]`
/// RGBA array. Leading `#` is optional. Returns `None` when the input is
/// not a 6- or 8-character hex string.
#[must_use]
pub fn hex_to_rgba(hex: &str) -> Option<[f32; 4]> {
    let trimmed = hex.trim_start_matches('#');
    if trimmed.len() != 6 && trimmed.len() != 8 {
        return None;
    }
    let parse_byte = |slice: &str| u8::from_str_radix(slice, 16).ok();
    let r = parse_byte(&trimmed[0..2])?;
    let g = parse_byte(&trimmed[2..4])?;
    let b = parse_byte(&trimmed[4..6])?;
    let a = if trimmed.len() == 8 {
        parse_byte(&trimmed[6..8])?
    } else {
        255
    };
    Some([
        f32::from(r) / 255.0,
        f32::from(g) / 255.0,
        f32::from(b) / 255.0,
        f32::from(a) / 255.0,
    ])
}
