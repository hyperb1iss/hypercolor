#![allow(dead_code, unused_imports)]

#[path = "../src/api/mod.rs"]
mod api;
#[path = "../src/display_utils.rs"]
mod display_utils;
#[path = "../src/style_utils.rs"]
mod style_utils;

use api::{DisplaySummary, SetDisplayFaceRequest, UpdateSimulatedDisplayRequest};
use display_utils::{
    display_preview_shell_url, display_preview_target_from_search, hex_to_rgba,
    is_simulator_display, json_to_face_control_value, parse_simulator_dimension,
};
use hypercolor_types::effect::{ControlDefinition, ControlKind, ControlType, ControlValue};
use style_utils::category_style;

fn dropdown_control(id: &str) -> ControlDefinition {
    ControlDefinition {
        id: id.to_owned(),
        name: id.to_owned(),
        kind: ControlKind::Combobox,
        control_type: ControlType::Dropdown,
        default_value: ControlValue::Enum(String::new()),
        min: None,
        max: None,
        step: None,
        labels: vec!["a".to_owned(), "b".to_owned()],
        group: None,
        tooltip: None,
        aspect_lock: None,
        preview_source: None,
        binding: None,
    }
}

fn color_control(id: &str) -> ControlDefinition {
    ControlDefinition {
        id: id.to_owned(),
        name: id.to_owned(),
        kind: ControlKind::Color,
        control_type: ControlType::ColorPicker,
        default_value: ControlValue::Color([0.0, 0.0, 0.0, 1.0]),
        min: None,
        max: None,
        step: None,
        labels: Vec::new(),
        group: None,
        tooltip: None,
        aspect_lock: None,
        preview_source: None,
        binding: None,
    }
}

fn display_summary(family: &str) -> DisplaySummary {
    DisplaySummary {
        id: "display-1".to_owned(),
        name: "Preview LCD".to_owned(),
        vendor: "Hypercolor".to_owned(),
        family: family.to_owned(),
        width: 480,
        height: 480,
        circular: true,
    }
}

#[test]
fn simulator_detection_is_case_insensitive() {
    assert!(is_simulator_display(&display_summary("simulator")));
    assert!(is_simulator_display(&display_summary("Simulator")));
    assert!(is_simulator_display(&display_summary("SIMULATOR")));
}

#[test]
fn simulator_detection_rejects_other_families() {
    assert!(!is_simulator_display(&display_summary("corsair")));
    assert!(!is_simulator_display(&display_summary("custom")));
}

#[test]
fn parse_simulator_dimension_accepts_trimmed_positive_values() {
    assert_eq!(parse_simulator_dimension(" 480 ", "Width"), Ok(480));
    assert_eq!(parse_simulator_dimension("1", "Height"), Ok(1));
}

#[test]
fn parse_simulator_dimension_rejects_invalid_values() {
    assert_eq!(
        parse_simulator_dimension("0", "Width"),
        Err("Width must be a positive number.".to_owned())
    );
    assert_eq!(
        parse_simulator_dimension("abc", "Height"),
        Err("Height must be a positive number.".to_owned())
    );
}

#[test]
fn update_simulated_display_request_skips_absent_fields() {
    let payload = serde_json::to_value(UpdateSimulatedDisplayRequest::default())
        .expect("default simulator update request should serialize");
    assert_eq!(payload, serde_json::json!({}));
}

#[test]
fn update_simulated_display_request_serializes_only_present_fields() {
    let payload = serde_json::to_value(UpdateSimulatedDisplayRequest {
        name: Some("Desk LCD".to_owned()),
        width: Some(600),
        height: None,
        circular: Some(false),
        enabled: None,
    })
    .expect("partial simulator update request should serialize");

    assert_eq!(
        payload,
        serde_json::json!({
            "name": "Desk LCD",
            "width": 600,
            "circular": false
        })
    );
}

#[test]
fn display_preview_shell_url_targets_selected_display() {
    assert_eq!(
        display_preview_shell_url("display-123"),
        "/preview?display=display-123"
    );
}

#[test]
fn display_preview_target_from_search_extracts_display_id() {
    assert_eq!(
        display_preview_target_from_search("?display=display-123"),
        Some("display-123".to_owned())
    );
    assert_eq!(
        display_preview_target_from_search("?foo=bar&display=display-456"),
        Some("display-456".to_owned())
    );
    assert_eq!(display_preview_target_from_search("?foo=bar"), None);
    assert_eq!(display_preview_target_from_search("?display="), None);
}

#[test]
fn set_display_face_request_skips_empty_controls() {
    let payload = serde_json::to_value(SetDisplayFaceRequest {
        effect_id: "face-1".to_owned(),
        controls: std::collections::HashMap::new(),
        blend_mode: None,
        opacity: None,
    })
    .expect("display-face request should serialize");

    assert_eq!(payload, serde_json::json!({ "effect_id": "face-1" }));
}

#[test]
fn set_display_face_request_serializes_present_controls() {
    let payload = serde_json::to_value(SetDisplayFaceRequest {
        effect_id: "face-2".to_owned(),
        controls: std::collections::HashMap::from([(
            "accent".to_owned(),
            ControlValue::Float(0.75),
        )]),
        blend_mode: None,
        opacity: None,
    })
    .expect("display-face request should serialize");

    assert_eq!(
        payload,
        serde_json::json!({
            "effect_id": "face-2",
            "controls": { "accent": { "float": 0.75 } }
        })
    );
}

#[test]
fn display_category_uses_coral_accent() {
    assert_eq!(
        category_style("display"),
        ("bg-coral/10 text-coral", "255, 106, 193")
    );
}

#[test]
fn json_to_face_control_value_maps_primitive_types() {
    let controls: Vec<ControlDefinition> = Vec::new();
    assert_eq!(
        json_to_face_control_value(&controls, "flag", &serde_json::json!(true)),
        Some(ControlValue::Boolean(true))
    );
    assert_eq!(
        json_to_face_control_value(&controls, "count", &serde_json::json!(7)),
        Some(ControlValue::Integer(7))
    );
    let Some(ControlValue::Float(v)) =
        json_to_face_control_value(&controls, "alpha", &serde_json::json!(0.25))
    else {
        panic!("float conversion should succeed");
    };
    assert!((v - 0.25).abs() < 1e-6);
}

#[test]
fn json_to_face_control_value_uses_control_type_for_strings() {
    let controls = vec![dropdown_control("mode"), color_control("accent")];

    assert_eq!(
        json_to_face_control_value(&controls, "mode", &serde_json::json!("high")),
        Some(ControlValue::Enum("high".to_owned()))
    );

    // Hex string on a color control becomes a normalized RGBA.
    let Some(ControlValue::Color(color)) =
        json_to_face_control_value(&controls, "accent", &serde_json::json!("#ff80c0"))
    else {
        panic!("hex string should convert to Color for color-picker control");
    };
    assert!((color[0] - 1.0).abs() < 1e-6);
    assert!((color[1] - 128.0 / 255.0).abs() < 1e-6);
    assert!((color[2] - 192.0 / 255.0).abs() < 1e-6);
    assert!((color[3] - 1.0).abs() < 1e-6);

    // Unknown control id falls back to Text.
    assert_eq!(
        json_to_face_control_value(&controls, "label", &serde_json::json!("hi")),
        Some(ControlValue::Text("hi".to_owned()))
    );
}

#[test]
fn json_to_face_control_value_accepts_rgba_arrays() {
    let controls: Vec<ControlDefinition> = Vec::new();
    let Some(ControlValue::Color(color)) = json_to_face_control_value(
        &controls,
        "accent",
        &serde_json::json!([1.0, 0.5, 0.25, 1.0]),
    ) else {
        panic!("four-element array should convert to Color");
    };
    assert!((color[0] - 1.0).abs() < 1e-6);
    assert!((color[1] - 0.5).abs() < 1e-6);
    assert!((color[2] - 0.25).abs() < 1e-6);
    assert!((color[3] - 1.0).abs() < 1e-6);
}

#[test]
fn json_to_face_control_value_rejects_malformed_input() {
    let controls: Vec<ControlDefinition> = Vec::new();
    assert!(json_to_face_control_value(&controls, "x", &serde_json::json!(null)).is_none());
    assert!(json_to_face_control_value(&controls, "x", &serde_json::json!([1, 2])).is_none());
    assert!(
        json_to_face_control_value(&controls, "x", &serde_json::json!(f64::INFINITY)).is_none()
    );
    assert!(json_to_face_control_value(&controls, "x", &serde_json::json!(f64::NAN)).is_none());
}

#[test]
fn hex_to_rgba_parses_with_and_without_leading_hash() {
    let Some(a) = hex_to_rgba("#80ffea") else {
        panic!("valid hex with leading # should parse");
    };
    let Some(b) = hex_to_rgba("80ffea") else {
        panic!("valid hex without leading # should parse");
    };
    assert!((a[0] - 128.0 / 255.0).abs() < 1e-6);
    assert_eq!(a, b);

    // Alpha channel.
    let Some(rgba) = hex_to_rgba("#ff000080") else {
        panic!("8-char hex should parse with alpha");
    };
    assert!((rgba[3] - 128.0 / 255.0).abs() < 1e-6);
}

#[test]
fn hex_to_rgba_rejects_malformed_input() {
    assert!(hex_to_rgba("").is_none());
    assert!(hex_to_rgba("#abc").is_none());
    assert!(hex_to_rgba("#nothexx").is_none());
    assert!(hex_to_rgba("abcdefghij").is_none());
}
