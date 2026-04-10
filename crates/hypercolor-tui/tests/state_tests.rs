//! Tests for TUI state types and their conversions.

use hypercolor_tui::state::{
    CanvasFrame, CanvasPreviewState, ConnectionStatus, ControlDefinition, ControlValue,
    DaemonState, DeviceSummary, EffectSummary, Notification, NotificationLevel, SpectrumSnapshot,
};

// ── ControlValue conversion tests ────────────────────────────────

#[test]
fn control_value_float_as_f32() {
    let v = ControlValue::Float(0.75);
    assert_eq!(v.as_f32(), Some(0.75));
}

#[test]
fn control_value_integer_as_f32() {
    let v = ControlValue::Integer(42);
    assert_eq!(v.as_f32(), Some(42.0));
}

#[test]
fn control_value_boolean_as_f32_returns_none() {
    let v = ControlValue::Boolean(true);
    assert!(v.as_f32().is_none());
}

#[test]
fn control_value_text_as_f32_returns_none() {
    let v = ControlValue::Text("hello".to_string());
    assert!(v.as_f32().is_none());
}

#[test]
fn control_value_color_as_f32_returns_none() {
    let v = ControlValue::Color([1.0, 0.0, 0.5, 1.0]);
    assert!(v.as_f32().is_none());
}

#[test]
fn control_value_boolean_as_bool() {
    assert_eq!(ControlValue::Boolean(true).as_bool(), Some(true));
    assert_eq!(ControlValue::Boolean(false).as_bool(), Some(false));
}

#[test]
fn control_value_non_boolean_as_bool_returns_none() {
    assert!(ControlValue::Float(1.0).as_bool().is_none());
    assert!(ControlValue::Integer(1).as_bool().is_none());
    assert!(ControlValue::Text("true".to_string()).as_bool().is_none());
}

// ── Serde round-trip tests ───────────────────────────────────────

#[test]
fn control_value_float_serde_roundtrip() {
    let v = ControlValue::Float(2.72);
    let json = serde_json::to_string(&v).expect("serialize");
    let parsed: ControlValue = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(parsed.as_f32(), Some(2.72));
}

#[test]
fn control_value_boolean_serde_roundtrip() {
    let v = ControlValue::Boolean(true);
    let json = serde_json::to_string(&v).expect("serialize");
    let parsed: ControlValue = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(parsed.as_bool(), Some(true));
}

#[test]
fn control_value_text_serde_roundtrip() {
    let v = ControlValue::Text("rainbow".to_string());
    let json = serde_json::to_string(&v).expect("serialize");
    let parsed: ControlValue = serde_json::from_str(&json).expect("deserialize");
    match parsed {
        ControlValue::Text(s) => assert_eq!(s, "rainbow"),
        other => panic!("expected Text, got {other:?}"),
    }
}

#[test]
fn daemon_state_serde_roundtrip() {
    let state = DaemonState {
        running: true,
        brightness: 75,
        fps_target: 30.0,
        fps_actual: 29.5,
        effect_name: Some("Aurora".to_string()),
        effect_id: Some("aurora-1".to_string()),
        profile_name: Some("Gaming".to_string()),
        device_count: 3,
        total_leds: 150,
    };
    let json = serde_json::to_string(&state).expect("serialize");
    let parsed: DaemonState = serde_json::from_str(&json).expect("deserialize");
    assert!(parsed.running);
    assert_eq!(parsed.brightness, 75);
    assert_eq!(parsed.device_count, 3);
    assert_eq!(parsed.effect_name.as_deref(), Some("Aurora"));
}

#[test]
fn effect_summary_deserialize_with_defaults() {
    // Minimal JSON — all #[serde(default)] fields should use defaults
    let json = r#"{"id": "test", "name": "Test Effect"}"#;
    let effect: EffectSummary = serde_json::from_str(json).expect("deserialize");
    assert_eq!(effect.id, "test");
    assert_eq!(effect.name, "Test Effect");
    assert!(effect.description.is_empty());
    assert!(effect.author.is_empty());
    assert!(effect.tags.is_empty());
    assert!(effect.controls.is_empty());
    assert!(!effect.audio_reactive);
}

#[test]
fn device_summary_serde_roundtrip() {
    let device = DeviceSummary {
        id: "razer-1".to_string(),
        name: "Razer Huntsman".to_string(),
        family: "razer".to_string(),
        led_count: 104,
        state: "connected".to_string(),
        fps: Some(30.0),
    };
    let json = serde_json::to_string(&device).expect("serialize");
    let parsed: DeviceSummary = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(parsed.id, "razer-1");
    assert_eq!(parsed.led_count, 104);
    assert_eq!(parsed.fps, Some(30.0));
}

#[test]
fn control_definition_full_roundtrip() {
    let ctrl = ControlDefinition {
        id: "speed".to_string(),
        name: "Speed".to_string(),
        control_type: "slider".to_string(),
        default_value: ControlValue::Float(0.5),
        min: Some(0.0),
        max: Some(1.0),
        step: Some(0.01),
        labels: vec![],
        group: Some("Animation".to_string()),
        tooltip: Some("Control speed".to_string()),
    };
    let json = serde_json::to_string(&ctrl).expect("serialize");
    let parsed: ControlDefinition = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(parsed.id, "speed");
    assert_eq!(parsed.control_type, "slider");
    assert_eq!(parsed.default_value.as_f32(), Some(0.5));
    assert_eq!(parsed.min, Some(0.0));
    assert_eq!(parsed.max, Some(1.0));
}

// ── Default / Clone tests ────────────────────────────────────────

#[test]
fn connection_status_default_is_disconnected() {
    assert_eq!(ConnectionStatus::default(), ConnectionStatus::Disconnected);
}

#[test]
fn notification_level_clone_eq() {
    let level = NotificationLevel::Warning;
    let cloned = level;
    assert_eq!(level, cloned);
}

#[test]
fn notification_clone() {
    let n = Notification {
        message: "Effect applied".to_string(),
        level: NotificationLevel::Success,
    };
    let cloned = n.clone();
    assert_eq!(cloned.message, "Effect applied");
    assert_eq!(cloned.level, NotificationLevel::Success);
}

#[test]
fn spectrum_snapshot_clone() {
    let snap = SpectrumSnapshot {
        timestamp_ms: 1000,
        level: 0.8,
        bass: 0.9,
        mid: 0.5,
        treble: 0.3,
        beat: true,
        beat_confidence: 0.95,
        bpm: Some(120.0),
        bins: vec![0.1, 0.2, 0.3],
    };
    let cloned = snap.clone();
    assert_eq!(cloned.timestamp_ms, 1000);
    assert_eq!(cloned.bins.len(), 3);
    assert_eq!(cloned.bpm, Some(120.0));
}

#[test]
fn canvas_preview_state_captures_frame_metadata_without_pixels() {
    let frame = CanvasFrame {
        frame_number: 42,
        timestamp_ms: 1337,
        width: 320,
        height: 200,
        pixels: vec![1, 2, 3, 4, 5, 6],
    };

    let preview = CanvasPreviewState::from(&frame);

    assert_eq!(
        preview,
        CanvasPreviewState {
            frame_number: 42,
            timestamp_ms: 1337,
            width: 320,
            height: 200,
        }
    );
}
