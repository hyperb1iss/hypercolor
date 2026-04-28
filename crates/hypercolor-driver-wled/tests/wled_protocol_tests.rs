//! Tests for the WLED device backend.
//!
//! Tests use parsing/unit checks plus local loopback UDP for streaming behavior.

use std::fs::OpenOptions;
use std::io::Write;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, Once, OnceLock};
use std::time::{Duration, Instant};

use hypercolor_core::device::DiscoveryConnectBehavior;
use hypercolor_core::device::{DeviceBackend, TransportScanner};
use hypercolor_driver_wled::{
    DdpPacket, DdpSequence, E131Packet, E131SequenceTracker, WledBackend, WledColorFormat,
    WledDeviceInfo, WledLiveReceiverConfig, WledProtocol, WledScanner, WledSegmentInfo,
    build_ddp_frame, universes_needed,
};
use hypercolor_types::device::DeviceId;
use mdns_sd::{ServiceDaemon, ServiceInfo};
use tokio::net::UdpSocket;
use tokio::sync::Mutex as AsyncMutex;
use tokio::time::timeout;

// ── DDP Constants ───────────────────────────────────────────────────────

const DDP_HEADER_SIZE: usize = 10;
const DDP_VERSION: u8 = 0x40;
const DDP_FLAG_PUSH: u8 = 0x01;
const DDP_DTYPE_RGB8: u8 = 0x0B;
const DDP_DTYPE_RGBW8: u8 = 0x1B;
const DDP_ID_DEFAULT: u8 = 0x01;

// ── DDP Header Layout Tests ────────────────────────────────────────────

#[test]
fn ddp_header_byte_0_flags_version_and_push() {
    // Final packet: version 1 + push
    let packet = DdpPacket::new(&[0xFF, 0x00, 0x00], 0, true, 1, DDP_DTYPE_RGB8);
    let bytes = packet.as_bytes();

    assert_eq!(
        bytes[0],
        DDP_VERSION | DDP_FLAG_PUSH,
        "byte 0: version 1 + push"
    );
}

#[test]
fn ddp_header_byte_0_flags_no_push() {
    // Non-final packet: version 1, no push
    let packet = DdpPacket::new(&[0xFF, 0x00, 0x00], 0, false, 1, DDP_DTYPE_RGB8);
    let bytes = packet.as_bytes();

    assert_eq!(bytes[0], DDP_VERSION, "byte 0: version 1, no push");
}

#[test]
fn ddp_header_byte_1_sequence_number() {
    let packet = DdpPacket::new(&[0x00; 3], 0, true, 7, DDP_DTYPE_RGB8);
    let bytes = packet.as_bytes();

    assert_eq!(bytes[1], 7, "byte 1: sequence number 7");
}

#[test]
fn ddp_header_byte_1_sequence_masked_to_low_nibble() {
    // Sequence values above 15 should be masked
    let packet = DdpPacket::new(&[0x00; 3], 0, true, 0xFF, DDP_DTYPE_RGB8);
    let bytes = packet.as_bytes();

    assert_eq!(bytes[1], 0x0F, "byte 1: sequence masked to low nibble");
}

#[test]
fn ddp_header_byte_2_data_type_rgb8() {
    let packet = DdpPacket::new(&[0x00; 3], 0, true, 1, DDP_DTYPE_RGB8);
    let bytes = packet.as_bytes();

    assert_eq!(bytes[2], 0x0B, "byte 2: RGB 8-bit data type");
}

#[test]
fn ddp_header_byte_2_data_type_rgbw8() {
    let packet = DdpPacket::new(&[0x00; 4], 0, true, 1, DDP_DTYPE_RGBW8);
    let bytes = packet.as_bytes();

    assert_eq!(bytes[2], 0x1B, "byte 2: RGBW 8-bit data type");
}

#[test]
fn ddp_header_byte_3_destination_id() {
    let packet = DdpPacket::new(&[0x00; 3], 0, true, 1, DDP_DTYPE_RGB8);
    let bytes = packet.as_bytes();

    assert_eq!(bytes[3], DDP_ID_DEFAULT, "byte 3: default destination ID");
}

#[test]
fn ddp_header_bytes_4_7_data_offset_zero() {
    let packet = DdpPacket::new(&[0x00; 3], 0, true, 1, DDP_DTYPE_RGB8);
    let bytes = packet.as_bytes();

    let offset = u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    assert_eq!(offset, 0, "bytes 4-7: data offset should be 0");
}

#[test]
fn ddp_header_bytes_4_7_data_offset_nonzero() {
    let packet = DdpPacket::new(&[0x00; 3], 1440, true, 1, DDP_DTYPE_RGB8);
    let bytes = packet.as_bytes();

    let offset = u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    assert_eq!(offset, 1440, "bytes 4-7: data offset should be 1440");
}

#[test]
fn ddp_header_bytes_8_9_data_length() {
    let pixel_data = vec![0xAA; 90]; // 30 RGB pixels
    let packet = DdpPacket::new(&pixel_data, 0, true, 1, DDP_DTYPE_RGB8);
    let bytes = packet.as_bytes();

    let length = u16::from_be_bytes([bytes[8], bytes[9]]);
    assert_eq!(length, 90, "bytes 8-9: data length should be 90");
}

#[test]
fn ddp_header_total_size_is_10() {
    let packet = DdpPacket::new(&[0x00; 3], 0, true, 1, DDP_DTYPE_RGB8);
    let bytes = packet.as_bytes();

    assert_eq!(
        bytes.len(),
        DDP_HEADER_SIZE + 3,
        "total packet = 10 header + 3 data"
    );
}

#[test]
fn ddp_payload_follows_header() {
    let pixel_data = [0xFF, 0x80, 0x00]; // orange pixel
    let packet = DdpPacket::new(&pixel_data, 0, true, 1, DDP_DTYPE_RGB8);
    let bytes = packet.as_bytes();

    assert_eq!(bytes[10], 0xFF, "R channel");
    assert_eq!(bytes[11], 0x80, "G channel");
    assert_eq!(bytes[12], 0x00, "B channel");
}

// ── DDP Fragmentation Tests ────────────────────────────────────────────

#[test]
fn ddp_single_packet_for_small_frame() {
    let mut seq = DdpSequence::default();
    let pixel_data = vec![0x00; 90]; // 30 RGB pixels = 90 bytes

    let packets = build_ddp_frame(&pixel_data, DDP_DTYPE_RGB8, &mut seq);
    assert_eq!(packets.len(), 1, "30 pixels should fit in one packet");

    // Single packet should have push flag set
    let bytes = packets[0].as_bytes();
    assert_eq!(bytes[0] & DDP_FLAG_PUSH, DDP_FLAG_PUSH, "push flag set");
}

#[test]
fn ddp_fragmentation_for_large_frame() {
    let mut seq = DdpSequence::default();
    // 600 RGB pixels = 1800 bytes, exceeds 1440 max payload
    let pixel_data = vec![0xAA; 1800];

    let packets = build_ddp_frame(&pixel_data, DDP_DTYPE_RGB8, &mut seq);
    assert_eq!(
        packets.len(),
        2,
        "600 pixels should be split into 2 packets"
    );

    // First packet: no push
    let p1 = packets[0].as_bytes();
    assert_eq!(p1[0] & DDP_FLAG_PUSH, 0, "first packet: no push");
    let p1_offset = u32::from_be_bytes([p1[4], p1[5], p1[6], p1[7]]);
    assert_eq!(p1_offset, 0, "first packet: offset 0");
    let p1_len = u16::from_be_bytes([p1[8], p1[9]]);
    assert_eq!(p1_len, 1440, "first packet: 1440 bytes");

    // Second packet: push
    let p2 = packets[1].as_bytes();
    assert_eq!(p2[0] & DDP_FLAG_PUSH, DDP_FLAG_PUSH, "second packet: push");
    let p2_offset = u32::from_be_bytes([p2[4], p2[5], p2[6], p2[7]]);
    assert_eq!(p2_offset, 1440, "second packet: offset 1440");
    let p2_len = u16::from_be_bytes([p2[8], p2[9]]);
    assert_eq!(p2_len, 360, "second packet: 360 bytes remaining");
}

#[test]
fn ddp_fragmentation_three_packets() {
    let mut seq = DdpSequence::default();
    // 1000 RGB pixels = 3000 bytes = 3 packets (1440 + 1440 + 120)
    let pixel_data = vec![0x55; 3000];

    let packets = build_ddp_frame(&pixel_data, DDP_DTYPE_RGB8, &mut seq);
    assert_eq!(
        packets.len(),
        3,
        "1000 pixels should be split into 3 packets"
    );

    // Only the last packet should have push set
    for (i, packet) in packets.iter().enumerate() {
        let bytes = packet.as_bytes();
        if i == 2 {
            assert_eq!(bytes[0] & DDP_FLAG_PUSH, DDP_FLAG_PUSH, "packet {i}: push");
        } else {
            assert_eq!(bytes[0] & DDP_FLAG_PUSH, 0, "packet {i}: no push");
        }
    }
}

#[test]
fn ddp_fragmentation_exactly_480_pixels_single_packet() {
    let mut seq = DdpSequence::default();
    // 480 RGB pixels = 1440 bytes, exactly max payload
    let pixel_data = vec![0x00; 1440];

    let packets = build_ddp_frame(&pixel_data, DDP_DTYPE_RGB8, &mut seq);
    assert_eq!(
        packets.len(),
        1,
        "480 pixels (1440 bytes) should fit in one packet"
    );
}

#[test]
fn ddp_fragmentation_481_pixels_two_packets() {
    let mut seq = DdpSequence::default();
    // 481 RGB pixels = 1443 bytes, one byte over max
    let pixel_data = vec![0x00; 1443];

    let packets = build_ddp_frame(&pixel_data, DDP_DTYPE_RGB8, &mut seq);
    assert_eq!(packets.len(), 2, "481 pixels should need 2 packets");
}

#[test]
fn ddp_all_fragments_share_sequence_number() {
    let mut seq = DdpSequence::default();
    let pixel_data = vec![0x00; 3000]; // 3 packets

    let packets = build_ddp_frame(&pixel_data, DDP_DTYPE_RGB8, &mut seq);

    let seq_num = packets[0].as_bytes()[1];
    for (i, packet) in packets.iter().enumerate() {
        assert_eq!(
            packet.as_bytes()[1],
            seq_num,
            "packet {i} should share the same sequence number"
        );
    }
}

// ── DDP Sequence Number Tests ──────────────────────────────────────────

#[test]
fn ddp_sequence_starts_at_one() {
    let mut seq = DdpSequence::default();
    assert_eq!(seq.advance(), 1, "first sequence should be 1");
}

#[test]
fn ddp_sequence_increments() {
    let mut seq = DdpSequence::default();
    assert_eq!(seq.advance(), 1);
    assert_eq!(seq.advance(), 2);
    assert_eq!(seq.advance(), 3);
}

#[test]
fn ddp_sequence_wraps_at_15() {
    let mut seq = DdpSequence::default();

    // Advance to 15
    for expected in 1..=15 {
        assert_eq!(seq.advance(), expected, "sequence should be {expected}");
    }

    // Should wrap back to 1 (not 0, since 0 means "not used")
    assert_eq!(seq.advance(), 1, "sequence should wrap from 15 to 1");
    assert_eq!(seq.advance(), 2, "sequence should continue after wrap");
}

#[test]
fn ddp_sequence_wraps_multiple_cycles() {
    let mut seq = DdpSequence::default();

    // Run through 3 full cycles
    for cycle in 0..3 {
        for expected in 1..=15 {
            let val = seq.advance();
            assert_eq!(
                val, expected,
                "cycle {cycle}, expected {expected}, got {val}"
            );
        }
    }
}

// ── E1.31 Packet Tests ─────────────────────────────────────────────────

#[test]
fn e131_packet_preamble() {
    let cid = uuid::Uuid::nil();
    let packet = E131Packet::new("Hypercolor", cid, 1, 150);
    let bytes = packet.as_bytes();

    // Preamble size: 0x0010
    assert_eq!(bytes[0], 0x00);
    assert_eq!(bytes[1], 0x10);

    // Postamble size: 0x0000
    assert_eq!(bytes[2], 0x00);
    assert_eq!(bytes[3], 0x00);
}

#[test]
fn e131_packet_acn_identifier() {
    let cid = uuid::Uuid::nil();
    let packet = E131Packet::new("Hypercolor", cid, 1, 150);
    let bytes = packet.as_bytes();

    let expected = b"ASC-E1.17\x00\x00\x00";
    assert_eq!(&bytes[4..16], expected, "ACN packet identifier");
}

#[test]
fn e131_packet_root_vector() {
    let cid = uuid::Uuid::nil();
    let packet = E131Packet::new("Hypercolor", cid, 1, 150);
    let bytes = packet.as_bytes();

    let vector = u32::from_be_bytes([bytes[18], bytes[19], bytes[20], bytes[21]]);
    assert_eq!(vector, 0x0000_0004, "root vector: VECTOR_ROOT_E131_DATA");
}

#[test]
fn e131_packet_cid() {
    let cid = uuid::Uuid::from_bytes([
        0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F,
        0x10,
    ]);
    let packet = E131Packet::new("Hypercolor", cid, 1, 150);
    let bytes = packet.as_bytes();

    assert_eq!(&bytes[22..38], cid.as_bytes(), "CID should match");
}

#[test]
fn e131_packet_framing_vector() {
    let cid = uuid::Uuid::nil();
    let packet = E131Packet::new("Hypercolor", cid, 1, 150);
    let bytes = packet.as_bytes();

    let vector = u32::from_be_bytes([bytes[40], bytes[41], bytes[42], bytes[43]]);
    assert_eq!(
        vector, 0x0000_0002,
        "framing vector: VECTOR_E131_DATA_PACKET"
    );
}

#[test]
fn e131_packet_source_name() {
    let cid = uuid::Uuid::nil();
    let packet = E131Packet::new("Hypercolor", cid, 1, 150);
    let bytes = packet.as_bytes();

    // Source name starts at byte 44, 64 bytes, null-padded
    let name_end = bytes[44..108].iter().position(|&b| b == 0).unwrap_or(64);
    let name = std::str::from_utf8(&bytes[44..44 + name_end]).expect("valid UTF-8");
    assert_eq!(name, "Hypercolor");
}

#[test]
fn e131_packet_priority() {
    let cid = uuid::Uuid::nil();
    let packet = E131Packet::new("Hypercolor", cid, 1, 150);
    let bytes = packet.as_bytes();

    assert_eq!(bytes[108], 150, "priority should be 150");
}

#[test]
fn e131_packet_universe_number() {
    let cid = uuid::Uuid::nil();
    let packet = E131Packet::new("Hypercolor", cid, 42, 150);
    let bytes = packet.as_bytes();

    let universe = u16::from_be_bytes([bytes[113], bytes[114]]);
    assert_eq!(universe, 42, "universe should be 42");
}

#[test]
fn e131_packet_universe_accessor() {
    let cid = uuid::Uuid::nil();
    let packet = E131Packet::new("Hypercolor", cid, 256, 100);
    assert_eq!(packet.universe(), 256);
}

#[test]
fn e131_packet_dmp_vector() {
    let cid = uuid::Uuid::nil();
    let packet = E131Packet::new("Hypercolor", cid, 1, 150);
    let bytes = packet.as_bytes();

    assert_eq!(bytes[117], 0x02, "DMP vector: VECTOR_DMP_SET_PROPERTY");
}

#[test]
fn e131_packet_dmx_start_code() {
    let cid = uuid::Uuid::nil();
    let packet = E131Packet::new("Hypercolor", cid, 1, 150);
    let bytes = packet.as_bytes();

    assert_eq!(bytes[125], 0x00, "DMX start code should be 0");
}

#[test]
fn e131_packet_set_channels() {
    let cid = uuid::Uuid::nil();
    let mut packet = E131Packet::new("Hypercolor", cid, 1, 150);

    // 3 RGB pixels = 9 channels
    let channels = [0xFF, 0x00, 0x00, 0x00, 0xFF, 0x00, 0x00, 0x00, 0xFF];
    packet.set_channels(&channels, 42);

    let bytes = packet.as_bytes();

    // Sequence number at byte 111
    assert_eq!(bytes[111], 42, "sequence number");

    // DMX data starts at byte 126
    assert_eq!(bytes[126], 0xFF, "pixel 0 R");
    assert_eq!(bytes[127], 0x00, "pixel 0 G");
    assert_eq!(bytes[128], 0x00, "pixel 0 B");
    assert_eq!(bytes[129], 0x00, "pixel 1 R");
    assert_eq!(bytes[130], 0xFF, "pixel 1 G");
    assert_eq!(bytes[131], 0x00, "pixel 1 B");
    assert_eq!(bytes[132], 0x00, "pixel 2 R");
    assert_eq!(bytes[133], 0x00, "pixel 2 G");
    assert_eq!(bytes[134], 0xFF, "pixel 2 B");

    // Property value count = channels + 1 (for start code)
    let prop_count = u16::from_be_bytes([bytes[123], bytes[124]]);
    assert_eq!(
        prop_count, 10,
        "property value count = 9 channels + 1 start code"
    );
}

#[test]
fn e131_packet_max_channels_fits_buffer() {
    let cid = uuid::Uuid::nil();
    let mut packet = E131Packet::new("Hypercolor", cid, 1, 150);
    let channels = vec![0x7f; 512];

    packet.set_channels(&channels, 7);

    assert_eq!(packet.as_bytes().len(), 638);
}

#[test]
fn e131_packet_length_fields_updated() {
    let cid = uuid::Uuid::nil();
    let mut packet = E131Packet::new("Hypercolor", cid, 1, 150);

    let channels = vec![0xAA; 510]; // 170 RGB pixels
    packet.set_channels(&channels, 1);

    let bytes = packet.as_bytes();

    // DMP layer length (from byte 115): count + 11
    let dmp_flags_len = u16::from_be_bytes([bytes[115], bytes[116]]);
    let dmp_len = dmp_flags_len & 0x0FFF;
    assert_eq!(dmp_len, 510 + 11, "DMP layer length");

    // Framing layer length (from byte 38): count + 88
    let frame_flags_len = u16::from_be_bytes([bytes[38], bytes[39]]);
    let frame_len = frame_flags_len & 0x0FFF;
    assert_eq!(frame_len, 510 + 88, "framing layer length");

    // Root layer length (from byte 16): count + 110
    let root_flags_len = u16::from_be_bytes([bytes[16], bytes[17]]);
    let root_len = root_flags_len & 0x0FFF;
    assert_eq!(root_len, 510 + 110, "root layer length");
}

// ── Multi-Universe Tests ────────────────────────────────────────────────

#[test]
fn universes_needed_rgb_small() {
    // 100 RGB pixels -> 1 universe (170 max)
    assert_eq!(universes_needed(100, 3), 1);
}

#[test]
fn universes_needed_rgb_exactly_170() {
    // 170 RGB pixels -> 1 universe
    assert_eq!(universes_needed(170, 3), 1);
}

#[test]
fn universes_needed_rgb_171() {
    // 171 RGB pixels -> 2 universes
    assert_eq!(universes_needed(171, 3), 2);
}

#[test]
fn universes_needed_rgb_340() {
    // 340 RGB pixels -> 2 universes
    assert_eq!(universes_needed(340, 3), 2);
}

#[test]
fn universes_needed_rgb_341() {
    // 341 RGB pixels -> 3 universes
    assert_eq!(universes_needed(341, 3), 3);
}

#[test]
fn universes_needed_rgbw() {
    // 128 RGBW pixels -> 512 channels -> 1 universe
    assert_eq!(universes_needed(128, 4), 1);
}

#[test]
fn universes_needed_rgbw_129() {
    // 129 RGBW pixels -> 2 universes
    assert_eq!(universes_needed(129, 4), 2);
}

#[test]
fn universes_needed_large_installation() {
    // 1000 RGB pixels -> ceil(1000/170) = 6 universes
    assert_eq!(universes_needed(1000, 3), 6);
}

// ── E1.31 Sequence Tracker Tests ────────────────────────────────────────

#[test]
fn e131_sequence_starts_at_one() {
    let mut tracker = E131SequenceTracker::default();
    assert_eq!(tracker.advance(1), 1);
}

#[test]
fn e131_sequence_per_universe() {
    let mut tracker = E131SequenceTracker::default();

    assert_eq!(tracker.advance(1), 1);
    assert_eq!(tracker.advance(1), 2);
    assert_eq!(tracker.advance(2), 1); // Different universe starts fresh
    assert_eq!(tracker.advance(1), 3); // Universe 1 continues
    assert_eq!(tracker.advance(2), 2); // Universe 2 continues
}

#[test]
fn e131_sequence_wraps_at_255() {
    let mut tracker = E131SequenceTracker::default();

    for _ in 0..255 {
        tracker.advance(1);
    }
    // After 255 calls, should be at 255
    assert_eq!(tracker.current(1), 255);

    // Next call should wrap to 0
    assert_eq!(tracker.advance(1), 0);
    assert_eq!(tracker.advance(1), 1);
}

// ── WLED JSON API Parsing Tests ────────────────────────────────────────

#[test]
fn parse_wled_info_basic() {
    let json: serde_json::Value = serde_json::json!({
        "ver": "0.15.3",
        "vid": 2_312_050,
        "mac": "aabbccddeeff",
        "name": "Kitchen LEDs",
        "leds": {
            "count": 300,
            "rgbw": false,
            "maxseg": 16,
            "fps": 42,
            "pwr": 1500,
            "maxpwr": 5000
        },
        "freeheap": 120_000,
        "uptime": 86400,
        "arch": "esp32",
        "wifi": {
            "bssid": "aa:bb:cc:dd:ee:ff"
        },
        "fxcount": 118,
        "palcount": 71
    });

    let info =
        hypercolor_driver_wled::backend::parse_wled_info(&json).expect("should parse valid info");

    assert_eq!(info.firmware_version, "0.15.3");
    assert_eq!(info.build_id, 2_312_050);
    assert_eq!(info.mac, "aabbccddeeff");
    assert_eq!(info.name, "Kitchen LEDs");
    assert_eq!(info.led_count, 300);
    assert!(!info.rgbw);
    assert_eq!(info.max_segments, 16);
    assert_eq!(info.fps, 42);
    assert_eq!(info.power_draw_ma, 1500);
    assert_eq!(info.max_power_ma, 5000);
    assert_eq!(info.free_heap, 120_000);
    assert_eq!(info.uptime_secs, 86400);
    assert_eq!(info.arch, "esp32");
    assert!(info.is_wifi);
    assert_eq!(info.effect_count, 118);
    assert_eq!(info.palette_count, 71);
}

#[test]
fn parse_wled_info_rgbw_device() {
    let json: serde_json::Value = serde_json::json!({
        "ver": "0.14.0",
        "vid": 0,
        "mac": "112233445566",
        "name": "RGBW Strip",
        "leds": {
            "count": 60,
            "rgbw": true,
            "maxseg": 8,
            "fps": 30,
            "pwr": 0,
            "maxpwr": 0
        },
        "freeheap": 50000,
        "uptime": 3600,
        "arch": "esp8266",
        "fxcount": 50,
        "palcount": 20
    });

    let info =
        hypercolor_driver_wled::backend::parse_wled_info(&json).expect("should parse RGBW info");

    assert!(info.rgbw, "should be RGBW");
    assert_eq!(info.led_count, 60);
    assert_eq!(info.arch, "esp8266");
}

#[test]
fn parse_wled_info_missing_ver_fails() {
    let json: serde_json::Value = serde_json::json!({
        "mac": "aabbccddeeff",
        "name": "Bad Device"
    });

    let result = hypercolor_driver_wled::backend::parse_wled_info(&json);
    assert!(result.is_err(), "missing 'ver' should fail");
}

#[test]
fn parse_wled_info_with_defaults() {
    // Minimal valid response — most fields missing
    let json: serde_json::Value = serde_json::json!({
        "ver": "0.10.0"
    });

    let info = hypercolor_driver_wled::backend::parse_wled_info(&json)
        .expect("should parse with defaults");

    assert_eq!(info.firmware_version, "0.10.0");
    assert_eq!(info.mac, "");
    assert_eq!(info.name, "WLED");
    assert_eq!(info.led_count, 0);
    assert!(!info.rgbw);
    assert!(!info.is_wifi);
}

#[test]
fn parse_wled_segments() {
    let json: serde_json::Value = serde_json::json!({
        "seg": [
            {
                "id": 0,
                "start": 0,
                "stop": 150,
                "grp": 1,
                "spc": 0,
                "on": true,
                "bri": 255,
                "lc": 1
            },
            {
                "id": 1,
                "start": 150,
                "stop": 300,
                "grp": 2,
                "spc": 1,
                "on": true,
                "bri": 128,
                "lc": 3
            }
        ]
    });

    let segments =
        hypercolor_driver_wled::backend::parse_wled_segments(&json).expect("should parse segments");

    assert_eq!(segments.len(), 2);

    // Segment 0
    assert_eq!(segments[0].id, 0);
    assert_eq!(segments[0].start, 0);
    assert_eq!(segments[0].stop, 150);
    assert!(!segments[0].rgbw);

    // Segment 1 — RGBW (lc bit 1 set)
    assert_eq!(segments[1].id, 1);
    assert_eq!(segments[1].start, 150);
    assert_eq!(segments[1].stop, 300);
    assert!(segments[1].rgbw, "lc=3 means RGB+W");
    assert_eq!(segments[1].brightness, 128);
}

#[test]
fn parse_wled_segments_missing_seg_array_fails() {
    let json: serde_json::Value = serde_json::json!({
        "on": true,
        "bri": 255
    });

    let result = hypercolor_driver_wled::backend::parse_wled_segments(&json);
    assert!(result.is_err(), "missing 'seg' array should fail");
}

#[test]
fn parse_wled_live_receiver_config_e131() {
    let json = serde_json::json!({
        "if": {
            "live": {
                "en": true,
                "rlm": true,
                "port": 5568,
                "dmx": {
                    "uni": 3,
                    "addr": 1,
                    "mode": 6
                }
            }
        }
    });

    let config = hypercolor_driver_wled::backend::parse_wled_live_receiver_config(&json)
        .expect("parse live config")
        .expect("live config present");

    assert_eq!(
        config,
        WledLiveReceiverConfig {
            enabled: true,
            realtime_mode_enabled: true,
            port: 5568,
            dmx_address: Some(1),
            dmx_universe: Some(3),
            dmx_mode: Some(6),
        }
    );
}

#[test]
fn parse_wled_live_receiver_config_missing_live_returns_none() {
    let json = serde_json::json!({
        "if": {
            "sync": {
                "port0": 21324
            }
        }
    });

    let config = hypercolor_driver_wled::backend::parse_wled_live_receiver_config(&json)
        .expect("parse config");

    assert!(config.is_none(), "missing live block should return none");
}

#[test]
fn ddp_receiver_config_mismatches_ignore_e131_fields() {
    let config = WledLiveReceiverConfig {
        enabled: true,
        realtime_mode_enabled: true,
        port: 5568,
        dmx_address: Some(1),
        dmx_universe: Some(1),
        dmx_mode: Some(4),
    };

    let mismatches = hypercolor_driver_wled::backend::wled_receiver_config_mismatches(
        &config,
        WledProtocol::Ddp,
        WledColorFormat::Rgb,
        1,
    );

    assert!(
        mismatches.is_empty(),
        "DDP should ignore E1.31-only /json/cfg fields"
    );
}

#[test]
fn e131_receiver_config_mismatches_report_port_and_mode() {
    let config = WledLiveReceiverConfig {
        enabled: true,
        realtime_mode_enabled: true,
        port: 4048,
        dmx_address: Some(5),
        dmx_universe: Some(2),
        dmx_mode: Some(4),
    };

    let mismatches = hypercolor_driver_wled::backend::wled_receiver_config_mismatches(
        &config,
        WledProtocol::E131,
        WledColorFormat::Rgbw,
        1,
    );

    assert_eq!(mismatches.len(), 4);
    assert!(
        mismatches
            .iter()
            .any(|m| m.contains("expected E1.31 port 5568"))
    );
    assert!(
        mismatches
            .iter()
            .any(|m| m.contains("expected start universe 1"))
    );
    assert!(
        mismatches
            .iter()
            .any(|m| m.contains("expected DMX start address 1"))
    );
    assert!(
        mismatches
            .iter()
            .any(|m| m.contains("expected DMX mode 6 (multiple_rgbw)"))
    );
}

// ── WledSegmentInfo pixel_count Tests ──────────────────────────────────

#[test]
fn segment_pixel_count_simple() {
    let seg = WledSegmentInfo {
        id: 0,
        start: 0,
        stop: 100,
        grouping: 1,
        spacing: 0,
        on: true,
        brightness: 255,
        rgbw: false,
        light_capabilities: 1,
    };

    assert_eq!(seg.pixel_count(), 100);
}

#[test]
fn segment_pixel_count_with_grouping_and_spacing() {
    let seg = WledSegmentInfo {
        id: 0,
        start: 0,
        stop: 30,
        grouping: 2,
        spacing: 1,
        on: true,
        brightness: 255,
        rgbw: false,
        light_capabilities: 1,
    };

    // group_size = 2 + 1 = 3, raw = 30
    // 30 / 3 * 2 = 20
    assert_eq!(seg.pixel_count(), 20);
}

// ── WledColorFormat Tests ──────────────────────────────────────────────

#[test]
fn color_format_bytes_per_pixel() {
    assert_eq!(WledColorFormat::Rgb.bytes_per_pixel(), 3);
    assert_eq!(WledColorFormat::Rgbw.bytes_per_pixel(), 4);
}

#[test]
fn color_format_ddp_data_type() {
    assert_eq!(WledColorFormat::Rgb.ddp_data_type(), DDP_DTYPE_RGB8);
    assert_eq!(WledColorFormat::Rgbw.ddp_data_type(), DDP_DTYPE_RGBW8);
}

// ── WledProtocol Tests ─────────────────────────────────────────────────

#[test]
fn protocol_default_is_ddp() {
    assert_eq!(WledProtocol::default(), WledProtocol::Ddp);
}

#[test]
fn negotiated_target_fps_clamps_reported_value() {
    let info = test_wled_info(300, false, 75, true);
    assert_eq!(info.negotiated_target_fps(), 60);

    let info = test_wled_info(300, false, 12, true);
    assert_eq!(info.negotiated_target_fps(), 15);

    let info = test_wled_info(300, false, 32, true);
    assert_eq!(info.negotiated_target_fps(), 32);
}

#[test]
fn negotiated_target_fps_uses_unreported_heuristics() {
    assert_eq!(
        test_wled_info(250, false, 0, true).negotiated_target_fps(),
        40
    );
    assert_eq!(
        test_wled_info(500, false, 0, true).negotiated_target_fps(),
        30
    );
    assert_eq!(
        test_wled_info(500, false, 0, false).negotiated_target_fps(),
        40
    );
    assert_eq!(
        test_wled_info(900, false, 0, true).negotiated_target_fps(),
        25
    );
    assert_eq!(
        test_wled_info(900, false, 0, false).negotiated_target_fps(),
        35
    );
}

// ── WledBackend Lifecycle Tests ────────────────────────────────────────

#[tokio::test]
async fn backend_info() {
    let backend = WledBackend::new(vec![]);
    let info = DeviceBackend::info(&backend);

    assert_eq!(info.id, "wled");
    assert!(!info.name.is_empty());
    assert!(!info.description.is_empty());
}

#[tokio::test]
async fn backend_discover_no_ips() {
    let mut backend = WledBackend::new(vec![]);
    let discovered = backend.discover().await.expect("discover should succeed");
    assert!(discovered.is_empty(), "no known IPs means no discoveries");
}

#[tokio::test]
async fn backend_discover_unreachable_ip_graceful() {
    // Use an RFC 5737 documentation address that won't be routable
    let ip: IpAddr = "192.0.2.1".parse().expect("valid IP");
    let mut backend = WledBackend::new(vec![ip]);

    let discovered = backend.discover().await.expect("discover should succeed");
    assert!(
        discovered.is_empty(),
        "unreachable IP should not produce discoveries"
    );
}

#[tokio::test]
async fn backend_connect_without_discover_fails() {
    let mut backend = WledBackend::new(vec![]);
    let unknown_id = hypercolor_types::device::DeviceId::new();

    let result = backend.connect(&unknown_id).await;
    assert!(
        result.is_err(),
        "connecting without prior discovery should fail"
    );
}

#[tokio::test]
async fn backend_disconnect_unknown_fails() {
    let mut backend = WledBackend::new(vec![]);
    let unknown_id = hypercolor_types::device::DeviceId::new();

    let result = backend.disconnect(&unknown_id).await;
    assert!(result.is_err(), "disconnecting unknown device should fail");
}

#[tokio::test]
async fn backend_disconnect_unused_device_sends_no_packets() {
    let _guard = ddp_test_port_guard().await;
    let receiver = UdpSocket::bind("127.0.0.1:4048")
        .await
        .expect("bind loopback DDP receiver");
    let mut backend = WledBackend::new(vec![]);
    backend.set_realtime_http_enabled(false);

    let device_id = DeviceId::new();
    backend.remember_device(
        device_id,
        "127.0.0.1".parse().expect("valid loopback IP"),
        test_wled_info(4, false, 30, true),
    );
    backend
        .connect(&device_id)
        .await
        .expect("connect should succeed");

    backend
        .disconnect(&device_id)
        .await
        .expect("disconnect should succeed");

    let mut packet = [0_u8; 64];
    assert!(
        timeout(Duration::from_millis(200), receiver.recv_from(&mut packet))
            .await
            .is_err(),
        "unused connect/disconnect should not send any UDP packets"
    );
}

#[tokio::test]
async fn backend_disconnect_sends_final_black_frame_after_output() {
    let _guard = ddp_test_port_guard().await;
    let receiver = UdpSocket::bind("127.0.0.1:4048")
        .await
        .expect("bind loopback DDP receiver");
    let mut backend = WledBackend::new(vec![]);
    backend.set_realtime_http_enabled(false);

    let device_id = DeviceId::new();
    backend.remember_device(
        device_id,
        "127.0.0.1".parse().expect("valid loopback IP"),
        test_wled_info(4, false, 30, true),
    );
    backend
        .connect(&device_id)
        .await
        .expect("connect should succeed");

    let colors = [[1, 2, 3], [4, 5, 6], [7, 8, 9], [10, 11, 12]];
    backend
        .write_colors(&device_id, &colors)
        .await
        .expect("write should succeed");

    let mut first_packet = [0_u8; 64];
    let (first_len, _) = timeout(
        Duration::from_millis(200),
        receiver.recv_from(&mut first_packet),
    )
    .await
    .expect("expected initial DDP frame")
    .expect("recv initial DDP frame");
    assert_eq!(first_len, DDP_HEADER_SIZE + 12);

    backend
        .disconnect(&device_id)
        .await
        .expect("disconnect should succeed");

    let mut packet = [0_u8; 64];
    let (len, _) = timeout(Duration::from_millis(200), receiver.recv_from(&mut packet))
        .await
        .expect("expected final black DDP frame")
        .expect("recv final black DDP frame");
    assert_eq!(len, DDP_HEADER_SIZE + 12);
    assert_eq!(&packet[10..22], &[0; 12]);
}

#[tokio::test]
async fn backend_write_to_disconnected_fails() {
    let mut backend = WledBackend::new(vec![]);
    let unknown_id = hypercolor_types::device::DeviceId::new();

    let colors = vec![[0xFF, 0x00, 0x00]; 30];
    let result = backend.write_colors(&unknown_id, &colors).await;
    assert!(
        result.is_err(),
        "writing to disconnected device should fail"
    );
}

#[tokio::test]
async fn backend_connect_reuses_shared_socket_and_allocates_e131_universes() {
    let mut backend = WledBackend::new(vec![]);
    backend.set_realtime_http_enabled(false);
    backend.set_protocol(WledProtocol::E131);

    let device_a = DeviceId::new();
    let device_b = DeviceId::new();

    backend.remember_device(
        device_a,
        "127.0.0.2".parse().expect("valid loopback IP"),
        test_wled_info(300, false, 30, true),
    );
    backend.remember_device(
        device_b,
        "127.0.0.3".parse().expect("valid loopback IP"),
        test_wled_info(300, false, 30, true),
    );

    backend.connect(&device_a).await.expect("connect device A");
    backend.connect(&device_b).await.expect("connect device B");

    let shared_addr = backend
        .shared_socket_local_addr()
        .expect("shared socket should be initialized");
    assert_eq!(
        backend.connected_socket_local_addr(&device_a),
        Some(shared_addr)
    );
    assert_eq!(
        backend.connected_socket_local_addr(&device_b),
        Some(shared_addr)
    );
    assert_eq!(backend.connected_e131_start_universe(&device_a), Some(1));
    assert_eq!(backend.connected_e131_start_universe(&device_b), Some(3));
    assert_eq!(backend.target_fps(&device_a), Some(30));
}

#[tokio::test]
async fn backend_write_colors_pads_dedups_and_keeps_alive() {
    let _guard = ddp_test_port_guard().await;
    let receiver = UdpSocket::bind("127.0.0.1:4048")
        .await
        .expect("bind loopback DDP receiver");
    let mut backend = WledBackend::new(vec![]);
    backend.set_realtime_http_enabled(false);

    let device_id = DeviceId::new();
    backend.remember_device(
        device_id,
        "127.0.0.1".parse().expect("valid loopback IP"),
        test_wled_info(4, false, 30, true),
    );
    backend
        .connect(&device_id)
        .await
        .expect("connect should succeed");

    let colors = [[1, 2, 3], [4, 5, 6]];
    backend
        .write_colors(&device_id, &colors)
        .await
        .expect("first write should succeed");

    let mut packet = [0_u8; 64];
    let (len, _) = timeout(Duration::from_millis(200), receiver.recv_from(&mut packet))
        .await
        .expect("expected initial DDP frame")
        .expect("recv initial DDP frame");
    assert_eq!(len, DDP_HEADER_SIZE + 12);
    assert_eq!(&packet[10..22], &[1, 2, 3, 4, 5, 6, 0, 0, 0, 0, 0, 0]);

    backend
        .write_colors(&device_id, &colors)
        .await
        .expect("duplicate write should succeed");
    let mut dedup_packet = [0_u8; 64];
    assert!(
        timeout(
            Duration::from_millis(150),
            receiver.recv_from(&mut dedup_packet)
        )
        .await
        .is_err(),
        "deduplicated frame should not send another UDP packet"
    );

    tokio::time::sleep(Duration::from_millis(2_100)).await;
    backend
        .write_colors(&device_id, &colors)
        .await
        .expect("keepalive write should succeed");
    let (len, _) = timeout(Duration::from_millis(200), receiver.recv_from(&mut packet))
        .await
        .expect("expected keepalive DDP frame")
        .expect("recv keepalive DDP frame");
    assert_eq!(len, DDP_HEADER_SIZE + 12);
}

#[tokio::test]
async fn backend_write_colors_allows_duplicate_frames_when_dedup_is_disabled() {
    let _guard = ddp_test_port_guard().await;
    let receiver = UdpSocket::bind("127.0.0.1:4048")
        .await
        .expect("bind loopback DDP receiver");
    let mut backend = WledBackend::new(vec![]);
    backend.set_realtime_http_enabled(false);
    backend.set_dedup_threshold(0);

    let device_id = DeviceId::new();
    backend.remember_device(
        device_id,
        "127.0.0.1".parse().expect("valid loopback IP"),
        test_wled_info(2, false, 30, true),
    );
    backend
        .connect(&device_id)
        .await
        .expect("connect should succeed");

    let colors = [[7, 8, 9], [1, 2, 3]];
    backend
        .write_colors(&device_id, &colors)
        .await
        .expect("first write should succeed");
    backend
        .write_colors(&device_id, &colors)
        .await
        .expect("duplicate write should succeed when dedup is disabled");

    let mut packet = [0_u8; 64];
    let (first_len, _) = timeout(Duration::from_millis(200), receiver.recv_from(&mut packet))
        .await
        .expect("expected first DDP frame")
        .expect("recv first DDP frame");
    let (second_len, _) = timeout(Duration::from_millis(200), receiver.recv_from(&mut packet))
        .await
        .expect("expected second DDP frame")
        .expect("recv second DDP frame");

    assert_eq!(first_len, DDP_HEADER_SIZE + 6);
    assert_eq!(second_len, DDP_HEADER_SIZE + 6);
}

#[tokio::test]
async fn backend_write_colors_preserves_chroma_on_rgbw_devices() {
    let _guard = ddp_test_port_guard().await;
    let receiver = UdpSocket::bind("127.0.0.1:4048")
        .await
        .expect("bind loopback DDP receiver");
    let mut backend = WledBackend::new(vec![]);
    backend.set_realtime_http_enabled(false);

    let device_id = DeviceId::new();
    backend.remember_device(
        device_id,
        "127.0.0.1".parse().expect("valid loopback IP"),
        test_wled_info(2, true, 30, true),
    );
    backend
        .connect(&device_id)
        .await
        .expect("connect should succeed");

    let colors = [[255, 0, 255], [240, 244, 250]];
    backend
        .write_colors(&device_id, &colors)
        .await
        .expect("RGBW write should succeed");

    let mut packet = [0_u8; 64];
    let (len, _) = timeout(Duration::from_millis(200), receiver.recv_from(&mut packet))
        .await
        .expect("expected DDP frame")
        .expect("recv DDP frame");

    assert_eq!(len, DDP_HEADER_SIZE + 6);
    assert_eq!(
        &packet[10..16],
        &[255, 0, 255, 240, 244, 250],
        "DDP should preserve RGB color data for RGBW WLED devices"
    );
}

#[tokio::test]
async fn backend_write_colors_truncates_oversized_frames() {
    let _guard = ddp_test_port_guard().await;
    let receiver = UdpSocket::bind("127.0.0.1:4048")
        .await
        .expect("bind loopback DDP receiver");
    let mut backend = WledBackend::new(vec![]);
    backend.set_realtime_http_enabled(false);

    let device_id = DeviceId::new();
    backend.remember_device(
        device_id,
        "127.0.0.1".parse().expect("valid loopback IP"),
        test_wled_info(2, false, 30, true),
    );
    backend
        .connect(&device_id)
        .await
        .expect("connect should succeed");

    backend
        .write_colors(&device_id, &[[9, 8, 7], [6, 5, 4], [3, 2, 1]])
        .await
        .expect("oversized write should succeed");

    let mut packet = [0_u8; 64];
    let (len, _) = timeout(Duration::from_millis(200), receiver.recv_from(&mut packet))
        .await
        .expect("expected truncated DDP frame")
        .expect("recv truncated DDP frame");
    assert_eq!(len, DDP_HEADER_SIZE + 6);
    assert_eq!(&packet[10..16], &[9, 8, 7, 6, 5, 4]);
}

// ── DdpPacket Edge Cases ───────────────────────────────────────────────

#[test]
fn ddp_packet_len_and_is_empty() {
    let packet = DdpPacket::new(&[0x00; 6], 0, true, 1, DDP_DTYPE_RGB8);
    assert_eq!(packet.len(), DDP_HEADER_SIZE + 6);
    assert!(!packet.is_empty());
}

#[test]
fn ddp_empty_payload() {
    let mut seq = DdpSequence::default();
    let packets = build_ddp_frame(&[], DDP_DTYPE_RGB8, &mut seq);

    // Empty pixel data produces no packets — nothing to transmit.
    assert_eq!(packets.len(), 0, "empty frame produces no packets");
}

#[test]
fn ddp_rgbw_fragmentation() {
    let mut seq = DdpSequence::default();
    // 360 RGBW pixels = 1440 bytes = exactly 1 packet
    let pixel_data = vec![0x00; 1440];
    let packets = build_ddp_frame(&pixel_data, DDP_DTYPE_RGBW8, &mut seq);
    assert_eq!(packets.len(), 1, "360 RGBW pixels should fit in one packet");

    // 361 RGBW pixels = 1444 bytes = 2 packets
    let pixel_data = vec![0x00; 1444];
    let packets = build_ddp_frame(&pixel_data, DDP_DTYPE_RGBW8, &mut seq);
    assert_eq!(packets.len(), 2, "361 RGBW pixels should need 2 packets");
}

// ── WledDeviceInfo serialization ────────────────────────────────────────

#[test]
fn wled_device_info_serializable() {
    let info = WledDeviceInfo {
        firmware_version: "0.15.3".to_owned(),
        build_id: 2_312_050,
        mac: "aabbccddeeff".to_owned(),
        name: "Test Device".to_owned(),
        led_count: 300,
        rgbw: false,
        max_segments: 16,
        fps: 42,
        power_draw_ma: 1500,
        max_power_ma: 5000,
        free_heap: 120_000,
        uptime_secs: 86400,
        arch: "esp32".to_owned(),
        is_wifi: true,
        effect_count: 118,
        palette_count: 71,
    };

    let json = serde_json::to_string(&info).expect("should serialize");
    let restored: WledDeviceInfo = serde_json::from_str(&json).expect("should deserialize");

    assert_eq!(restored.name, "Test Device");
    assert_eq!(restored.led_count, 300);
    assert_eq!(restored.firmware_version, "0.15.3");
    assert!(restored.is_wifi);
}

static MDNS_TEST_LOGGER: MdnsTestLogger = MdnsTestLogger;
static MDNS_TEST_LOGGER_INIT: Once = Once::new();
static DDP_TEST_PORT_LOCK: OnceLock<AsyncMutex<()>> = OnceLock::new();
static MDNS_TEST_LOG_MESSAGES: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
static MDNS_TEST_LOG_LOCK: OnceLock<AsyncMutex<()>> = OnceLock::new();
const TEST_LOCK_ACQUIRE_TIMEOUT: Duration = Duration::from_secs(90);
const TEST_LOCK_POLL_INTERVAL: Duration = Duration::from_millis(25);
const TEST_LOCK_STALE_AFTER: Duration = Duration::from_secs(300);

struct MdnsTestLogger;

impl log::Log for MdnsTestLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Error && metadata.target().starts_with("mdns_sd")
    }

    fn log(&self, record: &log::Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }

        mdns_test_log_messages()
            .lock()
            .expect("mDNS test log store should not be poisoned")
            .push(format!("{} {}", record.target(), record.args()));
    }

    fn flush(&self) {}
}

fn mdns_test_log_messages() -> &'static Mutex<Vec<String>> {
    MDNS_TEST_LOG_MESSAGES.get_or_init(|| Mutex::new(Vec::new()))
}

fn init_mdns_test_logger() {
    MDNS_TEST_LOGGER_INIT.call_once(|| {
        log::set_logger(&MDNS_TEST_LOGGER).expect("mDNS test logger should install once");
        log::set_max_level(log::LevelFilter::Trace);
    });
}

fn ddp_test_port_lock() -> &'static AsyncMutex<()> {
    DDP_TEST_PORT_LOCK.get_or_init(|| AsyncMutex::new(()))
}

fn mdns_test_log_lock() -> &'static AsyncMutex<()> {
    MDNS_TEST_LOG_LOCK.get_or_init(|| AsyncMutex::new(()))
}

struct CrossProcessTestLock {
    path: PathBuf,
}

impl Drop for CrossProcessTestLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

struct TestLockGuard {
    _local_guard: tokio::sync::MutexGuard<'static, ()>,
    _cross_process_guard: CrossProcessTestLock,
}

async fn ddp_test_port_guard() -> TestLockGuard {
    TestLockGuard {
        _local_guard: ddp_test_port_lock().lock().await,
        _cross_process_guard: acquire_cross_process_test_lock("hypercolor-wled-ddp-4048").await,
    }
}

async fn mdns_test_guard() -> TestLockGuard {
    TestLockGuard {
        _local_guard: mdns_test_log_lock().lock().await,
        _cross_process_guard: acquire_cross_process_test_lock("hypercolor-wled-mdns").await,
    }
}

async fn acquire_cross_process_test_lock(name: &str) -> CrossProcessTestLock {
    let path = std::env::temp_dir().join(format!("{name}.lock"));
    let started_at = Instant::now();

    loop {
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                let _ = writeln!(file, "pid={}", std::process::id());
                return CrossProcessTestLock { path };
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                clear_stale_test_lock(&path);
                assert!(
                    started_at.elapsed() < TEST_LOCK_ACQUIRE_TIMEOUT,
                    "timed out waiting for test lock at {}",
                    path.display()
                );
                tokio::time::sleep(TEST_LOCK_POLL_INTERVAL).await;
            }
            Err(error) => panic!("failed to acquire test lock at {}: {error}", path.display()),
        }
    }
}

fn clear_stale_test_lock(path: &Path) {
    let Ok(metadata) = std::fs::metadata(path) else {
        return;
    };
    let Ok(modified_at) = metadata.modified() else {
        return;
    };
    if modified_at.elapsed().unwrap_or_default() <= TEST_LOCK_STALE_AFTER {
        return;
    }
    let _ = std::fs::remove_file(path);
}

#[tokio::test(flavor = "current_thread")]
async fn scanner_shutdown_drains_mdns_status_receiver() {
    let _guard = mdns_test_guard().await;
    init_mdns_test_logger();
    mdns_test_log_messages()
        .lock()
        .expect("mDNS test log store should not be poisoned")
        .clear();

    let mut scanner = WledScanner::with_known_ips(Vec::new(), true, Duration::from_millis(50));
    scanner
        .scan()
        .await
        .expect("WLED scan should complete without mDNS shutdown errors");

    let messages = mdns_test_log_messages()
        .lock()
        .expect("mDNS test log store should not be poisoned")
        .clone();
    assert!(
        messages
            .iter()
            .all(|message| !message.contains("failed to send response of shutdown")),
        "unexpected mdns shutdown error logs: {messages:?}"
    );
}

#[tokio::test]
async fn scanner_skips_stale_known_ip_without_enrichment() {
    let ip: IpAddr = "192.0.2.1".parse().expect("valid documentation IP");
    let mut scanner = WledScanner::with_known_ips(vec![ip], false, Duration::from_millis(50));

    let discovered = scanner.scan().await.expect("scan should complete");
    assert!(
        discovered.is_empty(),
        "known IPs that cannot be enriched should not surface as placeholder devices"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn scanner_surfaces_mdns_only_wled_as_deferred_placeholder() {
    let _guard = mdns_test_guard().await;
    let mdns = ServiceDaemon::new().expect("mDNS daemon should start");
    let service_info = ServiceInfo::new(
        "_wled._tcp.local.",
        "hypercolor-placeholder-test",
        "wled-placeholder.local.",
        "",
        80,
        None,
    )
    .expect("service info should be valid")
    .enable_addr_auto();
    let service_fullname = service_info.get_fullname().to_string();

    mdns.register(service_info)
        .expect("placeholder service should register");
    tokio::time::sleep(Duration::from_millis(100)).await;

    let mut scanner = WledScanner::with_known_ips(Vec::new(), true, Duration::from_secs(2));
    let discovered = scanner.scan().await.expect("scan should complete");

    let placeholder = discovered
        .iter()
        .find(|device| {
            device
                .metadata
                .get("hostname")
                .is_some_and(|hostname| hostname.eq_ignore_ascii_case("wled-placeholder.local"))
        })
        .expect("scanner should surface the mDNS-only placeholder device");

    assert_eq!(
        placeholder.connect_behavior,
        DiscoveryConnectBehavior::Deferred
    );
    assert_eq!(placeholder.info.capabilities.led_count, 0);
    assert!(
        placeholder.metadata.contains_key("ip"),
        "placeholder discovery should still include the resolved address"
    );

    let unregister = mdns
        .unregister(&service_fullname)
        .expect("service should unregister");
    let _ = timeout(Duration::from_secs(1), unregister.recv_async()).await;

    let shutdown = mdns.shutdown().expect("mDNS daemon should shut down");
    let _ = timeout(Duration::from_secs(1), shutdown.recv_async()).await;
}

fn test_wled_info(led_count: u16, rgbw: bool, fps: u8, is_wifi: bool) -> WledDeviceInfo {
    WledDeviceInfo {
        firmware_version: "0.15.3".to_owned(),
        build_id: 2_312_050,
        mac: "aabbccddeeff".to_owned(),
        name: "Test Device".to_owned(),
        led_count,
        rgbw,
        max_segments: 16,
        fps,
        power_draw_ma: 1500,
        max_power_ma: 5000,
        free_heap: 120_000,
        uptime_secs: 86_400,
        arch: "esp32".to_owned(),
        is_wifi,
        effect_count: 118,
        palette_count: 71,
    }
}
