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

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use axum::Router;
use hypercolor_daemon::api::{self, AppState};
use hypercolor_types::effect::{EffectCategory, EffectId, EffectMetadata, EffectSource};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;
use uuid::Uuid;

// ── Test Harness ─────────────────────────────────────────────────────────

/// Spawn the full daemon router on an ephemeral TCP port.
///
/// Returns the bound address. The serve task runs until the test ends —
/// tokio tears it down when the runtime shuts down.
async fn spawn_test_daemon() -> std::net::SocketAddr {
    spawn_test_daemon_with_state(Arc::new(AppState::new())).await
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
        "metrics",
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
    assert_eq!(subscriptions[0], "events");
}

#[tokio::test]
async fn hello_handshake_reports_scene_backed_active_effect() {
    let state = Arc::new(AppState::new());
    let effect = insert_test_effect(&state, "Aurora").await;
    let layout = {
        let spatial = state.spatial_engine.read().await;
        spatial.layout().as_ref().clone()
    };
    {
        let mut scene_manager = state.scene_manager.write().await;
        scene_manager
            .upsert_primary_group(&effect, std::collections::HashMap::new(), None, layout)
            .expect("hello test should install a primary group");
    }

    let addr = spawn_test_daemon_with_state(state).await;
    let mut stream = ws_connect(addr)
        .await
        .expect("ws handshake should complete");

    let hello = recv_until_type(&mut stream, "hello")
        .await
        .expect("hello message should arrive");

    assert_eq!(hello["state"]["effect"]["id"], effect.id.to_string());
    assert_eq!(hello["state"]["effect"]["name"], effect.name);
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
        &json!({ "type": "subscribe", "channels": ["metrics"] }).to_string(),
    )
    .await
    .expect("send subscribe");

    let ack = recv_until_type(&mut stream, "subscribed")
        .await
        .expect("subscribed ack");
    let channels = ack["channels"].as_array().expect("ack.channels is array");
    assert_eq!(channels.len(), 1);
    assert_eq!(channels[0], "metrics");
    assert!(
        ack["config"].get("metrics").is_some(),
        "config should include metrics after subscribing"
    );

    // Unsubscribe from `metrics`. Default `events` stays subscribed.
    ws_send_text(
        &mut stream,
        &json!({ "type": "unsubscribe", "channels": ["metrics"] }).to_string(),
    )
    .await
    .expect("send unsubscribe");

    let ack = recv_until_type(&mut stream, "unsubscribed")
        .await
        .expect("unsubscribed ack");
    let removed = ack["channels"].as_array().expect("ack.channels is array");
    assert_eq!(removed.len(), 1);
    assert_eq!(removed[0], "metrics");
    let remaining = ack["remaining"]
        .as_array()
        .expect("ack.remaining is array")
        .iter()
        .map(|v| v.as_str().unwrap_or_default().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        remaining,
        vec!["events".to_owned()],
        "after unsubscribing metrics, only default events should remain"
    );

    // Re-subscribe to metrics. The ack should succeed and the config should
    // still include metrics — the previous unsubscribe must not have poisoned
    // anything.
    ws_send_text(
        &mut stream,
        &json!({ "type": "subscribe", "channels": ["metrics"] }).to_string(),
    )
    .await
    .expect("send re-subscribe");

    let ack = recv_until_type(&mut stream, "subscribed")
        .await
        .expect("re-subscribed ack");
    let channels = ack["channels"].as_array().expect("ack.channels is array");
    assert_eq!(channels.len(), 1);
    assert_eq!(channels[0], "metrics");
    assert!(
        ack["config"].get("metrics").is_some(),
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
            "channels": ["events", "frames", "metrics"],
        })
        .to_string(),
    )
    .await
    .expect("send multi-channel subscribe");

    let ack = recv_until_type(&mut stream, "subscribed")
        .await
        .expect("multi-channel subscribed ack");
    let mut channels = ack["channels"]
        .as_array()
        .expect("ack.channels is array")
        .iter()
        .map(|v| v.as_str().unwrap_or_default().to_owned())
        .collect::<Vec<_>>();
    channels.sort();
    assert_eq!(
        channels,
        vec![
            "events".to_owned(),
            "frames".to_owned(),
            "metrics".to_owned()
        ],
        "ack should echo the requested channels in sorted order"
    );

    // Config should include both frames and metrics stanzas (events has no
    // per-channel config block by design — see ChannelConfig::filtered_json).
    let config = &ack["config"];
    assert!(
        config.get("frames").is_some(),
        "config should include frames stanza"
    );
    assert!(
        config.get("metrics").is_some(),
        "config should include metrics stanza"
    );
    assert!(
        config.get("events").is_none(),
        "events has no per-channel config block"
    );
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
            "channels": ["lasers"],
        })
        .to_string(),
    )
    .await
    .expect("send bogus subscribe");

    let err = recv_until_type(&mut stream, "error")
        .await
        .expect("error response");
    assert_eq!(err["type"], "error");
    // `parse_channels` rejects the unknown channel with `invalid_request`
    // before ever reaching the `unsupported_channel` code path, so either
    // error code is acceptable.
    let code = err["code"].as_str().unwrap_or_default();
    assert!(
        code == "invalid_request" || code == "unsupported_channel",
        "expected invalid_request or unsupported_channel, got: {code}"
    );
    let message = err["message"].as_str().unwrap_or_default();
    assert!(
        message.to_lowercase().contains("lasers") || message.to_lowercase().contains("channel"),
        "error message should reference the channel; got: {message}"
    );

    // Crucially, the connection must stay open. Issue a legitimate subscribe
    // and confirm the server is still speaking to us.
    ws_send_text(
        &mut stream,
        &json!({ "type": "subscribe", "channels": ["metrics"] }).to_string(),
    )
    .await
    .expect("send follow-up subscribe");

    let ack = recv_until_type(&mut stream, "subscribed")
        .await
        .expect("connection should still be alive after an error");
    let channels = ack["channels"].as_array().expect("ack.channels is array");
    assert_eq!(channels.len(), 1);
    assert_eq!(channels[0], "metrics");
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
