use hypercolor_types::api::effects::EffectSourceKind;
use hypercolor_types::effect::EffectCategory;
use hypercolor_ui::api::{EffectCapabilitySet, EffectSummary};
use hypercolor_ui::components::canvas_preview::{
    canonical_injection_key, effect_wants_interaction, normalized_canvas_position,
    wheel_scroll_edge,
};
use hypercolor_ui::ws::interactive_preview::{
    InteractivePreviewLifecycle, InteractivePreviewLifecycleTracker,
    InteractivePreviewServerUpdate, close_message, closed_previews, input_inject_message,
    open_message, server_updates,
};
use hypercolor_ui::ws::messages::interactive_preview_supported;
use hypercolor_ui::ws::{
    InputEdgeButton, InputEdgeScrollPhase, InputEdgeScrollUnit, InputEdgeState, InputInjectEdge,
    InteractivePreviewRequest,
};

fn summary(input_reactive: bool, category: EffectCategory, tags: &[&str]) -> EffectSummary {
    EffectSummary {
        id: "fx".to_owned(),
        name: "Fx".to_owned(),
        description: String::new(),
        author: String::new(),
        category,
        source: EffectSourceKind::Html,
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
        controls: None,
        presets: None,
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
        InputInjectEdge::Scroll {
            delta_x_q16_16: 98_304,
            delta_y_q16_16: -131_072,
            unit: InputEdgeScrollUnit::Pixels,
            phase: InputEdgeScrollPhase::Changed,
            momentum_phase: InputEdgeScrollPhase::Began,
        },
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
                {
                    "kind": "scroll",
                    "delta_x_q16_16": 98304,
                    "delta_y_q16_16": -131072,
                    "unit": "pixels",
                    "phase": "changed",
                    "momentum_phase": "began"
                },
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
            "type": "subscribe",
            "topics": [{
                "topic": "interactive_preview",
                "key": "main",
                "config": {
                    "target": "active_scene",
                    "fps": 30,
                    "width": 640,
                    "height": 480,
                    "format": "jpeg",
                }
            }]
        })
    );
    assert_eq!(
        close_message("main"),
        serde_json::json!({
            "type": "unsubscribe",
            "topics": [{ "topic": "interactive_preview", "key": "main" }]
        })
    );
}

#[test]
fn an_acknowledgment_reports_open_and_closed_previews_together() {
    let ack = serde_json::json!({
        "type": "subscribed",
        "topics": [
            { "topic": "events" },
            {
                "topic": "interactive_preview",
                "key": "main",
                "config": { "fps": 30 },
                "publication_id": 11
            }
        ]
    });

    assert_eq!(
        server_updates(&ack),
        vec![InteractivePreviewServerUpdate::Opened {
            preview_id: "main".to_owned(),
            publication_id: 11,
        }]
    );
    // "inspector" is absent from the live set, so it has closed.
    assert_eq!(
        closed_previews(&ack, &["main".to_owned(), "inspector".to_owned()]),
        vec![InteractivePreviewServerUpdate::Closed {
            preview_id: "inspector".to_owned(),
        }]
    );
    assert!(closed_previews(&ack, &["main".to_owned()]).is_empty());

    // A publication id of zero is not an open preview.
    assert!(
        server_updates(&serde_json::json!({
            "type": "subscribed",
            "topics": [{ "topic": "interactive_preview", "key": "main", "publication_id": 0 }]
        }))
        .is_empty()
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
fn interactive_preview_lifecycle_fences_rapid_reopen_until_latest_ack() {
    let mut tracker = InteractivePreviewLifecycleTracker::default();
    tracker.request_open("main");
    tracker.request_close("main");
    tracker.request_open("main");

    tracker.apply(InteractivePreviewServerUpdate::Opened {
        preview_id: "main".to_owned(),
        publication_id: 11,
    });
    assert_eq!(
        tracker.lifecycles().get("main"),
        Some(&InteractivePreviewLifecycle::Requested)
    );
    tracker.apply(InteractivePreviewServerUpdate::Closed {
        preview_id: "main".to_owned(),
    });
    assert_eq!(
        tracker.lifecycles().get("main"),
        Some(&InteractivePreviewLifecycle::Requested)
    );
    tracker.apply(InteractivePreviewServerUpdate::Opened {
        preview_id: "main".to_owned(),
        publication_id: 12,
    });
    assert_eq!(
        tracker.lifecycles().get("main"),
        Some(&InteractivePreviewLifecycle::Opened { publication_id: 12 })
    );

    tracker.clear();
    assert!(tracker.lifecycles().is_empty());
}

#[test]
fn interactive_preview_rejection_is_addressed_and_terminal() {
    let updates = server_updates(&serde_json::json!({
        "type": "error",
        "code": "unavailable",
        "details": { "preview_id": "main" },
    }));
    assert_eq!(updates.len(), 1, "an addressed error should parse");
    let mut tracker = InteractivePreviewLifecycleTracker::default();
    tracker.request_open("main");
    tracker.apply(updates.into_iter().next().expect("one update"));
    assert_eq!(
        tracker.lifecycles().get("main"),
        Some(&InteractivePreviewLifecycle::Rejected)
    );
    assert!(
        server_updates(&serde_json::json!({
            "type": "error",
            "details": { "preview_id": "" },
        }))
        .is_empty()
    );
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
fn wheel_deltas_preserve_axes_and_dom_units() {
    assert_eq!(
        wheel_scroll_edge(12.5, 100.0, 0),
        Some(InputInjectEdge::Scroll {
            delta_x_q16_16: -819_200,
            delta_y_q16_16: -6_553_600,
            unit: InputEdgeScrollUnit::Pixels,
            phase: InputEdgeScrollPhase::None,
            momentum_phase: InputEdgeScrollPhase::None,
        })
    );
    assert_eq!(
        wheel_scroll_edge(0.0, 3.0, 1),
        Some(InputInjectEdge::Scroll {
            delta_x_q16_16: 0,
            delta_y_q16_16: -9_437_184,
            unit: InputEdgeScrollUnit::Line120,
            phase: InputEdgeScrollPhase::None,
            momentum_phase: InputEdgeScrollPhase::None,
        })
    );
    assert_eq!(
        wheel_scroll_edge(1.0, -0.5, 2),
        Some(InputInjectEdge::Scroll {
            delta_x_q16_16: -26_214_400,
            delta_y_q16_16: 13_107_200,
            unit: InputEdgeScrollUnit::Pixels,
            phase: InputEdgeScrollPhase::None,
            momentum_phase: InputEdgeScrollPhase::None,
        })
    );
    assert_eq!(wheel_scroll_edge(0.0, 0.0, 0), None);
    assert_eq!(wheel_scroll_edge(f64::NAN, 1.0, 0), None);
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
    assert!(effect_wants_interaction(&summary(
        true,
        EffectCategory::Ambient,
        &[]
    )));
    assert!(!effect_wants_interaction(&summary(
        false,
        EffectCategory::Interactive,
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
