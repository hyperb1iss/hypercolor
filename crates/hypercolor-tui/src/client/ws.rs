//! WebSocket client for the Hypercolor daemon.
//!
//! Subscribes to canvas frames, spectrum data, and events over a persistent
//! WebSocket connection. Binary frames are decoded inline.

use anyhow::{Context, Result};
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
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
            Message::Binary(data) => decode_binary_owned(data),
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

/// Decode a binary WebSocket message by its type header byte.
pub fn decode_binary(data: &[u8]) -> Option<WsMessage> {
    if data.is_empty() {
        return None;
    }

    match data[0] {
        0x03 => decode_canvas(data),
        0x02 => decode_spectrum(data),
        _ => {
            tracing::trace!("Unknown binary message type: 0x{:02x}", data[0]);
            None
        }
    }
}

fn decode_binary_owned(data: Bytes) -> Option<WsMessage> {
    if data.is_empty() {
        return None;
    }

    match data[0] {
        0x03 => decode_canvas_owned(data),
        0x02 => decode_spectrum(&data),
        _ => {
            tracing::trace!("Unknown binary message type: 0x{:02x}", data[0]);
            None
        }
    }
}

/// Decode a canvas frame (type 0x03).
///
/// Layout:
///   - 0:     header (0x03)
///   - 1-4:   `frame_number` (u32 LE)
///   - 5-8:   `timestamp_ms` (u32 LE)
///   - 9-10:  width (u16 LE)
///   - 11-12: height (u16 LE)
///   - 13:    format (0=RGB, 1=RGBA)
///   - 14+:   pixel data
pub fn decode_canvas(data: &[u8]) -> Option<WsMessage> {
    decode_canvas_impl(data, None)
}

fn decode_canvas_owned(data: Bytes) -> Option<WsMessage> {
    decode_canvas_impl(data.as_ref(), Some(data.clone()))
}

fn decode_canvas_impl(data: &[u8], owned: Option<Bytes>) -> Option<WsMessage> {
    if data.len() < 14 {
        return None;
    }

    let frame_number = u32::from_le_bytes([data[1], data[2], data[3], data[4]]);
    let timestamp_ms = u32::from_le_bytes([data[5], data[6], data[7], data[8]]);
    let width = u16::from_le_bytes([data[9], data[10]]);
    let height = u16::from_le_bytes([data[11], data[12]]);
    let format = data[13];

    let bpp: usize = if format == 0 { 3 } else { 4 };
    let expected_len = 14 + usize::from(width) * usize::from(height) * bpp;

    if data.len() < expected_len {
        tracing::trace!(
            "Canvas frame too short: got {} bytes, expected {expected_len}",
            data.len()
        );
        return None;
    }

    let pixel_data = &data[14..expected_len];

    // If RGBA, strip alpha to get RGB
    let pixels = if format == 0 {
        owned.map_or_else(
            || Bytes::copy_from_slice(pixel_data),
            |data| data.slice(14..expected_len),
        )
    } else {
        Bytes::from(rgba_to_rgb(pixel_data))
    };

    Some(WsMessage::Canvas(CanvasFrame {
        frame_number,
        timestamp_ms,
        width,
        height,
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

/// Decode a spectrum snapshot (type 0x02).
///
/// Layout:
///   - 0:     header (0x02)
///   - 1-4:   `timestamp_ms` (u32 LE)
///   - 5:     `bin_count` (u8)
///   - 6-9:   level (f32 LE)
///   - 10-13: bass (f32 LE)
///   - 14-17: mid (f32 LE)
///   - 18-21: treble (f32 LE)
///   - 22:    beat (u8, 0 or 1)
///   - 23-26: `beat_confidence` (f32 LE)
///   - 27+:   bins (`bin_count` * f32 LE)
pub fn decode_spectrum(data: &[u8]) -> Option<WsMessage> {
    if data.len() < 27 {
        return None;
    }

    let timestamp_ms = u32::from_le_bytes([data[1], data[2], data[3], data[4]]);
    let bin_count = usize::from(data[5]);
    let level = f32::from_le_bytes([data[6], data[7], data[8], data[9]]);
    let bass = f32::from_le_bytes([data[10], data[11], data[12], data[13]]);
    let mid = f32::from_le_bytes([data[14], data[15], data[16], data[17]]);
    let treble = f32::from_le_bytes([data[18], data[19], data[20], data[21]]);
    let beat = data[22] != 0;
    let beat_confidence = f32::from_le_bytes([data[23], data[24], data[25], data[26]]);

    let bins_start = 27;
    let bins_end = bins_start + bin_count * 4;
    let bins = if data.len() >= bins_end {
        data[bins_start..bins_end]
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect()
    } else {
        Vec::new()
    };

    Some(WsMessage::Spectrum(SpectrumSnapshot {
        timestamp_ms,
        level,
        bass,
        mid,
        treble,
        beat,
        beat_confidence,
        bpm: None, // BPM not in the binary spectrum format
        bins,
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
