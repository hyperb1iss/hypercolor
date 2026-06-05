use std::collections::HashMap;

use hypercolor_types::effect::ControlValue;

use crate::control_value_json::{control_value_to_json, controls_to_json};

pub(crate) fn bundled_preset_to_json(
    controls: &HashMap<String, ControlValue>,
) -> serde_json::Value {
    serde_json::Value::Object(controls_to_json(controls))
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
