//! WebSocket client for the Hypercolor daemon.
//!
//! Subscribes to canvas frames, spectrum data, and events over a persistent
//! WebSocket connection. Binary frames are decoded inline through the shared
//! wire codec in `hypercolor-leptos-ext` — the same one the web UI uses, so
//! the format has exactly one definition.

use anyhow::{Context, Result};
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use hypercolor_leptos_ext::ws::{
    PREVIEW_CANCEL_FRAME_TAG, PREVIEW_CHUNK_FRAME_TAG, PreviewCancelFrame, PreviewChunkReassembler,
    PreviewFrame, PreviewFrameChannel, PreviewPixelFormat, PreviewReassemblyLimits,
    PreviewStreamId, PreviewTransportCapability, ReassembledPreviewPublication, SPECTRUM_FRAME_TAG,
    SpectrumFrame, WIDE_ZONE_PREVIEW_FRAME_TAG, ZONE_PREVIEW_FRAME_TAG,
};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use crate::state::{CanvasFrame, SpectrumSnapshot};

const TUI_CANVAS_FPS: u8 = 60;
const TUI_MAX_PREVIEW_STREAMS: usize = 8;
const SUBSCRIPTION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

#[derive(Debug, serde::Deserialize)]
pub struct SubscribedAck {
    pub topics: Vec<serde_json::Value>,
    pub preview_transport: String,
}

/// Messages decoded from the WebSocket stream.
#[derive(Debug)]
pub enum WsMessage {
    /// Server hello with initial state.
    Hello(serde_json::Value),
    /// A canvas frame (binary, type 0x03).
    Canvas(CanvasFrame),
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
pub async fn connect(
    host: &str,
    port: u16,
    api_key: Option<&str>,
    tx: mpsc::UnboundedSender<WsMessage>,
) -> Result<()> {
    let url = build_ws_url(host, port, api_key);
    let (ws_stream, _response) = tokio_tungstenite::connect_async(&url)
        .await
        .with_context(|| format!("Failed to connect WebSocket at {url}"))?;

    let (mut write, mut read) = ws_stream.split();

    // Send subscription message
    let preview_transport = PreviewTransportCapability {
        max_streams: TUI_MAX_PREVIEW_STREAMS,
        ..PreviewTransportCapability::default()
    };
    let subscribe = serde_json::json!({
        "type": "subscribe",
        "preview_transport": preview_transport.encode(),
        "topics": [
            { "topic": "canvas", "config": { "fps": TUI_CANVAS_FPS, "format": "rgb" } },
            { "topic": "spectrum", "config": { "fps": 15, "bins": 64 } },
            { "topic": "events" },
            { "topic": "metrics", "config": { "interval_ms": 2000 } }
        ]
    });
    write
        .send(Message::Text(subscribe.to_string().into()))
        .await
        .context("Failed to send subscribe message")?;

    let mut binary_decoder = WsBinaryDecoder::new();
    let (hello, acknowledgment) =
        wait_for_subscription_ack(&mut read, &mut binary_decoder, SUBSCRIPTION_TIMEOUT).await?;
    tx.send(WsMessage::Subscribed(acknowledgment))
        .context("TUI bridge closed before subscription admission")?;
    if let Some(hello) = hello {
        tx.send(WsMessage::Hello(hello))
            .context("TUI bridge closed before hello delivery")?;
    }

    // Read loop
    loop {
        let next_message = if let Some(deadline) = binary_decoder.next_expiry_deadline() {
            tokio::select! {
                message = read.next() => message,
                () = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)) => {
                    binary_decoder.expire_now();
                    continue;
                }
            }
        } else {
            read.next().await
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

        let decoded = match msg {
            Message::Binary(data) => binary_decoder.decode(&data),
            Message::Text(text) => {
                let decoded = decode_json(&text);
                if let Some(WsMessage::Hello(hello)) = &decoded {
                    binary_decoder.apply_hello_capabilities(hello);
                }
                decoded
            }
            Message::Close(_) => Some(WsMessage::Closed),
            Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => None,
        };

        if let Some(ws_msg) = decoded {
            let is_closed = matches!(ws_msg, WsMessage::Closed);
            if tx.send(ws_msg).is_err() || is_closed {
                break;
            }
        }
    }

    let _ = tx.send(WsMessage::Closed);
    Ok(())
}

async fn wait_for_subscription_ack<S>(
    read: &mut S,
    binary_decoder: &mut WsBinaryDecoder,
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
                            binary_decoder.apply_hello_capabilities(&value);
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
/// Canvas frames are decoded zero-copy (the pixel payload is a refcounted
/// slice of the message). Preview channels the TUI doesn't render yet
/// (screen/web-viewport/display/zone previews) are recognized and dropped.
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
            preview_chunks: PreviewChunkReassembler::new(PreviewReassemblyLimits {
                max_streams: TUI_MAX_PREVIEW_STREAMS,
                ..PreviewReassemblyLimits::default()
            }),
            started_at: std::time::Instant::now(),
        }
    }

    fn apply_hello_capabilities(&mut self, message: &serde_json::Value) {
        let Some(capability) = message
            .get("capabilities")
            .and_then(serde_json::Value::as_array)
            .and_then(|capabilities| {
                PreviewTransportCapability::from_capabilities(
                    capabilities.iter().filter_map(serde_json::Value::as_str),
                )
            })
        else {
            return;
        };
        let limits = PreviewReassemblyLimits {
            max_streams: TUI_MAX_PREVIEW_STREAMS,
            ..PreviewReassemblyLimits::default()
        }
        .negotiated_with(capability);
        self.preview_chunks = PreviewChunkReassembler::new(limits);
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
    let frame = PreviewFrame::decode_bytes(&publication.encoded).ok()?;
    let PreviewStreamId::Passive(channel) = &publication.metadata.stream else {
        tracing::trace!("Ignoring non-passive chunked preview publication");
        return None;
    };
    if frame.channel != *channel
        || frame.frame_number != publication.metadata.frame_number
        || frame.timestamp_ms != publication.metadata.timestamp_ms
        || frame.width != publication.metadata.width
        || frame.height != publication.metadata.height
        || frame.format != publication.metadata.format
    {
        tracing::warn!("Rejected preview publication with mismatched chunk metadata");
        return None;
    }
    decode_preview(&publication.encoded)
}

fn decode_unchunked_binary(data: &Bytes) -> Option<WsMessage> {
    match *data.first()? {
        SPECTRUM_FRAME_TAG => decode_spectrum(data),
        ZONE_PREVIEW_FRAME_TAG | WIDE_ZONE_PREVIEW_FRAME_TAG => {
            tracing::trace!("Ignoring zone preview frame (not consumed yet)");
            None
        }
        _ => decode_preview(data),
    }
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

    let pixels = match frame.format {
        PreviewPixelFormat::Rgb => frame.payload,
        PreviewPixelFormat::Rgba => Bytes::from(rgba_to_rgb(&frame.payload)?),
        PreviewPixelFormat::Jpeg => {
            tracing::trace!("Ignoring JPEG canvas frame (TUI subscribes raw)");
            return None;
        }
    };

    Some(WsMessage::Canvas(CanvasFrame {
        frame_number: frame.frame_number,
        timestamp_ms: frame.timestamp_ms,
        width: frame.width,
        height: frame.height,
        pixels,
    }))
}

fn rgba_to_rgb(pixel_data: &[u8]) -> Option<Vec<u8>> {
    let rgb_len = (pixel_data.len() / 4).checked_mul(3)?;
    let mut rgb = Vec::new();
    rgb.try_reserve_exact(rgb_len).ok()?;
    for chunk in pixel_data.chunks_exact(4) {
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

    use super::{WsBinaryDecoder, build_ws_url, wait_for_subscription_ack};

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
        let mut decoder = WsBinaryDecoder::new();

        let error = wait_for_subscription_ack(&mut messages, &mut decoder, Duration::from_secs(1))
            .await
            .expect_err("subscription rejection must fail admission");

        assert!(error.to_string().contains("forbidden"));
    }

    #[tokio::test]
    async fn subscription_timeout_fails_connection_admission() {
        let mut messages = stream::pending::<Result<Message, Error>>();
        let mut decoder = WsBinaryDecoder::new();

        let error = wait_for_subscription_ack(&mut messages, &mut decoder, Duration::ZERO)
            .await
            .expect_err("missing acknowledgment must fail admission");

        assert!(error.to_string().contains("timed out"));
    }
}
