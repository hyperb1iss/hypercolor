use hypercolor_ui::api::{EffectCapabilitySet, EffectSummary};
use hypercolor_ui::components::canvas_preview::{
    canonical_injection_key, effect_wants_interaction, normalized_canvas_position,
    wheel_delta_hi_res,
};
use hypercolor_ui::ws::interactive_preview::{close_message, input_inject_message, open_message};
use hypercolor_ui::ws::messages::interactive_preview_supported;
use hypercolor_ui::ws::{
    InputEdgeButton, InputEdgeState, InputInjectEdge, InteractivePreviewRequest,
};

fn summary(input_reactive: bool, category: &str, tags: &[&str]) -> EffectSummary {
    EffectSummary {
        id: "fx".to_owned(),
        name: "Fx".to_owned(),
        description: String::new(),
        author: String::new(),
        category: category.to_owned(),
        source: "html".to_owned(),
        runnable: true,
        tags: tags.iter().map(|tag| (*tag).to_owned()).collect(),
        version: "1.0.0".to_owned(),
        audio_reactive: false,
        input_reactive,
        capabilities: EffectCapabilitySet {
            input_reactive,
            ..EffectCapabilitySet::default()
        },
        cover_image_url: None,
    }
}

#[test]
fn edges_serialize_to_daemon_wire_shape() {
    let edges = vec![
        InputInjectEdge::Key {
            key: "a".to_owned(),
            state: InputEdgeState::Pressed,
        },
        InputInjectEdge::Button {
            button: InputEdgeButton::Left,
            state: InputEdgeState::Released,
        },
        InputInjectEdge::Move { nx: 0.25, ny: 1.0 },
        InputInjectEdge::Wheel { delta_hi_res: -120 },
    ];
    let message = input_inject_message("main", &edges);
    assert_eq!(
        message,
        serde_json::json!({
            "type": "input_inject",
            "preview_id": "main",
            "events": [
                { "kind": "key", "key": "a", "state": "pressed" },
                { "kind": "button", "button": "left", "state": "released" },
                { "kind": "move", "nx": 0.25, "ny": 1.0 },
                { "kind": "wheel", "delta_hi_res": -120 },
            ],
        })
    );
}

#[test]
fn interactive_preview_control_messages_are_addressed() {
    let request = InteractivePreviewRequest {
        preview_id: "main".to_owned(),
        fps: 30,
        width: 640,
        height: 480,
    };
    assert_eq!(
        open_message(&request),
        serde_json::json!({
            "type": "interactive_preview_open",
            "preview_id": "main",
            "target": "active_scene",
            "fps": 30,
            "width": 640,
            "height": 480,
            "format": "jpeg",
        })
    );
    assert_eq!(
        close_message("main"),
        serde_json::json!({
            "type": "interactive_preview_close",
            "preview_id": "main",
        })
    );
}

#[test]
fn interactive_preview_requires_explicit_server_capability() {
    assert!(interactive_preview_supported(&serde_json::json!({
        "type": "hello",
        "capabilities": ["events", "interactive_previews"],
    })));
    assert!(!interactive_preview_supported(&serde_json::json!({
        "type": "hello",
        "capabilities": ["events"],
    })));
    assert!(!interactive_preview_supported(&serde_json::json!({
        "type": "event",
        "capabilities": ["interactive_previews"],
    })));
}

#[test]
fn key_state_serializes_all_variants() {
    for (state, expected) in [
        (InputEdgeState::Pressed, "pressed"),
        (InputEdgeState::Released, "released"),
        (InputEdgeState::Repeated, "repeated"),
    ] {
        assert_eq!(
            serde_json::to_value(state).expect("state serializes"),
            serde_json::json!(expected)
        );
    }
}

#[test]
fn pointer_buttons_map_to_wire_names() {
    assert_eq!(
        InputEdgeButton::from_pointer_button(0),
        Some(InputEdgeButton::Left)
    );
    assert_eq!(
        InputEdgeButton::from_pointer_button(1),
        Some(InputEdgeButton::Middle)
    );
    assert_eq!(
        InputEdgeButton::from_pointer_button(2),
        Some(InputEdgeButton::Right)
    );
    assert_eq!(InputEdgeButton::from_pointer_button(3), None);
    assert_eq!(InputEdgeButton::from_pointer_button(4), None);
}

#[test]
fn injection_keys_match_daemon_canonical_names() {
    assert_eq!(canonical_injection_key("KeyA"), Some("a".to_owned()));
    assert_eq!(canonical_injection_key("KeyZ"), Some("z".to_owned()));
    assert_eq!(canonical_injection_key("Digit0"), Some("0".to_owned()));
    assert_eq!(canonical_injection_key("Digit9"), Some("9".to_owned()));
    assert_eq!(canonical_injection_key("Space"), Some("Space".to_owned()));
    assert_eq!(
        canonical_injection_key("ArrowLeft"),
        Some("ArrowLeft".to_owned())
    );
    assert_eq!(
        canonical_injection_key("ShiftLeft"),
        Some("ShiftLeft".to_owned())
    );
    assert_eq!(canonical_injection_key("Minus"), Some("-".to_owned()));
    assert_eq!(canonical_injection_key("BracketLeft"), Some("[".to_owned()));
    assert_eq!(canonical_injection_key("Backslash"), Some("\\".to_owned()));
    assert_eq!(canonical_injection_key("Quote"), Some("'".to_owned()));
    assert_eq!(canonical_injection_key("Backquote"), Some("`".to_owned()));
    assert_eq!(canonical_injection_key("Slash"), Some("/".to_owned()));
    // Unknown named keys pass through untouched.
    assert_eq!(canonical_injection_key("F1"), Some("F1".to_owned()));
    assert_eq!(
        canonical_injection_key("Numpad1"),
        Some("Numpad1".to_owned())
    );
    assert_eq!(canonical_injection_key(""), None);
}

#[test]
fn wheel_deltas_scale_to_hi_res_notches() {
    // One standard pixel-mode notch (100px down) = -120 hi-res units.
    assert_eq!(wheel_delta_hi_res(100.0, 0), -120);
    assert_eq!(wheel_delta_hi_res(-100.0, 0), 120);
    // Firefox line mode: 3 lines per notch.
    assert_eq!(wheel_delta_hi_res(3.0, 1), -144);
    // Page mode scales through the page-height equivalent.
    assert_eq!(wheel_delta_hi_res(1.0, 2), -480);
    assert_eq!(wheel_delta_hi_res(0.0, 0), 0);
}

#[test]
fn normalized_positions_clamp_to_unit_square() {
    assert_eq!(
        normalized_canvas_position(50.0, 25.0, 0.0, 0.0, 100.0, 100.0),
        Some((0.5, 0.25))
    );
    // Outside the rect clamps rather than escaping [0, 1].
    assert_eq!(
        normalized_canvas_position(-10.0, 500.0, 0.0, 0.0, 100.0, 100.0),
        Some((0.0, 1.0))
    );
    // Offset rects subtract their origin.
    assert_eq!(
        normalized_canvas_position(150.0, 120.0, 100.0, 100.0, 200.0, 40.0),
        Some((0.25, 0.5))
    );
    // Degenerate rects (pre-layout) produce nothing.
    assert_eq!(
        normalized_canvas_position(10.0, 10.0, 0.0, 0.0, 0.0, 100.0),
        None
    );
}

#[test]
fn interaction_gate_uses_authoritative_capability() {
    assert!(effect_wants_interaction(&summary(true, "ambient", &[])));
    assert!(!effect_wants_interaction(&summary(
        false,
        "interactive",
        &["input", "mouse", "keyboard"]
    )));
}

#[test]
fn effect_summary_defaults_new_capabilities_for_older_payloads() {
    let summary: EffectSummary = serde_json::from_value(serde_json::json!({
        "id": "legacy",
        "name": "Legacy",
        "description": "",
        "author": "",
        "category": "interactive",
        "source": "html",
        "runnable": true,
        "tags": ["input"],
        "version": "1.0.0"
    }))
    .expect("older effect summary payload should deserialize");

    assert!(!summary.input_reactive);
    assert_eq!(summary.capabilities, EffectCapabilitySet::default());
}
