//! Golden-byte tests for E1.31 data and synchronization packets.
//!
//! Expected bytes are assembled field by field from ANSI E1.31-2018
//! Tables 4-1 (data packet) and 4-2 (synchronization packet), not from
//! the encoder, so a layout regression cannot hide behind itself.

use hypercolor_pixel_protocols::e131::{
    E131_OPTION_FORCE_SYNCHRONIZATION, E131_OPTION_PREVIEW_DATA, E131_OPTION_STREAM_TERMINATED,
    E131_SYNC_PACKET_SIZE, E131Packet, E131SequenceTracker, E131SyncPacket,
};
use uuid::Uuid;

const ACN_ID: [u8; 12] = [
    0x41, 0x53, 0x43, 0x2D, 0x45, 0x31, 0x2E, 0x31, 0x37, 0x00, 0x00, 0x00,
];

fn test_cid() -> Uuid {
    Uuid::from_bytes([
        0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1A, 0x1B, 0x1C, 0x1D, 0x1E,
        0x1F,
    ])
}

fn source_name_field(name: &str) -> [u8; 64] {
    let mut field = [0u8; 64];
    field[..name.len()].copy_from_slice(name.as_bytes());
    field
}

/// Expected data packet for `channels` per E1.31-2018 Table 4-1.
fn expected_data_packet(
    name: &str,
    priority: u8,
    sync_address: u16,
    sequence: u8,
    options: u8,
    universe: u16,
    channels: &[u8],
) -> Vec<u8> {
    let total = 126 + channels.len();
    let flags_len =
        |from: usize| (0x7000_u16 | u16::try_from(total - from).expect("fits")).to_be_bytes();
    let mut out = Vec::new();
    // Root layer
    out.extend_from_slice(&[0x00, 0x10]); // Preamble Size
    out.extend_from_slice(&[0x00, 0x00]); // Post-amble Size
    out.extend_from_slice(&ACN_ID); // ACN Packet Identifier
    out.extend_from_slice(&flags_len(16)); // Flags & Length
    out.extend_from_slice(&[0x00, 0x00, 0x00, 0x04]); // VECTOR_ROOT_E131_DATA
    out.extend_from_slice(test_cid().as_bytes()); // CID
    // Framing layer
    out.extend_from_slice(&flags_len(38));
    out.extend_from_slice(&[0x00, 0x00, 0x00, 0x02]); // VECTOR_E131_DATA_PACKET
    out.extend_from_slice(&source_name_field(name));
    out.push(priority);
    out.extend_from_slice(&sync_address.to_be_bytes());
    out.push(sequence);
    out.push(options);
    out.extend_from_slice(&universe.to_be_bytes());
    // DMP layer
    out.extend_from_slice(&flags_len(115));
    out.push(0x02); // VECTOR_DMP_SET_PROPERTY
    out.push(0xA1); // Address Type & Data Type
    out.extend_from_slice(&[0x00, 0x00]); // First Property Address
    out.extend_from_slice(&[0x00, 0x01]); // Address Increment
    out.extend_from_slice(
        &u16::try_from(channels.len() + 1)
            .expect("fits")
            .to_be_bytes(),
    ); // Property value count
    out.push(0x00); // DMX512-A START Code
    out.extend_from_slice(channels);
    out
}

#[test]
fn data_packet_matches_standard_layout() {
    let channels = [0xFF, 0x80, 0x01];
    let mut packet = E131Packet::new("Hypercolor", test_cid(), 1, 100);
    packet.set_channels(&channels, 7);

    let expected = expected_data_packet("Hypercolor", 100, 0, 7, 0x00, 1, &channels);
    assert_eq!(packet.as_bytes(), expected.as_slice());
    assert_eq!(packet.as_bytes().len(), 129);
}

#[test]
fn full_universe_data_packet_is_638_bytes() {
    let channels: Vec<u8> = (0..512_u16).map(|i| (i % 251) as u8).collect();
    let mut packet = E131Packet::new("Hypercolor", test_cid(), 63_999, 200);
    packet.set_channels(&channels, 255);

    let expected = expected_data_packet("Hypercolor", 200, 0, 255, 0x00, 63_999, &channels);
    assert_eq!(packet.as_bytes(), expected.as_slice());
    assert_eq!(packet.as_bytes().len(), 638);
}

#[test]
fn data_packet_carries_sync_address_and_options() {
    let channels = [1, 2, 3, 4];
    let mut packet = E131Packet::new("Rig", test_cid(), 12, 150);
    packet.set_sync_address(7_000);
    packet.set_stream_terminated(true);
    packet.set_channels(&channels, 42);

    let expected = expected_data_packet(
        "Rig",
        150,
        7_000,
        42,
        E131_OPTION_STREAM_TERMINATED,
        12,
        &channels,
    );
    assert_eq!(packet.as_bytes(), expected.as_slice());
    assert_eq!(packet.sync_address(), 7_000);
}

#[test]
fn option_bits_follow_standard_positions() {
    let mut packet = E131Packet::new("Rig", test_cid(), 1, 100);
    packet.set_preview_data(true);
    assert_eq!(packet.options(), 0b1000_0000, "Preview_Data is bit 7");
    packet.set_preview_data(false);
    packet.set_stream_terminated(true);
    assert_eq!(packet.options(), 0b0100_0000, "Stream_Terminated is bit 6");
    packet.set_stream_terminated(false);
    packet.set_force_synchronization(true);
    assert_eq!(
        packet.options(),
        0b0010_0000,
        "Force_Synchronization is bit 5"
    );
    packet.set_options(E131_OPTION_PREVIEW_DATA | E131_OPTION_FORCE_SYNCHRONIZATION);
    assert_eq!(packet.options(), 0xA0);
}

#[test]
fn source_name_truncates_on_utf8_boundary() {
    // 31 two-byte characters = 62 bytes, then a 3-byte character that
    // would cross the 63-byte limit and must be dropped whole.
    let name = format!("{}{}", "é".repeat(31), "✓");
    let mut packet = E131Packet::new(&name, test_cid(), 1, 100);
    packet.set_channels(&[0], 1);
    let field = &packet.as_bytes()[44..108];
    let end = field
        .iter()
        .position(|&b| b == 0)
        .expect("name is NUL terminated");
    assert_eq!(end, 62);
    assert_eq!(
        std::str::from_utf8(&field[..end]).expect("valid UTF-8"),
        "é".repeat(31)
    );
}

#[test]
fn long_ascii_source_name_keeps_null_terminator() {
    let name = "x".repeat(80);
    let mut packet = E131Packet::new(&name, test_cid(), 1, 100);
    packet.set_channels(&[0], 1);
    let field = &packet.as_bytes()[44..108];
    assert_eq!(&field[..63], "x".repeat(63).as_bytes());
    assert_eq!(field[63], 0, "byte 64 of the field stays NUL");
}

#[test]
fn sync_packet_matches_standard_layout() {
    let packet = E131SyncPacket::new(test_cid(), 7_000, 9);

    let mut expected = Vec::new();
    expected.extend_from_slice(&[0x00, 0x10, 0x00, 0x00]);
    expected.extend_from_slice(&ACN_ID);
    expected.extend_from_slice(&[0x70, 0x21]); // 0x7000 | 33
    expected.extend_from_slice(&[0x00, 0x00, 0x00, 0x08]); // VECTOR_ROOT_E131_EXTENDED
    expected.extend_from_slice(test_cid().as_bytes());
    expected.extend_from_slice(&[0x70, 0x0B]); // 0x7000 | 11
    expected.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]); // VECTOR_E131_EXTENDED_SYNCHRONIZATION
    expected.push(9); // Sequence Number
    expected.extend_from_slice(&7_000_u16.to_be_bytes()); // Synchronization Address
    expected.extend_from_slice(&[0x00, 0x00]); // Reserved

    assert_eq!(packet.as_bytes(), expected.as_slice());
    assert_eq!(packet.as_bytes().len(), E131_SYNC_PACKET_SIZE);
    assert_eq!(packet.sync_address(), 7_000);
}

#[test]
fn sequence_tracker_counts_per_universe_and_wraps() {
    let mut tracker = E131SequenceTracker::default();
    assert_eq!(tracker.advance(5), 1);
    assert_eq!(tracker.advance(6), 1);
    for _ in 0..254 {
        tracker.advance(5);
    }
    assert_eq!(tracker.current(5), 255);
    assert_eq!(
        tracker.advance(5),
        0,
        "E1.31 sequence numbers wrap through 0"
    );
}
