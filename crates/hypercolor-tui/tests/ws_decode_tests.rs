//! Tests for WebSocket binary frame decoding.

use hypercolor_tui::client::ws::{self, WsMessage};

/// Helper: build a minimal valid canvas frame (type 0x03, RGB format).
fn build_canvas_frame(width: u16, height: u16, pixels: &[u8]) -> Vec<u8> {
    let mut data = Vec::with_capacity(14 + pixels.len());
    data.push(0x03); // header
    data.extend_from_slice(&1u32.to_le_bytes()); // frame_number
    data.extend_from_slice(&42u32.to_le_bytes()); // timestamp_ms
    data.extend_from_slice(&width.to_le_bytes()); // width
    data.extend_from_slice(&height.to_le_bytes()); // height
    data.push(0x00); // format = RGB
    data.extend_from_slice(pixels);
    data
}

/// Helper: build a minimal valid spectrum frame (type 0x02).
fn build_spectrum_frame(bins: &[f32]) -> Vec<u8> {
    let bin_count = bins.len();
    let mut data = Vec::with_capacity(27 + bin_count * 4);
    data.push(0x02); // header
    data.extend_from_slice(&100u32.to_le_bytes()); // timestamp_ms
    // Test helper: bin_count is always small, safe to truncate.
    #[allow(clippy::cast_possible_truncation, clippy::as_conversions)]
    data.push(bin_count as u8); // bin_count
    data.extend_from_slice(&0.75_f32.to_le_bytes()); // level
    data.extend_from_slice(&0.9_f32.to_le_bytes()); // bass
    data.extend_from_slice(&0.5_f32.to_le_bytes()); // mid
    data.extend_from_slice(&0.3_f32.to_le_bytes()); // treble
    data.push(1); // beat = true
    data.extend_from_slice(&0.85_f32.to_le_bytes()); // beat_confidence
    for &bin in bins {
        data.extend_from_slice(&bin.to_le_bytes());
    }
    data
}

// ── Canvas decode tests ──────────────────────────────────────────

#[test]
fn decode_canvas_rgb_1x1() {
    let data = build_canvas_frame(1, 1, &[255, 0, 128]);
    let msg = ws::decode_canvas(&data);
    let WsMessage::Canvas(frame) = msg.expect("should decode") else {
        panic!("expected Canvas variant");
    };
    assert_eq!(frame.frame_number, 1);
    assert_eq!(frame.timestamp_ms, 42);
    assert_eq!(frame.width, 1);
    assert_eq!(frame.height, 1);
    assert_eq!(frame.pixels, vec![255, 0, 128]);
}

#[test]
fn decode_canvas_rgb_2x2() {
    // 2x2 image: 4 pixels × 3 bytes = 12 bytes
    let pixels: Vec<u8> = (0..12).collect();
    let data = build_canvas_frame(2, 2, &pixels);
    let msg = ws::decode_canvas(&data);
    let WsMessage::Canvas(frame) = msg.expect("should decode") else {
        panic!("expected Canvas variant");
    };
    assert_eq!(frame.width, 2);
    assert_eq!(frame.height, 2);
    assert_eq!(frame.pixels.len(), 12);
}

#[test]
fn decode_canvas_rgba_strips_alpha() {
    // RGBA format: 1x1 pixel → 4 bytes, should produce 3 bytes output
    let mut data = Vec::new();
    data.push(0x03);
    data.extend_from_slice(&1u32.to_le_bytes());
    data.extend_from_slice(&0u32.to_le_bytes());
    data.extend_from_slice(&1u16.to_le_bytes());
    data.extend_from_slice(&1u16.to_le_bytes());
    data.push(0x01); // format = RGBA
    data.extend_from_slice(&[100, 200, 50, 255]); // RGBA pixel

    let msg = ws::decode_canvas(&data);
    let WsMessage::Canvas(frame) = msg.expect("should decode") else {
        panic!("expected Canvas variant");
    };
    assert_eq!(frame.pixels, vec![100, 200, 50]); // alpha stripped
}

#[test]
fn decode_canvas_too_short_returns_none() {
    // Less than 14-byte header
    assert!(ws::decode_canvas(&[0x03; 10]).is_none());
}

#[test]
fn decode_canvas_truncated_pixels_returns_none() {
    // Header says 2x2 (needs 12 pixel bytes) but only provides 3
    let data = build_canvas_frame(2, 2, &[255, 0, 0]);
    assert!(ws::decode_canvas(&data).is_none());
}

// ── Spectrum decode tests ────────────────────────────────────────

#[test]
fn decode_spectrum_with_bins() {
    let bins = [0.1, 0.5, 0.9, 0.3];
    let data = build_spectrum_frame(&bins);
    let msg = ws::decode_spectrum(&data);
    let WsMessage::Spectrum(snap) = msg.expect("should decode") else {
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
    let data = build_spectrum_frame(&[]);
    let msg = ws::decode_spectrum(&data);
    let WsMessage::Spectrum(snap) = msg.expect("should decode") else {
        panic!("expected Spectrum variant");
    };
    assert!(snap.bins.is_empty());
}

#[test]
fn decode_spectrum_too_short_returns_none() {
    assert!(ws::decode_spectrum(&[0x02; 10]).is_none());
}

#[test]
fn decode_spectrum_beat_false() {
    let mut data = build_spectrum_frame(&[]);
    data[22] = 0; // beat = false
    let msg = ws::decode_spectrum(&data);
    let WsMessage::Spectrum(snap) = msg.expect("should decode") else {
        panic!("expected Spectrum variant");
    };
    assert!(!snap.beat);
}

// ── Binary dispatch tests ────────────────────────────────────────

#[test]
fn decode_binary_dispatches_canvas() {
    let data = build_canvas_frame(1, 1, &[0, 0, 0]);
    let msg = ws::decode_binary(&data);
    assert!(matches!(msg, Some(WsMessage::Canvas(_))));
}

#[test]
fn decode_binary_dispatches_spectrum() {
    let data = build_spectrum_frame(&[0.5]);
    let msg = ws::decode_binary(&data);
    assert!(matches!(msg, Some(WsMessage::Spectrum(_))));
}

#[test]
fn decode_binary_unknown_type_returns_none() {
    assert!(ws::decode_binary(&[0xFF, 0, 0, 0]).is_none());
}

#[test]
fn decode_binary_empty_returns_none() {
    assert!(ws::decode_binary(&[]).is_none());
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
