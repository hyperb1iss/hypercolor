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
    PreviewFrame, PreviewFrameChannel, PreviewPixelFormat, SPECTRUM_FRAME_TAG, SpectrumFrame,
    ZONE_PREVIEW_FRAME_TAG,
};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use crate::state::{CanvasFrame, SpectrumSnapshot};

const TUI_CANVAS_FPS: u8 = 60;

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
    let subscribe = serde_json::json!({
        "type": "subscribe",
        "channels": ["canvas", "spectrum", "events", "metrics"],
        "config": {
            "canvas": { "fps": TUI_CANVAS_FPS, "format": "rgb" },
            "spectrum": { "fps": 15, "bins": 64 },
            "metrics": { "interval_ms": 2000 }
        }
    });
    write
        .send(Message::Text(subscribe.to_string().into()))
        .await
        .context("Failed to send subscribe message")?;

    // Read loop
    while let Some(msg) = read.next().await {
        let msg = match msg {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("WebSocket error: {e}");
                break;
            }
        };

        let decoded = match msg {
            Message::Binary(data) => decode_binary(&data),
            Message::Text(text) => decode_json(&text),
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
pub fn decode_binary(data: &Bytes) -> Option<WsMessage> {
    match *data.first()? {
        SPECTRUM_FRAME_TAG => decode_spectrum(data),
        ZONE_PREVIEW_FRAME_TAG => {
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
        PreviewPixelFormat::Rgba => Bytes::from(rgba_to_rgb(&frame.payload)),
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

fn rgba_to_rgb(pixel_data: &[u8]) -> Vec<u8> {
    let mut rgb = Vec::with_capacity((pixel_data.len() / 4) * 3);
    for chunk in pixel_data.chunks_exact(4) {
        rgb.extend_from_slice(&chunk[..3]);
    }
    rgb
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
        "subscribed" | "unsubscribed" | "ack" => {
            tracing::debug!("WS ack: {msg_type}");
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
    use super::build_ws_url;

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
}
