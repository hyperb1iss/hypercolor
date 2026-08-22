#![cfg(all(feature = "ws-core", not(target_arch = "wasm32")))]

use bytes::Bytes;
use hypercolor_leptos_ext::ws::{
    DEFAULT_PREVIEW_MAX_DECODED_PUBLICATION_BYTES, DEFAULT_PREVIEW_MAX_MESSAGE_BYTES,
    DISPLAY_PREVIEW_FRAME_PREFIX_LEN, DISPLAY_PREVIEW_FRAME_TAG, DISPLAY_PREVIEW_ID_MAX_BYTES,
    DisplayPreviewFrame, EXTENDED_SCREEN_ZONES_FRAME_HEADER_LEN, EXTENDED_SCREEN_ZONES_FRAME_TAG,
    INTERACTIVE_PREVIEW_FRAME_PREFIX_LEN, INTERACTIVE_PREVIEW_FRAME_TAG,
    INTERACTIVE_PREVIEW_ID_MAX_BYTES, InteractivePreviewFrame, PREVIEW_CANCEL_FRAME_TAG,
    PREVIEW_CANCEL_SCHEMA, PREVIEW_CHUNK_FIXED_HEADER_LEN, PREVIEW_CHUNK_FRAME_TAG,
    PREVIEW_CHUNK_SCHEMA, PREVIEW_FRAME_HEADER_LEN, PREVIEW_MIN_MESSAGE_BYTES, PreviewCancelFrame,
    PreviewChunkError, PreviewChunkFrame, PreviewChunkReassembler, PreviewFrame,
    PreviewFrameChannel, PreviewFrameDecodeError, PreviewPixelFormat, PreviewPublicationMetadata,
    PreviewReassemblyLimits, PreviewStreamId, PreviewTransportLimits,
    SCREEN_ZONES_FRAME_HEADER_LEN, SCREEN_ZONES_FRAME_TAG, ScreenZonesFrame, SpectrumFrame,
    WIDE_DISPLAY_PREVIEW_FRAME_TAG, WIDE_INTERACTIVE_PREVIEW_FRAME_TAG, WIDE_PREVIEW_FRAME_TAG,
    WIDE_SCREEN_ZONES_FRAME_HEADER_LEN, WIDE_SCREEN_ZONES_FRAME_TAG, WIDE_ZONE_PREVIEW_FRAME_TAG,
    ZONE_PREVIEW_FRAME_HEADER_LEN, ZONE_PREVIEW_FRAME_TAG, ZonePreviewFrame, codec_binary_messages,
    codec_frame_layouts, split_preview_publication,
};

#[derive(Debug, PartialEq)]
enum ManifestFieldValue {
    U8(u8),
    U16(u16),
    U32(u32),
    U64(u64),
    F32(f32),
    Uuid([u8; 16]),
    Utf8(String),
    Bytes(Vec<u8>),
    F32List(Vec<f32>),
    RgbList(Vec<[u8; 3]>),
}

fn assert_manifest_layout(
    layout_name: &str,
    encoded: &[u8],
    expected: &[(&str, ManifestFieldValue)],
) {
    let mut layouts = codec_frame_layouts();
    layouts.extend(codec_binary_messages());
    let layout = layouts[layout_name]["layout"]
        .as_array()
        .expect("codec layout is an array");
    assert_eq!(layout.len(), expected.len(), "{layout_name} field count");

    let mut offset = 0;
    for (descriptor, (expected_name, expected_value)) in layout.iter().zip(expected) {
        let descriptor = descriptor.as_array().expect("field descriptor is an array");
        let encoding = descriptor[0].as_str().expect("field encoding is a string");
        let field_name = descriptor[1].as_str().expect("field name is a string");
        assert_eq!(
            field_name, *expected_name,
            "{layout_name} field at {offset}"
        );

        let actual = match (encoding, expected_value) {
            ("u8", ManifestFieldValue::U8(_)) => {
                let value = encoded[offset];
                offset += 1;
                ManifestFieldValue::U8(value)
            }
            ("u16_le", ManifestFieldValue::U16(_)) => {
                let value = u16::from_le_bytes(
                    encoded[offset..offset + 2]
                        .try_into()
                        .expect("u16 field is complete"),
                );
                offset += 2;
                ManifestFieldValue::U16(value)
            }
            ("u32_le", ManifestFieldValue::U32(_)) => {
                let value = u32::from_le_bytes(
                    encoded[offset..offset + 4]
                        .try_into()
                        .expect("u32 field is complete"),
                );
                offset += 4;
                ManifestFieldValue::U32(value)
            }
            ("u64_le", ManifestFieldValue::U64(_)) => {
                let value = u64::from_le_bytes(
                    encoded[offset..offset + 8]
                        .try_into()
                        .expect("u64 field is complete"),
                );
                offset += 8;
                ManifestFieldValue::U64(value)
            }
            ("f32_le", ManifestFieldValue::F32(_)) => {
                let value = f32::from_le_bytes(
                    encoded[offset..offset + 4]
                        .try_into()
                        .expect("f32 field is complete"),
                );
                offset += 4;
                ManifestFieldValue::F32(value)
            }
            ("uuid", ManifestFieldValue::Uuid(_)) => {
                let value = encoded[offset..offset + 16]
                    .try_into()
                    .expect("UUID field is complete");
                offset += 16;
                ManifestFieldValue::Uuid(value)
            }
            ("utf8", ManifestFieldValue::Utf8(value)) => {
                let length = value.len();
                let actual = String::from_utf8(encoded[offset..offset + length].to_vec())
                    .expect("UTF-8 field is valid");
                offset += length;
                ManifestFieldValue::Utf8(actual)
            }
            ("bytes", ManifestFieldValue::Bytes(value)) => {
                let length = value.len();
                let actual = encoded[offset..offset + length].to_vec();
                offset += length;
                ManifestFieldValue::Bytes(actual)
            }
            ("repeated_f32_le", ManifestFieldValue::F32List(value)) => {
                let actual = encoded[offset..offset + value.len() * 4]
                    .chunks_exact(4)
                    .map(|chunk| {
                        f32::from_le_bytes(chunk.try_into().expect("f32 list item is complete"))
                    })
                    .collect();
                offset += value.len() * 4;
                ManifestFieldValue::F32List(actual)
            }
            ("repeated_u8_rgb", ManifestFieldValue::RgbList(value)) => {
                let actual = encoded[offset..offset + value.len() * 3]
                    .chunks_exact(3)
                    .map(|chunk| <[u8; 3]>::try_from(chunk).expect("RGB list item is complete"))
                    .collect();
                offset += value.len() * 3;
                ManifestFieldValue::RgbList(actual)
            }
            _ => panic!(
                "{layout_name}.{field_name} declares {encoding}, incompatible with {expected_value:?}"
            ),
        };
        assert_eq!(actual, *expected_value, "{layout_name}.{field_name}");
    }
    assert_eq!(offset, encoded.len(), "{layout_name} encoded length");
}

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
fn codec_manifest_layouts_match_production_encoders_field_by_field() {
    let preview_payload = vec![0xA1, 0xB2, 0xC3];
    let preview = PreviewFrame {
        channel: PreviewFrameChannel::Canvas,
        frame_number: 0x0403_0201,
        timestamp_ms: 0x0807_0605,
        width: 1,
        height: 1,
        format: PreviewPixelFormat::Rgb,
        payload: Bytes::from(preview_payload.clone()),
    }
    .encode();
    assert_manifest_layout(
        "preview_frame",
        &preview,
        &[
            (
                "tag",
                ManifestFieldValue::U8(PreviewFrameChannel::Canvas.tag()),
            ),
            ("frame_number", ManifestFieldValue::U32(0x0403_0201)),
            ("timestamp_ms", ManifestFieldValue::U32(0x0807_0605)),
            ("width", ManifestFieldValue::U16(1)),
            ("height", ManifestFieldValue::U16(1)),
            (
                "format",
                ManifestFieldValue::U8(PreviewPixelFormat::Rgb.tag()),
            ),
            ("payload", ManifestFieldValue::Bytes(preview_payload)),
        ],
    );

    let wide_payload = vec![0x5A; 70_001 * 3];
    let wide_preview = PreviewFrame {
        channel: PreviewFrameChannel::WebViewportCanvas,
        frame_number: 0x1413_1211,
        timestamp_ms: 0x1817_1615,
        width: 70_001,
        height: 1,
        format: PreviewPixelFormat::Rgb,
        payload: Bytes::from(wide_payload.clone()),
    }
    .encode();
    assert_manifest_layout(
        "wide_preview_frame",
        &wide_preview,
        &[
            ("tag", ManifestFieldValue::U8(WIDE_PREVIEW_FRAME_TAG)),
            (
                "channel_tag",
                ManifestFieldValue::U8(PreviewFrameChannel::WebViewportCanvas.tag()),
            ),
            ("frame_number", ManifestFieldValue::U32(0x1413_1211)),
            ("timestamp_ms", ManifestFieldValue::U32(0x1817_1615)),
            ("width", ManifestFieldValue::U32(70_001)),
            ("height", ManifestFieldValue::U32(1)),
            (
                "format",
                ManifestFieldValue::U8(PreviewPixelFormat::Rgb.tag()),
            ),
            ("payload", ManifestFieldValue::Bytes(wide_payload)),
        ],
    );

    let scene_id = [0x31; 16];
    let zone_id = [0x42; 16];
    let zone_payload = vec![0x51, 0x62, 0x73, 0x84];
    let zone = ZonePreviewFrame {
        scene_id,
        zone_id,
        frame_number: 0x2423_2221,
        timestamp_ms: 0x2827_2625,
        width: 1,
        height: 1,
        format: PreviewPixelFormat::Rgba,
        payload: Bytes::from(zone_payload.clone()),
    }
    .encode();
    assert_manifest_layout(
        "zone_preview_frame",
        &zone,
        &[
            ("tag", ManifestFieldValue::U8(ZONE_PREVIEW_FRAME_TAG)),
            ("frame_number", ManifestFieldValue::U32(0x2423_2221)),
            ("timestamp_ms", ManifestFieldValue::U32(0x2827_2625)),
            ("scene_id", ManifestFieldValue::Uuid(scene_id)),
            ("zone_id", ManifestFieldValue::Uuid(zone_id)),
            ("width", ManifestFieldValue::U16(1)),
            ("height", ManifestFieldValue::U16(1)),
            (
                "format",
                ManifestFieldValue::U8(PreviewPixelFormat::Rgba.tag()),
            ),
            ("payload", ManifestFieldValue::Bytes(zone_payload)),
        ],
    );

    let wide_zone_payload = vec![0x6B; 70_001 * 3];
    let wide_zone = ZonePreviewFrame {
        scene_id,
        zone_id,
        frame_number: 0x3433_3231,
        timestamp_ms: 0x3837_3635,
        width: 70_001,
        height: 1,
        format: PreviewPixelFormat::Rgb,
        payload: Bytes::from(wide_zone_payload.clone()),
    }
    .encode();
    assert_manifest_layout(
        "wide_zone_preview_frame",
        &wide_zone,
        &[
            ("tag", ManifestFieldValue::U8(WIDE_ZONE_PREVIEW_FRAME_TAG)),
            ("frame_number", ManifestFieldValue::U32(0x3433_3231)),
            ("timestamp_ms", ManifestFieldValue::U32(0x3837_3635)),
            ("scene_id", ManifestFieldValue::Uuid(scene_id)),
            ("zone_id", ManifestFieldValue::Uuid(zone_id)),
            ("width", ManifestFieldValue::U32(70_001)),
            ("height", ManifestFieldValue::U32(1)),
            (
                "format",
                ManifestFieldValue::U8(PreviewPixelFormat::Rgb.tag()),
            ),
            ("payload", ManifestFieldValue::Bytes(wide_zone_payload)),
        ],
    );

    let interactive_id = "preview-layout".to_owned();
    let interactive_payload = vec![0x17, 0x28, 0x39];
    let interactive = InteractivePreviewFrame {
        preview_id: interactive_id.clone(),
        frame_number: 0x4443_4241,
        timestamp_ms: 0x4847_4645,
        width: 1,
        height: 1,
        format: PreviewPixelFormat::Rgb,
        payload: Bytes::from(interactive_payload.clone()),
    }
    .encode()
    .expect("interactive frame encodes");
    assert_manifest_layout(
        "interactive_preview_frame",
        &interactive,
        &[
            ("tag", ManifestFieldValue::U8(INTERACTIVE_PREVIEW_FRAME_TAG)),
            (
                "preview_id_len",
                ManifestFieldValue::U8(u8::try_from(interactive_id.len()).expect("ID fits u8")),
            ),
            ("frame_number", ManifestFieldValue::U32(0x4443_4241)),
            ("timestamp_ms", ManifestFieldValue::U32(0x4847_4645)),
            ("width", ManifestFieldValue::U16(1)),
            ("height", ManifestFieldValue::U16(1)),
            (
                "format",
                ManifestFieldValue::U8(PreviewPixelFormat::Rgb.tag()),
            ),
            (
                "preview_id",
                ManifestFieldValue::Utf8(interactive_id.clone()),
            ),
            ("payload", ManifestFieldValue::Bytes(interactive_payload)),
        ],
    );

    let wide_interactive_payload = vec![0x29; 70_001 * 3];
    let wide_interactive = InteractivePreviewFrame {
        preview_id: interactive_id.clone(),
        frame_number: 0x5453_5251,
        timestamp_ms: 0x5857_5655,
        width: 70_001,
        height: 1,
        format: PreviewPixelFormat::Rgb,
        payload: Bytes::from(wide_interactive_payload.clone()),
    }
    .encode()
    .expect("wide interactive frame encodes");
    assert_manifest_layout(
        "wide_interactive_preview_frame",
        &wide_interactive,
        &[
            (
                "tag",
                ManifestFieldValue::U8(WIDE_INTERACTIVE_PREVIEW_FRAME_TAG),
            ),
            (
                "preview_id_len",
                ManifestFieldValue::U8(u8::try_from(interactive_id.len()).expect("ID fits u8")),
            ),
            ("frame_number", ManifestFieldValue::U32(0x5453_5251)),
            ("timestamp_ms", ManifestFieldValue::U32(0x5857_5655)),
            ("width", ManifestFieldValue::U32(70_001)),
            ("height", ManifestFieldValue::U32(1)),
            (
                "format",
                ManifestFieldValue::U8(PreviewPixelFormat::Rgb.tag()),
            ),
            ("preview_id", ManifestFieldValue::Utf8(interactive_id)),
            (
                "payload",
                ManifestFieldValue::Bytes(wide_interactive_payload),
            ),
        ],
    );

    let device_id = "display-layout".to_owned();
    let display_payload = vec![0x3A, 0x4B, 0x5C, 0x6D];
    let display = DisplayPreviewFrame {
        device_id: device_id.clone(),
        frame_number: 0x6463_6261,
        timestamp_ms: 0x6867_6665,
        width: 1,
        height: 1,
        format: PreviewPixelFormat::Rgba,
        payload: Bytes::from(display_payload.clone()),
    }
    .encode()
    .expect("display frame encodes");
    assert_manifest_layout(
        "display_preview_frame",
        &display,
        &[
            ("tag", ManifestFieldValue::U8(DISPLAY_PREVIEW_FRAME_TAG)),
            (
                "device_id_len",
                ManifestFieldValue::U8(u8::try_from(device_id.len()).expect("ID fits u8")),
            ),
            ("frame_number", ManifestFieldValue::U32(0x6463_6261)),
            ("timestamp_ms", ManifestFieldValue::U32(0x6867_6665)),
            ("width", ManifestFieldValue::U16(1)),
            ("height", ManifestFieldValue::U16(1)),
            (
                "format",
                ManifestFieldValue::U8(PreviewPixelFormat::Rgba.tag()),
            ),
            ("device_id", ManifestFieldValue::Utf8(device_id.clone())),
            ("payload", ManifestFieldValue::Bytes(display_payload)),
        ],
    );

    let wide_display_payload = vec![0x7E; 70_001 * 3];
    let wide_display = DisplayPreviewFrame {
        device_id: device_id.clone(),
        frame_number: 0x7473_7271,
        timestamp_ms: 0x7877_7675,
        width: 70_001,
        height: 1,
        format: PreviewPixelFormat::Rgb,
        payload: Bytes::from(wide_display_payload.clone()),
    }
    .encode()
    .expect("wide display frame encodes");
    assert_manifest_layout(
        "wide_display_preview_frame",
        &wide_display,
        &[
            (
                "tag",
                ManifestFieldValue::U8(WIDE_DISPLAY_PREVIEW_FRAME_TAG),
            ),
            (
                "device_id_len",
                ManifestFieldValue::U8(u8::try_from(device_id.len()).expect("ID fits u8")),
            ),
            ("frame_number", ManifestFieldValue::U32(0x7473_7271)),
            ("timestamp_ms", ManifestFieldValue::U32(0x7877_7675)),
            ("width", ManifestFieldValue::U32(70_001)),
            ("height", ManifestFieldValue::U32(1)),
            (
                "format",
                ManifestFieldValue::U8(PreviewPixelFormat::Rgb.tag()),
            ),
            ("device_id", ManifestFieldValue::Utf8(device_id)),
            ("payload", ManifestFieldValue::Bytes(wide_display_payload)),
        ],
    );

    let screen_colors = vec![[0x10, 0x20, 0x30], [0x40, 0x50, 0x60]];
    let screen = ScreenZonesFrame {
        frame_number: 0x8483_8281,
        timestamp_ms: 0x8887_8685,
        source_width: 0x1211,
        source_height: 0x1413,
        grid_cols: 2,
        grid_rows: 1,
        letterbox: [3, 4, 5, 6],
        payload: Bytes::from(screen_colors.iter().flatten().copied().collect::<Vec<_>>()),
    }
    .encode();
    assert_manifest_layout(
        "screen_zones_frame",
        &screen,
        &[
            ("tag", ManifestFieldValue::U8(SCREEN_ZONES_FRAME_TAG)),
            ("frame_number", ManifestFieldValue::U32(0x8483_8281)),
            ("timestamp_ms", ManifestFieldValue::U32(0x8887_8685)),
            ("source_width", ManifestFieldValue::U16(0x1211)),
            ("source_height", ManifestFieldValue::U16(0x1413)),
            ("grid_cols", ManifestFieldValue::U8(2)),
            ("grid_rows", ManifestFieldValue::U8(1)),
            ("letterbox_top", ManifestFieldValue::U8(3)),
            ("letterbox_bottom", ManifestFieldValue::U8(4)),
            ("letterbox_left", ManifestFieldValue::U8(5)),
            ("letterbox_right", ManifestFieldValue::U8(6)),
            (
                "zone_colors",
                ManifestFieldValue::RgbList(screen_colors.clone()),
            ),
        ],
    );

    let wide_screen = ScreenZonesFrame {
        frame_number: 0x9493_9291,
        timestamp_ms: 0x9897_9695,
        source_width: 70_001,
        source_height: 70_002,
        grid_cols: 2,
        grid_rows: 1,
        letterbox: [7, 8, 9, 10],
        payload: Bytes::from(screen_colors.iter().flatten().copied().collect::<Vec<_>>()),
    }
    .encode();
    assert_manifest_layout(
        "wide_screen_zones_frame",
        &wide_screen,
        &[
            ("tag", ManifestFieldValue::U8(WIDE_SCREEN_ZONES_FRAME_TAG)),
            ("frame_number", ManifestFieldValue::U32(0x9493_9291)),
            ("timestamp_ms", ManifestFieldValue::U32(0x9897_9695)),
            ("source_width", ManifestFieldValue::U32(70_001)),
            ("source_height", ManifestFieldValue::U32(70_002)),
            ("grid_cols", ManifestFieldValue::U8(2)),
            ("grid_rows", ManifestFieldValue::U8(1)),
            ("letterbox_top", ManifestFieldValue::U8(7)),
            ("letterbox_bottom", ManifestFieldValue::U8(8)),
            ("letterbox_left", ManifestFieldValue::U8(9)),
            ("letterbox_right", ManifestFieldValue::U8(10)),
            (
                "zone_colors",
                ManifestFieldValue::RgbList(screen_colors.clone()),
            ),
        ],
    );

    let extended_colors = vec![[0x21, 0x32, 0x43]; 256];
    let extended_screen = ScreenZonesFrame {
        frame_number: 0xA4A3_A2A1,
        timestamp_ms: 0xA8A7_A6A5,
        source_width: 0x2423,
        source_height: 0x2625,
        grid_cols: 256,
        grid_rows: 1,
        letterbox: [257, 258, 259, 260],
        payload: Bytes::from(
            extended_colors
                .iter()
                .flatten()
                .copied()
                .collect::<Vec<_>>(),
        ),
    }
    .encode();
    assert_manifest_layout(
        "extended_screen_zones_frame",
        &extended_screen,
        &[
            (
                "tag",
                ManifestFieldValue::U8(EXTENDED_SCREEN_ZONES_FRAME_TAG),
            ),
            ("frame_number", ManifestFieldValue::U32(0xA4A3_A2A1)),
            ("timestamp_ms", ManifestFieldValue::U32(0xA8A7_A6A5)),
            ("source_width", ManifestFieldValue::U32(0x2423)),
            ("source_height", ManifestFieldValue::U32(0x2625)),
            ("grid_cols", ManifestFieldValue::U32(256)),
            ("grid_rows", ManifestFieldValue::U32(1)),
            ("letterbox_top", ManifestFieldValue::U32(257)),
            ("letterbox_bottom", ManifestFieldValue::U32(258)),
            ("letterbox_left", ManifestFieldValue::U32(259)),
            ("letterbox_right", ManifestFieldValue::U32(260)),
            ("zone_colors", ManifestFieldValue::RgbList(extended_colors)),
        ],
    );

    let chunk_payload = vec![0x81, 0x92, 0xA3, 0xB4];
    let chunk = PreviewChunkFrame {
        metadata: PreviewPublicationMetadata {
            stream: PreviewStreamId::Interactive("chunk-layout".to_owned()),
            publication_id: 0x0807_0605_0403_0201,
            frame_number: 0xB4B3_B2B1,
            timestamp_ms: 0xB8B7_B6B5,
            width: 0xC4C3_C2C1,
            height: 0xC8C7_C6C5,
            format: PreviewPixelFormat::Jpeg,
        },
        total_encoded_bytes: 9,
        chunk_offset: 5,
        chunk_index: 1,
        chunk_count: 2,
        payload: Bytes::from(chunk_payload.clone()),
    };
    let chunk_identity = b"chunk-layout".to_vec();
    let chunk = chunk.try_encode().expect("preview chunk encodes");
    assert_manifest_layout(
        "preview_chunk_frame",
        &chunk,
        &[
            ("tag", ManifestFieldValue::U8(PREVIEW_CHUNK_FRAME_TAG)),
            ("schema", ManifestFieldValue::U8(PREVIEW_CHUNK_SCHEMA)),
            ("stream_kind", ManifestFieldValue::U8(2)),
            (
                "channel_tag",
                ManifestFieldValue::U8(INTERACTIVE_PREVIEW_FRAME_TAG),
            ),
            (
                "format",
                ManifestFieldValue::U8(PreviewPixelFormat::Jpeg.tag()),
            ),
            (
                "stream_identity_len",
                ManifestFieldValue::U16(
                    u16::try_from(chunk_identity.len()).expect("identity fits u16"),
                ),
            ),
            (
                "publication_id",
                ManifestFieldValue::U64(0x0807_0605_0403_0201),
            ),
            ("frame_number", ManifestFieldValue::U32(0xB4B3_B2B1)),
            ("timestamp_ms", ManifestFieldValue::U32(0xB8B7_B6B5)),
            ("width", ManifestFieldValue::U32(0xC4C3_C2C1)),
            ("height", ManifestFieldValue::U32(0xC8C7_C6C5)),
            ("total_encoded_bytes", ManifestFieldValue::U64(9)),
            ("chunk_offset", ManifestFieldValue::U64(5)),
            ("chunk_index", ManifestFieldValue::U32(1)),
            ("chunk_count", ManifestFieldValue::U32(2)),
            ("stream_identity", ManifestFieldValue::Bytes(chunk_identity)),
            ("chunk_payload", ManifestFieldValue::Bytes(chunk_payload)),
        ],
    );

    let cancel_identity = [scene_id, zone_id].concat();
    let cancel = PreviewCancelFrame {
        stream: PreviewStreamId::Zone { scene_id, zone_id },
        publication_id: 0x1817_1615_1413_1211,
    }
    .try_encode()
    .expect("preview cancellation encodes");
    assert_manifest_layout(
        "preview_cancel_frame",
        &cancel,
        &[
            ("tag", ManifestFieldValue::U8(PREVIEW_CANCEL_FRAME_TAG)),
            ("schema", ManifestFieldValue::U8(PREVIEW_CANCEL_SCHEMA)),
            ("stream_kind", ManifestFieldValue::U8(1)),
            (
                "channel_tag",
                ManifestFieldValue::U8(ZONE_PREVIEW_FRAME_TAG),
            ),
            ("stream_identity_len", ManifestFieldValue::U16(32)),
            (
                "publication_id",
                ManifestFieldValue::U64(0x1817_1615_1413_1211),
            ),
            (
                "stream_identity",
                ManifestFieldValue::Bytes(cancel_identity),
            ),
        ],
    );

    let spectrum_bins = vec![0.125, 0.25, 0.5];
    let spectrum = SpectrumFrame {
        timestamp_ms: 0xD4D3_D2D1,
        level: 0.625,
        bass: 0.375,
        mid: 0.75,
        treble: 0.875,
        beat: true,
        beat_confidence: 0.9375,
        bins: spectrum_bins.clone(),
    }
    .encode();
    assert_manifest_layout(
        "spectrum",
        &spectrum,
        &[
            ("tag", ManifestFieldValue::U8(0x02)),
            ("timestamp_ms", ManifestFieldValue::U32(0xD4D3_D2D1)),
            (
                "bin_count",
                ManifestFieldValue::U8(u8::try_from(spectrum_bins.len()).expect("bin count fits")),
            ),
            ("level", ManifestFieldValue::F32(0.625)),
            ("bass", ManifestFieldValue::F32(0.375)),
            ("mid", ManifestFieldValue::F32(0.75)),
            ("treble", ManifestFieldValue::F32(0.875)),
            ("beat", ManifestFieldValue::U8(1)),
            ("beat_confidence", ManifestFieldValue::F32(0.9375)),
            ("bins", ManifestFieldValue::F32List(spectrum_bins)),
        ],
    );
}

fn assert_one_byte_chunks_reassemble(
    encoded: Bytes,
    metadata: PreviewPublicationMetadata,
    identity_len: usize,
) {
    let max_message_bytes = PREVIEW_CHUNK_FIXED_HEADER_LEN + identity_len + 1;
    let chunks = split_preview_publication(&encoded, &metadata, max_message_bytes)
        .expect("one-byte publication chunks");
    assert_eq!(chunks.len(), encoded.len());
    assert_eq!(
        PreviewChunkFrame::decode_bytes(&chunks[0])
            .expect("first chunk")
            .payload
            .len(),
        1,
    );

    let mut reassembler = PreviewChunkReassembler::new(PreviewReassemblyLimits::default());
    let mut completed = None;
    for chunk in chunks {
        completed = reassembler
            .push(&chunk)
            .expect("valid tiny chunk")
            .or(completed);
    }
    assert_eq!(
        completed,
        Some(hypercolor_leptos_ext::ws::ReassembledPreviewPublication { metadata, encoded })
    );
}

#[test]
fn passive_publication_reassembles_with_one_byte_payload_chunks() {
    let frame = PreviewFrame {
        channel: PreviewFrameChannel::Canvas,
        frame_number: 1,
        timestamp_ms: 11,
        width: 1,
        height: 1,
        format: PreviewPixelFormat::Rgb,
        payload: Bytes::from_static(&[1, 2, 3]),
    };
    let metadata = PreviewPublicationMetadata {
        stream: PreviewStreamId::Passive(frame.channel),
        publication_id: 1,
        frame_number: frame.frame_number,
        timestamp_ms: frame.timestamp_ms,
        width: frame.width,
        height: frame.height,
        format: frame.format,
    };
    assert_one_byte_chunks_reassemble(frame.encode(), metadata, 0);
}

#[test]
fn screen_zones_publication_reassembles_with_one_byte_payload_chunks() {
    let frame = ScreenZonesFrame {
        frame_number: 2,
        timestamp_ms: 22,
        source_width: 3840,
        source_height: 2160,
        grid_cols: 1,
        grid_rows: 1,
        letterbox: [0; 4],
        payload: Bytes::from_static(&[4, 5, 6]),
    };
    let metadata = PreviewPublicationMetadata {
        stream: PreviewStreamId::ScreenZones,
        publication_id: 2,
        frame_number: frame.frame_number,
        timestamp_ms: frame.timestamp_ms,
        width: frame.source_width,
        height: frame.source_height,
        format: PreviewPixelFormat::Rgb,
    };
    assert_one_byte_chunks_reassemble(frame.encode(), metadata, 0);
}

#[test]
fn zone_publication_reassembles_with_one_byte_payload_chunks() {
    let frame = ZonePreviewFrame {
        scene_id: [0x11; 16],
        zone_id: [0x22; 16],
        frame_number: 3,
        timestamp_ms: 33,
        width: 1,
        height: 1,
        format: PreviewPixelFormat::Rgb,
        payload: Bytes::from_static(&[7, 8, 9]),
    };
    let metadata = PreviewPublicationMetadata {
        stream: PreviewStreamId::Zone {
            scene_id: frame.scene_id,
            zone_id: frame.zone_id,
        },
        publication_id: 3,
        frame_number: frame.frame_number,
        timestamp_ms: frame.timestamp_ms,
        width: frame.width,
        height: frame.height,
        format: frame.format,
    };
    assert_one_byte_chunks_reassemble(frame.encode(), metadata, 32);
}

#[test]
fn interactive_publication_reassembles_with_one_byte_payload_chunks() {
    let frame = InteractivePreviewFrame {
        preview_id: "main".to_owned(),
        frame_number: 4,
        timestamp_ms: 44,
        width: 1,
        height: 1,
        format: PreviewPixelFormat::Rgb,
        payload: Bytes::from_static(&[10, 11, 12]),
    };
    let metadata = PreviewPublicationMetadata {
        stream: PreviewStreamId::Interactive(frame.preview_id.clone()),
        publication_id: 4,
        frame_number: frame.frame_number,
        timestamp_ms: frame.timestamp_ms,
        width: frame.width,
        height: frame.height,
        format: frame.format,
    };
    assert_one_byte_chunks_reassemble(
        frame.encode().expect("interactive frame"),
        metadata,
        frame.preview_id.len(),
    );
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
        channel: PreviewFrameChannel::WebViewportCanvas,
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
fn display_preview_frame_names_the_device_it_came_from() {
    let frame = DisplayPreviewFrame {
        device_id: "3f2504e0-4f89-11d3-9a0c-0305e82c3301".to_owned(),
        frame_number: 44,
        timestamp_ms: 1234,
        width: 2,
        height: 1,
        format: PreviewPixelFormat::Rgba,
        payload: Bytes::from_static(&[1, 2, 3, 4, 5, 6, 7, 8]),
    };
    let encoded = frame.encode().expect("display frame should encode");

    assert_eq!(encoded[0], DISPLAY_PREVIEW_FRAME_TAG);
    assert_eq!(
        encoded.len(),
        DISPLAY_PREVIEW_FRAME_PREFIX_LEN + frame.device_id.len() + 8
    );
    assert_eq!(DisplayPreviewFrame::decode(&encoded), Ok(frame));
}

#[test]
fn display_preview_decode_bytes_shares_payload_buffer() {
    let frame = DisplayPreviewFrame {
        device_id: "display-a".to_owned(),
        frame_number: 1,
        timestamp_ms: 2,
        width: 1,
        height: 1,
        format: PreviewPixelFormat::Rgb,
        payload: Bytes::from_static(&[9, 8, 7]),
    };
    let encoded = frame.encode().expect("display frame should encode");
    let decoded = DisplayPreviewFrame::decode_bytes(&encoded).expect("frame should decode");
    let payload_offset = DISPLAY_PREVIEW_FRAME_PREFIX_LEN + frame.device_id.len();

    assert_eq!(decoded, frame);
    assert_eq!(
        decoded.payload.as_ptr() as usize,
        encoded.as_ptr() as usize + payload_offset,
    );
}

#[test]
fn display_preview_frame_rejects_invalid_device_ids() {
    let empty = DisplayPreviewFrame {
        device_id: String::new(),
        frame_number: 1,
        timestamp_ms: 1,
        width: 1,
        height: 1,
        format: PreviewPixelFormat::Rgb,
        payload: Bytes::from_static(&[1, 2, 3]),
    };
    assert_eq!(empty.encode(), Err(PreviewFrameDecodeError::EmptyPreviewId));

    let too_long = DisplayPreviewFrame {
        device_id: "x".repeat(DISPLAY_PREVIEW_ID_MAX_BYTES + 1),
        ..empty
    };
    assert_eq!(
        too_long.encode(),
        Err(PreviewFrameDecodeError::PreviewIdTooLong {
            maximum: DISPLAY_PREVIEW_ID_MAX_BYTES,
            actual: DISPLAY_PREVIEW_ID_MAX_BYTES + 1,
        })
    );
}

#[test]
fn a_wide_display_preview_frame_keeps_its_full_dimensions() {
    let frame = DisplayPreviewFrame {
        device_id: "wall".to_owned(),
        frame_number: 5,
        timestamp_ms: 6,
        width: 70_001,
        height: 3,
        format: PreviewPixelFormat::Rgb,
        payload: Bytes::from(vec![0; 70_001 * 3 * 3]),
    };
    let encoded = frame.encode().expect("wide display frame should encode");

    assert_eq!(encoded[0], WIDE_DISPLAY_PREVIEW_FRAME_TAG);
    assert_eq!(DisplayPreviewFrame::decode(&encoded), Ok(frame));
}

#[test]
fn display_and_interactive_previews_do_not_decode_as_each_other() {
    let display = DisplayPreviewFrame {
        device_id: "display-a".to_owned(),
        frame_number: 1,
        timestamp_ms: 2,
        width: 1,
        height: 1,
        format: PreviewPixelFormat::Rgb,
        payload: Bytes::from_static(&[9, 8, 7]),
    }
    .encode()
    .expect("display frame should encode");

    assert!(InteractivePreviewFrame::decode(&display).is_err());
    assert!(PreviewFrame::decode(&display).is_err());
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
fn screen_zones_frame_uses_extended_layout_for_grid_and_letterbox_dimensions() {
    let payload = Bytes::from(vec![7_u8; 256 * 2 * 3]);
    let frame = ScreenZonesFrame {
        frame_number: 78,
        timestamp_ms: 123_457,
        source_width: 3840,
        source_height: 2160,
        grid_cols: 256,
        grid_rows: 2,
        letterbox: [0, 0, 256, 1],
        payload,
    };

    let encoded = frame.encode();
    assert_eq!(encoded[0], EXTENDED_SCREEN_ZONES_FRAME_TAG);
    assert_eq!(
        encoded.len(),
        EXTENDED_SCREEN_ZONES_FRAME_HEADER_LEN + 256 * 2 * 3
    );
    assert_eq!(ScreenZonesFrame::decode(&encoded), Ok(frame));
}

#[test]
fn screen_zones_frame_preserves_wide_source_layout() {
    let frame = ScreenZonesFrame {
        frame_number: 80,
        timestamp_ms: 123_459,
        source_width: 100_000,
        source_height: 2160,
        grid_cols: 2,
        grid_rows: 1,
        letterbox: [0, 0, 1, 0],
        payload: Bytes::from_static(&[1, 2, 3, 4, 5, 6]),
    };

    let encoded = frame.encode();
    assert_eq!(encoded[0], WIDE_SCREEN_ZONES_FRAME_TAG);
    assert_eq!(encoded.len(), WIDE_SCREEN_ZONES_FRAME_HEADER_LEN + 6);
    assert_eq!(
        encoded.as_ref(),
        &[
            0x0E, 0x50, 0x00, 0x00, 0x00, 0x43, 0xE2, 0x01, 0x00, 0xA0, 0x86, 0x01, 0x00, 0x70,
            0x08, 0x00, 0x00, 0x02, 0x01, 0x00, 0x00, 0x01, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05,
            0x06,
        ]
    );
    assert_eq!(ScreenZonesFrame::decode(&encoded), Ok(frame));
}

#[test]
fn screen_zones_frame_keeps_compact_layout_at_u8_boundaries() {
    let frame = ScreenZonesFrame {
        frame_number: 79,
        timestamp_ms: 123_458,
        source_width: u32::from(u16::MAX),
        source_height: u32::from(u16::MAX),
        grid_cols: u32::from(u8::MAX),
        grid_rows: 1,
        letterbox: [u32::from(u8::MAX), 0, 0, 0],
        payload: Bytes::from(vec![0; usize::from(u8::MAX) * 3]),
    };

    let encoded = frame.encode();
    assert_eq!(encoded[0], SCREEN_ZONES_FRAME_TAG);
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
fn compact_preview_layout_is_byte_for_byte_stable() {
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
        channel: PreviewFrameChannel::WebViewportCanvas,
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
    encoded[1..9].fill(0);
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
fn replacement_reuses_connection_and_state_budgets() {
    let old_metadata = interactive_metadata(1, 1);
    let new_metadata = interactive_metadata(2, 2);
    let old =
        split_preview_publication(&interactive_encoded(&old_metadata, 256), &old_metadata, 128)
            .expect("old chunks");
    let new =
        split_preview_publication(&interactive_encoded(&new_metadata, 256), &new_metadata, 128)
            .expect("new chunks");
    let mut reassembler = PreviewChunkReassembler::new(PreviewReassemblyLimits {
        max_encoded_publication_bytes: 256,
        max_connection_bytes: 256,
        max_message_bytes: 128,
        ..PreviewReassemblyLimits::default()
    });

    assert_eq!(reassembler.push(&old[0]), Ok(None));
    let state_bytes = reassembler.state_bytes();
    assert_eq!(reassembler.reserved_bytes(), 256);
    assert_eq!(reassembler.push(&new[0]), Ok(None));
    assert_eq!(reassembler.reserved_bytes(), 256);
    assert_eq!(reassembler.state_bytes(), state_bytes);
    assert_eq!(reassembler.superseded_publications(), 1);
}

#[test]
fn reassembly_charges_full_interactive_identity_at_exact_boundary() {
    let mut metadata = interactive_metadata(1, 1);
    metadata.stream = PreviewStreamId::Interactive("x".repeat(INTERACTIVE_PREVIEW_ID_MAX_BYTES));
    let chunks = split_preview_publication(
        &interactive_encoded(&metadata, 256),
        &metadata,
        PREVIEW_MIN_MESSAGE_BYTES,
    )
    .expect("maximum identity chunks");
    let mut probe = PreviewChunkReassembler::new(PreviewReassemblyLimits::default());
    assert_eq!(probe.push(&chunks[0]), Ok(None));
    let exact_state_bytes = probe.state_bytes();

    let limits = PreviewReassemblyLimits {
        max_reassembly_state_bytes: exact_state_bytes,
        ..PreviewReassemblyLimits::default()
    };
    let mut exact = PreviewChunkReassembler::new(limits);
    assert_eq!(exact.push(&chunks[0]), Ok(None));
    assert_eq!(exact.state_bytes(), exact_state_bytes);

    let mut short = PreviewChunkReassembler::new(PreviewReassemblyLimits {
        max_reassembly_state_bytes: exact_state_bytes - 1,
        ..limits
    });
    assert_eq!(
        short.push(&chunks[0]),
        Err(PreviewChunkError::ReassemblyStateBudgetExceeded {
            requested: exact_state_bytes,
            limit: exact_state_bytes - 1,
        })
    );
    assert_eq!(short.reserved_bytes(), 0);
    assert_eq!(short.state_bytes(), 0);
}

#[test]
fn reassembly_admits_streams_by_state_bytes() {
    let stream_count = 257_usize;
    let mut reassembler = PreviewChunkReassembler::new(PreviewReassemblyLimits {
        max_encoded_publication_bytes: 256,
        max_connection_bytes: 256 * stream_count,
        max_reassembly_state_bytes: 1024 * 1024,
        max_message_bytes: 128,
        ..PreviewReassemblyLimits::default()
    });

    for publication_index in 1..=stream_count {
        let publication_id = u64::try_from(publication_index).expect("fixture id fits u64");
        let frame_number = u32::try_from(publication_index).expect("fixture frame fits u32");
        let metadata = PreviewPublicationMetadata {
            stream: PreviewStreamId::Interactive(format!("v2-{publication_id}")),
            ..interactive_metadata(publication_id, frame_number)
        };
        let chunks =
            split_preview_publication(&interactive_encoded(&metadata, 256), &metadata, 128)
                .expect("tiny publication chunks");
        assert_eq!(reassembler.push(&chunks[0]), Ok(None));
    }

    assert_eq!(reassembler.partial_count(), stream_count);
    assert!(reassembler.state_bytes() <= 1024 * 1024);
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
fn screen_zones_reassembly_enforces_decoded_budget_at_exact_boundary() {
    let frame = ScreenZonesFrame {
        frame_number: 7,
        timestamp_ms: 8,
        source_width: 1920,
        source_height: 1080,
        grid_cols: 2,
        grid_rows: 1,
        letterbox: [0; 4],
        payload: Bytes::from_static(&[1, 2, 3, 4, 5, 6]),
    };
    let metadata = PreviewPublicationMetadata {
        stream: PreviewStreamId::ScreenZones,
        publication_id: 9,
        frame_number: frame.frame_number,
        timestamp_ms: frame.timestamp_ms,
        width: frame.source_width,
        height: frame.source_height,
        format: PreviewPixelFormat::Rgb,
    };
    let encoded = frame.encode();
    let decoded_limit = frame.payload.len();
    let chunks = split_preview_publication(&encoded, &metadata, DEFAULT_PREVIEW_MAX_MESSAGE_BYTES)
        .expect("screen-zone chunks split");
    let mut admitted = PreviewChunkReassembler::new(PreviewReassemblyLimits {
        max_decoded_publication_bytes: decoded_limit,
        ..PreviewReassemblyLimits::default()
    });

    assert!(
        admitted
            .push(&chunks[0])
            .expect("exact decoded limit is admitted")
            .is_some()
    );

    let oversized = ScreenZonesFrame {
        grid_cols: 3,
        payload: Bytes::from_static(&[1, 2, 3, 4, 5, 6, 7, 8, 9]),
        ..frame
    };
    let oversized_metadata = PreviewPublicationMetadata {
        publication_id: 10,
        ..metadata
    };
    let oversized_encoded = oversized.encode();
    let oversized_chunks = split_preview_publication(
        &oversized_encoded,
        &oversized_metadata,
        DEFAULT_PREVIEW_MAX_MESSAGE_BYTES,
    )
    .expect("oversized screen-zone chunks split");
    let mut rejected = PreviewChunkReassembler::new(PreviewReassemblyLimits {
        max_decoded_publication_bytes: decoded_limit,
        ..PreviewReassemblyLimits::default()
    });

    assert_eq!(
        rejected.push(&oversized_chunks[0]),
        Err(PreviewChunkError::DecodedPublicationBudgetExceeded {
            requested: oversized.payload.len(),
            limit: decoded_limit,
        })
    );
    assert_eq!(rejected.reserved_bytes(), 0);
}

#[test]
fn screen_zones_declared_overflow_is_rejected_before_reservation() {
    let declared = u64::try_from(
        DEFAULT_PREVIEW_MAX_DECODED_PUBLICATION_BYTES + EXTENDED_SCREEN_ZONES_FRAME_HEADER_LEN + 1,
    )
    .expect("fixture length fits u64");
    let chunk = PreviewChunkFrame {
        metadata: PreviewPublicationMetadata {
            stream: PreviewStreamId::ScreenZones,
            publication_id: 11,
            frame_number: 1,
            timestamp_ms: 2,
            width: 1920,
            height: 1080,
            format: PreviewPixelFormat::Rgb,
        },
        total_encoded_bytes: declared,
        chunk_offset: 0,
        chunk_index: 0,
        chunk_count: 2,
        payload: Bytes::from_static(&[SCREEN_ZONES_FRAME_TAG]),
    }
    .try_encode()
    .expect("malicious screen-zone declaration has a valid envelope");
    let mut reassembler = PreviewChunkReassembler::new(PreviewReassemblyLimits::default());

    assert!(matches!(
        reassembler.push(&chunk),
        Err(PreviewChunkError::DecodedPublicationBudgetExceeded { .. })
    ));
    assert_eq!(reassembler.reserved_bytes(), 0);
    assert_eq!(reassembler.partial_count(), 0);
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
    assert_eq!(reassembler.state_bytes(), 0);
    assert!(reassembler.tombstone_bytes() > 0);
    assert_eq!(
        reassembler.push(&chunks[0]),
        Err(PreviewChunkError::StalePublication {
            publication_id: 7,
            high_water: 7,
        })
    );
}

#[test]
fn tombstones_evict_by_identity_bytes_and_clear_releases_ledgers() {
    let long_stream = PreviewStreamId::Interactive("x".repeat(INTERACTIVE_PREVIEW_ID_MAX_BYTES));
    let mut probe = PreviewChunkReassembler::new(PreviewReassemblyLimits::default());
    probe
        .cancel_publication(&PreviewCancelFrame {
            stream: long_stream.clone(),
            publication_id: 1,
        })
        .expect("probe cancellation records tombstone");
    let exact_tombstone_bytes = probe.tombstone_bytes();

    let short_stream = PreviewStreamId::Interactive("x".to_owned());
    let mut reassembler = PreviewChunkReassembler::new(PreviewReassemblyLimits {
        max_tombstone_bytes: exact_tombstone_bytes,
        ..PreviewReassemblyLimits::default()
    });
    reassembler
        .cancel_publication(&PreviewCancelFrame {
            stream: short_stream,
            publication_id: 1,
        })
        .expect("short tombstone admitted");
    reassembler
        .cancel_publication(&PreviewCancelFrame {
            stream: long_stream,
            publication_id: 2,
        })
        .expect("long tombstone admitted after byte eviction");

    assert_eq!(reassembler.tombstone_count(), 1);
    assert_eq!(reassembler.tombstone_bytes(), exact_tombstone_bytes);
    reassembler.clear();
    assert_eq!(reassembler.reserved_bytes(), 0);
    assert_eq!(reassembler.state_bytes(), 0);
    assert_eq!(reassembler.tombstone_bytes(), 0);
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
fn chunk_admission_enforces_message_budget() {
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
}

#[test]
fn jpeg_intrinsic_dimensions_are_checked_before_publication_completion() {
    let metadata = PreviewPublicationMetadata {
        width: 16,
        height: 16,
        ..interactive_metadata(1, 1)
    };
    let PreviewStreamId::Interactive(preview_id) = &metadata.stream else {
        unreachable!();
    };
    let mut encoded = InteractivePreviewFrame {
        preview_id: preview_id.clone(),
        frame_number: metadata.frame_number,
        timestamp_ms: metadata.timestamp_ms,
        width: 32,
        height: 16,
        format: PreviewPixelFormat::Jpeg,
        payload: jpeg_payload(32, 16, 64),
    }
    .encode()
    .expect("valid fixture frame encodes")
    .to_vec();
    encoded[10..12].copy_from_slice(&metadata.width.to_le_bytes()[..2]);
    let encoded = Bytes::from(encoded);
    let chunks = split_preview_publication(&encoded, &metadata, 128).expect("fixture chunks");
    let mut reassembler = PreviewChunkReassembler::new(PreviewReassemblyLimits::default());

    for chunk in &chunks[..chunks.len() - 1] {
        assert_eq!(reassembler.push(chunk), Ok(None));
    }
    assert!(matches!(
        reassembler.push(chunks.last().expect("final chunk")),
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
fn preview_transport_limits_cover_chunking_and_connection_headroom() {
    let limits = PreviewTransportLimits::default();

    assert!(limits.max_message_bytes >= PREVIEW_MIN_MESSAGE_BYTES);
    assert!(limits.max_connection_bytes >= limits.max_encoded_publication_bytes * 2);
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
}
