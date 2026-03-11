//! Smoke tests for the tray applet state types.
//!
//! Verifies that the state management types compile correctly and that
//! basic operations (default construction, state updates) work as expected.

// The tray crate is a binary, so we test the types via inline module paths.
// Since the types are defined in library-style modules, we replicate the
// key structures here to verify serialization and construction.

use serde_json::json;

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

/// Mirrors the daemon status API envelope.
#[derive(Debug, serde::Deserialize)]
struct ApiEnvelope<T> {
    data: Option<T>,
}

#[derive(Debug, serde::Deserialize)]
struct StatusResponse {
    running: bool,
    active_effect: Option<String>,
    global_brightness: u8,
    device_count: usize,
}

#[derive(Debug, serde::Deserialize)]
struct ServerResponse {
    instance_id: String,
    instance_name: String,
    version: String,
    auth_required: bool,
}

#[derive(Debug, serde::Deserialize)]
struct EffectListResponse {
    items: Vec<EffectSummary>,
}

#[derive(Debug, serde::Deserialize)]
struct EffectSummary {
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
    assert_eq!(status.global_brightness, 80);
    assert_eq!(status.device_count, 2);
}

#[test]
fn parse_effect_list_response() {
    let raw = json!({
        "data": {
            "items": [
                { "id": "aaa", "name": "Effect A", "description": "", "author": "", "category": "ambient", "source": "native", "runnable": true, "tags": [], "version": "1.0", "audio_reactive": false },
                { "id": "bbb", "name": "Effect B", "description": "", "author": "", "category": "reactive", "source": "html", "runnable": true, "tags": [], "version": "1.0", "audio_reactive": true }
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
