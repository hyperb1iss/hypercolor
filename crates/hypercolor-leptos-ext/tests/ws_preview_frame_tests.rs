#![cfg(all(feature = "ws-core", not(target_arch = "wasm32")))]

use bytes::Bytes;
use hypercolor_leptos_ext::ws::{
    PREVIEW_FRAME_HEADER_LEN, PreviewFrame, PreviewFrameChannel, PreviewFrameDecodeError,
    PreviewPixelFormat, ZONE_PREVIEW_FRAME_HEADER_LEN, ZONE_PREVIEW_FRAME_TAG, ZonePreviewFrame,
};

#[test]
fn preview_frame_roundtrips_rgba_payload() {
    let frame = PreviewFrame {
        channel: PreviewFrameChannel::Canvas,
        frame_number: 42,
        timestamp_ms: 9001,
        width: 2,
        height: 1,
        format: PreviewPixelFormat::Rgba,
        payload: Bytes::from_static(&[1, 2, 3, 4, 5, 6, 7, 8]),
    };

    let encoded = frame.encode();

    assert_eq!(encoded.len(), PREVIEW_FRAME_HEADER_LEN + 8);
    assert_eq!(PreviewFrame::decode(&encoded), Ok(frame));
}

#[test]
fn preview_frame_keeps_jpeg_payload_variable_length() {
    let frame = PreviewFrame {
        channel: PreviewFrameChannel::DisplayPreview,
        frame_number: 7,
        timestamp_ms: 11,
        width: 640,
        height: 480,
        format: PreviewPixelFormat::Jpeg,
        payload: Bytes::from_static(b"jpeg-bytes"),
    };

    assert_eq!(PreviewFrame::decode(&frame.encode()), Ok(frame));
}

#[test]
fn zone_preview_frame_roundtrips_addressed_rgb_payload() {
    let frame = ZonePreviewFrame {
        scene_id: [0x11; 16],
        zone_id: [0x22; 16],
        frame_number: 42,
        timestamp_ms: 9001,
        width: 2,
        height: 1,
        format: PreviewPixelFormat::Rgb,
        payload: Bytes::from_static(&[1, 2, 3, 4, 5, 6]),
    };

    let encoded = frame.encode();

    assert_eq!(encoded[0], ZONE_PREVIEW_FRAME_TAG);
    assert_eq!(encoded.len(), ZONE_PREVIEW_FRAME_HEADER_LEN + 6);
    assert_eq!(ZonePreviewFrame::decode(&encoded), Ok(frame));
}

#[test]
fn preview_frame_rejects_unknown_channel() {
    let mut encoded = PreviewFrame {
        channel: PreviewFrameChannel::Canvas,
        frame_number: 1,
        timestamp_ms: 2,
        width: 1,
        height: 1,
        format: PreviewPixelFormat::Rgb,
        payload: Bytes::from_static(&[1, 2, 3]),
    }
    .encode()
    .to_vec();
    encoded[0] = 0xff;

    assert_eq!(
        PreviewFrame::decode(&encoded),
        Err(PreviewFrameDecodeError::UnknownChannel { actual: 0xff })
    );
}

#[test]
fn preview_frame_rejects_short_raw_payload() {
    let encoded = PreviewFrame {
        channel: PreviewFrameChannel::ScreenCanvas,
        frame_number: 1,
        timestamp_ms: 2,
        width: 2,
        height: 2,
        format: PreviewPixelFormat::Rgb,
        payload: Bytes::from_static(&[1, 2, 3]),
    }
    .encode();

    assert_eq!(
        PreviewFrame::decode(&encoded),
        Err(PreviewFrameDecodeError::PayloadTooShort {
            expected: 12,
            actual: 3,
        })
    );
}

#[test]
fn preview_frame_decode_bytes_matches_decode_and_shares_buffer() {
    let frame = PreviewFrame {
        channel: PreviewFrameChannel::Canvas,
        frame_number: 9,
        timestamp_ms: 100,
        width: 2,
        height: 2,
        format: PreviewPixelFormat::Rgb,
        payload: Bytes::from_static(&[0; 12]),
    };
    let encoded = frame.encode();

    let owned = PreviewFrame::decode(&encoded).expect("slice decode");
    let shared = PreviewFrame::decode_bytes(&encoded).expect("bytes decode");

    assert_eq!(owned, shared);
    // Zero-copy: the payload points into the encoded buffer.
    assert_eq!(
        shared.payload.as_ptr() as usize,
        encoded.as_ptr() as usize + PREVIEW_FRAME_HEADER_LEN,
    );
}

#[test]
fn zone_preview_frame_decode_bytes_matches_decode() {
    let frame = ZonePreviewFrame {
        scene_id: [0x0A; 16],
        zone_id: [0x0B; 16],
        frame_number: 3,
        timestamp_ms: 30,
        width: 1,
        height: 1,
        format: PreviewPixelFormat::Rgba,
        payload: Bytes::from_static(&[9, 8, 7, 6]),
    };
    let encoded = frame.encode();

    assert_eq!(
        ZonePreviewFrame::decode(&encoded).expect("slice decode"),
        ZonePreviewFrame::decode_bytes(&encoded).expect("bytes decode"),
    );
}
