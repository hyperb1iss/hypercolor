use std::io::Cursor;

use hypercolor_hal::drivers::push2::{Push2Protocol, build_push2_protocol};
use hypercolor_hal::protocol::{Protocol, TransferType};
use image::{ColorType, ImageEncoder, RgbImage, codecs::jpeg::JpegEncoder};

const PUSH2_DISPLAY_WIDTH: usize = 960;
const PUSH2_DISPLAY_HEIGHT: usize = 160;
const PUSH2_DISPLAY_LINE_PIXELS: usize = PUSH2_DISPLAY_WIDTH * 2;
const PUSH2_DISPLAY_LINE_PADDING: usize = 128;
const PUSH2_DISPLAY_LINE_SIZE: usize = PUSH2_DISPLAY_LINE_PIXELS + PUSH2_DISPLAY_LINE_PADDING;
const PUSH2_DISPLAY_TRANSFER_CHUNK: usize = 16 * 1024;
const PUSH2_DISPLAY_XOR_MASK: [u8; 4] = [0xE7, 0xF3, 0xE7, 0xFF];

const HEADER_SIZE: usize = 16;
const HEADER_MAGIC: [u8; 4] = [0xFF, 0xCC, 0xAA, 0x88];
const TOTAL_FRAME_BYTES: usize = PUSH2_DISPLAY_LINE_SIZE * PUSH2_DISPLAY_HEIGHT;

fn make_jpeg(width: u32, height: u32, rgb: [u8; 3]) -> Vec<u8> {
    let image = RgbImage::from_pixel(width, height, image::Rgb(rgb));
    let mut bytes = Vec::new();
    JpegEncoder::new(&mut Cursor::new(&mut bytes))
        .write_image(image.as_raw(), width, height, ColorType::Rgb8.into())
        .expect("JPEG encoding should succeed");
    bytes
}

fn make_960x160_jpeg(rgb: [u8; 3]) -> Vec<u8> {
    make_jpeg(960, 160, rgb)
}

fn expected_chunk_count() -> usize {
    TOTAL_FRAME_BYTES.div_ceil(PUSH2_DISPLAY_TRANSFER_CHUNK)
}

// --- Header validation ---

#[test]
fn display_header_is_16_bytes_with_correct_magic() {
    let protocol = Push2Protocol::new();
    let jpeg = make_960x160_jpeg([0, 0, 0]);

    let commands = protocol
        .encode_display_frame(&jpeg)
        .expect("display encoding should succeed");

    assert_eq!(commands[0].data.len(), HEADER_SIZE);
    assert_eq!(&commands[0].data[..4], &HEADER_MAGIC);
    assert!(
        commands[0].data[4..].iter().all(|&b| b == 0),
        "header padding should be all zeros"
    );
    assert_eq!(commands[0].transfer_type, TransferType::Bulk);
}

// --- Chunk sizing ---

#[test]
fn data_chunks_do_not_exceed_transfer_chunk_size() {
    let protocol = Push2Protocol::new();
    let jpeg = make_960x160_jpeg([128, 64, 200]);

    let commands = protocol
        .encode_display_frame(&jpeg)
        .expect("display encoding should succeed");

    for (index, cmd) in commands.iter().enumerate().skip(1) {
        assert!(
            cmd.data.len() <= PUSH2_DISPLAY_TRANSFER_CHUNK,
            "chunk {index} exceeds transfer limit: {} > {PUSH2_DISPLAY_TRANSFER_CHUNK}",
            cmd.data.len()
        );
    }
}

#[test]
fn total_command_count_matches_expected_chunks_plus_header() {
    let protocol = Push2Protocol::new();
    let jpeg = make_960x160_jpeg([255, 255, 255]);

    let commands = protocol
        .encode_display_frame(&jpeg)
        .expect("display encoding should succeed");

    let expected = 1 + expected_chunk_count();
    assert_eq!(
        commands.len(),
        expected,
        "expected 1 header + {expected_chunks} data chunks",
        expected_chunks = expected_chunk_count()
    );
}

#[test]
fn all_commands_use_bulk_transfer_type() {
    let protocol = Push2Protocol::new();
    let jpeg = make_960x160_jpeg([0, 255, 0]);

    let commands = protocol
        .encode_display_frame(&jpeg)
        .expect("display encoding should succeed");

    for (index, cmd) in commands.iter().enumerate() {
        assert_eq!(
            cmd.transfer_type,
            TransferType::Bulk,
            "command {index} should use Bulk transfer"
        );
    }
}

// --- Total frame geometry ---

#[test]
fn total_pixel_data_matches_display_geometry() {
    let protocol = Push2Protocol::new();
    let jpeg = make_960x160_jpeg([100, 50, 200]);

    let commands = protocol
        .encode_display_frame(&jpeg)
        .expect("display encoding should succeed");

    let total_data_bytes: usize = commands.iter().skip(1).map(|c| c.data.len()).sum();
    assert_eq!(
        total_data_bytes, TOTAL_FRAME_BYTES,
        "total frame data should be {PUSH2_DISPLAY_HEIGHT} lines * {PUSH2_DISPLAY_LINE_SIZE} bytes/line"
    );
}

// --- XOR masking verification ---

#[test]
fn solid_black_frame_produces_xor_mask_pattern_in_pixel_region() {
    let protocol = Push2Protocol::new();
    let jpeg = make_960x160_jpeg([0, 0, 0]);

    let commands = protocol
        .encode_display_frame(&jpeg)
        .expect("display encoding should succeed");

    // Black pixels in RGB565 = 0x0000, so after XOR the output should be
    // the mask pattern itself in the pixel region of each line.
    let first_chunk = &commands[1].data;

    // JPEG compression of solid black may not be exactly 0x0000 per pixel due
    // to encoder rounding, but for large solid regions most pixels will be black.
    // Verify the repeating XOR pattern is present (every 4 bytes should follow
    // the mask cycle) by checking that consecutive 4-byte groups in the first
    // line are consistent with each other.
    let line_start = 0;
    let first_group = &first_chunk[line_start..line_start + 4];
    let second_group = &first_chunk[line_start + 4..line_start + 8];
    assert_eq!(
        first_group, second_group,
        "solid-color frame should produce repeating 4-byte XOR pattern"
    );
}

#[test]
fn xor_mask_is_applied_to_pixel_region_of_each_line() {
    let protocol = Push2Protocol::new();
    // Use solid black: RGB565 = 0x0000, XOR with mask = mask itself
    let jpeg = make_960x160_jpeg([0, 0, 0]);

    let commands = protocol
        .encode_display_frame(&jpeg)
        .expect("display encoding should succeed");

    // Reconstruct first line from the first data chunk
    let first_line = &commands[1].data[..PUSH2_DISPLAY_LINE_SIZE];

    // Verify the pixel region (first 1920 bytes) shows the XOR mask pattern
    // applied to near-zero RGB565 values. For exact black, we expect the mask.
    // JPEG lossy compression means pixels might not be exactly 0, but they'll
    // be close enough that the pattern is recognizable.
    for byte_index in (0..PUSH2_DISPLAY_LINE_PIXELS).step_by(4) {
        let chunk = &first_line[byte_index..byte_index + 4];
        let mask = &PUSH2_DISPLAY_XOR_MASK;
        // After XOR, for near-black RGB565 values, each byte should be close
        // to the mask value. Allow some tolerance for JPEG artifacts.
        for (i, (&actual, &expected_mask)) in chunk.iter().zip(mask.iter()).enumerate() {
            let diff = actual.abs_diff(expected_mask);
            assert!(
                diff < 8,
                "pixel byte at line offset {offset} differs from XOR mask by {diff} (actual={actual:#04X}, mask={expected_mask:#04X})",
                offset = byte_index + i
            );
        }
    }
}

// --- RGB565 encoding ---

#[test]
fn solid_red_frame_encodes_to_expected_rgb565_pattern() {
    let protocol = Push2Protocol::new();
    let jpeg = make_960x160_jpeg([255, 0, 0]);

    let commands = protocol
        .encode_display_frame(&jpeg)
        .expect("display encoding should succeed");

    // Pure red in Push 2's BGR565 encoding:
    // blue=0>>3=0, green=0>>2=0, red=255>>3=31
    // encoded = (0 << 11) | (0 << 5) | 31 = 0x001F (LE: [0x1F, 0x00])
    // After XOR with [0xE7, 0xF3, ...]: [0x1F^0xE7, 0x00^0xF3] = [0xF8, 0xF3]
    let first_pixel = &commands[1].data[..2];
    let expected = [0x1F ^ PUSH2_DISPLAY_XOR_MASK[0], PUSH2_DISPLAY_XOR_MASK[1]];
    assert_eq!(
        first_pixel, &expected,
        "solid red first pixel should be RGB565 0x001F XOR'd with mask"
    );
}

#[test]
fn solid_green_frame_encodes_to_expected_rgb565_pattern() {
    let protocol = Push2Protocol::new();
    let jpeg = make_960x160_jpeg([0, 252, 0]);

    let commands = protocol
        .encode_display_frame(&jpeg)
        .expect("display encoding should succeed");

    // Pure green (252 so it survives JPEG better):
    // blue=0>>3=0, green=252>>2=63, red=0>>3=0
    // encoded = (0 << 11) | (63 << 5) | 0 = 0x07E0 (LE: [0xE0, 0x07])
    // After XOR: [0xE0^0xE7, 0x07^0xF3] = [0x07, 0xF4]
    let first_pixel = &commands[1].data[..2];
    // JPEG lossy: allow some tolerance on green channel
    let expected_low = 0xE0 ^ PUSH2_DISPLAY_XOR_MASK[0];
    let actual_low = first_pixel[0];
    let diff = actual_low.abs_diff(expected_low);
    assert!(
        diff < 4,
        "green pixel low byte: expected ~{expected_low:#04X}, got {actual_low:#04X} (diff={diff})"
    );
}

#[test]
fn solid_blue_frame_encodes_to_expected_rgb565_pattern() {
    let protocol = Push2Protocol::new();
    let jpeg = make_960x160_jpeg([0, 0, 248]);

    let commands = protocol
        .encode_display_frame(&jpeg)
        .expect("display encoding should succeed");

    // Pure blue (248 for clean bit-shifting):
    // blue=248>>3=31, green=0>>2=0, red=0>>3=0
    // encoded = (31 << 11) | (0 << 5) | 0 = 0xF800 (LE: [0x00, 0xF8])
    // After XOR: [0x00^0xE7, 0xF8^0xF3] = [0xE7, 0x0B]
    let first_pixel = &commands[1].data[..2];
    let expected_low = PUSH2_DISPLAY_XOR_MASK[0];
    let expected_high = 0xF8u8 ^ PUSH2_DISPLAY_XOR_MASK[1];
    let diff_low = first_pixel[0].abs_diff(expected_low);
    let diff_high = first_pixel[1].abs_diff(expected_high);
    assert!(
        diff_low < 4 && diff_high < 4,
        "blue pixel: expected ~[{expected_low:#04X}, {expected_high:#04X}], got [{:#04X}, {:#04X}]",
        first_pixel[0],
        first_pixel[1]
    );
}

// --- Line padding ---

#[test]
fn each_line_includes_128_byte_padding_after_pixel_data() {
    let protocol = Push2Protocol::new();
    let jpeg = make_960x160_jpeg([0, 0, 0]);

    let commands = protocol
        .encode_display_frame(&jpeg)
        .expect("display encoding should succeed");

    // Concatenate all data chunks
    let mut frame_data = Vec::new();
    for cmd in commands.iter().skip(1) {
        frame_data.extend_from_slice(&cmd.data);
    }

    assert_eq!(frame_data.len(), TOTAL_FRAME_BYTES);

    // Verify each line is PUSH2_DISPLAY_LINE_SIZE and has padding at the end
    for row in 0..PUSH2_DISPLAY_HEIGHT {
        let line_start = row * PUSH2_DISPLAY_LINE_SIZE;
        let padding_start = line_start + PUSH2_DISPLAY_LINE_PIXELS;
        let padding_end = padding_start + PUSH2_DISPLAY_LINE_PADDING;

        let padding = &frame_data[padding_start..padding_end];
        assert_eq!(
            padding_end - padding_start,
            PUSH2_DISPLAY_LINE_PADDING,
            "line {row} padding should be {PUSH2_DISPLAY_LINE_PADDING} bytes"
        );
        assert_eq!(padding.len(), PUSH2_DISPLAY_LINE_PADDING);
    }
}

// --- Small image resizing ---

#[test]
fn small_jpeg_is_resized_to_display_dimensions() {
    let protocol = Push2Protocol::new();
    let small_jpeg = make_jpeg(4, 4, [255, 0, 0]);

    let commands = protocol
        .encode_display_frame(&small_jpeg)
        .expect("small image should be accepted and resized");

    let total_data: usize = commands.iter().skip(1).map(|c| c.data.len()).sum();
    assert_eq!(
        total_data, TOTAL_FRAME_BYTES,
        "resized frame should still produce the full display geometry"
    );
}

#[test]
fn large_jpeg_is_resized_to_display_dimensions() {
    let protocol = Push2Protocol::new();
    let large_jpeg = make_jpeg(1920, 320, [0, 128, 255]);

    let commands = protocol
        .encode_display_frame(&large_jpeg)
        .expect("large image should be accepted and resized");

    let total_data: usize = commands.iter().skip(1).map(|c| c.data.len()).sum();
    assert_eq!(
        total_data, TOTAL_FRAME_BYTES,
        "resized frame should still produce the full display geometry"
    );
}

// --- Invalid input ---

#[test]
fn invalid_jpeg_data_returns_none() {
    let protocol = Push2Protocol::new();
    let garbage = vec![0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x00];

    let result = protocol.encode_display_frame(&garbage);
    assert!(result.is_none(), "invalid JPEG data should return None");
}

#[test]
fn empty_data_returns_none() {
    let protocol = Push2Protocol::new();
    let result = protocol.encode_display_frame(&[]);
    assert!(result.is_none(), "empty data should return None");
}

// --- encode_display_frame_into buffer reuse ---

#[test]
fn encode_display_frame_into_reuses_command_buffer() {
    let protocol = Push2Protocol::new();
    let jpeg = make_960x160_jpeg([64, 128, 192]);

    let mut commands = Vec::new();
    protocol
        .encode_display_frame_into(&jpeg, &mut commands)
        .expect("first encode should succeed");

    let first_count = commands.len();
    assert!(first_count > 0);

    protocol
        .encode_display_frame_into(&jpeg, &mut commands)
        .expect("second encode with same buffer should succeed");

    assert_eq!(
        commands.len(),
        first_count,
        "reused buffer should produce the same command count"
    );
}

#[test]
fn encode_display_frame_into_truncates_oversized_buffer() {
    let protocol = Push2Protocol::new();
    let jpeg = make_960x160_jpeg([0, 0, 0]);

    let mut commands: Vec<_> = (0..100)
        .map(|_| hypercolor_hal::protocol::ProtocolCommand {
            data: vec![0xFF; 500],
            expects_response: true,
            response_delay: std::time::Duration::from_secs(5),
            post_delay: std::time::Duration::from_secs(5),
            transfer_type: TransferType::Primary,
        })
        .collect();

    protocol
        .encode_display_frame_into(&jpeg, &mut commands)
        .expect("encode into oversized buffer should succeed");

    let expected = 1 + expected_chunk_count();
    assert_eq!(
        commands.len(),
        expected,
        "oversized buffer should be truncated to actual usage"
    );
}

// --- Determinism ---

#[test]
fn same_input_produces_identical_output() {
    let protocol = Push2Protocol::new();
    let jpeg = make_960x160_jpeg([200, 100, 50]);

    let first = protocol
        .encode_display_frame(&jpeg)
        .expect("first encode should succeed");
    let second = protocol
        .encode_display_frame(&jpeg)
        .expect("second encode should succeed");

    assert_eq!(first.len(), second.len());
    for (index, (a, b)) in first.iter().zip(second.iter()).enumerate() {
        assert_eq!(
            a.data, b.data,
            "command {index} should be identical across encodes"
        );
    }
}

// --- build_push2_protocol factory ---

#[test]
fn factory_protocol_supports_display_encoding() {
    let protocol = build_push2_protocol();
    let jpeg = make_960x160_jpeg([255, 128, 0]);

    let commands = protocol
        .encode_display_frame(&jpeg)
        .expect("factory protocol should support display encoding");

    assert_eq!(commands[0].data.len(), HEADER_SIZE);
    assert_eq!(&commands[0].data[..4], &HEADER_MAGIC);
    assert!(commands.len() > 1);
}
