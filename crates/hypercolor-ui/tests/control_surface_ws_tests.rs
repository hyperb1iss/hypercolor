#![allow(dead_code, unused_imports)]

#[path = "../src/api/mod.rs"]
mod api;

#[path = "../src/ws/messages.rs"]
mod messages;

#[test]
fn extracts_control_action_progress_hint() {
    let data = serde_json::json!({
        "kind": "action_progress",
        "surface_id": "driver:wled:device:Desk Strip",
        "revision": 7,
        "action_id": "identify",
        "status": "running",
        "progress": 1.5
    });

    let hint = messages::extract_control_surface_event_hint("control_surface_changed", &data)
        .expect("control surface event hint");

    assert_eq!(hint.event_type, "control_surface_changed");
    assert_eq!(hint.kind, "action_progress");
    assert_eq!(hint.surface_id, "driver:wled:device:Desk Strip");
    assert_eq!(hint.revision, Some(7));
    assert_eq!(hint.action_id.as_deref(), Some("identify"));
    assert_eq!(hint.status.as_deref(), Some("running"));
    assert_eq!(hint.progress, Some(1.0));
}
