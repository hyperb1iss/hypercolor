//! JSON -> [`ControlValue`](hypercolor_types::effect::ControlValue) helpers.

use hypercolor_types::effect::ControlValue;
use hypercolor_types::viewport::ViewportRect;

/// Convert arbitrary JSON into the strongly typed control value model.
#[must_use]
pub fn json_to_control_value(value: &serde_json::Value) -> Option<ControlValue> {
    if let Some(v) = value.as_i64() {
        let int = i32::try_from(v).ok()?;
        return Some(ControlValue::Integer(int));
    }
    if let Some(v) = value.as_f64() {
        let float = parse_f32(v)?;
        return Some(ControlValue::Float(float));
    }
    if let Some(v) = value.as_bool() {
        return Some(ControlValue::Boolean(v));
    }
    if let Some(v) = value.as_str() {
        return Some(ControlValue::Text(v.to_owned()));
    }
    if let Some(array) = value.as_array()
        && array.len() == 4
    {
        let mut color = [0.0f32; 4];
        for (idx, component) in array.iter().enumerate() {
            let parsed = component.as_f64()?;
            color[idx] = parse_f32(parsed)?;
        }
        return Some(ControlValue::Color(color));
    }
    if let Some(object) = value.as_object() {
        let x = parse_json_f32(object.get("x")?)?;
        let y = parse_json_f32(object.get("y")?)?;
        let width = parse_json_f32(object.get("width")?)?;
        let height = parse_json_f32(object.get("height")?)?;
        return Some(ControlValue::Rect(
            ViewportRect::new(x, y, width, height).clamp(),
        ));
    }
    None
}

#[expect(clippy::cast_possible_truncation, clippy::as_conversions)]
fn parse_f32(value: f64) -> Option<f32> {
    if !value.is_finite() || value < f64::from(f32::MIN) || value > f64::from(f32::MAX) {
        return None;
    }
    Some(value as f32)
}

fn parse_json_f32(value: &serde_json::Value) -> Option<f32> {
    parse_f32(value.as_f64()?)
}
