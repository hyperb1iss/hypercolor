use hypercolor_ui::ws::fps::FpsTarget;
use serde_json::json;

#[test]
fn event_admission_heals_the_hello_gap_and_rejects_older_queued_events() {
    let mut fps = FpsTarget::default();
    assert_eq!(
        fps.observe(&json!({
            "type": "hello", "capabilities": ["fps_events"], "state": { "fps": { "target": 60 } }
        })),
        Some(60)
    );
    assert_eq!(
        fps.observe(&json!({
            "type": "subscribed", "fps": { "target": 30, "revision": 2 }
        })),
        Some(30)
    );
    assert_eq!(
        fps.observe(&json!({
            "type": "event", "event": "fps_changed", "data": { "new_target": 45, "revision": 1 }
        })),
        None
    );
    assert_eq!(
        fps.observe(&json!({
            "type": "metrics", "data": { "fps": { "target": 60 } }
        })),
        None,
        "diagnostics cannot overwrite the authoritative target"
    );
    assert_eq!(
        fps.observe(&json!({
            "type": "event", "event": "fps_changed", "data": { "new_target": 60, "revision": 3 }
        })),
        Some(60)
    );
}

#[test]
fn a_newer_event_wins_even_if_the_subscription_snapshot_arrives_later() {
    let mut fps = FpsTarget::default();
    fps.observe(&json!({ "type": "hello", "capabilities": ["fps_events"] }));
    assert_eq!(
        fps.observe(&json!({
            "type": "event", "event": "fps_changed", "data": { "new_target": 45, "revision": 9 }
        })),
        Some(45)
    );
    for revision in [8, 9] {
        assert_eq!(
            fps.observe(&json!({
                "type": "subscribed", "fps": { "target": 60, "revision": revision }
            })),
            None
        );
    }
}

#[test]
fn a_new_connection_accepts_revision_zero_and_older_installations_use_metrics() {
    let mut fps = FpsTarget::default();
    for _ in 0..2 {
        fps.observe(&json!({ "type": "hello", "capabilities": ["fps_events"] }));
        assert_eq!(
            fps.observe(&json!({
                "type": "subscribed", "fps": { "target": 60, "revision": 0 }
            })),
            Some(60)
        );
    }
    fps.observe(&json!({ "type": "hello", "capabilities": ["metrics"] }));
    assert_eq!(
        fps.observe(&json!({
            "type": "metrics", "data": { "fps": { "target": 20 } }
        })),
        Some(20)
    );
}

#[test]
fn malformed_target_does_not_spend_a_revision() {
    let mut fps = FpsTarget::default();
    fps.observe(&json!({ "type": "hello", "capabilities": ["fps_events"] }));
    for target in [
        json!(0),
        json!(-1),
        json!(1.5),
        json!(u64::MAX),
        json!("30"),
    ] {
        assert_eq!(
            fps.observe(&json!({
                "type": "subscribed", "fps": { "target": target, "revision": 1 }
            })),
            None
        );
    }
    assert_eq!(
        fps.observe(&json!({
            "type": "subscribed", "fps": { "target": 30, "revision": 1 }
        })),
        Some(30)
    );
}
