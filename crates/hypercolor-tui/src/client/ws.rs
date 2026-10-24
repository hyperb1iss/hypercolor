//! WebSocket client for the Hypercolor daemon.
//!
//! Subscribes to preview frames, spectrum data, and events over a persistent
//! WebSocket connection. Binary frames are decoded inline through the shared
//! wire codec in `hypercolor-leptos-ext` — the same one the web UI uses, so
//! the format has exactly one definition.
//!
//! The preview follows a [`PreviewTarget`]: the scene canvas, or one zone's
//! own frames in a multi-zone scene. The app publishes the target on a
//! watch channel; the connection re-subscribes when it changes and drops
//! frames that belong to any other target.

use anyhow::{Context, Result};
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use hypercolor_leptos_ext::ws::topic::ActiveSubscription;
use hypercolor_leptos_ext::ws::{
    PREVIEW_CANCEL_FRAME_TAG, PREVIEW_CHUNK_FRAME_TAG, PreviewCancelFrame, PreviewChunkReassembler,
    PreviewFrame, PreviewFrameChannel, PreviewPixelFormat, PreviewReassemblyLimits,
    PreviewStreamId, ReassembledPreviewPublication, SPECTRUM_FRAME_TAG, SpectrumFrame,
    WIDE_ZONE_PREVIEW_FRAME_TAG, ZONE_PREVIEW_FRAME_TAG, ZonePreviewFrame,
};
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::tungstenite::Message;

use crate::state::{CanvasFrame, PreviewTarget, SpectrumSnapshot};

const TUI_CANVAS_FPS: u8 = 60;
const SUBSCRIPTION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

#[derive(Debug, serde::Deserialize)]
pub struct SubscribedAck {
    pub topics: Vec<ActiveSubscription>,
}

/// Messages decoded from the WebSocket stream.
#[derive(Debug)]
pub enum WsMessage {
    /// Server hello with initial state.
    Hello(serde_json::Value),
    /// A scene canvas frame (binary, type 0x03).
    Canvas(CanvasFrame),
    /// One zone's own frame from `zone_preview` (binary, type 0x08/0x0c),
    /// named by its scene and zone.
    ZonePreview {
        scene_id: [u8; 16],
        zone_id: [u8; 16],
        frame: CanvasFrame,
    },
    /// A spectrum snapshot (binary, type 0x02).
    Spectrum(SpectrumSnapshot),
    /// A JSON event from the events channel.
    Event(serde_json::Value),
    /// The daemon admitted the complete requested subscription set.
    Subscribed(SubscribedAck),
    /// A metrics snapshot.
    Metrics(serde_json::Value),
    /// Connection closed.
    Closed,
}

/// Connect to the daemon WebSocket and stream decoded messages.
///
/// The preview subscribes to whatever `preview_target` holds at connect
/// time and follows it from there. A daemon that refuses `zone_preview`
/// still admits the connection, and the preview falls back to the scene
/// canvas for the rest of it.
pub async fn connect(
    host: &str,
    port: u16,
    api_key: Option<&str>,
    tx: mpsc::UnboundedSender<WsMessage>,
    mut preview_target: watch::Receiver<PreviewTarget>,
) -> Result<()> {
    let url = build_ws_url(host, port, api_key);
    let (ws_stream, _response) = tokio_tungstenite::connect_async(&url)
        .await
        .with_context(|| format!("Failed to connect WebSocket at {url}"))?;

    let (mut write, mut read) = ws_stream.split();

    let mut requested = *preview_target.borrow_and_update();
    for message in opening_messages(&requested) {
        write
            .send(Message::Text(message.to_string().into()))
            .await
            .context("Failed to send subscribe message")?;
    }

    let mut binary_decoder = WsBinaryDecoder::new();
    let (hello, acknowledgment) =
        wait_for_subscription_ack(&mut read, SUBSCRIPTION_TIMEOUT).await?;
    tx.send(WsMessage::Subscribed(acknowledgment))
        .context("TUI bridge closed before subscription admission")?;
    if let Some(hello) = hello {
        tx.send(WsMessage::Hello(hello))
            .context("TUI bridge closed before hello delivery")?;
    }

    // Read loop
    let mut preview_target_open = true;
    let mut zone_preview_refused = false;
    let effective = |target: PreviewTarget, refused: bool| {
        if refused {
            PreviewTarget::SceneCanvas
        } else {
            target
        }
    };
    loop {
        let expiry_deadline = binary_decoder.next_expiry_deadline();
        let next_message = tokio::select! {
            message = read.next() => message,
            () = async {
                match expiry_deadline {
                    Some(deadline) => {
                        tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)).await;
                    }
                    None => std::future::pending().await,
                }
            } => {
                binary_decoder.expire_now();
                continue;
            }
            changed = preview_target.changed(), if preview_target_open => {
                if changed.is_err() {
                    preview_target_open = false;
                    continue;
                }
                let next = effective(*preview_target.borrow_and_update(), zone_preview_refused);
                for message in preview_transition_messages(&requested, &next) {
                    write
                        .send(Message::Text(message.to_string().into()))
                        .await
                        .context("Failed to retarget the preview subscription")?;
                }
                requested = next;
                continue;
            }
        };
        let Some(msg) = next_message else {
            break;
        };
        let msg = match msg {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("WebSocket error: {e}");
                break;
            }
        };

        if let Message::Text(text) = &msg
            && matches!(requested, PreviewTarget::Zone(_))
            && refuses_zone_preview(text)
        {
            tracing::warn!(
                "Daemon refused zone_preview ({text}); previewing the scene canvas instead"
            );
            zone_preview_refused = true;
            requested = PreviewTarget::SceneCanvas;
            write
                .send(Message::Text(
                    preview_subscribe_message(&requested).to_string().into(),
                ))
                .await
                .context("Failed to fall back to the canvas preview")?;
            continue;
        }

        let decoded = match msg {
            Message::Binary(data) => binary_decoder.decode(&data),
            Message::Text(text) => decode_json(&text),
            Message::Close(_) => Some(WsMessage::Closed),
            Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => None,
        };

        if let Some(ws_msg) = decoded {
            // Admission happened above; a later acknowledgment answers a
            // preview retarget and must not restart the bridge bootstrap.
            if let WsMessage::Subscribed(acknowledgment) = &ws_msg {
                tracing::debug!(topics = ?acknowledgment.topics, "Preview subscription updated");
                continue;
            }
            // Frames still in flight from before a retarget are dropped
            // against the newest target, before the re-subscribe lands.
            if !preview_shows(
                &effective(*preview_target.borrow(), zone_preview_refused),
                &ws_msg,
            ) {
                continue;
            }
            let is_closed = matches!(ws_msg, WsMessage::Closed);
            if tx.send(ws_msg).is_err() || is_closed {
                break;
            }
        }
    }

    let _ = tx.send(WsMessage::Closed);
    Ok(())
}

/// The connection's opening subscribe requests.
///
/// The first is admission: spectrum, events, and metrics, plus the scene
/// canvas for a scene-canvas target, which every daemon accepts. A zone
/// target follows in its own request, because the daemon applies a
/// subscribe all or nothing, and a daemon without zone previews would
/// otherwise refuse the whole connection.
#[must_use]
pub fn opening_messages(target: &PreviewTarget) -> Vec<serde_json::Value> {
    let mut admission = vec![
        serde_json::json!({ "topic": "spectrum", "config": { "fps": 15, "bins": 64 } }),
        serde_json::json!({ "topic": "events" }),
        serde_json::json!({ "topic": "metrics", "config": { "fps": 0.5 } }),
    ];
    match target {
        PreviewTarget::SceneCanvas => {
            admission.insert(0, preview_subscription(target));
            vec![serde_json::json!({ "type": "subscribe", "topics": admission })]
        }
        PreviewTarget::Zone(_) => vec![
            serde_json::json!({ "type": "subscribe", "topics": admission }),
            preview_subscribe_message(target),
        ],
    }
}

/// A subscribe request for `target`'s preview stream alone.
#[must_use]
pub fn preview_subscribe_message(target: &PreviewTarget) -> serde_json::Value {
    serde_json::json!({ "type": "subscribe", "topics": [preview_subscription(target)] })
}

/// Whether a text message is the daemon refusing a `zone_preview`
/// subscription: a protocol error that names the topic, whether it lacks
/// the topic or a config field such as `zones`.
#[must_use]
pub fn refuses_zone_preview(text: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return false;
    };
    if value.get("type").and_then(serde_json::Value::as_str) != Some("error") {
        return false;
    }
    let names_zone_preview = |field: Option<&serde_json::Value>| {
        field
            .and_then(serde_json::Value::as_str)
            .is_some_and(|text| text.contains("zone_preview"))
    };
    names_zone_preview(value.get("message"))
        || names_zone_preview(
            value
                .get("details")
                .and_then(|details| details.get("field")),
        )
}

/// The wire topic that carries `target`'s frames.
#[must_use]
pub const fn preview_topic(target: &PreviewTarget) -> &'static str {
    match target {
        PreviewTarget::SceneCanvas => "canvas",
        PreviewTarget::Zone(_) => "zone_preview",
    }
}

/// The subscribe entry for `target`'s preview stream.
///
/// Both topics take the same raw RGB shape at the TUI's full cadence. A
/// zone entry always names its zone: the daemon keeps a `zone_preview`
/// config across an unsubscribe, and an omitted list would mean every
/// zone.
#[must_use]
pub fn preview_subscription(target: &PreviewTarget) -> serde_json::Value {
    let mut config = serde_json::json!({ "fps": TUI_CANVAS_FPS, "format": "rgb" });
    if let PreviewTarget::Zone(zone_id) = target {
        config["zones"] = serde_json::json!([zone_id.to_string()]);
    }
    serde_json::json!({ "topic": preview_topic(target), "config": config })
}

/// The messages that move the preview from `previous` to `next`.
///
/// Switching between the scene canvas and a zone drops the old topic
/// first, so the socket never streams both. Moving from one zone to
/// another re-subscribes `zone_preview`, which patches its zone list in
/// place.
#[must_use]
pub fn preview_transition_messages(
    previous: &PreviewTarget,
    next: &PreviewTarget,
) -> Vec<serde_json::Value> {
    if previous == next {
        return Vec::new();
    }
    let mut messages = Vec::with_capacity(2);
    if preview_topic(previous) != preview_topic(next) {
        messages.push(serde_json::json!({
            "type": "unsubscribe",
            "topics": [{ "topic": preview_topic(previous) }]
        }));
    }
    messages.push(preview_subscribe_message(next));
    messages
}

/// Whether `message` may reach the preview while it shows `target`.
///
/// Scene canvas frames belong to a scene-canvas preview and a zone's
/// frames to a preview of that zone; every other message passes.
#[must_use]
pub fn preview_shows(target: &PreviewTarget, message: &WsMessage) -> bool {
    match message {
        WsMessage::Canvas(_) => *target == PreviewTarget::SceneCanvas,
        WsMessage::ZonePreview { zone_id, .. } => target.follows_zone(zone_id),
        _ => true,
    }
}

async fn wait_for_subscription_ack<S>(
    read: &mut S,
    timeout: std::time::Duration,
) -> Result<(Option<serde_json::Value>, SubscribedAck)>
where
    S: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    tokio::time::timeout(timeout, async {
        let mut hello = None;
        loop {
            match read.next().await {
                Some(Ok(Message::Text(text))) => {
                    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
                        continue;
                    };
                    match value.get("type").and_then(serde_json::Value::as_str) {
                        Some("hello") => {
                            hello = Some(value);
                        }
                        Some("subscribed") => {
                            let acknowledgment = serde_json::from_value(value)
                                .context("Malformed subscription acknowledgment")?;
                            return Ok((hello, acknowledgment));
                        }
                        Some("error") => {
                            let detail = value
                                .get("message")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or("subscription rejected");
                            anyhow::bail!("Daemon rejected WebSocket subscription: {detail}");
                        }
                        _ => {}
                    }
                }
                Some(Ok(Message::Close(_))) | None => {
                    anyhow::bail!("WebSocket closed before subscription acknowledgment");
                }
                Some(Err(error)) => return Err(error.into()),
                Some(Ok(_)) => {}
            }
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("WebSocket subscription acknowledgment timed out"))?
}

fn build_ws_url(host: &str, port: u16, api_key: Option<&str>) -> String {
    let base = format!("ws://{host}:{port}/api/v1/ws");
    api_key.map_or(base.clone(), |key| {
        format!("{base}?token={}", percent_encode(key))
    })
}

fn percent_encode(input: &str) -> String {
    let mut encoded = String::with_capacity(input.len());
    for byte in input.bytes() {
        let unreserved = byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~');
        if unreserved {
            encoded.push(char::from(byte));
        } else {
            let _ = std::fmt::Write::write_fmt(&mut encoded, format_args!("%{byte:02X}"));
        }
    }
    encoded
}

/// Decode a binary WebSocket message via the shared wire codec.
///
/// Canvas and zone frames are decoded zero-copy (the pixel payload is a
/// refcounted slice of the message). Preview channels the TUI doesn't
/// render yet (screen/web-viewport/display previews) are recognized and
/// dropped.
pub fn decode_binary(decoder: &mut WsBinaryDecoder, data: &Bytes) -> Option<WsMessage> {
    decoder.decode(data)
}

pub struct WsBinaryDecoder {
    preview_chunks: PreviewChunkReassembler,
    started_at: std::time::Instant,
}

impl Default for WsBinaryDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl WsBinaryDecoder {
    #[must_use]
    pub fn new() -> Self {
        Self {
            preview_chunks: PreviewChunkReassembler::new(PreviewReassemblyLimits::default()),
            started_at: std::time::Instant::now(),
        }
    }

    pub fn decode(&mut self, data: &Bytes) -> Option<WsMessage> {
        let now_ms = u64::try_from(self.started_at.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.decode_at(data, now_ms)
    }

    pub fn decode_at(&mut self, data: &Bytes, now_ms: u64) -> Option<WsMessage> {
        if data.first() == Some(&PREVIEW_CHUNK_FRAME_TAG) {
            return match self.preview_chunks.push_at(data, now_ms) {
                Ok(Some(publication)) => decode_reassembled_preview(&publication),
                Ok(None) => None,
                Err(error) => {
                    tracing::warn!(%error, "Rejected preview chunk publication");
                    None
                }
            };
        }
        if data.first() == Some(&PREVIEW_CANCEL_FRAME_TAG) {
            match PreviewCancelFrame::decode_bytes(data).and_then(|cancellation| {
                self.preview_chunks
                    .cancel_publication(&cancellation)
                    .map(|_| ())
            }) {
                Ok(()) => {}
                Err(error) => tracing::warn!(%error, "Rejected preview cancellation"),
            }
            return None;
        }

        decode_unchunked_binary(data)
    }

    fn next_expiry_deadline(&self) -> Option<std::time::Instant> {
        self.next_expiry_ms().and_then(|deadline_ms| {
            self.started_at
                .checked_add(std::time::Duration::from_millis(deadline_ms))
        })
    }

    fn expire_now(&mut self) {
        let now_ms = u64::try_from(self.started_at.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.expire_at(now_ms);
    }

    pub fn next_expiry_ms(&self) -> Option<u64> {
        self.preview_chunks.next_expiry_ms()
    }

    pub fn expire_at(&mut self, now_ms: u64) -> usize {
        self.preview_chunks.expire_at(now_ms)
    }
}

fn decode_reassembled_preview(publication: &ReassembledPreviewPublication) -> Option<WsMessage> {
    let metadata = &publication.metadata;
    match &metadata.stream {
        PreviewStreamId::Passive(channel) => {
            let frame = PreviewFrame::decode_bytes(&publication.encoded).ok()?;
            if frame.channel != *channel
                || frame.frame_number != metadata.frame_number
                || frame.timestamp_ms != metadata.timestamp_ms
                || frame.width != metadata.width
                || frame.height != metadata.height
                || frame.format != metadata.format
            {
                tracing::warn!("Rejected preview publication with mismatched chunk metadata");
                return None;
            }
            decode_preview(&publication.encoded)
        }
        // The reassembler already held the frame to its stream's scene,
        // zone, and shape before completing the publication.
        PreviewStreamId::Zone { .. } => decode_zone_preview(&publication.encoded),
        _ => {
            tracing::trace!("Ignoring chunked preview publication the TUI does not render");
            None
        }
    }
}

fn decode_unchunked_binary(data: &Bytes) -> Option<WsMessage> {
    match *data.first()? {
        SPECTRUM_FRAME_TAG => decode_spectrum(data),
        ZONE_PREVIEW_FRAME_TAG | WIDE_ZONE_PREVIEW_FRAME_TAG => decode_zone_preview(data),
        _ => decode_preview(data),
    }
}

fn decode_zone_preview(data: &Bytes) -> Option<WsMessage> {
    match ZonePreviewFrame::decode_bytes(data) {
        Ok(frame) => zone_preview_message(frame),
        Err(error) => {
            tracing::trace!(%error, "Failed to decode binary zone preview frame");
            None
        }
    }
}

fn zone_preview_message(frame: ZonePreviewFrame) -> Option<WsMessage> {
    Some(WsMessage::ZonePreview {
        scene_id: frame.scene_id,
        zone_id: frame.zone_id,
        frame: CanvasFrame {
            frame_number: frame.frame_number,
            timestamp_ms: frame.timestamp_ms,
            width: frame.width,
            height: frame.height,
            pixels: rgb_pixels(frame.format, frame.payload)?,
        },
    })
}

fn decode_preview(data: &Bytes) -> Option<WsMessage> {
    let frame = match PreviewFrame::decode_bytes(data) {
        Ok(frame) => frame,
        Err(error) => {
            tracing::trace!(%error, "Failed to decode binary preview frame");
            return None;
        }
    };

    if frame.channel != PreviewFrameChannel::Canvas {
        tracing::trace!(channel = ?frame.channel, "Ignoring non-canvas preview frame");
        return None;
    }

    Some(WsMessage::Canvas(CanvasFrame {
        frame_number: frame.frame_number,
        timestamp_ms: frame.timestamp_ms,
        width: frame.width,
        height: frame.height,
        pixels: rgb_pixels(frame.format, frame.payload)?,
    }))
}

/// Normalize a preview payload to packed RGB. RGB passes through without
/// a copy; JPEG is dropped, since the TUI subscribes raw.
fn rgb_pixels(format: PreviewPixelFormat, payload: Bytes) -> Option<Bytes> {
    match format {
        PreviewPixelFormat::Rgb => Some(payload),
        PreviewPixelFormat::Rgba => rgba_to_rgb(&payload).map(Bytes::from),
        PreviewPixelFormat::Jpeg => {
            tracing::trace!("Ignoring JPEG preview frame (TUI subscribes raw)");
            None
        }
    }
}

fn rgba_to_rgb(pixel_data: &[u8]) -> Option<Vec<u8>> {
    let rgb_len = (pixel_data.len() / 4).checked_mul(3)?;
    let mut rgb = Vec::new();
    rgb.try_reserve_exact(rgb_len).ok()?;
    for chunk in pixel_data.as_chunks::<4>().0 {
        rgb.extend_from_slice(&chunk[..3]);
    }
    Some(rgb)
}

fn decode_spectrum(data: &Bytes) -> Option<WsMessage> {
    let frame = match SpectrumFrame::decode(data) {
        Ok(frame) => frame,
        Err(error) => {
            tracing::trace!(%error, "Failed to decode binary spectrum frame");
            return None;
        }
    };

    Some(WsMessage::Spectrum(SpectrumSnapshot {
        timestamp_ms: frame.timestamp_ms,
        level: frame.level,
        bass: frame.bass,
        mid: frame.mid,
        treble: frame.treble,
        beat: frame.beat,
        beat_confidence: frame.beat_confidence,
        bpm: None, // BPM not in the binary spectrum format
        bins: frame.bins,
    }))
}

/// Decode a JSON text message.
pub fn decode_json(text: &str) -> Option<WsMessage> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let msg_type = value.get("type")?.as_str()?;

    match msg_type {
        "hello" => Some(WsMessage::Hello(value)),
        "event" => Some(WsMessage::Event(value)),
        "metrics" => Some(WsMessage::Metrics(value)),
        "subscribed" => serde_json::from_value(value)
            .map(WsMessage::Subscribed)
            .map_err(|error| tracing::debug!(%error, "Malformed subscribed acknowledgment"))
            .ok(),
        "unsubscribed" | "ack" => None,
        "error" => {
            tracing::warn!("WS error: {value}");
            None
        }
        "backpressure" => {
            tracing::warn!("WS backpressure: {value}");
            None
        }
        other => {
            tracing::trace!("Unknown WS message type: {other}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use futures_util::stream;
    use tokio_tungstenite::tungstenite::{Error, Message};

    use super::{build_ws_url, wait_for_subscription_ack};

    #[test]
    fn websocket_url_includes_percent_encoded_api_key() {
        assert_eq!(
            build_ws_url("192.168.1.10", 9420, Some("hc key/1")),
            "ws://192.168.1.10:9420/api/v1/ws?token=hc%20key%2F1"
        );
    }

    #[test]
    fn websocket_url_omits_token_without_api_key() {
        assert_eq!(
            build_ws_url("localhost", 9420, None),
            "ws://localhost:9420/api/v1/ws"
        );
    }

    #[tokio::test]
    async fn subscription_rejection_fails_connection_admission() {
        let mut messages = stream::iter([Ok::<_, Error>(Message::Text(
            r#"{"type":"error","message":"forbidden"}"#.into(),
        ))]);
        let error = wait_for_subscription_ack(&mut messages, Duration::from_secs(1))
            .await
            .expect_err("subscription rejection must fail admission");

        assert!(error.to_string().contains("forbidden"));
    }

    #[tokio::test]
    async fn subscription_timeout_fails_connection_admission() {
        let mut messages = stream::pending::<Result<Message, Error>>();
        let error = wait_for_subscription_ack(&mut messages, Duration::ZERO)
            .await
            .expect_err("missing acknowledgment must fail admission");

        assert!(error.to_string().contains("timed out"));
    }
}
