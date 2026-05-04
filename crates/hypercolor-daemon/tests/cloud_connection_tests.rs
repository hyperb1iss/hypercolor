#![cfg(feature = "cloud")]

use hypercolor_cloud_client::daemon_link::WelcomeFrame;
use hypercolor_daemon::cloud_connection::{CloudConnectionRuntime, CloudConnectionRuntimeState};

#[test]
fn cloud_connection_runtime_records_transitions_and_clears_stale_channels() {
    let mut runtime = CloudConnectionRuntime::default();

    let snapshot = runtime.snapshot();
    assert_eq!(snapshot.runtime_state, CloudConnectionRuntimeState::Idle);
    assert!(!snapshot.connected);

    runtime.mark_connecting();
    let snapshot = runtime.snapshot();
    assert_eq!(
        snapshot.runtime_state,
        CloudConnectionRuntimeState::Connecting
    );
    assert!(!snapshot.connected);
    assert!(snapshot.session_id.is_none());

    runtime.mark_connected(&welcome_fixture());
    let snapshot = runtime.snapshot();
    assert_eq!(
        snapshot.runtime_state,
        CloudConnectionRuntimeState::Connected
    );
    assert!(snapshot.connected);
    assert_eq!(
        snapshot.available_channels,
        vec!["control", "sync.notifications"]
    );
    assert_eq!(snapshot.denied_channels[0].name, "relay.ws");
    assert_eq!(snapshot.denied_channels[0].reason, "entitlement_missing");

    runtime.mark_backoff("cloud unavailable");
    let snapshot = runtime.snapshot();
    assert_eq!(snapshot.runtime_state, CloudConnectionRuntimeState::Backoff);
    assert!(!snapshot.connected);
    assert!(snapshot.session_id.is_none());
    assert!(snapshot.available_channels.is_empty());
    assert!(snapshot.denied_channels.is_empty());
    assert_eq!(snapshot.last_error.as_deref(), Some("cloud unavailable"));

    runtime.mark_idle();
    let snapshot = runtime.snapshot();
    assert_eq!(snapshot.runtime_state, CloudConnectionRuntimeState::Idle);
    assert!(snapshot.last_error.is_none());
}

fn welcome_fixture() -> WelcomeFrame {
    serde_json::from_value(serde_json::json!({
        "session_id": "00000000000000000000000000",
        "available_channels": ["control", "sync.notifications"],
        "denied_channels": [
            {
                "name": "relay.ws",
                "reason": "entitlement_missing",
                "feature": "hc.remote"
            }
        ],
        "server_capabilities": {
            "tunnel_resume": false,
            "compression": [],
            "max_frame_bytes": 65536
        },
        "heartbeat_interval_s": 25
    }))
    .expect("welcome fixture should deserialize")
}
