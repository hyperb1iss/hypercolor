//! Integration tests for the WebSocket protocol state machine.
//!
//! Complements the internal `src/api/ws/tests.rs` suite. Where the internal
//! tests exercise `pub(super)` relay/cache plumbing directly, this file drives
//! a real Axum server over a TCP socket using a hand-rolled minimal
//! RFC 6455 client. That lets us validate the end-to-end client-visible
//! wire format of the `/api/v1/ws` endpoint — the hello handshake,
//! subscription lifecycle, and protocol error responses — without touching
//! the internal module surface.
//!
//! The hand-rolled client only implements the subset we need:
//! text frames with 16-bit extended length, unmasked server frames,
//! masked client frames. No fragmentation, no binary parsing — our tests
//! never need to decode binary relay payloads, and any binary frame on the
//! wire is simply drained until we find a text frame.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use axum::Router;
use axum::extract::ws::Message;
use hypercolor_core::input::{
    DataSource, DataSourceKind, DataSourceRole, InputData, InputSource, ManagedSourceRole,
    SourceRoleBinding,
};
use hypercolor_daemon::api::local::{TrustedLocalApi, TrustedLocalWebSocket};
use hypercolor_daemon::api::{self, AppState};
use hypercolor_daemon::device_metrics::{DeviceMetrics, DeviceMetricsSnapshot};
use hypercolor_leptos_ext::ws::TimedInputEventPayload;
use hypercolor_types::effect::{EffectCategory, EffectId, EffectMetadata, EffectSource};
use hypercolor_types::event::{HypercolorEvent, InputButtonState, InputEvent, TimedInputEvent};
use hypercolor_types::library::PresetId;
use hypercolor_types::sensor::SystemSnapshot;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;
use uuid::Uuid;

// ── Test Harness ─────────────────────────────────────────────────────────

static TEST_DATA_DIR_COUNTER: AtomicUsize = AtomicUsize::new(0);

struct FixedSensorSource {
    snapshot: Arc<SystemSnapshot>,
    running: bool,
}

impl InputSource for FixedSensorSource {
    fn name(&self) -> &'static str {
        "fixed-sensors"
    }

    fn start(&mut self) -> anyhow::Result<()> {
        self.running = true;
        Ok(())
    }

    fn stop(&mut self) {
        self.running = false;
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        Ok(if self.running {
            InputData::Sensors(Arc::clone(&self.snapshot))
        } else {
            InputData::None
        })
    }

    fn is_running(&self) -> bool {
        self.running
    }
}

impl SourceRoleBinding for FixedSensorSource {
    type Role = DataSourceRole;
}

impl DataSource for FixedSensorSource {
    fn data_source_kind(&self) -> DataSourceKind {
        DataSourceKind::Sensors
    }
}

fn test_data_dir() -> PathBuf {
    let counter = TEST_DATA_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir()
        .join("hypercolor-ws-protocol-tests")
        .join(format!("{}-{counter}", std::process::id()))
}

fn test_app_state() -> Arc<AppState> {
    Arc::new(AppState::new_with_data_dir(test_data_dir()))
}

/// Spawn the full daemon router on an ephemeral TCP port.
///
/// Returns the bound address. The serve task runs until the test ends —
/// tokio tears it down when the runtime shuts down.
async fn spawn_test_daemon() -> std::net::SocketAddr {
    spawn_test_daemon_with_state(test_app_state()).await
}

async fn spawn_test_daemon_with_state(state: Arc<AppState>) -> std::net::SocketAddr {
    let router: Router = api::build_router(state, None);
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral WS port");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await;
    });
    addr
}

async fn insert_test_effect(state: &Arc<AppState>, name: &str) -> EffectMetadata {
    let metadata = EffectMetadata {
        id: EffectId::new(Uuid::now_v7()),
        name: name.to_owned(),
        author: "test".to_owned(),
        version: "0.1.0".to_owned(),
        description: format!("{name} ws effect"),
        category: EffectCategory::Ambient,
        tags: vec!["test".to_owned()],
        controls: Vec::new(),
        presets: Vec::new(),
        audio_reactive: false,
        screen_reactive: false,
        input_reactive: false,
        source: EffectSource::Native {
            path: format!("builtin/{name}").into(),
        },
        license: None,
    };
    let entry = hypercolor_core::effect::EffectEntry {
        metadata: metadata.clone(),
        source_path: format!("/tmp/{name}.rs").into(),
        modified: std::time::SystemTime::now(),
        state: hypercolor_types::effect::EffectState::Loading,
    };
    let mut registry = state.effect_registry.write().await;
    let _ = registry.register(entry);
    metadata
}

/// Open a WebSocket connection to `/api/v1/ws` and complete the upgrade.
///
/// Uses a constant `Sec-WebSocket-Key` — we never verify the server's
/// `Sec-WebSocket-Accept` echo, because the test only cares about the
/// post-upgrade protocol behavior. Returns the raw TCP stream positioned
/// right after the response headers.
async fn ws_connect(addr: std::net::SocketAddr) -> Result<TcpStream> {
    let mut stream = TcpStream::connect(addr)
        .await
        .context("connect ws test server")?;

    let request = format!(
        "GET /api/v1/ws HTTP/1.1\r\n\
         Host: {addr}\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
         Sec-WebSocket-Version: 13\r\n\
         Sec-WebSocket-Protocol: hypercolor-v1\r\n\
         \r\n"
    );
    stream
        .write_all(request.as_bytes())
        .await
        .context("write ws upgrade request")?;

    // Read the HTTP response up to the CRLF CRLF terminator.
    let mut buf = Vec::with_capacity(512);
    let mut byte = [0u8; 1];
    loop {
        stream
            .read_exact(&mut byte)
            .await
            .context("read ws upgrade response byte")?;
        buf.push(byte[0]);
        if buf.ends_with(b"\r\n\r\n") {
            break;
        }
        if buf.len() > 8192 {
            bail!("ws upgrade response exceeded 8KiB headers");
        }
    }
    let head = String::from_utf8_lossy(&buf);
    if !head.starts_with("HTTP/1.1 101") {
        bail!("expected 101 Switching Protocols, got: {head}");
    }
    Ok(stream)
}

/// Write a masked text frame to the server.
///
/// Client-to-server frames must always be masked per RFC 6455 §5.3.
async fn ws_send_text(stream: &mut TcpStream, payload: &str) -> Result<()> {
    let payload = payload.as_bytes();
    let mut frame = Vec::with_capacity(payload.len() + 14);
    frame.push(0x81); // FIN=1, opcode=text(0x1)

    let len = payload.len();
    if len < 126 {
        frame.push(0x80_u8 | u8::try_from(len).expect("len < 126")); // mask bit + 7-bit length
    } else if u16::try_from(len).is_ok() {
        frame.push(0x80_u8 | 0x7E);
        frame.extend_from_slice(&u16::try_from(len).expect("len fits in u16").to_be_bytes());
    } else {
        bail!("test payloads should never exceed 65535 bytes");
    }

    // Fixed mask key — RFC 6455 permits any 4 bytes. Using a constant keeps
    // tests deterministic; the server re-XORs regardless.
    let mask: [u8; 4] = [0xAA, 0xBB, 0xCC, 0xDD];
    frame.extend_from_slice(&mask);
    for (i, byte) in payload.iter().enumerate() {
        frame.push(byte ^ mask[i & 3]);
    }

    stream
        .write_all(&frame)
        .await
        .context("write ws text frame")?;
    Ok(())
}

/// Read exactly one text frame from the server, skipping binary/ping/pong.
///
/// Binary frames are drained silently — our protocol tests only inspect
/// JSON control messages. Ping frames are ignored (the server's keepalive
/// cadence is long enough that they won't interfere with sub-second tests).
async fn ws_recv_text(stream: &mut TcpStream) -> Result<String> {
    loop {
        let mut header = [0u8; 2];
        stream
            .read_exact(&mut header)
            .await
            .context("read ws frame header")?;
        let fin = header[0] & 0x80 != 0;
        let opcode = header[0] & 0x0F;
        let masked = header[1] & 0x80 != 0;
        let mut len = u64::from(header[1] & 0x7F);
        if len == 126 {
            let mut ext = [0u8; 2];
            stream
                .read_exact(&mut ext)
                .await
                .context("read ext16 len")?;
            len = u64::from(u16::from_be_bytes(ext));
        } else if len == 127 {
            let mut ext = [0u8; 8];
            stream
                .read_exact(&mut ext)
                .await
                .context("read ext64 len")?;
            len = u64::from_be_bytes(ext);
        }
        let mut mask = [0u8; 4];
        if masked {
            stream
                .read_exact(&mut mask)
                .await
                .context("read mask key")?;
        }
        let mut payload = vec![0u8; usize::try_from(len).context("frame length exceeds usize")?];
        stream
            .read_exact(&mut payload)
            .await
            .context("read frame payload")?;
        if masked {
            for (i, byte) in payload.iter_mut().enumerate() {
                *byte ^= mask[i & 3];
            }
        }
        if !fin {
            bail!("test client does not support fragmented frames");
        }
        match opcode {
            0x1 => {
                return String::from_utf8(payload).context("decode text payload");
            }
            0x2 | 0x9 | 0xA => {
                // binary, ping, pong — drained and ignored.
            }
            0x8 => bail!("server closed the WebSocket"),
            other => bail!("unknown ws opcode 0x{other:X}"),
        }
    }
}

/// Convenience: receive one JSON server message.
async fn recv_json(stream: &mut TcpStream) -> Result<Value> {
    let text = timeout(Duration::from_secs(2), ws_recv_text(stream))
        .await
        .context("timed out waiting for JSON server message")??;
    serde_json::from_str(&text).with_context(|| format!("parse JSON: {text}"))
}

/// Read until a message of the requested `type` arrives, discarding events.
///
/// The server eagerly pushes events on the default `events` subscription
/// (and may send a startup `effect_started`). Tests that look for a specific
/// ack type use this helper to skip over noise.
async fn recv_until_type(stream: &mut TcpStream, expected: &str) -> Result<Value> {
    // We allow up to 16 intermediate messages — realistic test flows see
    // at most a handful, but startup event bursts can stack up.
    for _ in 0..16 {
        let msg = recv_json(stream).await?;
        let ty = msg.get("type").and_then(Value::as_str).unwrap_or_default();
        if ty == expected {
            return Ok(msg);
        }
    }
    bail!("did not receive a {expected} message within 16 attempts");
}

async fn recv_trusted_until_type(
    socket: &mut TrustedLocalWebSocket,
    expected: &str,
) -> Result<Value> {
    for _ in 0..16 {
        let message = timeout(Duration::from_secs(2), socket.recv())
            .await
            .context("timed out waiting for trusted JSON server message")?
            .context("trusted WebSocket closed before the expected message")?;
        let Message::Text(text) = message else {
            continue;
        };
        let message: Value = serde_json::from_str(text.as_str())
            .with_context(|| format!("parse trusted JSON: {text}"))?;
        if message.get("type").and_then(Value::as_str) == Some(expected) {
            return Ok(message);
        }
    }
    bail!("did not receive a trusted {expected} message within 16 attempts");
}

// ── Scenario 5: Hello handshake success path ─────────────────────────────

#[tokio::test]
async fn hello_handshake_returns_expected_capability_set() {
    let addr = spawn_test_daemon().await;
    let mut stream = ws_connect(addr)
        .await
        .expect("ws handshake should complete");

    let hello = recv_until_type(&mut stream, "hello")
        .await
        .expect("first message should be hello");

    assert_eq!(hello["type"], "hello");
    assert_eq!(hello["version"], "1.0");
    assert!(
        hello.get("server").is_some(),
        "hello should include server identity"
    );
    assert!(
        hello.get("state").is_some(),
        "hello should include initial state snapshot"
    );

    let capabilities = hello["capabilities"]
        .as_array()
        .expect("capabilities should be an array")
        .iter()
        .map(|value| value.as_str().unwrap_or_default().to_owned())
        .collect::<Vec<_>>();
    // Every documented channel must appear in the capability set along with
    // the bidirectional commands channel.
    for expected in [
        "frames",
        "spectrum",
        "events",
        "canvas",
        "screen_canvas",
        "zone_preview",
        "metrics",
        "device_metrics",
        "sensors",
        "commands",
    ] {
        assert!(
            capabilities.iter().any(|cap| cap == expected),
            "hello capabilities missing {expected}: {capabilities:?}"
        );
    }

    let subscriptions = hello["subscriptions"]
        .as_array()
        .expect("subscriptions should be an array");
    // Default subscription set is exactly {events} per SubscriptionState::default.
    assert_eq!(subscriptions.len(), 1);
    assert_eq!(subscriptions[0]["topic"], "events");
    assert!(
        subscriptions[0].get("key").is_none(),
        "events is unkeyed, so it reports no key"
    );
    assert!(
        subscriptions[0].get("config").is_none(),
        "events takes no config, so it reports none"
    );
}

#[tokio::test]
async fn hello_handshake_names_the_scene_but_not_its_contents() {
    let state = test_app_state();
    let effect = insert_test_effect(&state, "Aurora").await;
    let preset_id = PresetId::stable("calm");
    let layout = {
        let spatial = state.spatial_engine.read().await;
        spatial.layout().as_ref().clone()
    };
    {
        let mut scene_manager = state.scene_manager.write().await;
        scene_manager
            .upsert_primary_group(
                &effect,
                std::collections::HashMap::new(),
                Some(preset_id),
                layout,
            )
            .expect("hello test should install a primary group");
    }

    let addr = spawn_test_daemon_with_state(state).await;
    let mut stream = ws_connect(addr)
        .await
        .expect("ws handshake should complete");

    let hello = recv_until_type(&mut stream, "hello")
        .await
        .expect("hello message should arrive");

    // The live tree is multi-zone, so one effect name could only ever
    // describe a corner of it. Clients read /scene for content and
    // follow the events channel for changes (Spec 78 §7.1).
    for singleton in ["effect", "active_preset_id"] {
        assert!(
            hello["state"].get(singleton).is_none(),
            "the handshake carries no {singleton}"
        );
    }
    assert_eq!(
        hello["state"]["scene"]["id"],
        hypercolor_types::scene::SceneId::DEFAULT.to_string()
    );
    assert_eq!(hello["state"]["scene"]["name"], "Default");
    assert_eq!(hello["state"]["scene"]["snapshot_locked"], false);
}

#[tokio::test]
async fn device_metrics_subscription_streams_seeded_snapshot() {
    let state = test_app_state();
    let device_id = hypercolor_types::device::DeviceId::new();
    state.device_metrics.store(Arc::new(DeviceMetricsSnapshot {
        taken_at_ms: 5_678,
        items: vec![DeviceMetrics {
            id: device_id,
            backend_id: "usb".to_owned(),
            mapped_layout_ids: vec!["layout-device".to_owned()],
            uses_frame_sink: true,
            worker_finished: false,
            worker_recoveries: 4,
            delivered_fps: 60.0,
            accepted_fps: 60.0,
            fps_sent: 60.0,
            fps_queued: 60.0,
            fps_actual: 60.0,
            fps_target: 60,
            target_interval_ms: Some(17),
            payload_bps_estimate: 2_048,
            avg_latency_ms: 9,
            avg_queue_wait_ms: 3,
            avg_write_ms: 6,
            avg_transport_latency_ms: 6,
            frames_received: 64,
            accepted: 65,
            frames_sent: 64,
            transport_started: 64,
            transport_completed: 64,
            transport_failed: 0,
            completed_payload_bytes: 32_768,
            frames_suppressed: 0,
            frames_dropped: 1,
            coalesced: 1,
            coalesced_target_cadence: 1,
            coalesced_backend_overrun: 0,
            errors_total: 0,
            write_failure_warnings_total: 0,
            last_error: None,
            last_sent_ago_ms: Some(14),
            last_sequence: 64,
            queue_generation: 9,
            last_transport_started_sequence: 64,
            last_transport_completed_sequence: 64,
            last_transport_failed_sequence: 0,
        }],
    }));
    let addr = spawn_test_daemon_with_state(state).await;
    let mut stream = ws_connect(addr)
        .await
        .expect("ws handshake should complete");
    let _ = recv_until_type(&mut stream, "hello").await.expect("hello");

    ws_send_text(
        &mut stream,
        &json!({
            "type": "subscribe",
            "topics": [{ "topic": "device_metrics", "config": { "interval_ms": 100 } }]
        })
        .to_string(),
    )
    .await
    .expect("send device_metrics subscribe");

    let ack = recv_until_type(&mut stream, "subscribed")
        .await
        .expect("device_metrics subscribed ack");
    let subscribed = subscription_map(&ack);
    assert_eq!(
        subscribed["device_metrics"]["interval_ms"], 100,
        "the ack echoes the live device_metrics config"
    );

    let message = recv_until_type(&mut stream, "device_metrics")
        .await
        .expect("device_metrics message should arrive");
    assert_eq!(message["data"]["taken_at_ms"], 5_678);
    assert_eq!(message["data"]["items"][0]["id"], device_id.to_string());
    assert_eq!(message["data"]["items"][0]["worker_recoveries"], 4);
    assert_eq!(message["data"]["items"][0]["delivered_fps"], 60.0);
    assert_eq!(message["data"]["items"][0]["accepted"], 65);
    assert_eq!(message["data"]["items"][0]["transport_completed"], 64);
    assert_eq!(message["data"]["items"][0]["coalesced_target_cadence"], 1);
    assert_eq!(message["data"]["items"][0]["payload_bps_estimate"], 2_048);
}

#[tokio::test]
async fn sensors_subscription_streams_seeded_snapshot() {
    let state = test_app_state();
    let mut snapshot = SystemSnapshot::empty();
    snapshot.cpu_load_percent = 37.5;
    snapshot.ram_used_percent = 64.0;
    snapshot.polled_at_ms = 8_901;
    {
        let input_manager = &state.input_manager;
        input_manager
            .add_source(ManagedSourceRole::data(Box::new(FixedSensorSource {
                snapshot: Arc::new(snapshot),
                running: false,
            })))
            .expect("fixed sensor source should register");
        input_manager
            .start_all()
            .expect("fixed sensor source starts");
        input_manager.sample_sources(0.0);
    }

    let addr = spawn_test_daemon_with_state(state).await;
    let mut stream = ws_connect(addr)
        .await
        .expect("ws handshake should complete");
    let _ = recv_until_type(&mut stream, "hello").await.expect("hello");

    ws_send_text(
        &mut stream,
        &json!({
            "type": "subscribe",
            "topics": [{ "topic": "sensors" }]
        })
        .to_string(),
    )
    .await
    .expect("send sensors subscribe");

    let ack = recv_until_type(&mut stream, "subscribed")
        .await
        .expect("sensors subscribed ack");
    assert!(
        subscribed_topics(&ack).contains(&"sensors".to_owned()),
        "the ack lists the sensors subscription"
    );

    let message = recv_until_type(&mut stream, "sensors")
        .await
        .expect("sensors message should arrive");
    assert_eq!(message["data"]["cpu_load_percent"], 37.5);
    assert_eq!(message["data"]["ram_used_percent"], 64.0);
    assert_eq!(message["data"]["polled_at_ms"], 8_901);
}

/// The topics an acknowledgment reports as live, in the order it sent them.
fn subscribed_topics(ack: &serde_json::Value) -> Vec<String> {
    ack["topics"]
        .as_array()
        .expect("ack.topics is an array")
        .iter()
        .map(|entry| entry["topic"].as_str().unwrap_or_default().to_owned())
        .collect()
}

/// Unkeyed subscriptions from an acknowledgment, viewed as `{topic: config}`.
fn subscription_map(ack: &serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    ack["topics"]
        .as_array()
        .expect("ack.topics is an array")
        .iter()
        .filter(|entry| entry.get("key").is_none())
        .filter_map(|entry| {
            let topic = entry["topic"].as_str()?.to_owned();
            Some((topic, entry.get("config")?.clone()))
        })
        .collect()
}

// ── Scenario 1: Subscribe → Unsubscribe → Subscribe cycle ────────────────

#[tokio::test]
async fn subscribe_unsubscribe_resubscribe_cycle_tracks_state() {
    let addr = spawn_test_daemon().await;
    let mut stream = ws_connect(addr).await.expect("ws handshake");
    let _ = recv_until_type(&mut stream, "hello").await.expect("hello");

    // Subscribe to `metrics`.
    ws_send_text(
        &mut stream,
        &json!({ "type": "subscribe", "topics": [{ "topic": "metrics" }] }).to_string(),
    )
    .await
    .expect("send subscribe");

    let ack = recv_until_type(&mut stream, "subscribed")
        .await
        .expect("subscribed ack");
    assert_eq!(
        subscribed_topics(&ack),
        vec!["events".to_owned(), "metrics".to_owned()],
        "the ack reports the whole live subscription set"
    );
    assert!(
        subscription_map(&ack).get("metrics").is_some(),
        "config should include metrics after subscribing"
    );

    // Unsubscribe from `metrics`. Default `events` stays subscribed.
    ws_send_text(
        &mut stream,
        &json!({ "type": "unsubscribe", "topics": [{ "topic": "metrics" }] }).to_string(),
    )
    .await
    .expect("send unsubscribe");

    let ack = recv_until_type(&mut stream, "unsubscribed")
        .await
        .expect("unsubscribed ack");
    assert_eq!(
        subscribed_topics(&ack),
        vec!["events".to_owned()],
        "after unsubscribing metrics, only default events should remain"
    );

    // Re-subscribe to metrics. The ack should succeed and the config should
    // still include metrics — the previous unsubscribe must not have poisoned
    // anything.
    ws_send_text(
        &mut stream,
        &json!({ "type": "subscribe", "topics": [{ "topic": "metrics" }] }).to_string(),
    )
    .await
    .expect("send re-subscribe");

    let ack = recv_until_type(&mut stream, "subscribed")
        .await
        .expect("re-subscribed ack");
    assert!(
        subscribed_topics(&ack).contains(&"metrics".to_owned()),
        "re-subscribe reinstates the metrics subscription"
    );
    assert!(
        subscription_map(&ack).get("metrics").is_some(),
        "re-subscribe should reinstate metrics config"
    );
}

// ── Scenario 2: Multi-channel subscribe ──────────────────────────────────

#[tokio::test]
async fn multi_channel_subscribe_returns_all_requested_channels() {
    let addr = spawn_test_daemon().await;
    let mut stream = ws_connect(addr).await.expect("ws handshake");
    let _ = recv_until_type(&mut stream, "hello").await.expect("hello");

    ws_send_text(
        &mut stream,
        &json!({
            "type": "subscribe",
            "topics": [
                { "topic": "events" },
                { "topic": "frames" },
                { "topic": "metrics" }
            ],
        })
        .to_string(),
    )
    .await
    .expect("send multi-topic subscribe");

    let ack = recv_until_type(&mut stream, "subscribed")
        .await
        .expect("multi-topic subscribed ack");
    assert_eq!(
        subscribed_topics(&ack),
        vec![
            "frames".to_owned(),
            "events".to_owned(),
            "metrics".to_owned()
        ],
        "the ack lists live subscriptions in registry declaration order"
    );

    // Config rides each entry, and a configless topic reports none.
    let config = subscription_map(&ack);
    assert!(config.get("frames").is_some(), "frames reports its config");
    assert!(
        config.get("metrics").is_some(),
        "metrics reports its config"
    );
    assert!(
        config.get("events").is_none(),
        "events takes no config, so it reports none"
    );
}

#[tokio::test]
async fn keyed_display_previews_are_independent_subscriptions() {
    let addr = spawn_test_daemon().await;
    let mut stream = ws_connect(addr).await.expect("ws handshake");
    let _ = recv_until_type(&mut stream, "hello").await.expect("hello");

    ws_send_text(
        &mut stream,
        &json!({
            "type": "subscribe",
            "topics": [
                { "topic": "display_preview", "key": "device-a", "config": { "fps": 5 } },
                { "topic": "display_preview", "key": "device-b", "config": { "fps": 25 } }
            ]
        })
        .to_string(),
    )
    .await
    .expect("send keyed subscribe");

    let ack = recv_until_type(&mut stream, "subscribed")
        .await
        .expect("keyed subscribed ack");
    let keyed: Vec<(String, i64)> = ack["topics"]
        .as_array()
        .expect("ack.topics is array")
        .iter()
        .filter(|entry| entry["topic"] == "display_preview")
        .map(|entry| {
            (
                entry["key"].as_str().unwrap_or_default().to_owned(),
                entry["config"]["fps"].as_i64().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        keyed,
        vec![("device-a".to_owned(), 5), ("device-b".to_owned(), 25)],
        "each device is its own subscription with its own cadence"
    );

    // Retiring one key leaves the other live.
    ws_send_text(
        &mut stream,
        &json!({
            "type": "unsubscribe",
            "topics": [{ "topic": "display_preview", "key": "device-a" }]
        })
        .to_string(),
    )
    .await
    .expect("send keyed unsubscribe");

    let ack = recv_until_type(&mut stream, "unsubscribed")
        .await
        .expect("keyed unsubscribed ack");
    let remaining: Vec<String> = ack["topics"]
        .as_array()
        .expect("ack.topics is array")
        .iter()
        .filter(|entry| entry["topic"] == "display_preview")
        .map(|entry| entry["key"].as_str().unwrap_or_default().to_owned())
        .collect();
    assert_eq!(remaining, vec!["device-b".to_owned()]);
}

#[tokio::test]
async fn a_keyed_topic_refuses_a_subscribe_without_its_key() {
    let addr = spawn_test_daemon().await;
    let mut stream = ws_connect(addr).await.expect("ws handshake");
    let _ = recv_until_type(&mut stream, "hello").await.expect("hello");

    ws_send_text(
        &mut stream,
        &json!({
            "type": "subscribe",
            "topics": [{ "topic": "display_preview" }]
        })
        .to_string(),
    )
    .await
    .expect("send keyless subscribe");

    let err = recv_until_type(&mut stream, "error")
        .await
        .expect("error response");
    assert_eq!(err["code"], "malformed_request");
    assert!(
        err["message"]
            .as_str()
            .unwrap_or_default()
            .contains("display_preview"),
        "the error names the topic: {err}"
    );
}

#[tokio::test]
async fn input_event_subscription_receives_canonical_timed_payload() {
    let state = test_app_state();
    let api = TrustedLocalApi::new(Arc::clone(&state));
    let mut socket = api
        .open_websocket("/api/v1/ws")
        .expect("trusted websocket should open");
    let _ = recv_trusted_until_type(&mut socket, "hello")
        .await
        .expect("hello");

    socket
        .send(Message::Text(
            json!({ "type": "subscribe", "topics": [{ "topic": "input_events" }] })
                .to_string()
                .into(),
        ))
        .await
        .expect("send input event subscription");
    let ack = recv_trusted_until_type(&mut socket, "subscribed")
        .await
        .expect("input event subscribed ack");
    assert!(subscribed_topics(&ack).contains(&"input_events".to_owned()));

    state
        .event_bus
        .publish(HypercolorEvent::InputEventReceived {
            event: TimedInputEvent {
                event: InputEvent::Key {
                    source_id: "host:integration-keyboard".into(),
                    key: "space".into(),
                    state: InputButtonState::Repeated,
                },
                at_ms: 5_000,
                seq: 88,
                physical_code: Some("win:e0:0039".into()),
                repeat_count: 6,
            },
        });

    let message = recv_trusted_until_type(&mut socket, "event")
        .await
        .expect("timed input event relay");
    assert_eq!(message["event"], "input_event_received");
    let decoded = TimedInputEventPayload::decode(&message["data"])
        .expect("daemon should emit the shared timed input schema");
    assert_eq!(decoded.at_ms, 5_000);
    assert_eq!(decoded.seq, 88);
    assert_eq!(decoded.physical_code.as_deref(), Some("win:e0:0039"));
    assert_eq!(decoded.repeat_count, 6);
    assert_eq!(decoded.event["source_id"], "host:integration-keyboard");
    assert_eq!(decoded.event["key"], "space");
    assert_eq!(decoded.event["state"], "repeated");

    socket.shutdown().await;
}

// ── Scenario 3: Subscribe with an unsupported channel ────────────────────

#[tokio::test]
async fn unsupported_channel_subscribe_returns_error_without_closing() {
    let addr = spawn_test_daemon().await;
    let mut stream = ws_connect(addr).await.expect("ws handshake");
    let _ = recv_until_type(&mut stream, "hello").await.expect("hello");

    ws_send_text(
        &mut stream,
        &json!({
            "type": "subscribe",
            "topics": [{ "topic": "lasers" }],
        })
        .to_string(),
    )
    .await
    .expect("send bogus subscribe");

    let err = recv_until_type(&mut stream, "error")
        .await
        .expect("error response");
    assert_eq!(err["type"], "error");
    assert_eq!(err["code"], "malformed_request");
    let message = err["message"].as_str().unwrap_or_default();
    assert!(
        message.to_lowercase().contains("lasers") || message.to_lowercase().contains("topic"),
        "error message should reference the topic; got: {message}"
    );

    // Crucially, the connection must stay open. Issue a legitimate subscribe
    // and confirm the server is still speaking to us.
    ws_send_text(
        &mut stream,
        &json!({ "type": "subscribe", "topics": [{ "topic": "metrics" }] }).to_string(),
    )
    .await
    .expect("send follow-up subscribe");

    let ack = recv_until_type(&mut stream, "subscribed")
        .await
        .expect("connection should still be alive after an error");
    assert!(subscribed_topics(&ack).contains(&"metrics".to_owned()));
}

// ── Deferred scenarios ───────────────────────────────────────────────────
//
// The following scenarios from the refactor plan §6.2 are intentionally
// deferred from this test file:
//
// * Scenario 4 (Subscribe before Hello): the server always sends hello
//   synchronously as the first frame, before spawning the inbound-message
//   loop. There is no observable window where a client could send a
//   Subscribe that the server processes "before" the hello. Exercising this
//   would require pausing the server task between its hello send and its
//   recv loop, which isn't possible without touching session.rs.
//
// * Scenario 6 (ChannelSet bit operations): `ChannelSet`, `WsChannel`, and
//   friends are all `pub(super)` inside `src/api/ws/protocol.rs`. External
//   integration tests cannot name these types. Bit-level unit tests belong
//   in the internal `src/api/ws/tests.rs` suite, which already has access
//   to the private surface.
//
// * Scenario 7 (Backpressure drop behavior): validating that the bounded
//   outbound queues drop frames under load requires either flooding the
//   event bus from inside the crate (also a `pub(super)` path) or pinning
//   the test client in a blocked read state while measuring relay behavior.
//   The internal relay tests already cover the lazy-subscribe and drop
//   metrics via the `WS_FRAME_PAYLOAD_*` counters. A dedicated load test
//   here would duplicate that coverage without adding fidelity.
