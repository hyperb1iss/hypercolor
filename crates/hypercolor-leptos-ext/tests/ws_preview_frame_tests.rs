#![cfg(feature = "ws-core")]

use bytes::Bytes;
use hypercolor_leptos_ext::ws::{
    PREVIEW_FRAME_HEADER_LEN, PreviewFrame, PreviewFrameChannel, PreviewFrameDecodeError,
    PreviewPixelFormat,
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
