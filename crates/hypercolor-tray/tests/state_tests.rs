//! Smoke tests for the tray applet state types.
//!
//! Verifies that the state management types compile correctly and that
//! basic operations (default construction, state updates) work as expected.

// The tray crate is a binary, so we test the types via inline module paths.
// Since the types are defined in library-style modules, we replicate the
// key structures here to verify serialization and construction.

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

/// Mirrors the daemon WebSocket event message format.
#[derive(Debug, serde::Deserialize)]
struct WsEventMessage {
    #[serde(rename = "type")]
    msg_type: String,
    #[serde(default)]
    event: String,
    #[serde(default)]
    data: serde_json::Value,
}

/// Mirrors the daemon WebSocket hello message format.
#[derive(Debug, serde::Deserialize)]
struct WsHello {
    #[serde(rename = "type")]
    msg_type: String,
    #[serde(default)]
    server: Option<ServerIdentity>,
    state: Option<WsHelloState>,
}

#[derive(Debug, serde::Deserialize)]
struct ServerIdentity {
    #[expect(
        dead_code,
        reason = "deserialized for structural completeness, asserted via Debug"
    )]
    instance_id: String,
    instance_name: String,
    version: String,
}

#[derive(Debug, serde::Deserialize)]
struct WsHelloState {
    running: bool,
    paused: bool,
    brightness: u8,
    device_count: usize,
    effect: Option<WsNameRef>,
}

#[derive(Debug, serde::Deserialize)]
struct WsNameRef {
    id: String,
    name: String,
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

    let effect = state.effect.expect("should have active effect");
    assert_eq!(effect.id, "abc-123");
    assert_eq!(effect.name, "Aurora Borealis");
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
    assert!(server.auth_required);
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
fn parse_ws_event_effect_changed() {
    let raw = json!({
        "type": "event",
        "event": "effect_changed",
        "timestamp": "2026-03-10T12:00:00Z",
        "data": {
            "previous": { "id": "abc-123", "name": "Aurora Borealis" },
            "current": { "id": "def-456", "name": "Cosmic Wave" },
            "trigger": "api"
        }
    });

    let msg: WsEventMessage = serde_json::from_value(raw).expect("should parse event");
    assert_eq!(msg.msg_type, "event");
    assert_eq!(msg.event, "effect_changed");
    assert_eq!(msg.data["current"]["id"], "def-456");
    assert_eq!(msg.data["current"]["name"], "Cosmic Wave");
}

#[test]
fn parse_ws_event_device_connected() {
    let raw = json!({
        "type": "event",
        "event": "device_connected",
        "timestamp": "2026-03-10T12:00:00Z",
        "data": {
            "device_id": "razer-blackwidow-v4-001",
            "name": "Razer BlackWidow V4",
            "backend": "razer",
            "led_count": 126
        }
    });

    let msg: WsEventMessage = serde_json::from_value(raw).expect("should parse event");
    assert_eq!(msg.event, "device_connected");
    assert_eq!(msg.data["device_id"], "razer-blackwidow-v4-001");
    assert_eq!(msg.data["led_count"], 126);
}

#[test]
fn parse_ws_event_brightness_changed() {
    let raw = json!({
        "type": "event",
        "event": "brightness_changed",
        "timestamp": "2026-03-10T12:00:00Z",
        "data": {
            "old": 75,
            "new_value": 50
        }
    });

    let msg: WsEventMessage = serde_json::from_value(raw).expect("should parse event");
    assert_eq!(msg.event, "brightness_changed");

    let new_value = msg.data["new_value"]
        .as_u64()
        .expect("should have new_value");
    assert_eq!(new_value, 50);
}

#[test]
fn parse_ws_event_paused() {
    let raw = json!({
        "type": "event",
        "event": "paused",
        "timestamp": "2026-03-10T12:00:00Z",
        "data": {}
    });

    let msg: WsEventMessage = serde_json::from_value(raw).expect("should parse event");
    assert_eq!(msg.event, "paused");
}

#[test]
fn parse_ws_event_active_scene_changed() {
    let raw = json!({
        "type": "event",
        "event": "active_scene_changed",
        "timestamp": "2026-04-13T12:00:00Z",
        "data": {
            "previous": "default",
            "current": "scene_movie_night",
            "reason": "user_activate"
        }
    });

    let msg: WsEventMessage = serde_json::from_value(raw).expect("should parse event");
    assert_eq!(msg.event, "active_scene_changed");
    assert_eq!(msg.data["current"], "scene_movie_night");
    assert_eq!(msg.data["reason"], "user_activate");
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
                { "id": "aaa", "name": "Effect A", "description": "", "author": "", "category": "ambient", "source": "native", "runnable": true, "tags": [], "version": "1.0", "audio_reactive": false },
                { "id": "bbb", "name": "Effect B", "description": "", "author": "", "category": "interactive", "source": "html", "runnable": true, "tags": [], "version": "1.0", "audio_reactive": true }
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
