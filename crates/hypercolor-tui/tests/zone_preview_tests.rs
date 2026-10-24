//! Tests for the one-zone-at-a-time preview.
//!
//! A multi-zone scene previews one zone through `zone_preview`, defaulting
//! to the primary zone and following the zone target; a single-zone scene
//! keeps the scene `canvas`. These cover the target policy, the wire
//! requests that follow it, zone frame routing, and a live connection that
//! retargets against a scripted daemon socket. Zone frame decoding lives
//! with the other decoder tests in `ws_decode_tests`.

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use axum::routing::get;
use bytes::Bytes;
use hypercolor_leptos_ext::ws::{
    PreviewFrame, PreviewFrameChannel, PreviewPixelFormat, ZonePreviewFrame,
};
use hypercolor_tui::client::ws::{self, WsBinaryDecoder, WsMessage};
use hypercolor_tui::state::{AppState, PreviewTarget, SceneDocument, ZoneResource};
use hypercolor_types::scene::ZoneId;
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::{Mutex, mpsc, watch};

const SCENE_ID: &str = "0198c5b6-2222-7000-8000-000000000001";
const OTHER_SCENE_ID: &str = "0198c5b6-2222-7000-8000-000000000009";
const ZONE_A: &str = "0198c5b6-2222-7000-8000-00000000000a";
const ZONE_B: &str = "0198c5b6-2222-7000-8000-00000000000b";
const ZONE_C: &str = "0198c5b6-2222-7000-8000-00000000000c";
const ZONE_D: &str = "0198c5b6-2222-7000-8000-00000000000d";
const STEP_TIMEOUT: Duration = Duration::from_secs(5);

// ── Fixtures ─────────────────────────────────────────────────────

fn zone(id: &str, name: &str, is_primary: bool) -> ZoneResource {
    zone_with_role(id, name, if is_primary { "primary" } else { "custom" })
}

fn display_zone(id: &str, name: &str) -> ZoneResource {
    zone_with_role(id, name, "display")
}

fn zone_with_role(id: &str, name: &str, role: &str) -> ZoneResource {
    serde_json::from_value(json!({
        "id": id,
        "name": name,
        "role": role,
        "enabled": true,
        "brightness": 1.0,
        "members": [],
        "layers": []
    }))
    .expect("zone fixture should decode")
}

fn scene(id: &str, zones: Vec<ZoneResource>) -> Arc<SceneDocument> {
    Arc::new(
        serde_json::from_value(json!({
            "id": id,
            "name": "Desk",
            "kind": "named",
            "is_default": false,
            "revision": 1,
            "zones": zones
        }))
        .expect("scene fixture should decode"),
    )
}

/// Three zones with the primary in the middle, so "primary" and "first"
/// never coincide.
fn three_zone_scene() -> Arc<SceneDocument> {
    scene(
        SCENE_ID,
        vec![
            zone(ZONE_A, "Side A", false),
            zone(ZONE_B, "Default", true),
            zone(ZONE_C, "Push 2", false),
        ],
    )
}

fn state_with(scene: Arc<SceneDocument>) -> AppState {
    AppState {
        active_scene: Some(scene),
        ..AppState::default()
    }
}

fn zone_id(id: &str) -> ZoneId {
    serde_json::from_value(json!(id)).expect("zone id fixture should decode")
}

fn zone_target(id: &str) -> PreviewTarget {
    PreviewTarget::Zone(zone_id(id))
}

fn zone_bytes(id: &str) -> [u8; 16] {
    *zone_id(id).0.as_bytes()
}

fn scene_bytes() -> [u8; 16] {
    let scene_id: hypercolor_types::scene::SceneId =
        serde_json::from_value(json!(SCENE_ID)).expect("scene id fixture should decode");
    *scene_id.0.as_bytes()
}

fn zone_frame(zone: &str, width: u32, format: PreviewPixelFormat, payload: &[u8]) -> Bytes {
    ZonePreviewFrame {
        scene_id: scene_bytes(),
        zone_id: zone_bytes(zone),
        frame_number: 7,
        timestamp_ms: 70,
        width,
        height: 1,
        format,
        payload: Bytes::copy_from_slice(payload),
    }
    .encode()
}

fn rgb_zone_frame(zone: &str, rgb: [u8; 3]) -> Bytes {
    zone_frame(zone, 1, PreviewPixelFormat::Rgb, &rgb)
}

fn canvas_frame(rgb: [u8; 3]) -> Bytes {
    PreviewFrame {
        channel: PreviewFrameChannel::Canvas,
        frame_number: 3,
        timestamp_ms: 30,
        width: 1,
        height: 1,
        format: PreviewPixelFormat::Rgb,
        payload: Bytes::copy_from_slice(&rgb),
    }
    .encode()
}

fn decoded_zone_message(zone: &str) -> WsMessage {
    ws::decode_binary(
        &mut WsBinaryDecoder::new(),
        &rgb_zone_frame(zone, [1, 2, 3]),
    )
    .expect("zone frame should decode")
}

fn decoded_canvas_message() -> WsMessage {
    ws::decode_binary(&mut WsBinaryDecoder::new(), &canvas_frame([1, 2, 3]))
        .expect("canvas frame should decode")
}

fn topic_names(subscribe: &Value) -> Vec<&str> {
    subscribe["topics"]
        .as_array()
        .expect("subscribe carries topics")
        .iter()
        .filter_map(|topic| topic["topic"].as_str())
        .collect()
}

fn topic<'a>(subscribe: &'a Value, name: &str) -> &'a Value {
    subscribe["topics"]
        .as_array()
        .expect("subscribe carries topics")
        .iter()
        .find(|topic| topic["topic"] == name)
        .unwrap_or_else(|| panic!("subscribe should carry `{name}`: {subscribe}"))
}

// ── Target policy ────────────────────────────────────────────────

#[test]
fn single_zone_scene_keeps_the_scene_canvas() {
    let single = state_with(scene(SCENE_ID, vec![zone(ZONE_A, "Default", true)]));
    assert_eq!(single.preview_target(), PreviewTarget::SceneCanvas);

    // One opening request, with the canvas, exactly as before zone
    // previews existed.
    let opening = ws::opening_messages(&single.preview_target());
    assert_eq!(opening.len(), 1);
    assert_eq!(
        topic_names(&opening[0]),
        vec!["canvas", "spectrum", "events", "metrics"]
    );
    assert_eq!(
        topic(&opening[0], "canvas")["config"],
        json!({ "fps": 60, "format": "rgb" })
    );

    // Nothing known about the scene yet: the scene canvas.
    assert_eq!(
        AppState::default().preview_target(),
        PreviewTarget::SceneCanvas
    );
}

#[test]
fn multi_zone_scene_previews_the_primary_zone_by_default() {
    let state = state_with(three_zone_scene());
    assert_eq!(state.preview_target(), zone_target(ZONE_B));

    // Admission carries no preview, so a daemon without zone previews
    // still admits the connection; the zone follows in its own request.
    let opening = ws::opening_messages(&state.preview_target());
    assert_eq!(opening.len(), 2);
    assert_eq!(
        topic_names(&opening[0]),
        vec!["spectrum", "events", "metrics"]
    );
    assert_eq!(topic_names(&opening[1]), vec!["zone_preview"]);
    assert_eq!(
        topic(&opening[1], "zone_preview")["config"],
        json!({ "fps": 60, "format": "rgb", "zones": [ZONE_B] })
    );
}

#[test]
fn display_zones_do_not_make_a_scene_multi_zone() {
    // One LED zone plus an LCD face keeps the scene canvas.
    let state = state_with(scene(
        SCENE_ID,
        vec![zone(ZONE_A, "Default", true), display_zone(ZONE_D, "LCD")],
    ));
    assert_eq!(state.preview_target(), PreviewTarget::SceneCanvas);
    assert!(state.preview_zone().is_none());
}

#[test]
fn a_display_zone_target_previews_the_primary_zone() {
    let mut state = state_with(scene(
        SCENE_ID,
        vec![
            zone(ZONE_A, "Side A", false),
            zone(ZONE_B, "Default", true),
            display_zone(ZONE_D, "LCD"),
        ],
    ));
    state.focused_zone = Some(ZONE_D.to_owned());
    assert_eq!(state.preview_target(), zone_target(ZONE_B));
    // The label helper names the zone on screen, not the edit target.
    assert_eq!(
        state.preview_zone().map(|zone| zone.name.as_str()),
        Some("Default")
    );
}

#[test]
fn a_scene_without_a_primary_zone_previews_its_first_led_zone() {
    let state = state_with(scene(
        SCENE_ID,
        vec![
            display_zone(ZONE_D, "LCD"),
            zone(ZONE_A, "Side A", false),
            zone(ZONE_C, "Push 2", false),
        ],
    ));
    assert_eq!(state.preview_target(), zone_target(ZONE_A));
}

#[test]
fn the_zone_target_is_the_previewed_zone() {
    let mut state = state_with(three_zone_scene());
    state.focused_zone = Some(ZONE_C.to_owned());
    assert_eq!(state.preview_target(), zone_target(ZONE_C));
}

#[test]
fn cycling_walks_the_preview_through_every_zone() {
    let mut state = state_with(three_zone_scene());

    // From the primary zone (index 1) forward, wrapping at the end.
    assert_eq!(state.cycle_zone_focus(true).as_deref(), Some("Push 2"));
    assert_eq!(state.preview_target(), zone_target(ZONE_C));
    assert_eq!(state.cycle_zone_focus(true).as_deref(), Some("Side A"));
    assert_eq!(state.preview_target(), zone_target(ZONE_A));
    assert_eq!(state.cycle_zone_focus(true).as_deref(), Some("Default"));
    assert_eq!(state.preview_target(), zone_target(ZONE_B));

    // Backward wraps the other way.
    assert_eq!(state.cycle_zone_focus(false).as_deref(), Some("Side A"));
    assert_eq!(state.cycle_zone_focus(false).as_deref(), Some("Push 2"));
    assert_eq!(state.preview_target(), zone_target(ZONE_C));
}

#[test]
fn cycling_a_single_zone_scene_changes_nothing() {
    let mut state = state_with(scene(SCENE_ID, vec![zone(ZONE_A, "Default", true)]));
    assert!(state.cycle_zone_focus(true).is_none());
    assert!(state.focused_zone.is_none());
    assert_eq!(state.preview_target(), PreviewTarget::SceneCanvas);
}

#[test]
fn preview_falls_back_to_primary_when_the_shown_zone_leaves() {
    let mut state = state_with(three_zone_scene());
    state.focused_zone = Some(ZONE_C.to_owned());

    let without_c = scene(
        SCENE_ID,
        vec![zone(ZONE_A, "Side A", false), zone(ZONE_B, "Default", true)],
    );
    assert!(state.set_active_scene(without_c));
    assert!(state.focused_zone.is_none());
    assert_eq!(state.preview_target(), zone_target(ZONE_B));
}

#[test]
fn preview_falls_back_to_the_new_scenes_primary_on_a_scene_switch() {
    let mut state = state_with(three_zone_scene());
    state.focused_zone = Some(ZONE_A.to_owned());

    let next = scene(
        OTHER_SCENE_ID,
        vec![zone(ZONE_C, "Desk", false), zone(ZONE_D, "Default", true)],
    );
    assert!(state.set_active_scene(next));
    assert_eq!(state.preview_target(), zone_target(ZONE_D));

    // A switch to a single-zone scene goes back to the scene canvas.
    let single = scene(OTHER_SCENE_ID, vec![zone(ZONE_D, "Default", true)]);
    state.set_active_scene(single);
    assert_eq!(state.preview_target(), PreviewTarget::SceneCanvas);
}

#[test]
fn preview_keeps_a_shown_zone_that_survives_a_scene_update() {
    let mut state = state_with(three_zone_scene());
    state.focused_zone = Some(ZONE_C.to_owned());

    let with_d = scene(
        SCENE_ID,
        vec![
            zone(ZONE_A, "Side A", false),
            zone(ZONE_B, "Default", true),
            zone(ZONE_C, "Push 2", false),
            zone(ZONE_D, "ROLI", false),
        ],
    );
    assert!(!state.set_active_scene(with_d));
    assert_eq!(state.focused_zone.as_deref(), Some(ZONE_C));
    assert_eq!(state.preview_target(), zone_target(ZONE_C));
}

// ── Wire requests ────────────────────────────────────────────────

#[test]
fn moving_between_canvas_and_a_zone_drops_the_old_topic_first() {
    let to_zone =
        ws::preview_transition_messages(&PreviewTarget::SceneCanvas, &zone_target(ZONE_A));
    assert_eq!(
        to_zone,
        vec![
            json!({ "type": "unsubscribe", "topics": [{ "topic": "canvas" }] }),
            json!({
                "type": "subscribe",
                "topics": [{
                    "topic": "zone_preview",
                    "config": { "fps": 60, "format": "rgb", "zones": [ZONE_A] }
                }]
            }),
        ]
    );

    let to_canvas =
        ws::preview_transition_messages(&zone_target(ZONE_A), &PreviewTarget::SceneCanvas);
    assert_eq!(
        to_canvas,
        vec![
            json!({ "type": "unsubscribe", "topics": [{ "topic": "zone_preview" }] }),
            json!({
                "type": "subscribe",
                "topics": [{ "topic": "canvas", "config": { "fps": 60, "format": "rgb" } }]
            }),
        ]
    );
}

#[test]
fn moving_between_zones_patches_the_zone_list_in_place() {
    let messages = ws::preview_transition_messages(&zone_target(ZONE_A), &zone_target(ZONE_B));
    assert_eq!(
        messages,
        vec![json!({
            "type": "subscribe",
            "topics": [{
                "topic": "zone_preview",
                "config": { "fps": 60, "format": "rgb", "zones": [ZONE_B] }
            }]
        })]
    );
}

#[test]
fn an_unchanged_target_sends_nothing() {
    assert!(ws::preview_transition_messages(&zone_target(ZONE_A), &zone_target(ZONE_A)).is_empty());
    assert!(
        ws::preview_transition_messages(&PreviewTarget::SceneCanvas, &PreviewTarget::SceneCanvas)
            .is_empty()
    );
}

// ── Frame routing ────────────────────────────────────────────────

#[test]
fn a_zone_preview_shows_only_its_own_zone() {
    let target = zone_target(ZONE_A);
    assert!(ws::preview_shows(&target, &decoded_zone_message(ZONE_A)));
    assert!(!ws::preview_shows(&target, &decoded_zone_message(ZONE_B)));
    assert!(!ws::preview_shows(&target, &decoded_canvas_message()));
}

#[test]
fn a_scene_canvas_preview_ignores_zone_frames() {
    let target = PreviewTarget::SceneCanvas;
    assert!(ws::preview_shows(&target, &decoded_canvas_message()));
    assert!(!ws::preview_shows(&target, &decoded_zone_message(ZONE_A)));
}

#[test]
fn routing_passes_every_non_preview_message() {
    let event =
        ws::decode_json(r#"{"type":"event","event":"zone_changed"}"#).expect("event should decode");
    assert!(ws::preview_shows(&zone_target(ZONE_A), &event));
    assert!(ws::preview_shows(&zone_target(ZONE_A), &WsMessage::Closed));
}

// ── Live connection ──────────────────────────────────────────────

/// A scripted daemon socket: every text message the client sends is
/// reported to the test, and every message the test queues is sent.
struct ScriptedSocket {
    client_messages: mpsc::UnboundedSender<Value>,
    outbound: Mutex<Option<mpsc::UnboundedReceiver<Message>>>,
}

async fn scripted_ws(ws: WebSocketUpgrade, State(script): State<Arc<ScriptedSocket>>) -> Response {
    ws.on_upgrade(move |socket| run_script(socket, script))
}

async fn run_script(mut socket: WebSocket, script: Arc<ScriptedSocket>) {
    let Some(mut outbound) = script.outbound.lock().await.take() else {
        return;
    };
    loop {
        tokio::select! {
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Text(text))) => {
                    let value = serde_json::from_str(&text).expect("client sends JSON");
                    let _ = script.client_messages.send(value);
                }
                Some(Ok(_)) => {}
                Some(Err(_)) | None => return,
            },
            queued = outbound.recv() => match queued {
                Some(message) => {
                    if socket.send(message).await.is_err() {
                        return;
                    }
                }
                None => return,
            },
        }
    }
}

async fn next_within<T>(receiver: &mut mpsc::UnboundedReceiver<T>, what: &str) -> T {
    tokio::time::timeout(STEP_TIMEOUT, receiver.recv())
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
        .unwrap_or_else(|| panic!("channel closed waiting for {what}"))
}

fn subscribed_ack() -> Message {
    Message::Text(
        json!({ "type": "subscribed", "topics": [] })
            .to_string()
            .into(),
    )
}

fn send_all(
    outbound: &mpsc::UnboundedSender<Message>,
    messages: impl IntoIterator<Item = Message>,
) {
    for message in messages {
        outbound.send(message).expect("script socket is open");
    }
}

fn expect_zone_frame(message: WsMessage, zone: &str, rgb: [u8; 3]) {
    let WsMessage::ZonePreview { zone_id, frame, .. } = message else {
        panic!("expected a frame of zone {zone}, got {message:?}");
    };
    assert_eq!(zone_id, zone_bytes(zone), "frame belongs to another zone");
    assert_eq!(frame.pixels.as_ref(), &rgb);
}

/// A daemon socket scripted by the test, and a client connected to it.
struct LiveHarness {
    client_messages: mpsc::UnboundedReceiver<Value>,
    outbound: mpsc::UnboundedSender<Message>,
    messages: mpsc::UnboundedReceiver<WsMessage>,
    target: watch::Sender<PreviewTarget>,
    client: tokio::task::JoinHandle<anyhow::Result<()>>,
    server: tokio::task::JoinHandle<()>,
}

impl LiveHarness {
    async fn start(target: PreviewTarget) -> Self {
        let (client_tx, client_messages) = mpsc::unbounded_channel();
        let (outbound, outbound_rx) = mpsc::unbounded_channel();
        let script = Arc::new(ScriptedSocket {
            client_messages: client_tx,
            outbound: Mutex::new(Some(outbound_rx)),
        });
        let router = Router::new()
            .route("/api/v1/ws", get(scripted_ws))
            .with_state(script);
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test listener");
        let port = listener.local_addr().expect("listener addr").port();
        let server = tokio::spawn(async move {
            axum::serve(listener, router)
                .await
                .expect("serve test router");
        });

        let (target, target_rx) = watch::channel(target);
        let (message_tx, messages) = mpsc::unbounded_channel();
        let client = tokio::spawn(async move {
            ws::connect("127.0.0.1", port, None, message_tx, target_rx).await
        });
        Self {
            client_messages,
            outbound,
            messages,
            target,
            client,
            server,
        }
    }

    async fn finish(self) {
        drop(self.outbound);
        let _ = tokio::time::timeout(STEP_TIMEOUT, self.client).await;
        self.server.abort();
    }
}

#[tokio::test]
async fn live_connection_follows_the_target_and_drops_other_frames() {
    let mut live = LiveHarness::start(zone_target(ZONE_A)).await;

    // Admission carries no preview; zone A's own frames follow in their
    // own request, and the scene canvas is never asked for.
    let admission = next_within(&mut live.client_messages, "admission").await;
    assert_eq!(
        topic_names(&admission),
        vec!["spectrum", "events", "metrics"]
    );
    let subscribe = next_within(&mut live.client_messages, "the zone subscribe").await;
    assert_eq!(topic_names(&subscribe), vec!["zone_preview"]);
    assert_eq!(
        topic(&subscribe, "zone_preview")["config"]["zones"],
        json!([ZONE_A])
    );
    // Two acknowledgments: only the first is admission.
    send_all(&live.outbound, [subscribed_ack(), subscribed_ack()]);
    assert!(matches!(
        next_within(&mut live.messages, "admission").await,
        WsMessage::Subscribed(_)
    ));

    // Only zone A's frame reaches the preview.
    send_all(
        &live.outbound,
        [
            Message::Binary(rgb_zone_frame(ZONE_B, [2, 2, 2])),
            Message::Binary(canvas_frame([9, 9, 9])),
            Message::Binary(rgb_zone_frame(ZONE_A, [1, 1, 1])),
        ],
    );
    expect_zone_frame(
        next_within(&mut live.messages, "zone A's frame").await,
        ZONE_A,
        [1, 1, 1],
    );

    // Zone A to zone B patches the zone list without dropping the topic.
    live.target.send_replace(zone_target(ZONE_B));
    let retarget = next_within(&mut live.client_messages, "the zone retarget").await;
    assert_eq!(retarget["type"], "subscribe");
    assert_eq!(
        topic(&retarget, "zone_preview")["config"]["zones"],
        json!([ZONE_B])
    );
    // The retarget's acknowledgment is not a second admission, and zone
    // A's straggler is dropped.
    send_all(
        &live.outbound,
        [
            subscribed_ack(),
            Message::Binary(rgb_zone_frame(ZONE_A, [1, 1, 1])),
            Message::Binary(rgb_zone_frame(ZONE_B, [2, 2, 2])),
        ],
    );
    expect_zone_frame(
        next_within(&mut live.messages, "zone B's frame").await,
        ZONE_B,
        [2, 2, 2],
    );

    // Back to a single-zone scene: drop `zone_preview`, then the canvas.
    live.target.send_replace(PreviewTarget::SceneCanvas);
    let unsubscribe = next_within(&mut live.client_messages, "the zone unsubscribe").await;
    assert_eq!(
        unsubscribe,
        json!({ "type": "unsubscribe", "topics": [{ "topic": "zone_preview" }] })
    );
    let canvas = next_within(&mut live.client_messages, "the canvas subscribe").await;
    assert_eq!(topic_names(&canvas), vec!["canvas"]);
    send_all(
        &live.outbound,
        [
            Message::Binary(rgb_zone_frame(ZONE_B, [2, 2, 2])),
            Message::Binary(canvas_frame([9, 9, 9])),
        ],
    );
    let WsMessage::Canvas(frame) = next_within(&mut live.messages, "the canvas frame").await else {
        panic!("expected the scene canvas frame");
    };
    assert_eq!(frame.pixels.as_ref(), &[9, 9, 9]);

    live.finish().await;
}

/// How a daemon from before zone selection refuses the `zones` field.
fn zones_field_refusal() -> Value {
    json!({
        "type": "error",
        "code": "validation_failed",
        "message": "Invalid configuration for config.zone_preview: unknown field `zones`",
        "details": { "field": "config.zone_preview", "reason": "unknown field `zones`" }
    })
}

#[test]
fn zone_preview_refusals_are_recognized() {
    assert!(ws::refuses_zone_preview(&zones_field_refusal().to_string()));
    let unknown_topic = json!({
        "type": "error",
        "code": "invalid_request",
        "message": "unknown topic `zone_preview`"
    });
    assert!(ws::refuses_zone_preview(&unknown_topic.to_string()));
    // The structured field is enough on its own.
    let field_only = json!({
        "type": "error",
        "code": "validation_failed",
        "message": "Invalid configuration",
        "details": { "field": "config.zone_preview.zones", "reason": "must not be empty" }
    });
    assert!(ws::refuses_zone_preview(&field_only.to_string()));

    let other_field = json!({
        "type": "error",
        "code": "validation_failed",
        "message": "Invalid configuration for config.canvas.fps: expected 1..=60",
        "details": { "field": "config.canvas.fps", "reason": "expected 1..=60" }
    });
    assert!(!ws::refuses_zone_preview(&other_field.to_string()));
    assert!(!ws::refuses_zone_preview(
        &json!({ "type": "event", "event": "zone_preview" }).to_string()
    ));
    assert!(!ws::refuses_zone_preview("not json"));
}

#[tokio::test]
async fn a_daemon_that_refuses_zone_previews_falls_back_to_the_canvas() {
    let mut live = LiveHarness::start(zone_target(ZONE_A)).await;
    let _admission = next_within(&mut live.client_messages, "admission").await;
    let _zone = next_within(&mut live.client_messages, "the zone subscribe").await;

    // Admission succeeds; only the zone request is refused.
    send_all(
        &live.outbound,
        [
            subscribed_ack(),
            Message::Text(zones_field_refusal().to_string().into()),
        ],
    );
    assert!(matches!(
        next_within(&mut live.messages, "admission").await,
        WsMessage::Subscribed(_)
    ));
    let fallback = next_within(&mut live.client_messages, "the canvas fallback").await;
    assert_eq!(
        fallback,
        json!({
            "type": "subscribe",
            "topics": [{ "topic": "canvas", "config": { "fps": 60, "format": "rgb" } }]
        })
    );

    // The canvas now reaches the preview although the target names a zone.
    send_all(
        &live.outbound,
        [subscribed_ack(), Message::Binary(canvas_frame([9, 9, 9]))],
    );
    assert!(matches!(
        next_within(&mut live.messages, "the canvas frame").await,
        WsMessage::Canvas(_)
    ));

    // Later zone changes stay on the canvas instead of asking again.
    live.target.send_replace(zone_target(ZONE_B));
    send_all(&live.outbound, [Message::Binary(canvas_frame([8, 8, 8]))]);
    let WsMessage::Canvas(frame) = next_within(&mut live.messages, "the next canvas frame").await
    else {
        panic!("expected the scene canvas frame");
    };
    assert_eq!(frame.pixels.as_ref(), &[8, 8, 8]);
    assert!(
        tokio::time::timeout(Duration::from_millis(200), live.client_messages.recv())
            .await
            .is_err(),
        "a refused zone preview must not be requested again"
    );

    live.finish().await;
}
