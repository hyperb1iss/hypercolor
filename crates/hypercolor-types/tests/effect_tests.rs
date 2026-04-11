//! Tests for effect metadata, controls, and lifecycle types.

use std::path::{Path, PathBuf};
use std::str::FromStr;

use hypercolor_types::effect::{
    ControlBinding, ControlDefinition, ControlKind, ControlType, ControlValue, EffectCategory, EffectId,
    EffectMetadata, EffectSource, EffectState, GradientStop,
};
use uuid::Uuid;

// ── EffectId ──────────────────────────────────────────────────────────────

#[test]
fn effect_id_from_uuid_round_trips() {
    let uuid = Uuid::now_v7();
    let id = EffectId::new(uuid);
    assert_eq!(*id.as_uuid(), uuid);
}

#[test]
fn effect_id_display_matches_uuid() {
    let uuid = Uuid::now_v7();
    let id = EffectId::new(uuid);
    assert_eq!(id.to_string(), uuid.to_string());
}

#[test]
fn effect_id_from_uuid_conversion() {
    let uuid = Uuid::now_v7();
    let id: EffectId = uuid.into();
    assert_eq!(*id.as_uuid(), uuid);
}

#[test]
fn effect_id_equality() {
    let uuid = Uuid::now_v7();
    let a = EffectId::new(uuid);
    let b = EffectId::new(uuid);
    assert_eq!(a, b);
}

#[test]
fn effect_id_inequality() {
    let a = EffectId::new(Uuid::now_v7());
    let b = EffectId::new(Uuid::now_v7());
    assert_ne!(a, b);
}

#[test]
fn effect_id_is_copy() {
    let id = EffectId::new(Uuid::now_v7());
    let id2 = id; // Copy
    assert_eq!(id, id2);
}

#[test]
fn effect_id_serde_round_trip() {
    let id = EffectId::new(Uuid::now_v7());
    let json = serde_json::to_string(&id).expect("serialize");
    let back: EffectId = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back, id);
}

// ── EffectCategory ────────────────────────────────────────────────────────

#[test]
fn effect_category_default_is_ambient() {
    assert_eq!(EffectCategory::default(), EffectCategory::Ambient);
}

#[test]
fn effect_category_all_variants_exist() {
    let categories = [
        EffectCategory::Ambient,
        EffectCategory::Audio,
        EffectCategory::Generative,
        EffectCategory::Particle,
        EffectCategory::Scenic,
        EffectCategory::Interactive,
        EffectCategory::Fun,
        EffectCategory::Utility,
    ];
    assert_eq!(categories.len(), 8);
}

#[test]
fn effect_category_is_copy() {
    let cat = EffectCategory::Particle;
    let cat2 = cat; // Copy
    assert_eq!(cat, cat2);
}

#[test]
fn effect_category_display_via_strum() {
    assert_eq!(EffectCategory::Ambient.to_string(), "ambient");
    assert_eq!(EffectCategory::Audio.to_string(), "audio");
    assert_eq!(EffectCategory::Generative.to_string(), "generative");
    assert_eq!(EffectCategory::Particle.to_string(), "particle");
    assert_eq!(EffectCategory::Scenic.to_string(), "scenic");
    assert_eq!(EffectCategory::Interactive.to_string(), "interactive");
    assert_eq!(EffectCategory::Fun.to_string(), "fun");
    assert_eq!(EffectCategory::Utility.to_string(), "utility");
}

#[test]
fn effect_category_from_str_via_strum() {
    assert_eq!(
        EffectCategory::from_str("ambient").expect("parse"),
        EffectCategory::Ambient
    );
    assert_eq!(
        EffectCategory::from_str("particle").expect("parse"),
        EffectCategory::Particle
    );
    assert_eq!(
        EffectCategory::from_str("scenic").expect("parse"),
        EffectCategory::Scenic
    );
    assert_eq!(
        EffectCategory::from_str("fun").expect("parse"),
        EffectCategory::Fun
    );
    assert_eq!(
        EffectCategory::from_str("generative").expect("parse"),
        EffectCategory::Generative
    );
}

#[test]
fn effect_category_from_str_invalid() {
    assert!(EffectCategory::from_str("nonexistent").is_err());
}

#[test]
fn effect_category_serde_round_trip() {
    for cat in [
        EffectCategory::Ambient,
        EffectCategory::Audio,
        EffectCategory::Generative,
        EffectCategory::Particle,
        EffectCategory::Scenic,
        EffectCategory::Interactive,
        EffectCategory::Fun,
        EffectCategory::Utility,
    ] {
        let json = serde_json::to_string(&cat).expect("serialize");
        let back: EffectCategory = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, cat);
    }
}

#[test]
fn effect_category_serde_snake_case() {
    let json = serde_json::to_string(&EffectCategory::Interactive).expect("serialize");
    assert_eq!(json, "\"interactive\"");
}

// ── EffectSource ──────────────────────────────────────────────────────────

#[test]
fn effect_source_native_path() {
    let src = EffectSource::Native {
        path: PathBuf::from("native/aurora.wgsl"),
    };
    assert_eq!(src.path(), Path::new("native/aurora.wgsl"));
}

#[test]
fn effect_source_html_path() {
    let src = EffectSource::Html {
        path: PathBuf::from("community/borealis.html"),
    };
    assert_eq!(src.path(), Path::new("community/borealis.html"));
}

#[test]
fn effect_source_shader_path() {
    let src = EffectSource::Shader {
        path: PathBuf::from("shaders/plasma.wgsl"),
    };
    assert_eq!(src.path(), Path::new("shaders/plasma.wgsl"));
}

#[test]
fn effect_source_serde_round_trip() {
    let sources = vec![
        EffectSource::Native {
            path: PathBuf::from("native/aurora.wgsl"),
        },
        EffectSource::Html {
            path: PathBuf::from("builtin/rainbow.html"),
        },
        EffectSource::Shader {
            path: PathBuf::from("shaders/compute.wgsl"),
        },
    ];

    for src in sources {
        let json = serde_json::to_string(&src).expect("serialize");
        let back: EffectSource = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, src);
    }
}

#[test]
fn effect_source_source_stem_uses_file_stem() {
    let src = EffectSource::Native {
        path: PathBuf::from("builtin/solid_color"),
    };

    assert_eq!(src.source_stem(), Some("solid_color"));
}

#[test]
fn effect_metadata_matches_display_name_and_native_source_alias() {
    let metadata = EffectMetadata {
        id: EffectId::new(Uuid::now_v7()),
        name: "Solid Color".to_owned(),
        author: "Hypercolor".to_owned(),
        version: "0.1.0".to_owned(),
        description: "test effect".to_owned(),
        category: EffectCategory::Utility,
        tags: vec!["solid".to_owned()],
        controls: Vec::new(),
        presets: Vec::new(),
        audio_reactive: false,
        screen_reactive: false,
        source: EffectSource::Native {
            path: PathBuf::from("builtin/solid_color"),
        },
        license: Some("Apache-2.0".to_owned()),
    };

    assert!(metadata.matches_lookup("Solid Color"));
    assert!(metadata.matches_lookup("solid_color"));
    assert!(!metadata.matches_lookup("solid-color-extra"));
}

// ── EffectState ───────────────────────────────────────────────────────────

#[test]
fn effect_state_default_is_loading() {
    assert_eq!(EffectState::default(), EffectState::Loading);
}

#[test]
fn effect_state_all_variants_exist() {
    let states = [
        EffectState::Loading,
        EffectState::Initializing,
        EffectState::Running,
        EffectState::Paused,
        EffectState::Destroying,
    ];
    assert_eq!(states.len(), 5);
}

#[test]
fn effect_state_is_copy() {
    let state = EffectState::Running;
    let state2 = state; // Copy
    assert_eq!(state, state2);
}

#[test]
fn effect_state_serde_round_trip() {
    for state in [
        EffectState::Loading,
        EffectState::Initializing,
        EffectState::Running,
        EffectState::Paused,
        EffectState::Destroying,
    ] {
        let json = serde_json::to_string(&state).expect("serialize");
        let back: EffectState = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, state);
    }
}

#[test]
fn effect_state_serde_snake_case() {
    assert_eq!(
        serde_json::to_string(&EffectState::Initializing).expect("serialize"),
        "\"initializing\""
    );
    assert_eq!(
        serde_json::to_string(&EffectState::Destroying).expect("serialize"),
        "\"destroying\""
    );
}

// ── GradientStop ──────────────────────────────────────────────────────────

#[test]
fn gradient_stop_construction() {
    let stop = GradientStop {
        position: 0.5,
        color: [1.0, 0.0, 0.5, 1.0],
    };
    assert!((stop.position - 0.5).abs() < f32::EPSILON);
    assert!((stop.color[0] - 1.0).abs() < f32::EPSILON);
    assert!((stop.color[2] - 0.5).abs() < f32::EPSILON);
}

#[test]
fn gradient_stop_serde_round_trip() {
    let stop = GradientStop {
        position: 0.75,
        color: [0.2, 0.4, 0.6, 0.8],
    };
    let json = serde_json::to_string(&stop).expect("serialize");
    let back: GradientStop = serde_json::from_str(&json).expect("deserialize");
    assert!((back.position - stop.position).abs() < f32::EPSILON);
    for i in 0..4 {
        assert!((back.color[i] - stop.color[i]).abs() < f32::EPSILON);
    }
}

// ── ControlType ───────────────────────────────────────────────────────────

#[test]
fn control_type_all_variants_exist() {
    let types = [
        ControlType::Slider,
        ControlType::Toggle,
        ControlType::ColorPicker,
        ControlType::GradientEditor,
        ControlType::Dropdown,
        ControlType::TextInput,
    ];
    assert_eq!(types.len(), 6);
}

#[test]
fn control_type_serde_round_trip() {
    for ct in [
        ControlType::Slider,
        ControlType::Toggle,
        ControlType::ColorPicker,
        ControlType::GradientEditor,
        ControlType::Dropdown,
        ControlType::TextInput,
    ] {
        let json = serde_json::to_string(&ct).expect("serialize");
        let back: ControlType = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, ct);
    }
}

// ── ControlValue ──────────────────────────────────────────────────────────

#[test]
fn control_value_float() {
    let val = ControlValue::Float(3.5);
    assert!((val.as_f32().expect("should be numeric") - 3.5).abs() < f32::EPSILON);
}

#[test]
fn control_value_integer() {
    let val = ControlValue::Integer(42);
    assert!((val.as_f32().expect("should be numeric") - 42.0).abs() < f32::EPSILON);
}

#[test]
fn control_value_boolean_as_f32() {
    assert!((ControlValue::Boolean(true).as_f32().expect("numeric") - 1.0).abs() < f32::EPSILON);
    assert!(
        ControlValue::Boolean(false)
            .as_f32()
            .expect("numeric")
            .abs()
            < f32::EPSILON
    );
}

#[test]
fn control_value_color_not_numeric() {
    let val = ControlValue::Color([1.0, 0.0, 0.5, 1.0]);
    assert!(val.as_f32().is_none());
}

#[test]
fn control_value_gradient_not_numeric() {
    let val = ControlValue::Gradient(vec![GradientStop {
        position: 0.0,
        color: [0.0, 0.0, 0.0, 1.0],
    }]);
    assert!(val.as_f32().is_none());
}

#[test]
fn control_value_enum_not_numeric() {
    let val = ControlValue::Enum("option_a".into());
    assert!(val.as_f32().is_none());
}

#[test]
fn control_value_text_not_numeric() {
    let val = ControlValue::Text("hello".into());
    assert!(val.as_f32().is_none());
}

#[test]
fn control_value_js_literal_float() {
    let val = ControlValue::Float(5.0);
    assert_eq!(val.to_js_literal(), "5");
}

#[test]
fn control_value_js_literal_integer() {
    let val = ControlValue::Integer(42);
    assert_eq!(val.to_js_literal(), "42");
}

#[test]
fn control_value_js_literal_boolean() {
    assert_eq!(ControlValue::Boolean(true).to_js_literal(), "true");
    assert_eq!(ControlValue::Boolean(false).to_js_literal(), "false");
}

#[test]
fn control_value_js_literal_color() {
    let val = ControlValue::Color([1.0, 0.5, 0.0, 1.0]);
    assert_eq!(val.to_js_literal(), "\"#ff8000\"");
}

#[test]
fn control_value_js_literal_color_hex_roundtrip() {
    // #001e01 → Color([0.0, 30/255, 1/255, 1.0]) → "#001e01"
    let val = ControlValue::Color([0.0_f32, 30.0 / 255.0, 1.0 / 255.0, 1.0]);
    assert_eq!(val.to_js_literal(), "\"#001e01\"");
}

#[test]
fn control_value_js_literal_color_black_white() {
    assert_eq!(
        ControlValue::Color([0.0, 0.0, 0.0, 1.0]).to_js_literal(),
        "\"#000000\""
    );
    assert_eq!(
        ControlValue::Color([1.0, 1.0, 1.0, 1.0]).to_js_literal(),
        "\"#ffffff\""
    );
}

#[test]
fn control_value_js_literal_enum_escapes_quotes() {
    let val = ControlValue::Enum("say \"hello\"".into());
    assert_eq!(val.to_js_literal(), r#""say \"hello\"""#);
}

#[test]
fn control_value_js_literal_text_escapes_backslash() {
    let val = ControlValue::Text(r"path\to\file".into());
    assert_eq!(val.to_js_literal(), r#""path\\to\\file""#);
}

#[test]
fn control_value_js_literal_gradient() {
    let val = ControlValue::Gradient(vec![
        GradientStop {
            position: 0.0,
            color: [1.0, 0.0, 0.0, 1.0],
        },
        GradientStop {
            position: 1.0,
            color: [0.0, 0.0, 1.0, 1.0],
        },
    ]);
    let js = val.to_js_literal();
    assert!(js.starts_with('['));
    assert!(js.ends_with(']'));
    assert!(js.contains("pos:0"));
    assert!(js.contains("pos:1"));
}

#[test]
fn control_value_serde_round_trip() {
    let values = vec![
        ControlValue::Float(2.5),
        ControlValue::Integer(-10),
        ControlValue::Boolean(true),
        ControlValue::Color([0.1, 0.2, 0.3, 0.4]),
        ControlValue::Gradient(vec![
            GradientStop {
                position: 0.0,
                color: [1.0, 1.0, 1.0, 1.0],
            },
            GradientStop {
                position: 1.0,
                color: [0.0, 0.0, 0.0, 1.0],
            },
        ]),
        ControlValue::Enum("option_b".into()),
        ControlValue::Text("hello world".into()),
    ];

    for val in values {
        let json = serde_json::to_string(&val).expect("serialize");
        let back: ControlValue = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, val);
    }
}

// ── ControlDefinition ─────────────────────────────────────────────────────

fn sample_slider_control() -> ControlDefinition {
    ControlDefinition {
        id: "speed".into(),
        name: "Speed".into(),
        kind: ControlKind::Number,
        control_type: ControlType::Slider,
        default_value: ControlValue::Float(5.0),
        min: Some(1.0),
        max: Some(20.0),
        step: Some(0.5),
        labels: vec![],
        group: Some("Animation".into()),
        tooltip: Some("Animation speed multiplier".into()),
        binding: None,
    }
}

fn sample_dropdown_control() -> ControlDefinition {
    ControlDefinition {
        id: "palette".into(),
        name: "Palette".into(),
        kind: ControlKind::Combobox,
        control_type: ControlType::Dropdown,
        default_value: ControlValue::Enum("Aurora".into()),
        min: None,
        max: None,
        step: None,
        labels: vec!["Aurora".into(), "Sunset".into(), "Ocean".into()],
        group: None,
        tooltip: None,
        binding: None,
    }
}

fn sample_color_picker_control() -> ControlDefinition {
    ControlDefinition {
        id: "zone_1".into(),
        name: "Zone 1".into(),
        kind: ControlKind::Color,
        control_type: ControlType::ColorPicker,
        default_value: ControlValue::Color([1.0, 1.0, 1.0, 1.0]),
        min: None,
        max: None,
        step: None,
        labels: vec![],
        group: Some("Colors".into()),
        tooltip: None,
        binding: None,
    }
}

fn sample_control_binding() -> ControlBinding {
    ControlBinding {
        sensor: " cpu_temp ".into(),
        sensor_min: 30.0,
        sensor_max: 100.0,
        target_min: 0.0,
        target_max: 1.0,
        deadband: -2.0,
        smoothing: 1.5,
    }
}

#[test]
fn control_definition_slider() {
    let ctrl = sample_slider_control();
    assert_eq!(ctrl.id, "speed");
    assert_eq!(ctrl.name, "Speed");
    assert_eq!(ctrl.kind, ControlKind::Number);
    assert_eq!(ctrl.control_type, ControlType::Slider);
    assert_eq!(ctrl.min, Some(1.0));
    assert_eq!(ctrl.max, Some(20.0));
    assert_eq!(ctrl.step, Some(0.5));
    assert_eq!(ctrl.group, Some("Animation".into()));
    assert!(ctrl.tooltip.is_some());
}

#[test]
fn control_definition_dropdown() {
    let ctrl = sample_dropdown_control();
    assert_eq!(ctrl.id, "palette");
    assert_eq!(ctrl.name, "Palette");
    assert_eq!(ctrl.kind, ControlKind::Combobox);
    assert_eq!(ctrl.control_type, ControlType::Dropdown);
    assert_eq!(ctrl.labels.len(), 3);
    assert!(ctrl.labels.contains(&"Aurora".into()));
}

#[test]
fn control_definition_serde_round_trip() {
    let ctrl = sample_slider_control();
    let json = serde_json::to_string_pretty(&ctrl).expect("serialize");
    let back: ControlDefinition = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.id, ctrl.id);
    assert_eq!(back.name, ctrl.name);
    assert_eq!(back.kind, ctrl.kind);
    assert_eq!(back.control_type, ctrl.control_type);
    assert_eq!(back.min, ctrl.min);
    assert_eq!(back.max, ctrl.max);
    assert_eq!(back.step, ctrl.step);
    assert_eq!(back.tooltip, ctrl.tooltip);
}

#[test]
fn control_binding_normalized_clamps_runtime_fields() {
    let binding = sample_control_binding().normalized();

    assert_eq!(binding.sensor, "cpu_temp");
    assert_eq!(binding.deadband, 0.0);
    assert!((binding.smoothing - 0.99).abs() < f32::EPSILON);
}

#[test]
fn control_binding_serde_round_trip() {
    let binding = sample_control_binding();
    let json = serde_json::to_string_pretty(&binding).expect("serialize");
    let back: ControlBinding = serde_json::from_str(&json).expect("deserialize");

    assert_eq!(back, binding);
}

#[test]
fn color_picker_validation_normalizes_hex_text_to_color() {
    let control = sample_color_picker_control();
    let validated = control
        .validate_value(&ControlValue::Text("#80ffea".into()))
        .expect("hex text should validate");

    match validated {
        ControlValue::Color([r, g, b, a]) => {
            assert!(r > 0.2, "red should be converted from hex");
            assert!(g > 0.9, "green should be converted from hex");
            assert!(b > 0.8, "blue should be converted from hex");
            assert!((a - 1.0).abs() < f32::EPSILON);
        }
        other => panic!("expected normalized color, got {other:?}"),
    }
}

#[test]
fn non_color_picker_color_control_preserves_text_values() {
    let mut control = sample_color_picker_control();
    control.control_type = ControlType::TextInput;

    let validated = control
        .validate_value(&ControlValue::Text("brand-accent".into()))
        .expect("text color token should validate");

    assert_eq!(validated, ControlValue::Text("brand-accent".into()));
}

// ── EffectMetadata ────────────────────────────────────────────────────────

fn sample_metadata() -> EffectMetadata {
    EffectMetadata {
        id: EffectId::new(Uuid::now_v7()),
        name: "Aurora".into(),
        author: "hyperb1iss".into(),
        version: "1.0.0".into(),
        description: "Northern lights simulation with audio-reactive wave intensity".into(),
        category: EffectCategory::Ambient,
        tags: vec!["ambient".into(), "audio-reactive".into(), "nature".into()],
        controls: vec![sample_slider_control(), sample_dropdown_control()],
        presets: Vec::new(),
        audio_reactive: true,
        screen_reactive: false,
        source: EffectSource::Native {
            path: PathBuf::from("native/aurora.wgsl"),
        },
        license: Some("Apache-2.0".into()),
    }
}

#[test]
fn effect_metadata_construction() {
    let meta = sample_metadata();
    assert_eq!(meta.name, "Aurora");
    assert_eq!(meta.author, "hyperb1iss");
    assert_eq!(meta.version, "1.0.0");
    assert_eq!(meta.category, EffectCategory::Ambient);
    assert_eq!(meta.tags.len(), 3);
    assert_eq!(meta.controls.len(), 2);
    assert!(meta.audio_reactive);
    assert_eq!(meta.license, Some("Apache-2.0".into()));
}

#[test]
fn effect_metadata_default_version() {
    let json = r#"{
        "id": "01935e7c-3333-7000-aaaa-bbbbccccdddd",
        "name": "Test",
        "author": "test",
        "description": "A test effect",
        "category": "utility",
        "controls": [],
        "source": { "native": { "path": "test.wgsl" } }
    }"#;
    let meta: EffectMetadata = serde_json::from_str(json).expect("deserialize");
    assert_eq!(meta.version, "0.1.0");
}

#[test]
fn effect_metadata_serde_json_round_trip() {
    let meta = sample_metadata();
    let json = serde_json::to_string_pretty(&meta).expect("serialize");
    let back: EffectMetadata = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.name, meta.name);
    assert_eq!(back.author, meta.author);
    assert_eq!(back.version, meta.version);
    assert_eq!(back.description, meta.description);
    assert_eq!(back.category, meta.category);
    assert_eq!(back.tags, meta.tags);
    assert_eq!(back.controls, meta.controls);
    assert_eq!(back.audio_reactive, meta.audio_reactive);
    assert_eq!(back.license, meta.license);
}

#[test]
fn effect_metadata_serde_toml_round_trip() {
    let meta = sample_metadata();
    let toml_str = toml::to_string_pretty(&meta).expect("toml serialize");
    let back: EffectMetadata = toml::from_str(&toml_str).expect("toml deserialize");
    assert_eq!(back.name, meta.name);
    assert_eq!(back.author, meta.author);
    assert_eq!(back.category, meta.category);
}

#[test]
fn effect_metadata_empty_tags_default() {
    let json = r#"{
        "id": "01935e7c-3333-7000-aaaa-bbbbccccdddd",
        "name": "Minimal",
        "author": "test",
        "description": "Minimal effect",
        "category": "utility",
        "source": { "html": { "path": "test.html" } }
    }"#;
    let meta: EffectMetadata = serde_json::from_str(json).expect("deserialize");
    assert!(meta.tags.is_empty());
    assert!(meta.license.is_none());
}
