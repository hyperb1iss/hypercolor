use std::collections::HashMap;

use hypercolor_types::canvas::linear_to_srgb;
use hypercolor_types::effect::ControlValue;

pub(crate) fn controls_to_json(
    values: &HashMap<String, ControlValue>,
) -> serde_json::Map<String, serde_json::Value> {
    values
        .iter()
        .map(|(key, value)| (key.clone(), control_value_to_json(value)))
        .collect()
}

pub(crate) fn bundled_preset_to_json(
    controls: &HashMap<String, ControlValue>,
) -> serde_json::Value {
    serde_json::Value::Object(
        controls
            .iter()
            .map(|(key, value)| (key.clone(), control_value_to_json(value)))
            .collect(),
    )
}

pub(crate) fn control_value_to_json(value: &ControlValue) -> serde_json::Value {
    match value {
        ControlValue::Float(number) => serde_json::json!(number),
        ControlValue::Integer(number) => serde_json::json!(number),
        ControlValue::Boolean(boolean) => serde_json::json!(boolean),
        ControlValue::Text(text) | ControlValue::Enum(text) => serde_json::json!(text),
        ControlValue::Color(rgba) => {
            serde_json::json!(format!(
                "#{:02x}{:02x}{:02x}",
                color_channel_to_byte(rgba[0]),
                color_channel_to_byte(rgba[1]),
                color_channel_to_byte(rgba[2]),
            ))
        }
        ControlValue::Gradient(stops) => serde_json::json!(stops),
    }
}

pub(crate) fn user_preset_matches_controls(
    current_values: &HashMap<String, ControlValue>,
    preset_controls: &HashMap<String, serde_json::Value>,
) -> bool {
    let current_json = controls_to_json(current_values);
    preset_controls
        .iter()
        .all(|(key, expected)| current_json.get(key) == Some(expected))
}

pub(crate) fn bundled_preset_matches_controls(
    current_values: &HashMap<String, ControlValue>,
    preset_controls: &HashMap<String, ControlValue>,
) -> bool {
    let current_json = controls_to_json(current_values);
    preset_controls
        .iter()
        .all(|(key, expected)| current_json.get(key) == Some(&control_value_to_json(expected)))
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::as_conversions
)]
fn color_channel_to_byte(channel: f32) -> u8 {
    (linear_to_srgb(channel.clamp(0.0, 1.0)) * 255.0).round() as u8
}
