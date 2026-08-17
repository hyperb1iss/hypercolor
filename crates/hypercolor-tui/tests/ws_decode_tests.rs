//! Tests for WebSocket binary frame decoding.
//!
//! Binary fixtures are built with the SHARED wire codec
//! (`hypercolor-leptos-ext`) — the same encoder family the daemon conforms
//! to — so these tests prove the TUI decodes exactly what the daemon sends.

use bytes::Bytes;
use hypercolor_leptos_ext::ws::{
    PreviewCancelFrame, PreviewFrame, PreviewFrameChannel, PreviewPixelFormat,
    PreviewPublicationMetadata, PreviewStreamId, SpectrumFrame, TimedInputEventPayload,
    ZONE_PREVIEW_FRAME_TAG, ZonePreviewFrame, split_preview_publication,
};
use hypercolor_tui::client::ws::{self, WsBinaryDecoder, WsMessage};

fn decode_binary_once(data: &Bytes) -> Option<WsMessage> {
    ws::decode_binary(&mut WsBinaryDecoder::new(), data)
}

fn canvas_frame(format: PreviewPixelFormat, width: u32, height: u32, pixels: &[u8]) -> Bytes {
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

fn jpeg_payload(width: u16, height: u16) -> Vec<u8> {
    vec![
        0xFF,
        0xD8,
        0xFF,
        0xC0,
        0x00,
        0x07,
        0x08,
        height.to_be_bytes()[0],
        height.to_be_bytes()[1],
        width.to_be_bytes()[0],
        width.to_be_bytes()[1],
    ]
}

// ── Canvas decode tests ──────────────────────────────────────────

#[test]
fn decode_canvas_rgb_roundtrip_is_zero_copy() {
    let data = canvas_frame(PreviewPixelFormat::Rgb, 1, 1, &[255, 0, 128]);
    let msg = decode_binary_once(&data);
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
    let Some(WsMessage::Canvas(frame)) = decode_binary_once(&data) else {
        panic!("expected Canvas variant");
    };
    assert_eq!(frame.pixels, Bytes::from(vec![100, 200, 50]));
}

#[test]
fn decode_wide_canvas_preserves_u32_dimensions() {
    let pixels = vec![0_u8; 65_536 * 3];
    let data = canvas_frame(PreviewPixelFormat::Rgb, 65_536, 1, &pixels);
    let Some(WsMessage::Canvas(frame)) = decode_binary_once(&data) else {
        panic!("wide canvas should decode");
    };
    assert_eq!(frame.width, 65_536);
    assert_eq!(frame.height, 1);
}

#[test]
fn stateful_decoder_reassembles_chunked_canvas_once() {
    let encoded = canvas_frame(PreviewPixelFormat::Rgb, 2, 1, &[1, 2, 3, 4, 5, 6]);
    let metadata = PreviewPublicationMetadata {
        stream: PreviewStreamId::Passive(PreviewFrameChannel::Canvas),
        publication_id: 9,
        frame_number: 1,
        timestamp_ms: 42,
        width: 2,
        height: 1,
        format: PreviewPixelFormat::Rgb,
    };
    let chunks = split_preview_publication(&encoded, &metadata, 70).expect("frame chunks");
    assert!(chunks.len() > 1);
    let mut decoder = WsBinaryDecoder::new();
    for chunk in &chunks[..chunks.len() - 1] {
        assert!(ws::decode_binary(&mut decoder, chunk).is_none());
    }
    let Some(WsMessage::Canvas(frame)) =
        ws::decode_binary(&mut decoder, chunks.last().expect("last chunk"))
    else {
        panic!("complete publication should activate once");
    };
    assert_eq!(frame.width, 2);
    assert_eq!(frame.height, 1);
    assert_eq!(frame.pixels.as_ref(), &[1, 2, 3, 4, 5, 6]);
    assert!(decoder.decode(chunks.last().expect("last chunk")).is_none());
}

#[test]
fn cancellation_aborts_partial_canvas_publication() {
    let encoded = canvas_frame(PreviewPixelFormat::Rgb, 2, 1, &[1, 2, 3, 4, 5, 6]);
    let metadata = PreviewPublicationMetadata {
        stream: PreviewStreamId::Passive(PreviewFrameChannel::Canvas),
        publication_id: 11,
        frame_number: 1,
        timestamp_ms: 42,
        width: 2,
        height: 1,
        format: PreviewPixelFormat::Rgb,
    };
    let chunks = split_preview_publication(&encoded, &metadata, 70).expect("frame chunks");
    let cancellation = PreviewCancelFrame {
        stream: metadata.stream.clone(),
        publication_id: metadata.publication_id,
    }
    .try_encode()
    .expect("cancellation encodes");
    let mut decoder = WsBinaryDecoder::new();

    assert!(decoder.decode_at(&chunks[0], 100).is_none());
    assert!(decoder.decode_at(&cancellation, 101).is_none());
    for chunk in &chunks[1..] {
        assert!(decoder.decode_at(chunk, 102).is_none());
    }
}

#[test]
fn silent_partial_canvas_expires_on_wall_clock_deadline() {
    let encoded = canvas_frame(PreviewPixelFormat::Rgb, 2, 1, &[1, 2, 3, 4, 5, 6]);
    let metadata = PreviewPublicationMetadata {
        stream: PreviewStreamId::Passive(PreviewFrameChannel::Canvas),
        publication_id: 12,
        frame_number: 1,
        timestamp_ms: 42,
        width: 2,
        height: 1,
        format: PreviewPixelFormat::Rgb,
    };
    let chunks = split_preview_publication(&encoded, &metadata, 70).expect("frame chunks");
    let mut decoder = WsBinaryDecoder::new();

    assert!(decoder.decode_at(&chunks[0], 100).is_none());
    let deadline = decoder.next_expiry_ms().expect("partial has a deadline");
    assert_eq!(decoder.expire_at(deadline), 1);
    for chunk in &chunks[1..] {
        assert!(decoder.decode_at(chunk, deadline + 1).is_none());
    }
}

#[test]
fn decode_canvas_jpeg_returns_none() {
    let data = canvas_frame(PreviewPixelFormat::Jpeg, 320, 200, &jpeg_payload(320, 200));
    assert!(decode_binary_once(&data).is_none());
}

#[test]
fn decode_canvas_truncated_pixels_returns_none() {
    // Header says 2x2 RGB (needs 12 payload bytes); hand-truncate to 3.
    let full = canvas_frame(PreviewPixelFormat::Rgb, 2, 2, &[0; 12]);
    let truncated = full.slice(..14 + 3);
    assert!(decode_binary_once(&truncated).is_none());
}

#[test]
fn decode_non_canvas_preview_channels_return_none() {
    for channel in [
        PreviewFrameChannel::ScreenCanvas,
        PreviewFrameChannel::WebViewportCanvas,
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
            decode_binary_once(&data).is_none(),
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
    assert!(decode_binary_once(&data).is_none());
}

// ── Spectrum decode tests ────────────────────────────────────────

#[test]
fn decode_spectrum_with_bins() {
    let data = spectrum_frame(vec![0.1, 0.5, 0.9, 0.3]);
    let Some(WsMessage::Spectrum(snap)) = decode_binary_once(&data) else {
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
    let Some(WsMessage::Spectrum(snap)) = decode_binary_once(&data) else {
        panic!("expected Spectrum variant");
    };
    assert!(snap.bins.is_empty());
}

#[test]
fn decode_spectrum_too_short_returns_none() {
    assert!(decode_binary_once(&Bytes::from_static(&[0x02; 10])).is_none());
}

#[test]
fn decode_spectrum_beat_false() {
    let mut data = spectrum_frame(Vec::new()).to_vec();
    data[22] = 0; // beat = false
    let Some(WsMessage::Spectrum(snap)) = decode_binary_once(&Bytes::from(data)) else {
        panic!("expected Spectrum variant");
    };
    assert!(!snap.beat);
}

// ── Binary dispatch tests ────────────────────────────────────────

#[test]
fn decode_binary_unknown_type_returns_none() {
    assert!(decode_binary_once(&Bytes::from_static(&[0xFF, 0, 0, 0])).is_none());
}

#[test]
fn decode_binary_empty_returns_none() {
    assert!(decode_binary_once(&Bytes::new()).is_none());
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
fn decode_json_preserves_canonical_timed_input_event() {
    let json = r#"{"type":"event","event":"input_event_received","timestamp":"2026-07-27T00:00:00.000Z","data":{"event":{"kind":"key","source_id":"host:kbd","key":"a","state":"repeated"},"at_ms":900,"seq":12,"physical_code":"evdev:key:30","repeat_count":5}}"#;
    let Some(WsMessage::Event(message)) = ws::decode_json(json) else {
        panic!("expected event message");
    };
    let decoded = TimedInputEventPayload::decode(&message["data"])
        .expect("TUI event data should use the shared input schema");

    assert_eq!(decoded.at_ms, 900);
    assert_eq!(decoded.seq, 12);
    assert_eq!(decoded.physical_code.as_deref(), Some("evdev:key:30"));
    assert_eq!(decoded.repeat_count, 5);
    assert_eq!(decoded.event["key"], "a");
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
