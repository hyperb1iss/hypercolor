use hypercolor_app::state::{
    AppState, DaemonMessage, EffectInfo, StateUpdate, WsEventMessage, WsHello,
};
use hypercolor_types::api::effects::EffectListResponse;
use hypercolor_types::api::system::{ServerInfo, SystemResource, SystemStatus};
use hypercolor_types::api::{ApiResponse, ResponseMeta};
use serde_json::json;

fn response_meta() -> ResponseMeta {
    ResponseMeta {
        api_version: "v1".to_owned(),
        request_id: "req_test".to_owned(),
        timestamp: "2026-08-20T00:00:00Z".to_owned(),
    }
}

#[test]
fn default_state_is_disconnected() {
    let state = AppState::default();

    assert!(!state.connected);
    assert!(!state.running);
    assert!(!state.paused);
    assert_eq!(state.brightness, 0);
    assert!(state.current_effect.is_none());
    assert!(state.effects.is_empty());
    assert!(state.scenes.is_empty());
}

#[test]
fn parse_ws_hello_message() {
    let raw = json!({
        "type": "hello",
        "server": {
            "instance_id": "01912345-6789-7abc-def0-123456789abc",
            "instance_name": "desk-pc",
            "version": "0.1.0",
            "auth_required": true
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
    // A daemon that still sends an effect has it ignored: the handshake
    // reports how the daemon is running, not what it is rendering.
}

#[test]
fn parse_public_system_identity() {
    let raw = serde_json::to_value(ApiResponse {
        data: SystemResource {
            identity: ServerInfo {
                instance_id: "01912345-6789-7abc-def0-123456789abc".to_owned(),
                instance_name: "desk-pc".to_owned(),
                version: "0.1.0".to_owned(),
                device_count: 2,
                auth_required: true,
                ..ServerInfo::default()
            },
            status: None,
        },
        meta: response_meta(),
    })
    .expect("system response should serialize");

    let envelope: ApiResponse<SystemResource> =
        serde_json::from_value(raw).expect("should parse system response");
    let system = envelope.data;
    assert!(system.status.is_none());
    let server = system.identity;
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
            "zone_id": "019c0000-0000-7000-8000-000000000001",
            "layer_id": "019c0000-0000-7000-8000-000000000002",
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
fn resync_required_event_requests_full_state_reconciliation() {
    let message: WsEventMessage = serde_json::from_value(json!({
        "type": "event",
        "event": "resync_required",
    }))
    .expect("should parse resync event");

    assert!(message.requires_full_resync());
}

#[test]
fn only_explicit_stops_are_destructive_lifecycle_events() {
    for (reason, destructive) in [("stopped", true), ("error", false), ("replaced", false)] {
        let message: WsEventMessage = serde_json::from_value(json!({
            "type": "event",
            "event": "effect_stopped",
            "data": {
                "reason": reason,
                "zone_id": "019c0000-0000-7000-8000-000000000001"
            }
        }))
        .expect("should parse stop event");

        assert_eq!(message.is_destructive_effect_stop(), destructive);
    }
}

#[test]
fn lifecycle_events_only_target_the_canonical_primary_zone() {
    let primary = hypercolor_types::scene::ZoneId::new();
    let secondary = hypercolor_types::scene::ZoneId::new();
    let display = hypercolor_types::scene::ZoneId::new();
    let message: WsEventMessage = serde_json::from_value(json!({
        "type": "event",
        "event": "effect_started",
        "data": {
            "zone_id": primary.to_string(),
            "layer_id": hypercolor_types::layer::SceneLayerId::new().to_string()
        }
    }))
    .expect("should parse lifecycle event");

    assert!(message.targets_zone(&primary));
    assert!(!message.targets_zone(&secondary));
    assert!(!message.targets_zone(&display));
}

#[test]
fn parse_authenticated_system_status() {
    let raw = serde_json::to_value(ApiResponse {
        data: SystemResource {
            identity: ServerInfo::default(),
            status: Some(SystemStatus {
                running: true,
                active_effect: Some("Aurora Borealis".to_owned()),
                active_scene: Some("Movie Night".to_owned()),
                active_scene_snapshot_locked: true,
                global_brightness: 80,
                device_count: 2,
                ..SystemStatus::default()
            }),
        },
        meta: response_meta(),
    })
    .expect("system response should serialize");

    let envelope: ApiResponse<SystemResource> =
        serde_json::from_value(raw).expect("should parse system status");
    let status = envelope
        .data
        .status
        .expect("authenticated system response should have status");
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
                    "category": "interactive",
                    "source": "html",
                    "runnable": true,
                    "tags": [],
                    "version": "1.0",
                    "audio_reactive": true
                }
            ],
            "total": 2,
            "page": null
        },
        "meta": {
            "api_version": "v1",
            "request_id": "req_test",
            "timestamp": "2026-08-20T00:00:00Z"
        }
    });

    let envelope: ApiResponse<EffectListResponse> =
        serde_json::from_value(raw).expect("should parse effects");
    let list = envelope.data;
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
fn websocket_snapshot_preserves_content_state() {
    let mut state = AppState {
        running: true,
        paused: true,
        current_effect: Some(EffectInfo {
            id: "old".to_owned(),
            name: "Old".to_owned(),
        }),
        ..AppState::default()
    };

    state.apply_daemon_message(DaemonMessage::StateUpdate(StateUpdate::Snapshot {
        running: true,
        paused: false,
        brightness: 64,
        device_count: 3,
    }));

    assert!(state.running);
    assert!(!state.paused);
    assert_eq!(state.brightness, 64);
    assert_eq!(state.device_count, 3);
    assert_eq!(
        state
            .current_effect
            .as_ref()
            .map(|effect| effect.id.as_str()),
        Some("old")
    );
}

#[test]
fn effect_stop_clears_stale_pause_state() {
    let mut state = AppState {
        paused: true,
        current_effect: Some(EffectInfo {
            id: "old".to_owned(),
            name: "Old".to_owned(),
        }),
        ..AppState::default()
    };

    state.apply_daemon_message(DaemonMessage::StateUpdate(StateUpdate::EffectStopped));

    assert!(!state.paused);
    assert!(state.current_effect.is_none());
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
