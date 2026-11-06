//! Golden-byte tests for DDP data packets and frame fragmentation.
//!
//! Expected headers follow the 3waylabs DDP header table: flags
//! (`VV x T S R Q P`), sequence nibble, data type (`C R TTT SSS`),
//! destination ID, 32-bit byte offset, 16-bit data length, all MSB first.

use hypercolor_pixel_protocols::ddp::{
    DDP_DTYPE_RGB8, DDP_DTYPE_RGBW8, DDP_DTYPE_UNDEFINED, DDP_ID_DEFAULT, DDP_MAX_PAYLOAD,
    DDP_PORT, DdpPacket, DdpSequence, DdpSpan, build_ddp_frame, build_ddp_spans, encode_ddp_packet,
    pixel_aligned_chunk_len,
};

#[test]
fn ddp_port_is_4048() {
    assert_eq!(DDP_PORT, 4048);
}

#[test]
fn data_type_bytes_follow_ttt_sss_encoding() {
    // TTT=001 (RGB), SSS=011 (8 bits per element)
    assert_eq!(DDP_DTYPE_RGB8, 0b00_001_011);
    // TTT=011 (RGBW), SSS=011
    assert_eq!(DDP_DTYPE_RGBW8, 0b00_011_011);
    // "set to zero if not used or undefined"
    assert_eq!(DDP_DTYPE_UNDEFINED, 0);
}

#[test]
fn packet_matches_standard_layout() {
    let packet = DdpPacket::with_destination(&[1, 2, 3, 4, 5, 6], 0x0001_0203, true, 5, 0x0B, 1);
    let expected = [
        0x41, // version 01, push
        0x05, // sequence 5
        0x0B, // RGB, 8-bit elements
        0x01, // default output device
        0x00, 0x01, 0x02, 0x03, // data offset
        0x00, 0x06, // data length
        1, 2, 3, 4, 5, 6,
    ];
    assert_eq!(packet.as_bytes(), expected.as_slice());
}

#[test]
fn non_final_packet_clears_push_and_masks_sequence() {
    let packet = DdpPacket::new(&[0xFF; 3], 0, false, 0x1F, DDP_DTYPE_RGB8);
    assert_eq!(&packet.as_bytes()[..4], &[0x40, 0x0F, 0x0B, DDP_ID_DEFAULT]);
}

#[test]
fn encode_reuses_buffer() {
    let mut buf = vec![0xEE; 64];
    encode_ddp_packet(&mut buf, &[9, 8], 4, true, 2, DDP_DTYPE_UNDEFINED, 7);
    assert_eq!(
        buf,
        vec![
            0x41, 0x02, 0x00, 0x07, 0x00, 0x00, 0x00, 0x04, 0x00, 0x02, 9, 8
        ]
    );
}

#[test]
fn chunk_len_keeps_whole_pixels() {
    assert_eq!(pixel_aligned_chunk_len(3), 1440);
    assert_eq!(pixel_aligned_chunk_len(4), 1440);
    assert_eq!(pixel_aligned_chunk_len(7), 1435);
    assert_eq!(pixel_aligned_chunk_len(0), DDP_MAX_PAYLOAD);
}

#[test]
fn spans_number_every_packet_and_push_only_last() {
    let first = vec![0x11; 1500];
    let second = vec![0x22; 30];
    let mut sequence = DdpSequence::default();
    let packets = build_ddp_spans(
        &[
            DdpSpan {
                offset: 0,
                data: &first,
            },
            DdpSpan {
                offset: 6000,
                data: &second,
            },
        ],
        DDP_DTYPE_RGB8,
        DDP_ID_DEFAULT,
        1440,
        &mut sequence,
    );

    let headers: Vec<_> = packets
        .iter()
        .map(|packet| {
            let bytes = packet.as_bytes();
            (
                bytes[0],
                bytes[1],
                u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
                u16::from_be_bytes([bytes[8], bytes[9]]),
            )
        })
        .collect();
    assert_eq!(
        headers,
        vec![(0x40, 1, 0, 1440), (0x40, 2, 1440, 60), (0x41, 3, 6000, 30)],
        "each new packet takes the next sequence number"
    );
    assert_eq!(sequence.current(), 3);
}

#[test]
fn span_sequence_wraps_from_15_to_1_inside_a_frame() {
    let data = vec![0x44; 1440 * 3];
    let mut sequence = DdpSequence::default();
    for _ in 0..13 {
        sequence.advance();
    }
    let packets = build_ddp_spans(
        &[DdpSpan {
            offset: 0,
            data: &data,
        }],
        DDP_DTYPE_RGB8,
        DDP_ID_DEFAULT,
        1440,
        &mut sequence,
    );
    let sequences: Vec<u8> = packets.iter().map(|packet| packet.as_bytes()[1]).collect();
    assert_eq!(
        sequences,
        vec![14, 15, 1],
        "sequence 0 means unused and is skipped"
    );
}

#[test]
fn frame_builder_keeps_one_sequence_per_frame() {
    // The WLED driver's framing: every fragment of a frame shares a number.
    let data = vec![0x55; 3000];
    let mut sequence = DdpSequence::default();
    let packets = build_ddp_frame(&data, DDP_DTYPE_RGB8, &mut sequence);
    let sequences: Vec<u8> = packets.iter().map(|packet| packet.as_bytes()[1]).collect();
    assert_eq!(sequences, vec![1, 1, 1]);
    assert_eq!(
        packets.last().map(|packet| packet.as_bytes()[0]),
        Some(0x41)
    );
}

#[test]
fn odd_stride_chunks_never_split_a_pixel() {
    let data = vec![0x33; 7 * 300];
    let mut sequence = DdpSequence::default();
    let packets = build_ddp_spans(
        &[DdpSpan {
            offset: 0,
            data: &data,
        }],
        DDP_DTYPE_UNDEFINED,
        DDP_ID_DEFAULT,
        pixel_aligned_chunk_len(7),
        &mut sequence,
    );
    for packet in &packets {
        let bytes = packet.as_bytes();
        let offset = u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        let len = u16::from_be_bytes([bytes[8], bytes[9]]);
        assert_eq!(offset % 7, 0);
        assert_eq!(len % 7, 0);
    }
}

#[test]
fn single_span_payloads_match_frame_builder() {
    let data: Vec<u8> = (0..3000_u16).map(|i| (i % 256) as u8).collect();
    let mut a = DdpSequence::default();
    let mut b = DdpSequence::default();
    let frame = build_ddp_frame(&data, DDP_DTYPE_RGB8, &mut a);
    let spans = build_ddp_spans(
        &[DdpSpan {
            offset: 0,
            data: &data,
        }],
        DDP_DTYPE_RGB8,
        DDP_ID_DEFAULT,
        DDP_MAX_PAYLOAD,
        &mut b,
    );
    assert_eq!(frame.len(), spans.len());
    for (left, right) in frame.iter().zip(&spans) {
        // Same flags, type, destination, offset, length, and data; only the
        // sequence byte differs between the two framings.
        assert_eq!(left.as_bytes()[0], right.as_bytes()[0]);
        assert_eq!(left.as_bytes()[2..], right.as_bytes()[2..]);
    }
}
