use hypercolor_types::canvas::srgb_to_linear;
use hypercolor_types::effect::{ControlDefinition, ControlKind, ControlType, ControlValue};
use hypercolor_ui::api::{ComponentBindingRequest, PairDeviceRequest, SetDisplayFaceRequest};
use hypercolor_ui::control_value_json::{
    controls_to_json, hex_to_rgba, hex_to_rgba_json, json_to_control_value,
};
use hypercolor_ui::display_utils::display_preview_shell_url;
use hypercolor_ui::optimistic_controls::{
    apply_raw_control_updates, merge_control_values, raw_control_updates_payload,
};
use hypercolor_ui::style_utils::category_style;

fn display_preview_target_from_search(search: &str) -> Option<String> {
    let query = search.strip_prefix('?').unwrap_or(search);
    query
        .split('&')
        .filter(|segment| !segment.is_empty())
        .find_map(|segment| {
            let (key, value) = segment.split_once('=')?;
            if key != "display" {
                return None;
            }
            let trimmed = value.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_owned())
        })
}

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

#[test]
fn pair_device_request_serializes_canonical_shape() {
    // The shared canonical type (hypercolor-types::pairing) always emits
    // `values`; the daemon deserializes it with #[serde(default)], so an
    // empty map and a missing key are equivalent on the wire.
    let payload = serde_json::to_value(PairDeviceRequest {
        values: std::collections::HashMap::new(),
        activate_after_pair: true,
    })
    .expect("pair request should serialize");

    assert_eq!(
        payload,
        serde_json::json!({
            "values": {},
            "activate_after_pair": true
        })
    );
}

#[test]
fn attachment_binding_request_keeps_explicit_defaults_on_wire() {
    let payload = serde_json::to_value(ComponentBindingRequest {
        slot_id: "slot-1".to_owned(),
        template_id: "template-1".to_owned(),
        name: None,
        enabled: true,
        instances: 1,
        led_offset: 0,
    })
    .expect("attachment binding request should serialize");

    assert_eq!(
        payload,
        serde_json::json!({
            "slot_id": "slot-1",
            "template_id": "template-1",
            "enabled": true,
            "instances": 1,
            "led_offset": 0
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
        blend_mode: Some(hypercolor_types::scene::DisplayFaceBlendMode::Replace),
        opacity: Some(1.0),
        scope: hypercolor_ui::api::DisplayFaceScope::Default,
    })
    .expect("display-face request should serialize");

    assert_eq!(
        payload,
        serde_json::json!({
            "effect_id": "face-1",
            "blend_mode": "replace",
            "opacity": 1.0,
            "scope": "default"
        })
    );
}

#[test]
fn set_display_face_request_serializes_present_controls() {
    let payload = serde_json::to_value(SetDisplayFaceRequest {
        effect_id: "face-2".to_owned(),
        controls: std::collections::HashMap::from([(
            "accent".to_owned(),
            ControlValue::Float(0.75),
        )]),
        blend_mode: Some(hypercolor_types::scene::DisplayFaceBlendMode::Replace),
        opacity: Some(1.0),
        scope: hypercolor_ui::api::DisplayFaceScope::Scene,
    })
    .expect("display-face request should serialize");

    assert_eq!(
        payload,
        serde_json::json!({
            "effect_id": "face-2",
            "controls": { "accent": { "float": 0.75 } },
            "blend_mode": "replace",
            "opacity": 1.0,
            "scope": "scene"
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
fn json_to_control_value_maps_primitive_types() {
    let controls: Vec<ControlDefinition> = Vec::new();
    assert_eq!(
        json_to_control_value("flag", &controls, &serde_json::json!(true)),
        Some(ControlValue::Boolean(true))
    );
    assert_eq!(
        json_to_control_value("count", &controls, &serde_json::json!(7)),
        Some(ControlValue::Integer(7))
    );
    let Some(ControlValue::Float(v)) =
        json_to_control_value("alpha", &controls, &serde_json::json!(0.25))
    else {
        panic!("float conversion should succeed");
    };
    assert!((v - 0.25).abs() < 1e-6);
}

#[test]
fn json_to_control_value_uses_control_type_for_strings() {
    let controls = vec![dropdown_control("mode"), color_control("accent")];

    assert_eq!(
        json_to_control_value("mode", &controls, &serde_json::json!("high")),
        Some(ControlValue::Enum("high".to_owned()))
    );

    let Some(ControlValue::Color(color)) =
        json_to_control_value("accent", &controls, &serde_json::json!("#ff80c0"))
    else {
        panic!("hex string should convert to Color for color-picker control");
    };
    assert!((color[0] - 1.0).abs() < 1e-6);
    assert!((color[1] - srgb_to_linear(128.0 / 255.0)).abs() < 1e-6);
    assert!((color[2] - srgb_to_linear(192.0 / 255.0)).abs() < 1e-6);
    assert!((color[3] - 1.0).abs() < 1e-6);

    // Unknown control id falls back to Text.
    assert_eq!(
        json_to_control_value("label", &controls, &serde_json::json!("hi")),
        Some(ControlValue::Text("hi".to_owned()))
    );
}

#[test]
fn json_to_control_value_accepts_rgba_arrays() {
    let controls: Vec<ControlDefinition> = Vec::new();
    let Some(ControlValue::Color(color)) = json_to_control_value(
        "accent",
        &controls,
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
fn json_to_control_value_rejects_malformed_input() {
    let controls: Vec<ControlDefinition> = Vec::new();
    assert!(json_to_control_value("x", &controls, &serde_json::json!(null)).is_none());
    assert!(json_to_control_value("x", &controls, &serde_json::json!([1, 2])).is_none());
    assert!(json_to_control_value("x", &controls, &serde_json::json!(f64::INFINITY)).is_none());
    assert!(json_to_control_value("x", &controls, &serde_json::json!(f64::NAN)).is_none());
}

#[test]
fn controls_to_json_serializes_typed_values_for_api_payloads() {
    let values = std::collections::HashMap::from([
        ("speed".to_owned(), ControlValue::Float(0.75)),
        ("count".to_owned(), ControlValue::Integer(7)),
        ("enabled".to_owned(), ControlValue::Boolean(true)),
        ("mode".to_owned(), ControlValue::Enum("high".to_owned())),
        (
            "accent".to_owned(),
            ControlValue::Color([
                srgb_to_linear(128.0 / 255.0),
                srgb_to_linear(255.0 / 255.0),
                srgb_to_linear(234.0 / 255.0),
                1.0,
            ]),
        ),
    ]);

    let json = controls_to_json(&values);

    assert_eq!(json.get("speed"), Some(&serde_json::json!(0.75)));
    assert_eq!(json.get("count"), Some(&serde_json::json!(7)));
    assert_eq!(json.get("enabled"), Some(&serde_json::json!(true)));
    assert_eq!(json.get("mode"), Some(&serde_json::json!("high")));
    assert_eq!(json.get("accent"), Some(&serde_json::json!("#80ffea")));
}

#[test]
fn optimistic_control_updates_apply_raw_values() {
    let controls = vec![dropdown_control("mode"), color_control("accent")];
    let updates = vec![
        ("mode".to_owned(), serde_json::json!("high")),
        ("accent".to_owned(), serde_json::json!("#ff80c0")),
    ];
    let mut values = std::collections::HashMap::new();

    apply_raw_control_updates(&mut values, &controls, &updates);

    assert_eq!(
        values.get("mode"),
        Some(&ControlValue::Enum("high".to_owned()))
    );
    let Some(ControlValue::Color(color)) = values.get("accent") else {
        panic!("accent should be converted to a color");
    };
    assert!((color[1] - srgb_to_linear(128.0 / 255.0)).abs() < 1e-6);
}

#[test]
fn optimistic_control_helpers_merge_and_payload_pending_updates() {
    let mut values = std::collections::HashMap::from([(
        "mode".to_owned(),
        ControlValue::Enum("low".to_owned()),
    )]);
    let next = std::collections::HashMap::from([(
        "mode".to_owned(),
        ControlValue::Enum("high".to_owned()),
    )]);

    merge_control_values(&mut values, &next);

    assert_eq!(values, next);

    let payload = raw_control_updates_payload(std::collections::HashMap::from([(
        "mode".to_owned(),
        serde_json::json!("high"),
    )]));
    assert_eq!(payload, serde_json::json!({ "mode": "high" }));
}

#[test]
fn hex_to_rgba_parses_with_and_without_leading_hash() {
    let Some(a) = hex_to_rgba("#80ffea") else {
        panic!("valid hex with leading # should parse");
    };
    let Some(b) = hex_to_rgba("80ffea") else {
        panic!("valid hex without leading # should parse");
    };
    assert!((a[0] - srgb_to_linear(128.0 / 255.0)).abs() < 1e-6);
    assert_eq!(a, b);

    // Alpha channel.
    let Some(rgba) = hex_to_rgba("#ff000080") else {
        panic!("8-char hex should parse with alpha");
    };
    assert!((rgba[3] - 128.0 / 255.0).abs() < 1e-6);
}

#[test]
fn hex_to_rgba_json_preserves_alpha_and_linearizes_rgb() {
    let Some(value) = hex_to_rgba_json("#80ffea80") else {
        panic!("8-char hex should convert to a JSON RGBA payload");
    };
    let Some(values) = value.as_array() else {
        panic!("RGBA payload should be an array");
    };

    assert_eq!(values.len(), 4);
    assert!(
        (values[0].as_f64().expect("red channel should be numeric")
            - f64::from(srgb_to_linear(128.0 / 255.0)))
        .abs()
            < 1e-6
    );
    assert!(
        (values[3].as_f64().expect("alpha channel should be numeric") - (128.0 / 255.0)).abs()
            < 1e-6
    );
}

#[test]
fn hex_to_rgba_rejects_malformed_input() {
    assert!(hex_to_rgba("").is_none());
    assert!(hex_to_rgba("#abc").is_none());
    assert!(hex_to_rgba("#nothexx").is_none());
    assert!(hex_to_rgba("abcdefghij").is_none());
}
