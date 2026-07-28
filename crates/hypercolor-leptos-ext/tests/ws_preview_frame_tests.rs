#![cfg(all(feature = "ws-core", not(target_arch = "wasm32")))]

use bytes::Bytes;
use hypercolor_leptos_ext::ws::{
    DEFAULT_PREVIEW_MAX_CHUNK_COUNT, DEFAULT_PREVIEW_MAX_MESSAGE_BYTES,
    INTERACTIVE_PREVIEW_FRAME_PREFIX_LEN, INTERACTIVE_PREVIEW_FRAME_TAG,
    INTERACTIVE_PREVIEW_ID_MAX_BYTES, InteractivePreviewFrame, PREVIEW_CANCEL_FRAME_TAG,
    PREVIEW_CHUNK_FIXED_HEADER_LEN, PREVIEW_CHUNK_FRAME_TAG, PREVIEW_FRAME_HEADER_LEN,
    PreviewCancelFrame, PreviewChunkError, PreviewChunkFrame, PreviewChunkReassembler,
    PreviewFrame, PreviewFrameChannel, PreviewFrameDecodeError, PreviewPixelFormat,
    PreviewPublicationMetadata, PreviewReassemblyLimits, PreviewStreamId,
    PreviewTransportCapability, SCREEN_ZONES_FRAME_HEADER_LEN, SCREEN_ZONES_FRAME_TAG,
    ScreenZonesFrame, WIDE_INTERACTIVE_PREVIEW_FRAME_TAG, WIDE_PREVIEW_FRAME_TAG,
    WIDE_SCREEN_ZONES_FRAME_TAG, WIDE_ZONE_PREVIEW_FRAME_TAG, ZONE_PREVIEW_FRAME_HEADER_LEN,
    ZONE_PREVIEW_FRAME_TAG, ZonePreviewFrame, split_preview_publication,
};

fn jpeg_payload(width: u32, height: u32, len: usize) -> Bytes {
    let width = u16::try_from(width).expect("JPEG fixture width fits u16");
    let height = u16::try_from(height).expect("JPEG fixture height fits u16");
    let mut payload = vec![
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
    ];
    assert!(len >= payload.len());
    payload.resize(len, 0);
    Bytes::from(payload)
}

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
        payload: jpeg_payload(640, 480, 32),
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
    let mut encoded = PreviewFrame {
        channel: PreviewFrameChannel::ScreenCanvas,
        frame_number: 1,
        timestamp_ms: 2,
        width: 2,
        height: 2,
        format: PreviewPixelFormat::Rgb,
        payload: Bytes::from_static(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]),
    }
    .encode()
    .to_vec();
    encoded.truncate(PREVIEW_FRAME_HEADER_LEN + 3);

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

#[test]
fn interactive_preview_frame_roundtrips_address_and_payload() {
    let frame = InteractivePreviewFrame {
        preview_id: "main-preview".to_owned(),
        frame_number: 44,
        timestamp_ms: 1234,
        width: 2,
        height: 1,
        format: PreviewPixelFormat::Rgba,
        payload: Bytes::from_static(&[1, 2, 3, 4, 5, 6, 7, 8]),
    };
    let encoded = frame.encode().expect("interactive frame should encode");

    assert_eq!(encoded[0], INTERACTIVE_PREVIEW_FRAME_TAG);
    assert_eq!(
        encoded.len(),
        INTERACTIVE_PREVIEW_FRAME_PREFIX_LEN + frame.preview_id.len() + 8
    );
    assert_eq!(InteractivePreviewFrame::decode(&encoded), Ok(frame));
}

#[test]
fn interactive_preview_decode_bytes_shares_payload_buffer() {
    let frame = InteractivePreviewFrame {
        preview_id: "preview-a".to_owned(),
        frame_number: 1,
        timestamp_ms: 2,
        width: 1,
        height: 1,
        format: PreviewPixelFormat::Rgb,
        payload: Bytes::from_static(&[9, 8, 7]),
    };
    let encoded = frame.encode().expect("interactive frame should encode");
    let decoded = InteractivePreviewFrame::decode_bytes(&encoded).expect("frame should decode");
    let payload_offset = INTERACTIVE_PREVIEW_FRAME_PREFIX_LEN + frame.preview_id.len();

    assert_eq!(decoded, frame);
    assert_eq!(
        decoded.payload.as_ptr() as usize,
        encoded.as_ptr() as usize + payload_offset,
    );
}

#[test]
fn interactive_preview_frame_rejects_invalid_ids() {
    let empty = InteractivePreviewFrame {
        preview_id: String::new(),
        frame_number: 1,
        timestamp_ms: 1,
        width: 1,
        height: 1,
        format: PreviewPixelFormat::Rgb,
        payload: Bytes::from_static(&[1, 2, 3]),
    };
    assert_eq!(empty.encode(), Err(PreviewFrameDecodeError::EmptyPreviewId));

    let too_long = InteractivePreviewFrame {
        preview_id: "x".repeat(INTERACTIVE_PREVIEW_ID_MAX_BYTES + 1),
        ..empty
    };
    assert_eq!(
        too_long.encode(),
        Err(PreviewFrameDecodeError::PreviewIdTooLong {
            maximum: INTERACTIVE_PREVIEW_ID_MAX_BYTES,
            actual: INTERACTIVE_PREVIEW_ID_MAX_BYTES + 1,
        })
    );
}

#[test]
fn interactive_preview_frame_rejects_truncated_id_and_payload() {
    let frame = InteractivePreviewFrame {
        preview_id: "preview-b".to_owned(),
        frame_number: 1,
        timestamp_ms: 2,
        width: 2,
        height: 1,
        format: PreviewPixelFormat::Rgb,
        payload: Bytes::from_static(&[1, 2, 3, 4, 5, 6]),
    };
    let encoded = frame.encode().expect("interactive frame should encode");
    assert!(matches!(
        InteractivePreviewFrame::decode(
            &encoded[..INTERACTIVE_PREVIEW_FRAME_PREFIX_LEN + frame.preview_id.len() - 1]
        ),
        Err(PreviewFrameDecodeError::TooShort { .. })
    ));
    assert_eq!(
        InteractivePreviewFrame::decode(&encoded[..encoded.len() - 1]),
        Err(PreviewFrameDecodeError::PayloadTooShort {
            expected: 6,
            actual: 5,
        })
    );
}

// ── Screen Zones Frames ───────────────────────────────────────────────────

#[test]
fn screen_zones_frame_round_trips() {
    let payload: Vec<u8> = (0..(4 * 3 * 3))
        .map(|i| u8::try_from(i).unwrap_or(0))
        .collect();
    let frame = ScreenZonesFrame {
        frame_number: 77,
        timestamp_ms: 123_456,
        source_width: 2560,
        source_height: 1440,
        grid_cols: 4,
        grid_rows: 3,
        letterbox: [1, 1, 0, 0],
        payload: Bytes::from(payload),
    };

    let encoded = frame.encode();
    assert_eq!(encoded[0], SCREEN_ZONES_FRAME_TAG);
    assert_eq!(encoded.len(), SCREEN_ZONES_FRAME_HEADER_LEN + 4 * 3 * 3);
    assert_eq!(ScreenZonesFrame::decode(&encoded), Ok(frame));
}

#[test]
fn screen_zones_frame_zone_rgb_indexing() {
    let mut payload = vec![0u8; 2 * 2 * 3];
    payload[3..6].copy_from_slice(&[10, 20, 30]); // row 0, col 1
    payload[6..9].copy_from_slice(&[40, 50, 60]); // row 1, col 0
    let frame = ScreenZonesFrame {
        frame_number: 1,
        timestamp_ms: 1,
        source_width: 100,
        source_height: 100,
        grid_cols: 2,
        grid_rows: 2,
        letterbox: [0; 4],
        payload: Bytes::from(payload),
    };

    assert_eq!(frame.zone_rgb(0, 1), Some([10, 20, 30]));
    assert_eq!(frame.zone_rgb(1, 0), Some([40, 50, 60]));
    assert_eq!(frame.zone_rgb(2, 0), None);
    assert_eq!(frame.zone_rgb(0, 2), None);
}

#[test]
fn screen_zones_frame_rejects_truncated_payload() {
    let frame = ScreenZonesFrame {
        frame_number: 1,
        timestamp_ms: 1,
        source_width: 100,
        source_height: 100,
        grid_cols: 4,
        grid_rows: 4,
        letterbox: [0; 4],
        payload: Bytes::from(vec![0u8; 4 * 4 * 3]),
    };
    let encoded = frame.encode();
    let truncated = &encoded[..encoded.len() - 1];

    assert!(matches!(
        ScreenZonesFrame::decode(truncated),
        Err(PreviewFrameDecodeError::PayloadLengthMismatch { .. })
    ));
}

#[test]
fn raw_and_screen_zone_frames_reject_trailing_bytes() {
    let raw = PreviewFrame {
        channel: PreviewFrameChannel::Canvas,
        frame_number: 1,
        timestamp_ms: 1,
        width: 1,
        height: 1,
        format: PreviewPixelFormat::Rgba,
        payload: Bytes::from_static(&[1, 2, 3, 4]),
    };
    let mut raw_encoded = raw.encode().to_vec();
    raw_encoded.push(5);
    assert!(matches!(
        PreviewFrame::decode(&raw_encoded),
        Err(PreviewFrameDecodeError::PayloadLengthMismatch { .. })
    ));

    let zones = ScreenZonesFrame {
        frame_number: 1,
        timestamp_ms: 1,
        source_width: 1,
        source_height: 1,
        grid_cols: 1,
        grid_rows: 1,
        letterbox: [0; 4],
        payload: Bytes::from_static(&[1, 2, 3]),
    };
    let mut zones_encoded = zones.encode().to_vec();
    zones_encoded.push(4);
    assert!(matches!(
        ScreenZonesFrame::decode(&zones_encoded),
        Err(PreviewFrameDecodeError::PayloadLengthMismatch { .. })
    ));
}

#[test]
fn screen_zones_frame_rejects_wrong_tag() {
    let frame = ScreenZonesFrame {
        frame_number: 1,
        timestamp_ms: 1,
        source_width: 1,
        source_height: 1,
        grid_cols: 1,
        grid_rows: 1,
        letterbox: [0; 4],
        payload: Bytes::from(vec![0u8; 3]),
    };
    let mut encoded = frame.encode().to_vec();
    encoded[0] = ZONE_PREVIEW_FRAME_TAG;

    assert!(matches!(
        ScreenZonesFrame::decode(&encoded),
        Err(PreviewFrameDecodeError::UnknownChannel { .. })
    ));
}

#[test]
fn legacy_preview_layout_is_byte_for_byte_stable() {
    let encoded = PreviewFrame {
        channel: PreviewFrameChannel::Canvas,
        frame_number: 0x0403_0201,
        timestamp_ms: 0x0807_0605,
        width: 2,
        height: 1,
        format: PreviewPixelFormat::Rgb,
        payload: Bytes::from_static(&[0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]),
    }
    .encode();

    assert_eq!(
        encoded.as_ref(),
        &[
            0x03, 1, 2, 3, 4, 5, 6, 7, 8, 2, 0, 1, 0, 0, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF,
        ],
    );
}

#[test]
fn wide_preview_layouts_round_trip_without_truncation() {
    let passive = PreviewFrame {
        channel: PreviewFrameChannel::DisplayPreview,
        frame_number: 1,
        timestamp_ms: 2,
        width: 70_001,
        height: 3,
        format: PreviewPixelFormat::Rgb,
        payload: Bytes::from(vec![0; 70_001 * 3 * 3]),
    };
    let passive_bytes = passive.encode();
    assert_eq!(passive_bytes[0], WIDE_PREVIEW_FRAME_TAG);
    assert_eq!(PreviewFrame::decode(&passive_bytes), Ok(passive));

    let zone = ZonePreviewFrame {
        scene_id: [1; 16],
        zone_id: [2; 16],
        frame_number: 3,
        timestamp_ms: 4,
        width: 7680,
        height: 4320,
        format: PreviewPixelFormat::Jpeg,
        payload: jpeg_payload(7680, 4320, 32),
    };
    let zone_bytes = zone.encode();
    assert_eq!(zone_bytes[0], ZONE_PREVIEW_FRAME_TAG);
    assert_eq!(ZonePreviewFrame::decode(&zone_bytes), Ok(zone));

    let interactive = InteractivePreviewFrame {
        preview_id: "wide".to_owned(),
        frame_number: 5,
        timestamp_ms: 6,
        width: 2,
        height: 70_003,
        format: PreviewPixelFormat::Rgb,
        payload: Bytes::from(vec![0; 2 * 70_003 * 3]),
    };
    let interactive_bytes = interactive.encode().expect("wide frame encodes");
    assert_eq!(interactive_bytes[0], WIDE_INTERACTIVE_PREVIEW_FRAME_TAG);
    assert_eq!(
        InteractivePreviewFrame::decode(&interactive_bytes),
        Ok(interactive),
    );

    let screen_zones = ScreenZonesFrame {
        frame_number: 7,
        timestamp_ms: 8,
        source_width: 100_000,
        source_height: 1,
        grid_cols: 1,
        grid_rows: 1,
        letterbox: [0; 4],
        payload: Bytes::from_static(&[1, 2, 3]),
    };
    let screen_bytes = screen_zones.encode();
    assert_eq!(screen_bytes[0], WIDE_SCREEN_ZONES_FRAME_TAG);
    assert_eq!(ScreenZonesFrame::decode(&screen_bytes), Ok(screen_zones));

    let wide_zone = ZonePreviewFrame {
        scene_id: [3; 16],
        zone_id: [4; 16],
        frame_number: 9,
        timestamp_ms: 10,
        width: 65_536,
        height: 1,
        format: PreviewPixelFormat::Rgb,
        payload: Bytes::from(vec![0; 65_536 * 3]),
    };
    assert_eq!(wide_zone.encode()[0], WIDE_ZONE_PREVIEW_FRAME_TAG);
}

fn interactive_metadata(publication_id: u64, frame_number: u32) -> PreviewPublicationMetadata {
    PreviewPublicationMetadata {
        stream: PreviewStreamId::Interactive("preview-a".to_owned()),
        publication_id,
        frame_number,
        timestamp_ms: frame_number,
        width: 4096,
        height: 4096,
        format: PreviewPixelFormat::Jpeg,
    }
}

fn interactive_encoded(metadata: &PreviewPublicationMetadata, total_len: usize) -> Bytes {
    let PreviewStreamId::Interactive(preview_id) = &metadata.stream else {
        panic!("interactive fixture requires an interactive stream");
    };
    let header_len = INTERACTIVE_PREVIEW_FRAME_PREFIX_LEN + preview_id.len();
    let payload_len = total_len
        .checked_sub(header_len)
        .expect("fixture total includes its header");
    InteractivePreviewFrame {
        preview_id: preview_id.clone(),
        frame_number: metadata.frame_number,
        timestamp_ms: metadata.timestamp_ms,
        width: metadata.width,
        height: metadata.height,
        format: PreviewPixelFormat::Jpeg,
        payload: jpeg_payload(metadata.width, metadata.height, payload_len),
    }
    .encode()
    .expect("interactive fixture encodes")
}

fn reassembler(max_bytes: usize) -> PreviewChunkReassembler {
    PreviewChunkReassembler::new(PreviewReassemblyLimits {
        max_decoded_publication_bytes: 128 * 1024 * 1024,
        max_encoded_publication_bytes: max_bytes,
        max_connection_bytes: max_bytes.checked_mul(2).expect("fixture budget fits"),
        max_streams: 2,
        max_message_bytes: max_bytes,
        ..PreviewReassemblyLimits::default()
    })
}

#[test]
fn raw_4096_square_publication_chunks_and_reassembles() {
    let payload_len = 4096_usize * 4096 * 4;
    let encoded_len = PREVIEW_FRAME_HEADER_LEN + payload_len;
    let mut encoded = Vec::new();
    encoded
        .try_reserve_exact(encoded_len)
        .expect("fixture allocates");
    encoded.resize(encoded_len, 0x5A);
    encoded[0] = PreviewFrameChannel::Canvas.tag();
    encoded[9..11].copy_from_slice(&4096_u16.to_le_bytes());
    encoded[11..13].copy_from_slice(&4096_u16.to_le_bytes());
    encoded[13] = PreviewPixelFormat::Rgba.tag();
    let encoded = Bytes::from(encoded);
    let metadata = PreviewPublicationMetadata {
        stream: PreviewStreamId::Passive(PreviewFrameChannel::Canvas),
        publication_id: 1,
        frame_number: 0,
        timestamp_ms: 0,
        width: 4096,
        height: 4096,
        format: PreviewPixelFormat::Rgba,
    };
    let chunks = split_preview_publication(&encoded, &metadata, DEFAULT_PREVIEW_MAX_MESSAGE_BYTES)
        .expect("large raw publication chunks");
    assert!(chunks.len() > 1);
    assert!(
        chunks
            .iter()
            .all(|chunk| chunk.len() <= DEFAULT_PREVIEW_MAX_MESSAGE_BYTES)
    );

    let mut reassembler = reassembler(encoded_len);
    let mut complete = None;
    for chunk in chunks {
        complete = reassembler
            .push(&chunk)
            .expect("chunk accepted")
            .or(complete);
    }
    let complete = complete.expect("publication completes");
    assert_eq!(complete.encoded, encoded);
    assert_eq!(
        PreviewFrame::decode(&complete.encoded)
            .expect("frame decodes")
            .width,
        4096
    );
}

#[test]
fn chunk_reassembly_rejects_gap_overlap_duplicate_and_metadata_change() {
    let metadata = interactive_metadata(10, 1);
    let encoded = interactive_encoded(&metadata, 256);
    let chunks = split_preview_publication(&encoded, &metadata, 128).expect("chunks split");
    assert!(chunks.len() >= 3);

    let mut gaps = reassembler(1024);
    assert_eq!(gaps.push(&chunks[0]), Ok(None));
    assert_eq!(
        gaps.push(&chunks[2]),
        Err(PreviewChunkError::NonContiguousChunk),
    );

    let mut duplicate = reassembler(1024);
    assert_eq!(duplicate.push(&chunks[0]), Ok(None));
    assert_eq!(
        duplicate.push(&chunks[0]),
        Err(PreviewChunkError::DuplicateChunk),
    );

    let mut overlap_frame = PreviewChunkFrame::decode_bytes(&chunks[1]).expect("chunk decodes");
    overlap_frame.chunk_offset -= 1;
    let overlap = overlap_frame.try_encode().expect("overlap encodes");
    let mut overlap_reassembler = reassembler(1024);
    assert_eq!(overlap_reassembler.push(&chunks[0]), Ok(None));
    assert_eq!(
        overlap_reassembler.push(&overlap),
        Err(PreviewChunkError::NonContiguousChunk),
    );

    let mut changed_frame = PreviewChunkFrame::decode_bytes(&chunks[1]).expect("chunk decodes");
    changed_frame.metadata.width += 1;
    let changed = changed_frame.try_encode().expect("changed chunk encodes");
    let mut changed_reassembler = reassembler(1024);
    assert_eq!(changed_reassembler.push(&chunks[0]), Ok(None));
    assert_eq!(
        changed_reassembler.push(&changed),
        Err(PreviewChunkError::MetadataChanged),
    );
}

#[test]
fn newer_publication_reclaims_superseded_partial() {
    let old_metadata = interactive_metadata(1, 1);
    let new_metadata = interactive_metadata(2, 2);
    let old =
        split_preview_publication(&interactive_encoded(&old_metadata, 256), &old_metadata, 128)
            .expect("old chunks");
    let new =
        split_preview_publication(&interactive_encoded(&new_metadata, 128), &new_metadata, 128)
            .expect("new chunks");
    let mut reassembler = reassembler(1024);
    assert_eq!(reassembler.push(&old[0]), Ok(None));
    assert_eq!(reassembler.partial_count(), 1);
    assert_eq!(reassembler.push(&new[0]), Ok(None));
    assert_eq!(reassembler.partial_count(), 1);
    assert_eq!(reassembler.superseded_publications(), 1);
    assert_eq!(reassembler.reserved_bytes(), 128);
}

#[test]
fn chunk_reassembly_enforces_publication_and_connection_byte_budgets() {
    let metadata = interactive_metadata(1, 1);
    let encoded = interactive_encoded(&metadata, 512);
    let chunks = split_preview_publication(&encoded, &metadata, 128).expect("chunks split");
    let mut limited = reassembler(511);
    assert_eq!(
        limited.push(&chunks[0]),
        Err(PreviewChunkError::PublicationBudgetExceeded {
            requested: 512,
            limit: 511,
        }),
    );

    let first = PreviewPublicationMetadata {
        stream: PreviewStreamId::Interactive("one".to_owned()),
        ..interactive_metadata(1, 1)
    };
    let second = PreviewPublicationMetadata {
        stream: PreviewStreamId::Interactive("two".to_owned()),
        ..interactive_metadata(2, 2)
    };
    let first_chunks = split_preview_publication(&interactive_encoded(&first, 400), &first, 128)
        .expect("first chunks");
    let second_chunks = split_preview_publication(&interactive_encoded(&second, 400), &second, 128)
        .expect("second chunks");
    let mut connection_limited = PreviewChunkReassembler::new(PreviewReassemblyLimits {
        max_decoded_publication_bytes: 128 * 1024 * 1024,
        max_encoded_publication_bytes: 512,
        max_connection_bytes: 700,
        max_streams: 2,
        ..PreviewReassemblyLimits::default()
    });
    assert_eq!(connection_limited.push(&first_chunks[0]), Ok(None));
    assert_eq!(
        connection_limited.push(&second_chunks[0]),
        Err(PreviewChunkError::ConnectionBudgetExceeded {
            requested: 800,
            limit: 700,
        }),
    );
}

#[test]
fn chunk_reassembly_validates_raw_geometry_before_reserving_peer_bytes() {
    let metadata = PreviewPublicationMetadata {
        stream: PreviewStreamId::Passive(PreviewFrameChannel::Canvas),
        publication_id: 1,
        frame_number: 1,
        timestamp_ms: 1,
        width: 1,
        height: 1,
        format: PreviewPixelFormat::Rgba,
    };
    let declared = 512_u64 * 1024 * 1024;
    let chunk = PreviewChunkFrame {
        metadata,
        total_encoded_bytes: declared,
        chunk_offset: 0,
        chunk_index: 0,
        chunk_count: 2,
        payload: Bytes::from_static(&[0]),
    }
    .try_encode()
    .expect("malicious declaration has a valid chunk envelope");
    let mut reassembler = PreviewChunkReassembler::new(PreviewReassemblyLimits::default());

    assert_eq!(
        reassembler.push(&chunk),
        Err(PreviewChunkError::RawPublicationLengthMismatch {
            expected: PREVIEW_FRAME_HEADER_LEN + 4,
            actual: usize::try_from(declared).expect("fixture fits usize"),
        })
    );
    assert_eq!(reassembler.reserved_bytes(), 0);
    assert_eq!(reassembler.partial_count(), 0);
}

#[test]
fn chunk_reassembly_enforces_decoded_budget_separately_from_encoded_budget() {
    let metadata = PreviewPublicationMetadata {
        width: 16,
        height: 16,
        format: PreviewPixelFormat::Jpeg,
        ..interactive_metadata(1, 1)
    };
    let encoded = interactive_encoded(&metadata, 128);
    let chunks = split_preview_publication(&encoded, &metadata, 128).expect("chunks split");
    let mut reassembler = PreviewChunkReassembler::new(PreviewReassemblyLimits {
        max_decoded_publication_bytes: 1023,
        max_encoded_publication_bytes: 1024,
        max_connection_bytes: 2048,
        max_streams: 2,
        ..PreviewReassemblyLimits::default()
    });

    assert_eq!(
        reassembler.push(&chunks[0]),
        Err(PreviewChunkError::DecodedPublicationBudgetExceeded {
            requested: 1024,
            limit: 1023,
        })
    );
    assert_eq!(reassembler.reserved_bytes(), 0);
}

#[test]
fn chunk_layout_rejects_more_nonempty_chunks_than_total_bytes() {
    let frame = PreviewChunkFrame {
        metadata: interactive_metadata(1, 1),
        total_encoded_bytes: 2,
        chunk_offset: 0,
        chunk_index: 0,
        chunk_count: 3,
        payload: Bytes::from_static(&[1]),
    };

    assert_eq!(
        frame.try_encode(),
        Err(PreviewChunkError::ImpossibleChunkCount {
            chunks: 3,
            total_bytes: 2,
        })
    );
}

#[test]
fn stream_high_water_rejects_delayed_older_publication() {
    let old_metadata = interactive_metadata(10, 10);
    let new_metadata = interactive_metadata(11, 11);
    let old =
        split_preview_publication(&interactive_encoded(&old_metadata, 256), &old_metadata, 128)
            .expect("old chunks");
    let new =
        split_preview_publication(&interactive_encoded(&new_metadata, 256), &new_metadata, 128)
            .expect("new chunks");
    let mut reassembler = reassembler(1024);

    assert_eq!(reassembler.push(&new[0]), Ok(None));
    assert_eq!(
        reassembler.push(&old[0]),
        Err(PreviewChunkError::StalePublication {
            publication_id: 10,
            high_water: 11,
        })
    );
    assert_eq!(reassembler.reserved_bytes(), 256);
}

#[test]
fn cancellation_releases_bytes_but_keeps_stream_high_water() {
    let metadata = interactive_metadata(7, 7);
    let chunks = split_preview_publication(&interactive_encoded(&metadata, 256), &metadata, 128)
        .expect("chunks split");
    let mut reassembler = reassembler(1024);

    assert_eq!(reassembler.push(&chunks[0]), Ok(None));
    assert_eq!(
        reassembler.cancel_publication(&PreviewCancelFrame {
            stream: metadata.stream.clone(),
            publication_id: metadata.publication_id,
        }),
        Ok(true)
    );
    assert_eq!(reassembler.reserved_bytes(), 0);
    assert_eq!(
        reassembler.push(&chunks[0]),
        Err(PreviewChunkError::StalePublication {
            publication_id: 7,
            high_water: 7,
        })
    );
}

#[test]
fn cancellation_frame_roundtrips_publication_identity() {
    let cancellation = PreviewCancelFrame {
        stream: PreviewStreamId::Interactive("cancel-me".to_owned()),
        publication_id: 42,
    };
    let encoded = cancellation.try_encode().expect("cancellation encodes");

    assert_eq!(encoded[0], PREVIEW_CANCEL_FRAME_TAG);
    assert_eq!(PreviewCancelFrame::decode_bytes(&encoded), Ok(cancellation));
}

#[test]
fn bounded_tombstones_do_not_consume_active_stream_capacity() {
    let mut reassembler = PreviewChunkReassembler::new(PreviewReassemblyLimits {
        max_encoded_publication_bytes: 512,
        max_connection_bytes: 512,
        max_streams: 1,
        max_tombstones: 2,
        max_message_bytes: 128,
        ..PreviewReassemblyLimits::default()
    });
    let mut newest = None;
    for publication_id in 1..=3 {
        let metadata = PreviewPublicationMetadata {
            stream: PreviewStreamId::Interactive(format!("stream-{publication_id}")),
            ..interactive_metadata(publication_id, publication_id as u32)
        };
        let chunks =
            split_preview_publication(&interactive_encoded(&metadata, 128), &metadata, 128)
                .expect("fixture chunks");
        assert_eq!(reassembler.push_at(&chunks[0], publication_id), Ok(None));
        reassembler
            .cancel_publication(&PreviewCancelFrame {
                stream: metadata.stream.clone(),
                publication_id,
            })
            .expect("cancellation records high-water");
        newest = Some((metadata, chunks));
    }

    assert_eq!(reassembler.partial_count(), 0);
    assert_eq!(reassembler.tombstone_count(), 2);
    let (newest_metadata, newest_chunks) = newest.expect("latest fixture");
    assert!(matches!(
        reassembler.push_at(&newest_chunks[0], 4),
        Err(PreviewChunkError::StalePublication {
            publication_id: 3,
            high_water: 3,
        })
    ));

    let reused = PreviewPublicationMetadata {
        stream: PreviewStreamId::Interactive("stream-1".to_owned()),
        ..interactive_metadata(4, 4)
    };
    let reused_chunks = split_preview_publication(&interactive_encoded(&reused, 128), &reused, 128)
        .expect("reused stream chunks");
    assert_eq!(reassembler.push_at(&reused_chunks[0], 5), Ok(None));
    assert_eq!(reassembler.partial_count(), 1);
    assert_ne!(newest_metadata.stream, reused.stream);
}

#[test]
fn chunk_admission_enforces_message_and_chunk_count_budgets() {
    let metadata = interactive_metadata(1, 1);
    let chunks = split_preview_publication(&interactive_encoded(&metadata, 256), &metadata, 128)
        .expect("fixture chunks");
    let mut message_limited = PreviewChunkReassembler::new(PreviewReassemblyLimits {
        max_message_bytes: chunks[0].len() - 1,
        ..PreviewReassemblyLimits::default()
    });
    assert!(matches!(
        message_limited.push(&chunks[0]),
        Err(PreviewChunkError::MessageBudgetExceeded { .. })
    ));

    let mut chunk_limited = PreviewChunkReassembler::new(PreviewReassemblyLimits {
        max_chunk_count: 1,
        ..PreviewReassemblyLimits::default()
    });
    assert!(matches!(
        chunk_limited.push(&chunks[0]),
        Err(PreviewChunkError::ChunkCountBudgetExceeded { .. })
    ));
}

#[test]
fn jpeg_intrinsic_dimensions_are_checked_before_reservation() {
    let metadata = PreviewPublicationMetadata {
        width: 16,
        height: 16,
        ..interactive_metadata(1, 1)
    };
    let PreviewStreamId::Interactive(preview_id) = &metadata.stream else {
        unreachable!();
    };
    let encoded = InteractivePreviewFrame {
        preview_id: preview_id.clone(),
        frame_number: metadata.frame_number,
        timestamp_ms: metadata.timestamp_ms,
        width: 32,
        height: 16,
        format: PreviewPixelFormat::Jpeg,
        payload: jpeg_payload(32, 16, 64),
    }
    .encode()
    .expect("mismatched fixture frame encodes");
    let chunks = split_preview_publication(&encoded, &metadata, 128).expect("fixture chunks");
    let mut reassembler = PreviewChunkReassembler::new(PreviewReassemblyLimits::default());

    assert!(matches!(
        reassembler.push(&chunks[0]),
        Err(PreviewChunkError::Frame(
            PreviewFrameDecodeError::JpegDimensionsMismatch { .. }
        ))
    ));
    assert_eq!(reassembler.reserved_bytes(), 0);
}

#[test]
fn idle_expiry_releases_bytes_but_keeps_stream_high_water() {
    let first = PreviewPublicationMetadata {
        stream: PreviewStreamId::Interactive("first".to_owned()),
        ..interactive_metadata(5, 5)
    };
    let first_chunks = split_preview_publication(&interactive_encoded(&first, 256), &first, 128)
        .expect("first chunks");
    let mut reassembler = PreviewChunkReassembler::new(PreviewReassemblyLimits {
        max_decoded_publication_bytes: 128 * 1024 * 1024,
        max_encoded_publication_bytes: 1024,
        max_connection_bytes: 2048,
        max_streams: 2,
        max_idle_ms: 10,
        ..PreviewReassemblyLimits::default()
    });

    assert_eq!(reassembler.push_at(&first_chunks[0], 100), Ok(None));
    assert_eq!(reassembler.next_expiry_ms(), Some(110));
    assert_eq!(reassembler.expire_at(109), 0);
    assert_eq!(reassembler.expire_at(110), 1);
    assert_eq!(reassembler.expired_publications(), 1);
    assert_eq!(reassembler.reserved_bytes(), 0);
    assert_eq!(
        reassembler.push_at(&first_chunks[0], 111),
        Err(PreviewChunkError::StalePublication {
            publication_id: 5,
            high_water: 5,
        })
    );
}

#[test]
fn chunked_zone_publication_reassembles_for_shared_ui_decoder() {
    let frame = ZonePreviewFrame {
        scene_id: [3; 16],
        zone_id: [4; 16],
        frame_number: 9,
        timestamp_ms: 10,
        width: 2,
        height: 2,
        format: PreviewPixelFormat::Rgba,
        payload: Bytes::from_static(&[5; 16]),
    };
    let encoded = frame.try_encode().expect("zone frame encodes");
    let metadata = PreviewPublicationMetadata {
        stream: PreviewStreamId::Zone {
            scene_id: frame.scene_id,
            zone_id: frame.zone_id,
        },
        publication_id: 12,
        frame_number: frame.frame_number,
        timestamp_ms: frame.timestamp_ms,
        width: frame.width,
        height: frame.height,
        format: frame.format,
    };
    let chunks = split_preview_publication(&encoded, &metadata, 128).expect("zone chunks split");
    assert!(chunks.len() > 1);
    let mut reassembler = reassembler(1024);
    let mut completed = None;
    for chunk in chunks {
        completed = reassembler
            .push(&chunk)
            .expect("chunk admitted")
            .or(completed);
    }
    let completed = completed.expect("zone publication completes");
    let decoded = ZonePreviewFrame::decode_bytes(&completed.encoded).expect("zone frame decodes");
    assert_eq!(decoded, frame);
}

#[test]
fn preview_transport_capability_roundtrips_shared_resource_budgets() {
    let capability = PreviewTransportCapability::default();
    let encoded = capability.encode();

    assert_eq!(PreviewTransportCapability::decode(&encoded), Ok(capability));
    assert_eq!(
        PreviewTransportCapability::from_capabilities(["preview_chunking", encoded.as_str()]),
        Some(capability)
    );
    assert!(capability.max_connection_bytes >= capability.max_encoded_publication_bytes * 2);
}

#[test]
fn preview_transport_negotiation_uses_each_peers_physical_minimum() {
    let local = PreviewReassemblyLimits::default();
    let peer = PreviewTransportCapability {
        max_decoded_publication_bytes: local.max_decoded_publication_bytes / 2,
        max_encoded_publication_bytes: local.max_encoded_publication_bytes / 2,
        max_connection_bytes: local.max_connection_bytes / 2,
        max_streams: local.max_streams / 2,
        max_tombstones: local.max_tombstones / 2,
        max_idle_ms: local.max_idle_ms / 2,
        max_message_bytes: local.max_message_bytes / 2,
        max_chunk_count: local.max_chunk_count / 2,
    };

    assert_eq!(
        local.negotiated_with(peer),
        PreviewReassemblyLimits {
            max_decoded_publication_bytes: peer.max_decoded_publication_bytes,
            max_encoded_publication_bytes: peer.max_encoded_publication_bytes,
            max_connection_bytes: peer.max_connection_bytes,
            max_streams: peer.max_streams,
            max_tombstones: peer.max_tombstones,
            max_idle_ms: peer.max_idle_ms,
            max_message_bytes: peer.max_message_bytes,
            max_chunk_count: peer.max_chunk_count,
        }
    );
}

#[test]
fn chunk_envelope_rejects_unknown_tag_and_schema() {
    let chunks = split_preview_publication(
        &Bytes::from_static(b"publication"),
        &interactive_metadata(1, 1),
        128,
    )
    .expect("chunks split");
    assert_eq!(chunks[0][0], PREVIEW_CHUNK_FRAME_TAG);
    let mut unknown_tag = chunks[0].to_vec();
    unknown_tag[0] = 0xFF;
    assert_eq!(
        PreviewChunkFrame::decode_bytes(&Bytes::from(unknown_tag)),
        Err(PreviewChunkError::UnknownTag { actual: 0xFF }),
    );
    let mut unknown_schema = chunks[0].to_vec();
    unknown_schema[1] = 0xFF;
    assert_eq!(
        PreviewChunkFrame::decode_bytes(&Bytes::from(unknown_schema)),
        Err(PreviewChunkError::UnknownSchema { actual: 0xFF }),
    );
}

#[test]
fn publication_split_enforces_advertised_message_and_chunk_limits() {
    let metadata = interactive_metadata(1, 1);
    assert!(matches!(
        split_preview_publication(
            &Bytes::from_static(b"publication"),
            &metadata,
            DEFAULT_PREVIEW_MAX_MESSAGE_BYTES + 1,
        ),
        Err(PreviewChunkError::MessageBudgetExceeded { .. })
    ));

    let PreviewStreamId::Interactive(preview_id) = &metadata.stream else {
        unreachable!();
    };
    let one_byte_payload_message = PREVIEW_CHUNK_FIXED_HEADER_LEN + preview_id.len() + 1;
    let encoded = Bytes::from(vec![
        0;
        usize::try_from(DEFAULT_PREVIEW_MAX_CHUNK_COUNT)
            .expect("chunk limit fits usize")
            + 1
    ]);
    assert!(matches!(
        split_preview_publication(&encoded, &metadata, one_byte_payload_message),
        Err(PreviewChunkError::ChunkCountBudgetExceeded { .. })
    ));
}
