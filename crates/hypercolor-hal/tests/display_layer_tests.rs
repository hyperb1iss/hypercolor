//! Engine-level coverage for the shared display encoding layer.

use std::time::Duration;

use hypercolor_hal::display::{
    ChunkCommandPolicy, ChunkContext, DisplayChunkLayout, DisplayEncodeError, LineRepack,
    Packed16Format, WireKeepalive, encode_chunked_display_frame, encode_chunked_display_frame_into,
    encode_prefixed_display_frame,
};
use hypercolor_hal::drivers::corsair::CorsairLcdProtocol;
use hypercolor_hal::drivers::corsair::framing::{LCD_DATA_PER_PACKET, LCD_MAX_DISPLAY_CHUNKS};
use hypercolor_hal::protocol::{
    CommandBuffer, Protocol, ProtocolCommand, ResponsePlan, ResponseTolerance, TransferType,
};
use hypercolor_types::device::{DisplayFrameFormat, DisplayFramePayload};

const MARKER: u8 = 0xA5;
const HEADER_LEN: usize = 8;
const PACKET_LEN: usize = 16;
const MAX_PAYLOAD: usize = PACKET_LEN - HEADER_LEN;

/// Sequence-counter encodings a layout might have to satisfy.
#[derive(Clone, Copy, Debug)]
enum Sequence {
    U8,
    U16Le,
    U16Be,
    U32Be,
}

/// Per-chunk command policies the engine must apply verbatim.
#[derive(Clone, Copy, Debug)]
enum Policy {
    FireAndForget,
    AckOnFinal,
}

/// Header layout: `[0]` marker, `[1]` final flag, `[2..6]` sequence,
/// `[6..8]` total frame length little-endian.
struct TestLayout {
    packet_len: usize,
    max_payload: usize,
    payload_offset: usize,
    sequence: Sequence,
    max_chunks: u32,
    policy: Policy,
}

impl Default for TestLayout {
    fn default() -> Self {
        Self {
            packet_len: PACKET_LEN,
            max_payload: MAX_PAYLOAD,
            payload_offset: HEADER_LEN,
            sequence: Sequence::U8,
            max_chunks: u32::MAX,
            policy: Policy::FireAndForget,
        }
    }
}

impl TestLayout {
    fn with_sequence(sequence: Sequence) -> Self {
        Self {
            sequence,
            ..Self::default()
        }
    }
}

impl DisplayChunkLayout for TestLayout {
    fn packet_len(&self) -> usize {
        self.packet_len
    }

    fn max_payload(&self) -> usize {
        self.max_payload
    }

    fn payload_offset(&self) -> usize {
        self.payload_offset
    }

    fn write_header(&self, packet: &mut [u8], ctx: &ChunkContext<'_>) {
        packet[0] = MARKER;
        packet[1] = u8::from(ctx.is_final);
        match self.sequence {
            Sequence::U8 => packet[2] = u8::try_from(ctx.packet_index).unwrap_or(u8::MAX),
            Sequence::U16Le => packet[2..4].copy_from_slice(
                &u16::try_from(ctx.packet_index)
                    .unwrap_or(u16::MAX)
                    .to_le_bytes(),
            ),
            Sequence::U16Be => packet[2..4].copy_from_slice(
                &u16::try_from(ctx.packet_index)
                    .unwrap_or(u16::MAX)
                    .to_be_bytes(),
            ),
            Sequence::U32Be => packet[2..6].copy_from_slice(&ctx.packet_index.to_be_bytes()),
        }
        packet[6..8].copy_from_slice(
            &u16::try_from(ctx.total_len)
                .unwrap_or(u16::MAX)
                .to_le_bytes(),
        );
    }

    fn command_policy(&self, ctx: &ChunkContext<'_>) -> ChunkCommandPolicy {
        match self.policy {
            Policy::FireAndForget => ChunkCommandPolicy::fire_and_forget(TransferType::Bulk),
            Policy::AckOnFinal => ChunkCommandPolicy {
                transfer_type: TransferType::Bulk,
                expects_response: ctx.is_final,
                response_delay: Duration::from_millis(3),
                post_delay: (!ctx.is_final).then(|| Duration::from_millis(2)),
                response: ResponsePlan {
                    count: 1,
                    timeout: Some(Duration::from_secs(2)),
                    capacity: Some(511),
                    tolerance: ResponseTolerance::Optional,
                },
            },
        }
    }

    fn max_chunks(&self) -> u32 {
        self.max_chunks
    }
}

fn stale_commands(count: usize) -> Vec<ProtocolCommand> {
    (0..count)
        .map(|_| ProtocolCommand {
            data: vec![0xDE; 64],
            expects_response: true,
            response_delay: Duration::from_secs(9),
            post_delay: Duration::from_secs(9),
            transfer_type: TransferType::Primary,
            ..Default::default()
        })
        .collect()
}

// --- Chunk boundaries ---

#[test]
fn payload_exactly_at_max_payload_emits_one_chunk() {
    let layout = TestLayout::default();
    let mut commands = Vec::new();

    encode_chunked_display_frame(&layout, &[0x11; MAX_PAYLOAD], &mut commands)
        .expect("exact-capacity payload should encode");

    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].data.len(), PACKET_LEN);
    assert_eq!(commands[0].data[1], 0x01, "single chunk is the final chunk");
    assert!(
        commands[0].data[HEADER_LEN..].iter().all(|&b| b == 0x11),
        "the whole payload window should be filled"
    );
}

#[test]
fn payload_one_byte_over_max_payload_emits_two_chunks() {
    let layout = TestLayout::default();
    let mut commands = Vec::new();

    encode_chunked_display_frame(&layout, &[0x22; MAX_PAYLOAD + 1], &mut commands)
        .expect("boundary+1 payload should encode");

    assert_eq!(commands.len(), 2);
    assert_eq!(commands[0].data[1], 0x00, "first chunk is not final");
    assert_eq!(commands[1].data[1], 0x01, "second chunk is final");
    assert_eq!(commands[1].data[HEADER_LEN], 0x22, "carry byte");
    assert!(
        commands[1].data[HEADER_LEN + 1..].iter().all(|&b| b == 0),
        "the short final chunk should be zero-padded to packet_len"
    );
}

#[test]
fn zero_length_data_emits_no_commands() {
    let layout = TestLayout::default();
    let mut commands = stale_commands(4);

    encode_chunked_display_frame(&layout, &[], &mut commands)
        .expect("zero-length data is not an error");

    assert!(
        commands.is_empty(),
        "an empty frame must not put anything on the wire"
    );
}

#[test]
fn single_chunk_frame_is_marked_final() {
    let layout = TestLayout::default();
    let mut commands = Vec::new();

    encode_chunked_display_frame(&layout, &[0x33], &mut commands).expect("one byte should encode");

    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].data[0], MARKER);
    assert_eq!(commands[0].data[1], 0x01);
    assert_eq!(commands[0].data[2], 0x00, "sequence starts at zero");
}

#[test]
fn final_flag_is_set_only_on_the_last_packet() {
    let layout = TestLayout::default();
    let mut commands = Vec::new();

    encode_chunked_display_frame(&layout, &[0x44; MAX_PAYLOAD * 4 + 3], &mut commands)
        .expect("multi-chunk payload should encode");

    assert_eq!(commands.len(), 5);
    for (index, command) in commands.iter().enumerate() {
        let is_last = index + 1 == commands.len();
        assert_eq!(
            command.data[1],
            u8::from(is_last),
            "final flag for packet {index}"
        );
        assert_eq!(command.data.len(), PACKET_LEN, "packet {index} length");
    }
}

#[test]
fn every_header_repeats_the_total_frame_length() {
    let layout = TestLayout::default();
    let total = MAX_PAYLOAD * 2 + 5;
    let mut commands = Vec::new();

    encode_chunked_display_frame(&layout, &vec![0x55; total], &mut commands)
        .expect("multi-chunk payload should encode");

    for (index, command) in commands.iter().enumerate() {
        let declared = u16::from_le_bytes([command.data[6], command.data[7]]);
        assert_eq!(
            usize::from(declared),
            total,
            "total length in header of packet {index}"
        );
    }
}

// --- Sequence counters ---

#[test]
fn sequence_counters_honor_layout_width_and_endianness() {
    let data = vec![0x66; MAX_PAYLOAD * 3];

    for sequence in [
        Sequence::U8,
        Sequence::U16Le,
        Sequence::U16Be,
        Sequence::U32Be,
    ] {
        let layout = TestLayout::with_sequence(sequence);
        let mut commands = Vec::new();
        encode_chunked_display_frame(&layout, &data, &mut commands)
            .expect("three-chunk payload should encode");
        assert_eq!(commands.len(), 3);

        for (index, command) in commands.iter().enumerate() {
            let counter = &command.data[2..6];
            let expected: [u8; 4] = match sequence {
                Sequence::U8 => [u8::try_from(index).unwrap_or(u8::MAX), 0, 0, 0],
                Sequence::U16Le => {
                    let bytes = u16::try_from(index).unwrap_or(u16::MAX).to_le_bytes();
                    [bytes[0], bytes[1], 0, 0]
                }
                Sequence::U16Be => {
                    let bytes = u16::try_from(index).unwrap_or(u16::MAX).to_be_bytes();
                    [bytes[0], bytes[1], 0, 0]
                }
                Sequence::U32Be => u32::try_from(index).unwrap_or(u32::MAX).to_be_bytes(),
            };
            assert_eq!(counter, expected, "{sequence:?} counter for packet {index}");
        }
    }
}

// --- Command policy ---

#[test]
fn chunk_command_policy_is_applied_per_chunk() {
    let layout = TestLayout {
        policy: Policy::AckOnFinal,
        ..TestLayout::default()
    };
    let mut commands = Vec::new();

    encode_chunked_display_frame(&layout, &[0x77; MAX_PAYLOAD * 2], &mut commands)
        .expect("two-chunk payload should encode");

    assert_eq!(commands.len(), 2);
    assert!(!commands[0].expects_response, "non-final chunk is unacked");
    assert_eq!(commands[0].post_delay, Duration::from_millis(2));
    assert!(commands[1].expects_response, "final chunk is acked");
    assert_eq!(commands[1].response_delay, Duration::from_millis(3));
    assert_eq!(
        commands[1].response,
        ResponsePlan {
            count: 1,
            timeout: Some(Duration::from_secs(2)),
            capacity: Some(511),
            tolerance: ResponseTolerance::Optional,
        },
        "the policy's response plan rides the engine-emitted command"
    );
    assert_eq!(
        commands[0].response,
        ResponsePlan::default(),
        "an unacked chunk carries the default plan"
    );
    assert_eq!(
        commands[1].post_delay,
        Duration::ZERO,
        "a None post_delay means no pacing"
    );
    for command in &commands {
        assert_eq!(command.transfer_type, TransferType::Bulk);
    }
}

// --- Errors ---

#[test]
fn too_many_chunks_errors_and_emits_nothing() {
    let layout = TestLayout {
        max_chunks: 2,
        ..TestLayout::default()
    };
    let mut commands = stale_commands(3);

    let error = encode_chunked_display_frame(&layout, &[0x88; MAX_PAYLOAD * 3], &mut commands)
        .expect_err("three chunks should exceed a two-chunk counter");

    assert!(
        matches!(
            error,
            DisplayEncodeError::TooManyChunks { needed: 3, max: 2 }
        ),
        "unexpected error: {error}"
    );
    assert!(
        commands.is_empty(),
        "a rejected frame must leave no commands behind"
    );
}

#[test]
fn payload_window_past_the_packet_end_is_rejected() {
    let layout = TestLayout {
        max_payload: PACKET_LEN,
        ..TestLayout::default()
    };
    let mut commands = Vec::new();

    let error = encode_chunked_display_frame(&layout, &[0x99; 4], &mut commands)
        .expect_err("payload window must fit inside the packet");

    assert!(
        matches!(
            error,
            DisplayEncodeError::PayloadTooLarge {
                actual: 24,
                capacity: PACKET_LEN
            }
        ),
        "unexpected error: {error}"
    );
    assert!(commands.is_empty());
}

/// The display seam carries the engine's error to the actor, which fails
/// that one delivery; the protocol keeps working for the next frame.
#[test]
fn a_protocol_surfaces_a_frame_the_engine_rejects_and_keeps_going() {
    let protocol = CorsairLcdProtocol::new("Test LCD", 480, 480, 0x40, 0x40, true, 0);
    let past_the_counter = usize::try_from(LCD_MAX_DISPLAY_CHUNKS)
        .unwrap_or(usize::MAX)
        .saturating_add(1);
    let oversized = vec![0x5A; LCD_DATA_PER_PACKET * past_the_counter];
    let mut commands = stale_commands(3);

    let error = protocol
        .encode_display_payload_into(DisplayFramePayload::jpeg(&oversized), &mut commands)
        .expect_err("a frame the wire format cannot address is an error, not a silent drop");
    assert!(
        matches!(error, DisplayEncodeError::TooManyChunks { .. }),
        "unexpected error: {error}"
    );
    assert!(
        commands.is_empty(),
        "a rejected frame leaves nothing for the actor to send"
    );

    protocol
        .encode_display_payload_into(DisplayFramePayload::jpeg(&[0x11; 64]), &mut commands)
        .expect("the next frame should still encode");
    assert_eq!(
        commands.len(),
        2,
        "one bulk packet plus the keepalive the rejected frame never consumed"
    );
}

/// A protocol that drives no display, or none in the offered format, says so
/// through the same channel instead of pretending the frame went out.
#[test]
fn an_unsupported_payload_format_is_an_error_not_a_silent_drop() {
    let protocol = CorsairLcdProtocol::new("Test LCD", 480, 480, 0x40, 0x40, true, 0);
    let mut commands = Vec::new();
    let pixels = vec![0; 480 * 480 * 3];

    let error = protocol
        .encode_display_payload_into(
            DisplayFramePayload {
                format: DisplayFrameFormat::Rgb,
                width: 480,
                height: 480,
                data: &pixels,
            },
            &mut commands,
        )
        .expect_err("a JPEG panel cannot take raw RGB");
    assert!(
        matches!(
            error,
            DisplayEncodeError::Unsupported {
                format: DisplayFrameFormat::Rgb
            }
        ),
        "unexpected error: {error}"
    );
}

// --- Buffer reuse ---

#[test]
fn reused_buffer_carries_no_stale_bytes_between_frames() {
    let layout = TestLayout::default();
    let mut commands = Vec::new();

    encode_chunked_display_frame(&layout, &[0xAA; MAX_PAYLOAD * 3], &mut commands)
        .expect("first frame should encode");
    assert_eq!(commands.len(), 3);

    encode_chunked_display_frame(&layout, &[0xBB; 2], &mut commands)
        .expect("second frame should encode");

    assert_eq!(commands.len(), 1, "buffer truncates to the new frame");
    assert_eq!(commands[0].data.len(), PACKET_LEN);
    assert_eq!(&commands[0].data[HEADER_LEN..HEADER_LEN + 2], &[0xBB, 0xBB]);
    assert!(
        commands[0].data[HEADER_LEN + 2..].iter().all(|&b| b == 0),
        "no bytes from the previous frame may survive"
    );
    assert_eq!(commands[0].data[1], 0x01, "the reused command re-headers");
}

#[test]
fn chunking_into_a_command_buffer_preserves_a_frame_preamble() {
    let layout = TestLayout::default();
    let mut commands = stale_commands(6);

    {
        let mut buffer = CommandBuffer::new(&mut commands);
        buffer.push_slice(
            &[0xF0, 0x0D],
            false,
            Duration::ZERO,
            Duration::ZERO,
            TransferType::Bulk,
        );
        encode_chunked_display_frame_into(&layout, &[0xCC; MAX_PAYLOAD + 1], &mut buffer)
            .expect("two-chunk payload should encode");
        buffer.push_slice(
            &[0x0F, 0xF0],
            false,
            Duration::ZERO,
            Duration::ZERO,
            TransferType::Bulk,
        );
        buffer.finish();
    }

    assert_eq!(commands.len(), 4, "preamble + two chunks + trailer");
    assert_eq!(commands[0].data, vec![0xF0, 0x0D]);
    assert_eq!(commands[1].data[1], 0x00);
    assert_eq!(commands[2].data[1], 0x01);
    assert_eq!(commands[3].data, vec![0x0F, 0xF0]);
}

// --- Prefixed frames ---

#[test]
fn prefixed_frame_zero_pads_to_the_fixed_length() {
    let mut commands = stale_commands(2);

    encode_prefixed_display_frame(
        4,
        |frame, ctx| {
            frame[0] = MARKER;
            frame[1..3].copy_from_slice(
                &u16::try_from(ctx.payload.len())
                    .unwrap_or(u16::MAX)
                    .to_be_bytes(),
            );
            frame[3] = u8::try_from(ctx.header_len).unwrap_or(u8::MAX);
        },
        &[0x01, 0x02, 0x03],
        Some(32),
        ChunkCommandPolicy::fire_and_forget(TransferType::Bulk),
        &mut commands,
    )
    .expect("prefixed frame should encode");

    assert_eq!(commands.len(), 1);
    let frame = &commands[0].data;
    assert_eq!(frame.len(), 32, "fixed_frame_len is the on-wire length");
    assert_eq!(frame[0], MARKER);
    assert_eq!(u16::from_be_bytes([frame[1], frame[2]]), 3);
    assert_eq!(frame[3], 4, "header_len reaches the header writer");
    assert_eq!(&frame[4..7], &[0x01, 0x02, 0x03]);
    assert!(
        frame[7..].iter().all(|&b| b == 0),
        "the tail must be zero-padded"
    );
}

#[test]
fn prefixed_frame_without_a_fixed_length_is_header_plus_payload() {
    let mut commands = Vec::new();

    encode_prefixed_display_frame(
        2,
        |frame, ctx| {
            frame[0] = MARKER;
            frame[1] = u8::try_from(ctx.frame_len).unwrap_or(u8::MAX);
        },
        &[0xAB; 6],
        None,
        ChunkCommandPolicy::fire_and_forget(TransferType::HidReport),
        &mut commands,
    )
    .expect("prefixed frame should encode");

    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].data.len(), 8);
    assert_eq!(
        commands[0].data[1], 8,
        "frame_len reaches the header writer"
    );
    assert_eq!(commands[0].transfer_type, TransferType::HidReport);
}

/// Unlike the chunk engine, a prefixed frame with no payload is still a
/// frame: the header alone is the message.
#[test]
fn prefixed_frame_with_no_payload_is_still_emitted() {
    let mut commands = Vec::new();

    encode_prefixed_display_frame(
        4,
        |frame, ctx| {
            frame[0] = MARKER;
            frame[1] = u8::try_from(ctx.payload.len()).unwrap_or(u8::MAX);
        },
        &[],
        Some(8),
        ChunkCommandPolicy::fire_and_forget(TransferType::Primary),
        &mut commands,
    )
    .expect("an empty payload is not an error for a prefixed frame");

    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].data.len(), 8);
    assert_eq!(commands[0].data[0], MARKER);
    assert_eq!(commands[0].data[1], 0, "the header sees an empty payload");
    assert!(commands[0].data[2..].iter().all(|&b| b == 0));
}

#[test]
fn prefixed_frame_rejects_a_payload_that_does_not_fit() {
    let mut commands = stale_commands(1);

    let error = encode_prefixed_display_frame(
        8,
        |_frame, _ctx| unreachable!("the header writer must not run for a rejected frame"),
        &[0x00; 40],
        Some(32),
        ChunkCommandPolicy::fire_and_forget(TransferType::Primary),
        &mut commands,
    )
    .expect_err("40 payload bytes cannot fit 32 minus an 8-byte header");

    assert!(
        matches!(
            error,
            DisplayEncodeError::PayloadTooLarge {
                actual: 40,
                capacity: 24
            }
        ),
        "unexpected error: {error}"
    );
    assert!(commands.is_empty());
}

// --- Pixel repack ---

fn push2_like_repack() -> LineRepack<'static> {
    const MASK: [u8; 4] = [0xE7, 0xF3, 0xE7, 0xFF];

    LineRepack {
        width: 4,
        height: 2,
        format: Packed16Format::Bgr565,
        line_len: 12,
        filler: 0x00,
        xor_mask: &MASK,
    }
}

#[test]
fn bgr565_packing_places_blue_in_the_high_bits() {
    assert_eq!(Packed16Format::Bgr565.pack(255, 0, 0), 0x001F);
    assert_eq!(Packed16Format::Bgr565.pack(0, 0, 248), 0xF800);
    assert_eq!(Packed16Format::Bgr565.pack(0, 252, 0), 0x07E0);
    assert_eq!(Packed16Format::Rgb565.pack(255, 0, 0), 0xF800);
    assert_eq!(Packed16Format::Rgb565.pack(0, 0, 248), 0x001F);
}

#[test]
fn repack_writes_packed_pixels_filler_and_xor_mask() {
    let repack = push2_like_repack();
    let source = vec![0x00; 4 * 2 * 3];
    let mut out = Vec::new();

    repack
        .repack_rgb888(&source, &mut out)
        .expect("black frame should repack");

    assert_eq!(out.len(), 24, "two lines of twelve bytes");
    for line in out.chunks_exact(12) {
        // Black packs to 0x0000, so every output byte is the mask itself, and
        // the four filler bytes continue the same mask phase.
        for (index, byte) in line.iter().enumerate() {
            assert_eq!(
                *byte,
                repack.xor_mask[index % repack.xor_mask.len()],
                "masked byte {index}"
            );
        }
    }
}

#[test]
fn repack_reuses_its_buffer_without_stale_bytes() {
    let repack = push2_like_repack();
    let mut out = Vec::new();

    repack
        .repack_rgb888(&[0xFF; 4 * 2 * 3], &mut out)
        .expect("white frame should repack");
    let white = out.clone();

    repack
        .repack_rgb888(&[0x00; 4 * 2 * 3], &mut out)
        .expect("black frame should repack");

    assert_eq!(out.len(), white.len());
    assert_ne!(out, white, "the second frame must overwrite the first");

    let mut fresh = Vec::new();
    repack
        .repack_rgb888(&[0x00; 4 * 2 * 3], &mut fresh)
        .expect("black frame should repack into a fresh buffer");
    assert_eq!(
        out, fresh,
        "reused and fresh buffers must agree byte for byte"
    );
}

#[test]
fn repack_rejects_a_short_source_and_a_short_line() {
    let repack = push2_like_repack();
    let mut out = Vec::new();

    let short_source = repack
        .repack_rgb888(&[0x00; 8], &mut out)
        .expect_err("a short source must not be padded silently");
    assert!(
        matches!(
            short_source,
            hypercolor_hal::display::RepackError::SourceTooSmall {
                expected: 24,
                actual: 8,
                ..
            }
        ),
        "unexpected error: {short_source}"
    );

    let narrow = LineRepack {
        line_len: 4,
        ..push2_like_repack()
    };
    let short_line = narrow
        .repack_rgb888(&[0x00; 24], &mut out)
        .expect_err("a line that cannot hold its pixels must not truncate");
    assert!(
        matches!(
            short_line,
            hypercolor_hal::display::RepackError::LineTooShort {
                line_len: 4,
                required: 8
            }
        ),
        "unexpected error: {short_line}"
    );
}

// --- Wire keepalive ---

#[test]
fn wire_keepalive_is_due_before_the_first_send_and_suppressed_after() {
    let keepalive = WireKeepalive::new(Duration::from_secs(30));
    assert!(keepalive.due(), "no keepalive has gone out yet");

    keepalive.mark_sent();
    assert!(
        !keepalive.due(),
        "a fresh keepalive suppresses the next one"
    );
}

#[test]
fn wire_keepalive_with_a_zero_interval_is_always_due() {
    let keepalive = WireKeepalive::new(Duration::ZERO);
    keepalive.mark_sent();
    assert!(keepalive.due(), "a zero interval never suppresses");
}
