use hypercolor_hal::drivers::corsair::framing::{
    LCD_DATA_PER_PACKET, LCD_PACKET_SIZE, build_lcd_display_packet,
};
use hypercolor_hal::drivers::corsair::{
    CorsairLcdProtocol, build_icue_link_lcd_protocol, build_xc7_rgb_elite_lcd_protocol,
    build_xd6_elite_lcd_protocol,
};
use hypercolor_hal::protocol::{Protocol, TransferType};

fn make_standard_lcd() -> CorsairLcdProtocol {
    CorsairLcdProtocol::new("Test LCD", 480, 480, 0x40, 0x40, true, 0)
}

fn make_standard_lcd_with_ring() -> CorsairLcdProtocol {
    CorsairLcdProtocol::new("Test LCD Ring", 480, 480, 0x40, 0x40, true, 16)
}

// --- build_lcd_display_packet header validation ---

#[test]
fn lcd_packet_header_bytes_match_wire_spec() {
    let packet = build_lcd_display_packet(0x40, false, 0x03, &[0xAA; 100]);

    assert_eq!(packet.len(), LCD_PACKET_SIZE);
    assert_eq!(packet[0], 0x02, "command byte");
    assert_eq!(packet[1], 0x05, "sub-command byte");
    assert_eq!(packet[2], 0x40, "zone byte");
    assert_eq!(packet[3], 0x00, "is_final=false");
    assert_eq!(packet[4], 0x03, "packet_number");
    assert_eq!(packet[5], 0x00, "reserved");
    let data_length = u16::from_le_bytes([packet[6], packet[7]]);
    assert_eq!(
        data_length,
        u16::try_from(LCD_DATA_PER_PACKET).expect("LCD_DATA_PER_PACKET should fit in u16"),
        "data_length field always declares full capacity"
    );
}

#[test]
fn lcd_packet_final_flag_sets_correctly() {
    let non_final = build_lcd_display_packet(0x40, false, 0, &[0xFF]);
    assert_eq!(non_final[3], 0x00);

    let final_pkt = build_lcd_display_packet(0x40, true, 0, &[0xFF]);
    assert_eq!(final_pkt[3], 0x01);
}

#[test]
fn lcd_packet_zone_byte_propagates_to_header() {
    for zone in [0x01, 0x1F, 0x40, 0xFF] {
        let packet = build_lcd_display_packet(zone, true, 0, &[0x00]);
        assert_eq!(
            packet[2], zone,
            "zone byte {zone:#04X} should appear at offset 2"
        );
    }
}

#[test]
fn lcd_packet_payload_appears_at_offset_8_and_remainder_is_zero_padded() {
    let payload = vec![0xDE, 0xAD, 0xBE, 0xEF];
    let packet = build_lcd_display_packet(0x40, true, 0, &payload);

    assert_eq!(&packet[8..12], &[0xDE, 0xAD, 0xBE, 0xEF]);
    assert!(
        packet[12..].iter().all(|&b| b == 0),
        "bytes beyond payload should be zero-padded"
    );
}

#[test]
fn lcd_packet_full_capacity_payload_fills_entire_data_region() {
    let payload = vec![0x42; LCD_DATA_PER_PACKET];
    let packet = build_lcd_display_packet(0x40, true, 0, &payload);

    assert!(
        packet[8..].iter().all(|&b| b == 0x42),
        "full-capacity payload should fill the entire data region"
    );
}

#[test]
fn lcd_packet_oversized_payload_is_truncated_not_panicked() {
    let oversized = vec![0xBB; LCD_DATA_PER_PACKET + 500];
    let packet = build_lcd_display_packet(0x40, true, 0, &oversized);

    assert_eq!(packet.len(), LCD_PACKET_SIZE);
    assert!(
        packet[8..].iter().all(|&b| b == 0xBB),
        "truncated payload should still fill the data region"
    );
}

// --- Protocol-level encode_display_frame ---

#[test]
fn single_chunk_jpeg_produces_one_data_packet_plus_keepalive() {
    let protocol = make_standard_lcd();
    let jpeg = vec![0x55; 100];

    let commands = protocol
        .encode_display_frame(&jpeg)
        .expect("display encoding should succeed");

    assert_eq!(commands.len(), 2);
    assert_eq!(commands[0].transfer_type, TransferType::Bulk);
    assert_eq!(commands[0].data.len(), LCD_PACKET_SIZE);
    assert_eq!(
        commands[0].data[3], 0x01,
        "single packet is also the final packet"
    );
    assert_eq!(commands[0].data[4], 0x00, "first packet is sequence 0");
    assert_eq!(commands[1].transfer_type, TransferType::HidReport);
}

#[test]
fn exact_boundary_jpeg_produces_one_chunk() {
    let protocol = make_standard_lcd();
    let jpeg = vec![0x77; LCD_DATA_PER_PACKET];

    let commands = protocol
        .encode_display_frame(&jpeg)
        .expect("display encoding should succeed");

    let data_commands: Vec<_> = commands
        .iter()
        .filter(|c| c.transfer_type == TransferType::Bulk)
        .collect();
    assert_eq!(data_commands.len(), 1);
    assert_eq!(data_commands[0].data[3], 0x01, "single-chunk is final");
}

#[test]
fn boundary_plus_one_jpeg_produces_two_chunks() {
    let protocol = make_standard_lcd();
    let jpeg = vec![0x77; LCD_DATA_PER_PACKET + 1];

    let commands = protocol
        .encode_display_frame(&jpeg)
        .expect("display encoding should succeed");

    let data_commands: Vec<_> = commands
        .iter()
        .filter(|c| c.transfer_type == TransferType::Bulk)
        .collect();
    assert_eq!(data_commands.len(), 2);
    assert_eq!(data_commands[0].data[3], 0x00, "first chunk is not final");
    assert_eq!(data_commands[0].data[4], 0x00, "first chunk is sequence 0");
    assert_eq!(data_commands[1].data[3], 0x01, "second chunk is final");
    assert_eq!(data_commands[1].data[4], 0x01, "second chunk is sequence 1");
}

#[test]
fn multi_chunk_jpeg_has_correct_packet_count_and_sequence_numbers() {
    let protocol = make_standard_lcd();
    let jpeg_size = LCD_DATA_PER_PACKET * 5 + 200;
    let jpeg = vec![0xAA; jpeg_size];

    let commands = protocol
        .encode_display_frame(&jpeg)
        .expect("display encoding should succeed");

    let data_commands: Vec<_> = commands
        .iter()
        .filter(|c| c.transfer_type == TransferType::Bulk)
        .collect();
    assert_eq!(data_commands.len(), 6);

    for (index, cmd) in data_commands.iter().enumerate() {
        assert_eq!(cmd.data.len(), LCD_PACKET_SIZE, "packet {index} size");
        assert_eq!(
            cmd.data[4],
            u8::try_from(index).unwrap_or(u8::MAX),
            "sequence number for packet {index}"
        );

        let is_last = index == data_commands.len() - 1;
        assert_eq!(
            cmd.data[3],
            u8::from(is_last),
            "final flag for packet {index}"
        );
    }
}

#[test]
fn all_bulk_packets_are_exactly_lcd_packet_size() {
    let protocol = make_standard_lcd();
    let jpeg = vec![0x11; 3000];

    let commands = protocol
        .encode_display_frame(&jpeg)
        .expect("display encoding should succeed");

    for (index, cmd) in commands.iter().enumerate() {
        if cmd.transfer_type == TransferType::Bulk {
            assert_eq!(
                cmd.data.len(),
                LCD_PACKET_SIZE,
                "bulk packet {index} should be exactly LCD_PACKET_SIZE"
            );
        }
    }
}

#[test]
fn empty_jpeg_still_produces_one_packet() {
    let protocol = make_standard_lcd();
    let commands = protocol.encode_display_frame(&[]);

    // div_ceil(0, N) = 0 chunks, so no data packets are emitted
    // but the keepalive still fires, meaning we get just 1 keepalive command
    // OR we might get 0 data + 1 keepalive = 1 total.
    // Verify this doesn't panic and check the invariant.
    match commands {
        Some(cmds) => {
            let bulk_count = cmds
                .iter()
                .filter(|c| c.transfer_type == TransferType::Bulk)
                .count();
            // Zero-length JPEG has 0 chunks, so 0 bulk packets
            assert_eq!(
                bulk_count, 0,
                "empty JPEG should produce no bulk data packets"
            );
        }
        None => panic!("encode_display_frame should always return Some for LCD protocols"),
    }
}

#[test]
fn minimal_one_byte_jpeg_produces_one_data_packet() {
    let protocol = make_standard_lcd();
    let commands = protocol
        .encode_display_frame(&[0xFF])
        .expect("display encoding should succeed");

    let bulk_commands: Vec<_> = commands
        .iter()
        .filter(|c| c.transfer_type == TransferType::Bulk)
        .collect();
    assert_eq!(bulk_commands.len(), 1);
    assert_eq!(bulk_commands[0].data[8], 0xFF, "payload byte at offset 8");
    assert!(
        bulk_commands[0].data[9..LCD_PACKET_SIZE]
            .iter()
            .all(|&b| b == 0),
        "remaining bytes should be zero-padded"
    );
}

// --- encode_display_frame_into buffer reuse ---

#[test]
fn encode_display_frame_into_shrinks_oversized_buffer() {
    let protocol = make_standard_lcd();
    let mut commands = Vec::with_capacity(20);
    for _ in 0..10 {
        commands.push(hypercolor_hal::protocol::ProtocolCommand {
            data: vec![0xDE; 100],
            expects_response: true,
            response_delay: std::time::Duration::from_secs(1),
            post_delay: std::time::Duration::from_secs(1),
            transfer_type: TransferType::Primary,
        });
    }

    let jpeg = vec![0x33; 50];
    protocol
        .encode_display_frame_into(&jpeg, &mut commands)
        .expect("buffer reuse should succeed");

    // 1 data packet + 1 keepalive = 2 commands total
    assert_eq!(
        commands.len(),
        2,
        "buffer should be truncated to actual usage"
    );
    assert_eq!(commands[0].transfer_type, TransferType::Bulk);
    assert_eq!(commands[1].transfer_type, TransferType::HidReport);
}

#[test]
fn encode_display_frame_into_grows_empty_buffer() {
    let protocol = make_standard_lcd();
    let mut commands = Vec::new();

    let jpeg = vec![0x99; LCD_DATA_PER_PACKET * 3];
    protocol
        .encode_display_frame_into(&jpeg, &mut commands)
        .expect("buffer growth should succeed");

    let bulk_count = commands
        .iter()
        .filter(|c| c.transfer_type == TransferType::Bulk)
        .count();
    assert_eq!(bulk_count, 3, "3 full chunks should produce 3 bulk packets");
}

// --- Model-specific display encoding ---

#[test]
fn xc7_display_encoding_uses_zone_byte_0x1f() {
    let protocol = build_xc7_rgb_elite_lcd_protocol();
    let jpeg = vec![0x55; 500];

    let commands = protocol
        .encode_display_frame(&jpeg)
        .expect("XC7 display encoding should succeed");

    let bulk_cmd = commands
        .iter()
        .find(|c| c.transfer_type == TransferType::Bulk)
        .expect("should have at least one bulk data packet");
    assert_eq!(bulk_cmd.data[2], 0x1F, "XC7 data zone byte");

    let keepalive = commands
        .iter()
        .find(|c| c.transfer_type == TransferType::HidReport)
        .expect("should have a keepalive");
    assert_eq!(keepalive.data[2], 0x1C, "XC7 keepalive zone byte");
}

#[test]
fn icue_link_lcd_display_encoding_uses_zone_byte_0x40() {
    let protocol = build_icue_link_lcd_protocol();
    let jpeg = vec![0x55; 500];

    let commands = protocol
        .encode_display_frame(&jpeg)
        .expect("iCUE LINK LCD display encoding should succeed");

    let bulk_cmd = commands
        .iter()
        .find(|c| c.transfer_type == TransferType::Bulk)
        .expect("should have at least one bulk data packet");
    assert_eq!(bulk_cmd.data[2], 0x40, "iCUE LINK data zone byte");
}

#[test]
fn xd6_lcd_display_encoding_uses_zone_byte_0x01() {
    let protocol = build_xd6_elite_lcd_protocol();
    let jpeg = vec![0x55; 500];

    let commands = protocol
        .encode_display_frame(&jpeg)
        .expect("XD6 LCD display encoding should succeed");

    let bulk_cmd = commands
        .iter()
        .find(|c| c.transfer_type == TransferType::Bulk)
        .expect("should have at least one bulk data packet");
    assert_eq!(bulk_cmd.data[2], 0x01, "XD6 data zone byte");
}

// --- LCD with ring LEDs: display encoding is independent of ring ---

#[test]
fn lcd_with_ring_still_encodes_display_frames_identically() {
    let no_ring = make_standard_lcd();
    let with_ring = make_standard_lcd_with_ring();
    let jpeg = vec![0xCC; 2000];

    let no_ring_cmds = no_ring
        .encode_display_frame(&jpeg)
        .expect("no-ring LCD should encode display");
    let with_ring_cmds = with_ring
        .encode_display_frame(&jpeg)
        .expect("ring LCD should encode display");

    let no_ring_bulk: Vec<_> = no_ring_cmds
        .iter()
        .filter(|c| c.transfer_type == TransferType::Bulk)
        .collect();
    let with_ring_bulk: Vec<_> = with_ring_cmds
        .iter()
        .filter(|c| c.transfer_type == TransferType::Bulk)
        .collect();

    assert_eq!(no_ring_bulk.len(), with_ring_bulk.len());
    for (index, (a, b)) in no_ring_bulk.iter().zip(with_ring_bulk.iter()).enumerate() {
        assert_eq!(a.data, b.data, "bulk packet {index} should be identical");
    }
}

// --- Keepalive integration with display frames ---

#[test]
fn keepalive_packet_reports_correct_chunk_count_and_data_length() {
    let protocol = make_standard_lcd();
    let jpeg_size = LCD_DATA_PER_PACKET * 3 + 100;
    let jpeg = vec![0x88; jpeg_size];

    let commands = protocol
        .encode_display_frame(&jpeg)
        .expect("display encoding should succeed");

    let keepalive = commands
        .iter()
        .find(|c| c.transfer_type == TransferType::HidReport)
        .expect("keepalive should be present on first encode");

    assert_eq!(keepalive.data[0], 0x03, "keepalive command byte");
    assert_eq!(keepalive.data[1], 0x19, "keepalive sub-command byte");
    assert_eq!(keepalive.data[3], 0x01, "keepalive final_packet flag");
    assert_eq!(keepalive.data[4], 0x04, "packets_sent count (4 chunks)");
    let data_length = u16::from_le_bytes([keepalive.data[6], keepalive.data[7]]);
    assert_eq!(
        data_length,
        u16::try_from(LCD_DATA_PER_PACKET).expect("LCD_DATA_PER_PACKET should fit in u16"),
        "keepalive reports LCD_DATA_PER_PACKET as data_length"
    );
}

#[test]
fn second_encode_suppresses_keepalive_within_interval() {
    let protocol = make_standard_lcd();
    let jpeg = vec![0x55; 100];

    let first = protocol
        .encode_display_frame(&jpeg)
        .expect("first encode should succeed");
    let keepalive_count_1 = first
        .iter()
        .filter(|c| c.transfer_type == TransferType::HidReport)
        .count();
    assert_eq!(
        keepalive_count_1, 1,
        "first encode should include keepalive"
    );

    let second = protocol
        .encode_display_frame(&jpeg)
        .expect("second encode should succeed");
    let keepalive_count_2 = second
        .iter()
        .filter(|c| c.transfer_type == TransferType::HidReport)
        .count();
    assert_eq!(
        keepalive_count_2, 0,
        "second encode within interval should suppress keepalive"
    );
}

// --- Payload integrity across chunks ---

#[test]
fn jpeg_payload_bytes_are_preserved_across_chunks() {
    let protocol = make_standard_lcd();
    let jpeg: Vec<u8> = (0_u16..2500_u16)
        .map(|v| u8::try_from(v % 251).unwrap_or_default())
        .collect();

    let commands = protocol
        .encode_display_frame(&jpeg)
        .expect("display encoding should succeed");

    let mut reassembled = Vec::new();
    for cmd in &commands {
        if cmd.transfer_type == TransferType::Bulk {
            let actual_payload_end = if reassembled.len() + LCD_DATA_PER_PACKET <= jpeg.len() {
                8 + LCD_DATA_PER_PACKET
            } else {
                8 + (jpeg.len() - reassembled.len())
            };
            reassembled.extend_from_slice(&cmd.data[8..actual_payload_end]);
        }
    }

    assert_eq!(reassembled.len(), jpeg.len());
    assert_eq!(
        reassembled, jpeg,
        "reassembled payload should match original JPEG data"
    );
}
