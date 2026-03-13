#[path = "../src/components/preset_matching.rs"]
mod preset_matching;

use std::collections::HashMap;

use hypercolor_types::effect::ControlValue;
use preset_matching::{
    bundled_preset_matches_controls, bundled_preset_to_json, user_preset_matches_controls,
};

#[test]
fn bundled_presets_match_normalized_color_values() {
    let current_values = HashMap::from([
        ("color".to_owned(), ControlValue::Text("#4033ff".to_owned())),
        (
            "color2".to_owned(),
            ControlValue::Text("#ff369f".to_owned()),
        ),
        (
            "color3".to_owned(),
            ControlValue::Text("#2effe0".to_owned()),
        ),
        (
            "bgColor".to_owned(),
            ControlValue::Text("#000000".to_owned()),
        ),
        (
            "colorMode".to_owned(),
            ControlValue::Enum("Palette Blend".to_owned()),
        ),
        (
            "theme".to_owned(),
            ControlValue::Enum("Jellyfish".to_owned()),
        ),
        ("speed".to_owned(), ControlValue::Float(8.0)),
        ("count".to_owned(), ControlValue::Float(65.0)),
        ("size".to_owned(), ControlValue::Float(7.0)),
    ]);
    let preset_controls = HashMap::from([
        (
            "color".to_owned(),
            ControlValue::Color([0.25415218, 0.20155624, 1.0, 1.0]),
        ),
        (
            "color2".to_owned(),
            ControlValue::Color([1.0, 0.21223073, 0.62396044, 1.0]),
        ),
        (
            "color3".to_owned(),
            ControlValue::Color([0.18116423, 1.0, 0.87962234, 1.0]),
        ),
        (
            "bgColor".to_owned(),
            ControlValue::Color([0.000607054, 0.000303527, 0.002428216, 1.0]),
        ),
        (
            "colorMode".to_owned(),
            ControlValue::Enum("Palette Blend".to_owned()),
        ),
        (
            "theme".to_owned(),
            ControlValue::Enum("Jellyfish".to_owned()),
        ),
        ("speed".to_owned(), ControlValue::Float(8.0)),
        ("count".to_owned(), ControlValue::Float(65.0)),
        ("size".to_owned(), ControlValue::Float(7.0)),
    ]);

    assert!(bundled_preset_matches_controls(
        &current_values,
        &preset_controls,
    ));
}

#[test]
fn bundled_presets_do_not_match_when_a_normalized_value_differs() {
    let current_values = HashMap::from([
        ("color".to_owned(), ControlValue::Text("#4033ff".to_owned())),
        (
            "theme".to_owned(),
            ControlValue::Enum("Cyber Pop".to_owned()),
        ),
    ]);
    let preset_controls = HashMap::from([
        (
            "color".to_owned(),
            ControlValue::Color([0.25415218, 0.20155624, 1.0, 1.0]),
        ),
        (
            "theme".to_owned(),
            ControlValue::Enum("Jellyfish".to_owned()),
        ),
    ]);

    assert!(!bundled_preset_matches_controls(
        &current_values,
        &preset_controls,
    ));
}

#[test]
fn user_presets_match_saved_json_controls() {
    let current_values = HashMap::from([
        ("color".to_owned(), ControlValue::Text("#4033ff".to_owned())),
        (
            "theme".to_owned(),
            ControlValue::Enum("Jellyfish".to_owned()),
        ),
        ("speed".to_owned(), ControlValue::Float(8.0)),
    ]);
    let preset_controls = HashMap::from([
        ("color".to_owned(), serde_json::json!("#4033ff")),
        ("theme".to_owned(), serde_json::json!("Jellyfish")),
        ("speed".to_owned(), serde_json::json!(8.0)),
    ]);

    assert!(user_preset_matches_controls(
        &current_values,
        &preset_controls,
    ));
}

#[test]
fn bundled_presets_serialize_colors_to_hex_for_patch_requests() {
    let preset_controls = HashMap::from([
        (
            "color".to_owned(),
            ControlValue::Color([0.25415218, 0.20155624, 1.0, 1.0]),
        ),
        ("speed".to_owned(), ControlValue::Float(8.0)),
    ]);

    assert_eq!(
        bundled_preset_to_json(&preset_controls),
        serde_json::json!({
            "color": "#4033ff",
            "speed": 8.0,
        })
    );
}
