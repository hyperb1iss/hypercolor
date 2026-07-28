#![cfg(all(feature = "ws-client-wasm", target_arch = "wasm32"))]

use bytes::Bytes;
use hypercolor_leptos_ext::ws::{
    INTERACTIVE_PREVIEW_FRAME_PREFIX_LEN, InteractivePreviewFrame, InteractivePreviewFrameView,
    PREVIEW_FRAME_HEADER_LEN, PreviewFrame, PreviewFrameChannel, PreviewFrameDecodeError,
    PreviewFrameView, PreviewPixelFormat, ZONE_PREVIEW_FRAME_HEADER_LEN, ZonePreviewFrame,
    ZonePreviewFrameView,
};
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

wasm_bindgen_test_configure!(run_in_browser);

fn jpeg_payload(width: u16, height: u16) -> Bytes {
    Bytes::from(vec![
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
    ])
}

fn mismatch_intrinsic_width(encoded: &mut [u8], payload_offset: usize) {
    encoded[payload_offset + 9..payload_offset + 11].copy_from_slice(&31_u16.to_be_bytes());
}

fn array_buffer(encoded: &[u8]) -> js_sys::ArrayBuffer {
    js_sys::Uint8Array::from(encoded).buffer()
}

#[wasm_bindgen_test]
fn direct_preview_rejects_mismatched_jpeg_dimensions() {
    let mut encoded = PreviewFrame {
        channel: PreviewFrameChannel::Canvas,
        frame_number: 1,
        timestamp_ms: 2,
        width: 32,
        height: 16,
        format: PreviewPixelFormat::Jpeg,
        payload: jpeg_payload(32, 16),
    }
    .encode()
    .to_vec();
    mismatch_intrinsic_width(&mut encoded, PREVIEW_FRAME_HEADER_LEN);

    assert!(matches!(
        PreviewFrameView::decode_array_buffer(&array_buffer(&encoded)),
        Err(PreviewFrameDecodeError::JpegDimensionsMismatch { .. })
    ));
}

#[wasm_bindgen_test]
fn direct_zone_preview_rejects_mismatched_jpeg_dimensions() {
    let mut encoded = ZonePreviewFrame {
        scene_id: [1; 16],
        zone_id: [2; 16],
        frame_number: 1,
        timestamp_ms: 2,
        width: 32,
        height: 16,
        format: PreviewPixelFormat::Jpeg,
        payload: jpeg_payload(32, 16),
    }
    .encode()
    .to_vec();
    mismatch_intrinsic_width(&mut encoded, ZONE_PREVIEW_FRAME_HEADER_LEN);

    assert!(matches!(
        ZonePreviewFrameView::decode_array_buffer(&array_buffer(&encoded)),
        Err(PreviewFrameDecodeError::JpegDimensionsMismatch { .. })
    ));
}

#[wasm_bindgen_test]
fn direct_interactive_preview_rejects_mismatched_jpeg_dimensions() {
    let preview_id = "main";
    let mut encoded = InteractivePreviewFrame {
        preview_id: preview_id.to_owned(),
        frame_number: 1,
        timestamp_ms: 2,
        width: 32,
        height: 16,
        format: PreviewPixelFormat::Jpeg,
        payload: jpeg_payload(32, 16),
    }
    .encode()
    .expect("interactive frame")
    .to_vec();
    mismatch_intrinsic_width(
        &mut encoded,
        INTERACTIVE_PREVIEW_FRAME_PREFIX_LEN + preview_id.len(),
    );

    assert!(matches!(
        InteractivePreviewFrameView::decode_array_buffer(&array_buffer(&encoded)),
        Err(PreviewFrameDecodeError::JpegDimensionsMismatch { .. })
    ));
}
