//! Tests for WebSocket binary frame decoding.
//!
//! Binary fixtures are built with the SHARED wire codec
//! (`hypercolor-leptos-ext`) — the same encoder family the daemon conforms
//! to — so these tests prove the TUI decodes exactly what the daemon sends.

use bytes::Bytes;
use hypercolor_leptos_ext::ws::{
    PreviewFrame, PreviewFrameChannel, PreviewPixelFormat, SpectrumFrame, ZONE_PREVIEW_FRAME_TAG,
    ZonePreviewFrame,
};
use hypercolor_tui::client::ws::{self, WsMessage};

fn canvas_frame(format: PreviewPixelFormat, width: u16, height: u16, pixels: &[u8]) -> Bytes {
    PreviewFrame {
        channel: PreviewFrameChannel::Canvas,
        frame_number: 1,
        timestamp_ms: 42,
        width,
        height,
        format,
        payload: Bytes::copy_from_slice(pixels),
    }
    .encode()
}

fn spectrum_frame(bins: Vec<f32>) -> Bytes {
    SpectrumFrame {
        timestamp_ms: 100,
        level: 0.75,
        bass: 0.9,
        mid: 0.5,
        treble: 0.3,
        beat: true,
        beat_confidence: 0.85,
        bins,
    }
    .encode()
}

// ── Canvas decode tests ──────────────────────────────────────────

#[test]
fn decode_canvas_rgb_roundtrip_is_zero_copy() {
    let data = canvas_frame(PreviewPixelFormat::Rgb, 1, 1, &[255, 0, 128]);
    let msg = ws::decode_binary(&data);
    let Some(WsMessage::Canvas(frame)) = msg else {
        panic!("expected Canvas variant");
    };
    assert_eq!(frame.frame_number, 1);
    assert_eq!(frame.timestamp_ms, 42);
    assert_eq!(frame.width, 1);
    assert_eq!(frame.height, 1);
    assert_eq!(frame.pixels, Bytes::from(vec![255, 0, 128]));
    // Zero-copy: pixels point into the original message buffer.
    assert_eq!(frame.pixels.as_ptr() as usize, data.as_ptr() as usize + 14);
}

#[test]
fn decode_canvas_rgba_strips_alpha() {
    let data = canvas_frame(PreviewPixelFormat::Rgba, 1, 1, &[100, 200, 50, 255]);
    let Some(WsMessage::Canvas(frame)) = ws::decode_binary(&data) else {
        panic!("expected Canvas variant");
    };
    assert_eq!(frame.pixels, Bytes::from(vec![100, 200, 50]));
}

#[test]
fn decode_canvas_jpeg_returns_none() {
    let data = canvas_frame(PreviewPixelFormat::Jpeg, 320, 200, b"jpeg-bytes");
    assert!(ws::decode_binary(&data).is_none());
}

#[test]
fn decode_canvas_truncated_pixels_returns_none() {
    // Header says 2x2 RGB (needs 12 payload bytes); hand-truncate to 3.
    let full = canvas_frame(PreviewPixelFormat::Rgb, 2, 2, &[0; 12]);
    let truncated = full.slice(..14 + 3);
    assert!(ws::decode_binary(&truncated).is_none());
}

#[test]
fn decode_non_canvas_preview_channels_return_none() {
    for channel in [
        PreviewFrameChannel::ScreenCanvas,
        PreviewFrameChannel::WebViewportCanvas,
        PreviewFrameChannel::DisplayPreview,
    ] {
        let data = PreviewFrame {
            channel,
            frame_number: 1,
            timestamp_ms: 1,
            width: 1,
            height: 1,
            format: PreviewPixelFormat::Rgb,
            payload: Bytes::from_static(&[1, 2, 3]),
        }
        .encode();
        assert!(
            ws::decode_binary(&data).is_none(),
            "channel {channel:?} should be recognized but dropped"
        );
    }
}

#[test]
fn decode_zone_preview_returns_none_for_now() {
    let data = ZonePreviewFrame {
        scene_id: [0x11; 16],
        zone_id: [0x22; 16],
        frame_number: 1,
        timestamp_ms: 1,
        width: 1,
        height: 1,
        format: PreviewPixelFormat::Rgb,
        payload: Bytes::from_static(&[1, 2, 3]),
    }
    .encode();
    assert_eq!(data[0], ZONE_PREVIEW_FRAME_TAG);
    assert!(ws::decode_binary(&data).is_none());
}

// ── Spectrum decode tests ────────────────────────────────────────

#[test]
fn decode_spectrum_with_bins() {
    let data = spectrum_frame(vec![0.1, 0.5, 0.9, 0.3]);
    let Some(WsMessage::Spectrum(snap)) = ws::decode_binary(&data) else {
        panic!("expected Spectrum variant");
    };
    assert_eq!(snap.timestamp_ms, 100);
    assert!((snap.level - 0.75).abs() < f32::EPSILON);
    assert!((snap.bass - 0.9).abs() < f32::EPSILON);
    assert!((snap.mid - 0.5).abs() < f32::EPSILON);
    assert!((snap.treble - 0.3).abs() < f32::EPSILON);
    assert!(snap.beat);
    assert!((snap.beat_confidence - 0.85).abs() < f32::EPSILON);
    assert_eq!(snap.bins.len(), 4);
    assert!((snap.bins[2] - 0.9).abs() < f32::EPSILON);
    assert!(snap.bpm.is_none());
}

#[test]
fn decode_spectrum_no_bins() {
    let data = spectrum_frame(Vec::new());
    let Some(WsMessage::Spectrum(snap)) = ws::decode_binary(&data) else {
        panic!("expected Spectrum variant");
    };
    assert!(snap.bins.is_empty());
}

#[test]
fn decode_spectrum_too_short_returns_none() {
    assert!(ws::decode_binary(&Bytes::from_static(&[0x02; 10])).is_none());
}

#[test]
fn decode_spectrum_beat_false() {
    let mut data = spectrum_frame(Vec::new()).to_vec();
    data[22] = 0; // beat = false
    let Some(WsMessage::Spectrum(snap)) = ws::decode_binary(&Bytes::from(data)) else {
        panic!("expected Spectrum variant");
    };
    assert!(!snap.beat);
}

// ── Binary dispatch tests ────────────────────────────────────────

#[test]
fn decode_binary_unknown_type_returns_none() {
    assert!(ws::decode_binary(&Bytes::from_static(&[0xFF, 0, 0, 0])).is_none());
}

#[test]
fn decode_binary_empty_returns_none() {
    assert!(ws::decode_binary(&Bytes::new()).is_none());
}

// ── JSON decode tests ────────────────────────────────────────────

#[test]
fn decode_json_hello() {
    let json = r#"{"type": "hello", "state": {}}"#;
    let msg = ws::decode_json(json);
    assert!(matches!(msg, Some(WsMessage::Hello(_))));
}

#[test]
fn decode_json_event() {
    let json = r#"{"type": "event", "data": "test"}"#;
    let msg = ws::decode_json(json);
    assert!(matches!(msg, Some(WsMessage::Event(_))));
}

#[test]
fn decode_json_metrics() {
    let json = r#"{"type": "metrics", "fps": 30}"#;
    let msg = ws::decode_json(json);
    assert!(matches!(msg, Some(WsMessage::Metrics(_))));
}

#[test]
fn decode_json_metrics_with_data_envelope() {
    let json = r#"{"type":"metrics","data":{"fps":{"target":60,"actual":59.7},"devices":{"connected":2,"total_leds":180}}}"#;
    let msg = ws::decode_json(json);
    assert!(matches!(msg, Some(WsMessage::Metrics(_))));
}

#[test]
fn decode_json_ack_returns_none() {
    let json = r#"{"type": "subscribed"}"#;
    assert!(ws::decode_json(json).is_none());
}

#[test]
fn decode_json_unknown_type_returns_none() {
    let json = r#"{"type": "unknown_msg"}"#;
    assert!(ws::decode_json(json).is_none());
}

#[test]
fn decode_json_invalid_json_returns_none() {
    assert!(ws::decode_json("not json at all").is_none());
}

#[test]
fn decode_json_missing_type_returns_none() {
    assert!(ws::decode_json(r#"{"data": "no type field"}"#).is_none());
}
