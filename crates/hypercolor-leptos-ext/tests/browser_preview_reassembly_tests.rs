#![cfg(all(target_arch = "wasm32", feature = "ws-client-wasm"))]

//! The JS-backed reassembler shares the native state machine, so every
//! step here runs through both and they must agree on the outcome and on
//! the bytes a completed publication carries.

use bytes::Bytes;
use hypercolor_leptos_ext::ws::{
    DEFAULT_PREVIEW_MAX_MESSAGE_BYTES, JsPreviewPublicationBuffer, PreviewCancelFrame,
    PreviewChunkError, PreviewChunkReassembler, PreviewFrame, PreviewFrameChannel,
    PreviewFrameView, PreviewPixelFormat, PreviewPublicationMetadata, PreviewReassemblyLimits,
    PreviewStreamId, ReassembledPreviewBuffer, ReassembledPreviewPublication, ScreenZonesFrame,
    ZonePreviewFrame, split_preview_publication,
};
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

wasm_bindgen_test_configure!(run_in_browser);

type NativeOutcome = Result<Option<ReassembledPreviewPublication>, PreviewChunkError>;
type JsOutcome = Result<Option<ReassembledPreviewBuffer>, PreviewChunkError>;

struct Pair {
    native: PreviewChunkReassembler,
    js: PreviewChunkReassembler<JsPreviewPublicationBuffer>,
}

impl Pair {
    fn new(limits: PreviewReassemblyLimits) -> Self {
        Self {
            native: PreviewChunkReassembler::new(limits),
            js: PreviewChunkReassembler::with_limits(limits),
        }
    }

    /// Push one chunk through both and return the completed bytes, after
    /// checking both sides agree.
    fn push(&mut self, chunk: &Bytes, now_ms: u64) -> Result<Option<Bytes>, PreviewChunkError> {
        let native = self.native.push_at(chunk, now_ms);
        let js = self.js.push_array_buffer_at(&array_buffer(chunk), now_ms);
        assert_outcomes_agree(&native, &js);
        assert_eq!(self.native.partial_count(), self.js.partial_count());
        assert_eq!(self.native.reserved_bytes(), self.js.reserved_bytes());
        assert_eq!(self.native.tombstone_count(), self.js.tombstone_count());
        assert_eq!(self.native.state_bytes(), self.js.state_bytes());
        assert_eq!(self.native.tombstone_bytes(), self.js.tombstone_bytes());
        assert_eq!(
            self.native.superseded_publications(),
            self.js.superseded_publications()
        );
        assert_eq!(
            self.native.expired_publications(),
            self.js.expired_publications()
        );
        native.map(|completed| completed.map(|publication| publication.encoded))
    }

    fn cancel(&mut self, cancellation: &PreviewCancelFrame) {
        assert_eq!(
            self.native.cancel_publication(cancellation),
            self.js.cancel_publication(cancellation)
        );
        assert_eq!(self.native.partial_count(), self.js.partial_count());
        assert_eq!(self.native.reserved_bytes(), self.js.reserved_bytes());
    }
}

fn array_buffer(bytes: &[u8]) -> js_sys::ArrayBuffer {
    js_sys::Uint8Array::from(bytes).buffer()
}

fn js_bytes(buffer: &js_sys::ArrayBuffer) -> Vec<u8> {
    js_sys::Uint8Array::new(buffer).to_vec()
}

fn assert_outcomes_agree(native: &NativeOutcome, js: &JsOutcome) {
    match (native, js) {
        (Ok(None), Ok(None)) => {}
        (Ok(Some(native)), Ok(Some(js))) => {
            assert_eq!(native.metadata, js.metadata);
            assert_eq!(native.encoded.as_ref(), js_bytes(&js.encoded).as_slice());
        }
        (Err(native), Err(js)) => assert_eq!(native, js),
        (native, js) => panic!("reassemblers disagree: native {native:?}, js {js:?}"),
    }
}

fn canvas_frame(frame_number: u32, width: u32, height: u32) -> (Bytes, PreviewPublicationMetadata) {
    let pixel_bytes = usize::try_from(width * height * 4).expect("fixture fits");
    let payload: Vec<u8> = (0..pixel_bytes)
        .map(|index| u8::try_from(index % 251).expect("below 251"))
        .collect();
    let frame = PreviewFrame {
        channel: PreviewFrameChannel::Canvas,
        frame_number,
        timestamp_ms: frame_number * 16,
        width,
        height,
        format: PreviewPixelFormat::Rgba,
        payload: Bytes::from(payload),
    };
    let metadata = PreviewPublicationMetadata {
        stream: PreviewStreamId::Passive(frame.channel),
        publication_id: u64::from(frame_number),
        frame_number,
        timestamp_ms: frame.timestamp_ms,
        width,
        height,
        format: frame.format,
    };
    (frame.encode(), metadata)
}

#[wasm_bindgen_test]
fn a_chunked_canvas_frame_reassembles_in_js_byte_for_byte() {
    let (encoded, metadata) = canvas_frame(7, 640, 480);
    let chunks = split_preview_publication(&encoded, &metadata, DEFAULT_PREVIEW_MAX_MESSAGE_BYTES)
        .expect("chunks split");
    assert!(
        chunks.len() > 1,
        "a 640x480 RGBA frame needs several messages"
    );

    let mut pair = Pair::new(PreviewReassemblyLimits::default());
    let mut completed = None;
    for (index, chunk) in chunks.iter().enumerate() {
        let outcome = pair.push(chunk, 1).expect("valid chunk");
        assert_eq!(outcome.is_some(), index + 1 == chunks.len());
        completed = outcome.or(completed);
    }
    assert_eq!(completed.as_ref(), Some(&encoded));

    let mut js = PreviewChunkReassembler::<JsPreviewPublicationBuffer>::with_limits(
        PreviewReassemblyLimits::default(),
    );
    let mut assembled = None;
    for chunk in &chunks {
        assembled = js
            .push_array_buffer_at(&array_buffer(chunk), 1)
            .expect("valid chunk")
            .or(assembled);
    }
    let assembled = assembled.expect("publication completes");
    let view = PreviewFrameView::decode_array_buffer(&assembled.encoded).expect("view decodes");
    assert_eq!((view.width, view.height), (640, 480));
    assert_eq!(view.format, PreviewPixelFormat::Rgba);
    assert_eq!(view.payload.length(), 640 * 480 * 4);
    assert_eq!(view.rgba_at(1), Some([4, 5, 6, 7]));
}

#[wasm_bindgen_test]
fn one_byte_chunks_reassemble_in_js_for_screen_zones_and_zone_streams() {
    let screen_zones = ScreenZonesFrame {
        frame_number: 2,
        timestamp_ms: 22,
        source_width: 3840,
        source_height: 2160,
        grid_cols: 1,
        grid_rows: 1,
        letterbox: [0; 4],
        payload: Bytes::from_static(&[4, 5, 6]),
    };
    let screen_metadata = PreviewPublicationMetadata {
        stream: PreviewStreamId::ScreenZones,
        publication_id: 2,
        frame_number: 2,
        timestamp_ms: 22,
        width: 3840,
        height: 2160,
        format: PreviewPixelFormat::Rgb,
    };
    let zone = ZonePreviewFrame {
        scene_id: [0x11; 16],
        zone_id: [0x22; 16],
        frame_number: 3,
        timestamp_ms: 33,
        width: 1,
        height: 1,
        format: PreviewPixelFormat::Rgb,
        payload: Bytes::from_static(&[7, 8, 9]),
    };
    let zone_metadata = PreviewPublicationMetadata {
        stream: PreviewStreamId::Zone {
            scene_id: zone.scene_id,
            zone_id: zone.zone_id,
        },
        publication_id: 3,
        frame_number: 3,
        timestamp_ms: 33,
        width: 1,
        height: 1,
        format: PreviewPixelFormat::Rgb,
    };
    for (encoded, metadata, identity_len) in [
        (screen_zones.encode(), screen_metadata, 0),
        (zone.encode(), zone_metadata, 32),
    ] {
        let max_message_bytes =
            hypercolor_leptos_ext::ws::PREVIEW_CHUNK_FIXED_HEADER_LEN + identity_len + 1;
        let chunks = split_preview_publication(&encoded, &metadata, max_message_bytes)
            .expect("one-byte chunks");
        assert_eq!(chunks.len(), encoded.len());
        let mut pair = Pair::new(PreviewReassemblyLimits::default());
        let mut completed = None;
        for chunk in &chunks {
            completed = pair.push(chunk, 1).expect("valid chunk").or(completed);
        }
        assert_eq!(completed, Some(encoded));
    }
}

#[wasm_bindgen_test]
fn js_and_native_reassemblers_agree_on_misordered_superseded_and_cancelled_chunks() {
    let (first, first_metadata) = canvas_frame(1, 64, 48);
    let (second, second_metadata) = canvas_frame(2, 64, 48);
    let (third, third_metadata) = canvas_frame(3, 64, 48);
    let first_chunks = split_preview_publication(&first, &first_metadata, 4096).expect("split");
    let second_chunks = split_preview_publication(&second, &second_metadata, 4096).expect("split");
    let third_chunks = split_preview_publication(&third, &third_metadata, 4096).expect("split");
    assert!(first_chunks.len() >= 3);

    let mut pair = Pair::new(PreviewReassemblyLimits::default());
    // A publication cannot start mid-way.
    assert!(pair.push(&first_chunks[1], 10).is_err());
    assert_eq!(pair.push(&first_chunks[0], 11), Ok(None));
    // A repeated chunk and a skipped one are both rejected.
    assert!(pair.push(&first_chunks[0], 12).is_err());
    assert!(pair.push(&first_chunks[2], 13).is_err());
    // A newer publication supersedes the partial one, and the older is stale.
    assert_eq!(pair.push(&second_chunks[0], 14), Ok(None));
    assert!(pair.push(&first_chunks[0], 15).is_err());
    let mut completed = None;
    for chunk in &second_chunks[1..] {
        completed = pair
            .push(chunk, 16)
            .expect("second continues")
            .or(completed);
    }
    assert_eq!(completed, Some(second));
    // A cancelled publication drops its partial state on both sides.
    assert_eq!(pair.push(&third_chunks[0], 17), Ok(None));
    pair.cancel(&PreviewCancelFrame {
        stream: third_metadata.stream.clone(),
        publication_id: third_metadata.publication_id,
    });
    assert!(pair.push(&third_chunks[1], 18).is_err());
}

#[wasm_bindgen_test]
fn js_and_native_reassemblers_expire_an_idle_partial_together() {
    let (encoded, metadata) = canvas_frame(4, 64, 48);
    let chunks = split_preview_publication(&encoded, &metadata, 4096).expect("split");
    let limits = PreviewReassemblyLimits::default();
    let mut pair = Pair::new(limits);
    assert_eq!(pair.push(&chunks[0], 100), Ok(None));
    let later = 100 + limits.max_idle_ms;
    assert_eq!(pair.native.expire_at(later), pair.js.expire_at(later));
    assert_eq!(pair.native.partial_count(), 0);
    assert_eq!(pair.js.partial_count(), 0);
    assert!(pair.push(&chunks[1], later + 1).is_err());
}

#[wasm_bindgen_test]
fn an_oversized_message_is_rejected_before_it_is_decoded() {
    let limits = PreviewReassemblyLimits {
        max_message_bytes: 64,
        ..PreviewReassemblyLimits::default()
    };
    let mut pair = Pair::new(limits);
    let garbage = Bytes::from(vec![0x0F; 65]);
    assert!(matches!(
        pair.push(&garbage, 1),
        Err(PreviewChunkError::MessageBudgetExceeded { .. })
    ));
    let short = Bytes::from_static(&[0x0F, 1, 0]);
    assert!(matches!(
        pair.push(&short, 2),
        Err(PreviewChunkError::TooShort { .. })
    ));
}
