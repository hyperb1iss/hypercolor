#![cfg(feature = "ws-core")]

use hypercolor_leptos_ext::ws::{INPUT_EVENT_PAYLOAD_SCHEMA, TimedInputEventPayload};

#[test]
fn timed_input_event_payload_round_trips_losslessly() {
    let wire = serde_json::json!({
        "event": {
            "kind": "key",
            "source_id": "host:keyboard-1",
            "key": "a",
            "state": "repeated"
        },
        "at_ms": 4_321,
        "seq": 91,
        "physical_code": "evdev:key:30",
        "repeat_count": 4,
        "future_metadata": {"generation": 7}
    });

    let decoded = TimedInputEventPayload::decode(&wire).expect("decode timed event");
    assert_eq!(INPUT_EVENT_PAYLOAD_SCHEMA, 1);
    assert_eq!(decoded.at_ms, 4_321);
    assert_eq!(decoded.seq, 91);
    assert_eq!(decoded.physical_code.as_deref(), Some("evdev:key:30"));
    assert_eq!(decoded.repeat_count, 4);
    assert_eq!(decoded.encode(), wire);
}

#[test]
fn timed_input_event_payload_accepts_prior_event_only_shape() {
    let legacy = serde_json::json!({
        "event": {
            "kind": "mouse_button",
            "source_id": "host:mouse-1",
            "button": "left",
            "state": "pressed"
        }
    });

    let decoded = TimedInputEventPayload::decode(&legacy).expect("decode legacy event");
    assert_eq!(decoded.at_ms, 0);
    assert_eq!(decoded.seq, 0);
    assert_eq!(decoded.physical_code, None);
    assert_eq!(decoded.repeat_count, 1);
}

#[test]
fn timed_input_event_payload_rejects_zero_repeat_multiplicity() {
    let invalid = serde_json::json!({
        "event": {"kind": "key"},
        "repeat_count": 0
    });

    assert!(TimedInputEventPayload::decode(&invalid).is_err());
}
