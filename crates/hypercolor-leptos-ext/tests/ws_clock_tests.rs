#![cfg(feature = "ws-core")]

use hypercolor_leptos_ext::ws::{HelloClock, PublicationStamp, ResponseStamp};

#[test]
fn publication_stamp_decodes_from_a_whole_event_message() {
    let wire = serde_json::json!({
        "type": "event",
        "event": "frame_rendered",
        "timestamp": "2027-01-15T08:00:00.000Z",
        "mono_us": 12_345_678,
        "data": { "frame_number": 8 }
    });

    let stamp: PublicationStamp = serde_json::from_value(wire).expect("decode stamp");
    assert_eq!(stamp.timestamp, "2027-01-15T08:00:00.000Z");
    assert_eq!(stamp.mono_us, 12_345_678);
}

#[test]
fn publication_stamp_round_trips_losslessly() {
    let stamp = PublicationStamp {
        timestamp: "2027-01-15T07:00:00.000Z".to_owned(),
        mono_us: u64::MAX,
    };

    let wire = serde_json::to_value(&stamp).expect("encode stamp");
    assert_eq!(
        wire,
        serde_json::json!({
            "timestamp": "2027-01-15T07:00:00.000Z",
            "mono_us": u64::MAX
        })
    );
    let decoded: PublicationStamp = serde_json::from_value(wire).expect("decode stamp");
    assert_eq!(decoded, stamp);
}

#[test]
fn publication_stamp_requires_the_monotonic_half() {
    let wall_only = serde_json::json!({
        "type": "metrics",
        "timestamp": "2027-01-15T08:00:00.000Z",
        "data": {}
    });

    assert!(serde_json::from_value::<PublicationStamp>(wall_only).is_err());
}

#[test]
fn hello_clock_decodes_from_a_whole_hello_and_round_trips() {
    let wire = serde_json::json!({
        "type": "hello",
        "version": "1.0",
        "clock_epoch": "6f1d3c1e-9a4b-4e0c-8a51-2b7f0c9d4e21",
        "mono_us": 42,
        "state": { "running": true },
        "capabilities": [],
        "subscriptions": []
    });

    let clock: HelloClock = serde_json::from_value(wire).expect("decode hello clock");
    assert_eq!(clock.clock_epoch, "6f1d3c1e-9a4b-4e0c-8a51-2b7f0c9d4e21");
    assert_eq!(clock.mono_us, 42);

    let encoded = serde_json::to_value(&clock).expect("encode hello clock");
    let decoded: HelloClock = serde_json::from_value(encoded).expect("decode hello clock");
    assert_eq!(decoded, clock);
}

#[test]
fn response_stamp_decodes_from_a_whole_response_and_round_trips() {
    let wire = serde_json::json!({
        "type": "response",
        "id": "cmd-7",
        "status": 200,
        "mono_us": 81_250_000,
        "data": {}
    });

    let stamp: ResponseStamp = serde_json::from_value(wire).expect("decode response stamp");
    assert_eq!(stamp.mono_us, Some(81_250_000));

    let encoded = serde_json::to_value(stamp).expect("encode response stamp");
    assert_eq!(encoded, serde_json::json!({ "mono_us": 81_250_000 }));
    let decoded: ResponseStamp = serde_json::from_value(encoded).expect("decode response stamp");
    assert_eq!(decoded, stamp);
}

#[test]
fn response_stamp_tolerates_a_daemon_without_it() {
    let wire = serde_json::json!({ "type": "response", "id": "cmd-7", "status": 200 });

    let stamp: ResponseStamp = serde_json::from_value(wire).expect("decode response stamp");
    assert_eq!(stamp.mono_us, None);
    assert_eq!(
        serde_json::to_value(stamp).expect("encode empty stamp"),
        serde_json::json!({})
    );
}
