use hypercolor_app::state::{
    ApiEnvelope, AppState, DaemonMessage, EffectInfo, EffectListResponse, ServerResponse,
    StateUpdate, StatusResponse, WsEventMessage, WsHello,
};
use serde_json::json;

#[test]
fn default_state_is_disconnected() {
    let state = AppState::default();

    assert!(!state.connected);
    assert!(!state.running);
    assert!(!state.paused);
    assert_eq!(state.brightness, 0);
    assert!(state.current_effect.is_none());
    assert!(state.effects.is_empty());
    assert!(state.profiles.is_empty());
}

#[test]
fn parse_ws_hello_message() {
    let raw = json!({
        "type": "hello",
        "server": {
            "instance_id": "01912345-6789-7abc-def0-123456789abc",
            "instance_name": "desk-pc",
            "version": "0.1.0"
        },
        "version": "1.0",
        "state": {
            "running": true,
            "paused": false,
            "brightness": 75,
            "fps": { "target": 60, "actual": 59.8 },
            "effect": { "id": "abc-123", "name": "Aurora Borealis" },
            "device_count": 3,
            "total_leds": 180
        },
        "capabilities": ["events", "frames", "spectrum"],
        "subscriptions": ["events"]
    });

    let hello: WsHello = serde_json::from_value(raw).expect("should parse hello");
    assert_eq!(hello.msg_type, "hello");
    let server = hello.server.expect("hello should include server metadata");
    assert_eq!(server.instance_name, "desk-pc");
    assert_eq!(server.version, "0.1.0");

    let state = hello.state.expect("hello should have state");
    assert!(state.running);
    assert!(!state.paused);
    assert_eq!(state.brightness, 75);
    assert_eq!(state.device_count, 3);

    let effect = state.effect.expect("should have active effect");
    assert_eq!(effect.id, "abc-123");
    assert_eq!(effect.name, "Aurora Borealis");
}

#[test]
fn parse_server_response() {
    let raw = json!({
        "data": {
            "instance_id": "01912345-6789-7abc-def0-123456789abc",
            "instance_name": "desk-pc",
            "version": "0.1.0",
            "device_count": 2,
            "auth_required": true
        }
    });

    let envelope: ApiEnvelope<ServerResponse> =
        serde_json::from_value(raw).expect("should parse server response");
    let server = envelope.data.expect("should have data");
    assert_eq!(server.instance_id, "01912345-6789-7abc-def0-123456789abc");
    assert_eq!(server.instance_name, "desk-pc");
    assert_eq!(server.version, "0.1.0");
}

#[test]
fn parse_ws_event_effect_started() {
    let raw = json!({
        "type": "event",
        "event": "effect_started",
        "timestamp": "2026-03-10T12:00:00Z",
        "data": {
            "effect": {
                "id": "def-456",
                "name": "Cosmic Wave",
                "engine": "native"
            },
            "trigger": "api",
            "previous": null,
            "transition": null
        }
    });

    let msg: WsEventMessage = serde_json::from_value(raw).expect("should parse event");
    assert_eq!(msg.msg_type, "event");
    assert_eq!(msg.event, "effect_started");
    assert_eq!(msg.data["effect"]["id"], "def-456");
    assert_eq!(msg.data["effect"]["name"], "Cosmic Wave");
}

#[test]
fn parse_status_response() {
    let raw = json!({
        "data": {
            "running": true,
            "version": "0.1.0",
            "config_path": "/home/user/.config/hypercolor/hypercolor.toml",
            "data_dir": "/home/user/.local/share/hypercolor",
            "cache_dir": "/home/user/.cache/hypercolor",
            "uptime_seconds": 3600,
            "device_count": 2,
            "effect_count": 15,
            "scene_count": 3,
            "active_effect": "Aurora Borealis",
            "active_scene": "Movie Night",
            "active_scene_snapshot_locked": true,
            "global_brightness": 80,
            "audio_available": true,
            "capture_available": false,
            "render_loop": {
                "state": "running",
                "fps_tier": "standard",
                "total_frames": 216_000
            },
            "event_bus_subscribers": 1
        }
    });

    let envelope: ApiEnvelope<StatusResponse> =
        serde_json::from_value(raw).expect("should parse status");
    let status = envelope.data.expect("should have data");
    assert!(status.running);
    assert_eq!(status.active_effect.as_deref(), Some("Aurora Borealis"));
    assert_eq!(status.active_scene.as_deref(), Some("Movie Night"));
    assert!(status.active_scene_snapshot_locked);
    assert_eq!(status.global_brightness, 80);
    assert_eq!(status.device_count, 2);
}

#[test]
fn parse_effect_list_response() {
    let raw = json!({
        "data": {
            "items": [
                {
                    "id": "aaa",
                    "name": "Effect A",
                    "description": "",
                    "author": "",
                    "category": "ambient",
                    "source": "native",
                    "runnable": true,
                    "tags": [],
                    "version": "1.0",
                    "audio_reactive": false
                },
                {
                    "id": "bbb",
                    "name": "Effect B",
                    "description": "",
                    "author": "",
                    "category": "reactive",
                    "source": "html",
                    "runnable": true,
                    "tags": [],
                    "version": "1.0",
                    "audio_reactive": true
                }
            ],
            "pagination": { "offset": 0, "limit": 50, "total": 2, "has_more": false }
        }
    });

    let envelope: ApiEnvelope<EffectListResponse> =
        serde_json::from_value(raw).expect("should parse effects");
    let list = envelope.data.expect("should have data");
    assert_eq!(list.items.len(), 2);
    assert_eq!(list.items[0].name, "Effect A");
    assert_eq!(list.items[1].id, "bbb");
}

#[test]
fn state_update_applies_dynamic_tray_changes() {
    let mut state = AppState::default();

    state.apply_daemon_message(DaemonMessage::Connected(AppState {
        connected: true,
        running: true,
        brightness: 50,
        ..AppState::default()
    }));
    state.apply_daemon_message(DaemonMessage::StateUpdate(StateUpdate::EffectChanged {
        id: "aurora".to_owned(),
        name: "Aurora".to_owned(),
    }));
    state.apply_daemon_message(DaemonMessage::StateUpdate(StateUpdate::BrightnessChanged(
        80,
    )));
    state.apply_daemon_message(DaemonMessage::StateUpdate(StateUpdate::EffectsRefreshed(
        vec![EffectInfo {
            id: "wave".to_owned(),
            name: "Wave".to_owned(),
        }],
    )));

    assert!(state.connected);
    assert!(!state.paused);
    assert_eq!(state.brightness, 80);
    assert_eq!(
        state
            .current_effect
            .as_ref()
            .map(|effect| effect.id.as_str()),
        Some("aurora")
    );
    assert_eq!(state.effects[0].name, "Wave");
}

#[test]
fn disconnected_message_preserves_discovered_servers() {
    let mut state = AppState {
        connected: true,
        running: true,
        active_server: Some(0),
        device_count: 4,
        servers: Vec::new(),
        ..AppState::default()
    };

    state.apply_daemon_message(DaemonMessage::Disconnected);

    assert!(!state.connected);
    assert!(!state.running);
    assert_eq!(state.device_count, 0);
    assert_eq!(state.active_server, None);
}
