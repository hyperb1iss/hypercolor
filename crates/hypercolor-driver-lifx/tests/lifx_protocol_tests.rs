//! Golden bytes for every message the driver sends and parsing of every
//! State message it reads (Spec 88 §2 to §6).

mod support;

use hypercolor_driver_lifx::color::Hsbk;
use hypercolor_driver_lifx::protocol::{
    ApplyMode, FirmwareVersion, FrameAddress, Message, ProtocolError, Serial, State, decode_packet,
    decode_state, encode_to_vec,
};
use support::hex;

const TARGET: Serial = Serial([0xd0, 0x73, 0xd5, 0x00, 0x13, 0x37]);

fn unicast(sequence: u8) -> FrameAddress {
    FrameAddress {
        source: 2,
        target: TARGET,
        ack_required: false,
        res_required: false,
        sequence,
    }
}

fn encode(address: &FrameAddress, message: &Message<'_>) -> Vec<u8> {
    encode_to_vec(address, message).expect("message encodes")
}

/// Header of a unicast message from source 2 to `d073d5001337`.
fn unicast_header(size: &str, address_flags: &str, sequence: &str, message_type: &str) -> String {
    format!(
        "{size} 0014 02000000 d073d50013370000 000000000000 {address_flags} {sequence} 0000000000000000 {message_type} 0000"
    )
}

#[test]
fn set_color_matches_the_documented_example() {
    let address = FrameAddress {
        source: 2,
        target: TARGET,
        ack_required: true,
        res_required: false,
        sequence: 1,
    };
    let bytes = encode(
        &address,
        &Message::SetColor {
            color: Hsbk::new(21_845, 65_535, 65_535, 3_500),
            duration_ms: 0,
        },
    );
    assert_eq!(
        bytes,
        hex(
            "3100001402000000d073d500133700000000000000000201000000000000000066000000005555ffffffffac0d00000000"
        )
    );
}

#[test]
fn get_service_is_tagged_with_a_zero_target() {
    let address = FrameAddress {
        source: 2,
        target: Serial::BROADCAST,
        ack_required: false,
        res_required: false,
        sequence: 0x10,
    };
    assert_eq!(
        encode(&address, &Message::GetService),
        hex("2400 0034 02000000 0000000000000000 000000000000 00 10 0000000000000000 0200 0000")
    );
}

#[test]
fn empty_get_messages_encode_header_only() {
    for (message, message_type) in [
        (Message::GetHostFirmware, "0e00"),
        (Message::GetLabel, "1700"),
        (Message::GetVersion, "2000"),
        (Message::GetExtendedColorZones, "ff01"),
        (Message::GetDeviceChain, "bd02"),
    ] {
        assert_eq!(
            encode(&unicast(0x10), &message),
            hex(&unicast_header("2400", "00", "10", message_type)),
            "{message:?}"
        );
    }
}

#[test]
fn get_color_zones_carries_the_zone_range() {
    assert_eq!(
        encode(
            &unicast(0x10),
            &Message::GetColorZones {
                start_index: 0,
                end_index: 255,
            }
        ),
        hex(&format!(
            "{} 00ff",
            unicast_header("2600", "00", "10", "f601")
        ))
    );
}

#[test]
fn set_light_power_with_ack() {
    let address = FrameAddress {
        ack_required: true,
        ..unicast(0x10)
    };
    assert_eq!(
        encode(
            &address,
            &Message::SetLightPower {
                level: 65_535,
                duration_ms: 500,
            }
        ),
        hex(&format!(
            "{} ffff f4010000",
            unicast_header("2a00", "02", "10", "7500")
        ))
    );
}

#[test]
fn res_and_ack_flags_share_one_byte() {
    let address = FrameAddress {
        ack_required: true,
        res_required: true,
        ..unicast(7)
    };
    let bytes = encode(&address, &Message::GetVersion);
    assert_eq!(bytes[22], 0x03);
    assert_eq!(bytes[23], 7);
}

#[test]
fn set_color_zones_layout() {
    assert_eq!(
        encode(
            &unicast(0x10),
            &Message::SetColorZones {
                start_index: 2,
                end_index: 5,
                color: Hsbk::new(0x5555, 0xffff, 0x8000, 3_500),
                duration_ms: 250,
                apply: ApplyMode::Apply,
            }
        ),
        hex(&format!(
            "{} 02 05 5555 ffff 0080 ac0d fa000000 01",
            unicast_header("3300", "00", "10", "f501")
        ))
    );
}

#[test]
fn set_extended_color_zones_pads_to_82_colors() {
    let colors = [
        Hsbk::new(0x0102, 0x0304, 0x0506, 0x0708),
        Hsbk::new(0x1112, 0x1314, 0x1516, 0x1718),
    ];
    let bytes = encode(
        &unicast(0x10),
        &Message::SetExtendedColorZones {
            duration_ms: 0,
            apply: ApplyMode::NoApply,
            zone_index: 82,
            colors: &colors,
        },
    );
    let mut expected = hex(&format!(
        "{} 00000000 00 5200 02 0201 0403 0605 0807 1211 1413 1615 1817",
        unicast_header("bc02", "00", "10", "fe01")
    ));
    expected.resize(700, 0);
    assert_eq!(bytes, expected);
}

#[test]
fn set64_pads_to_64_colors() {
    let colors = [Hsbk::new(0xaaaa, 0xbbbb, 0xcccc, 0x0dac); 3];
    let bytes = encode(
        &unicast(0x10),
        &Message::Set64 {
            tile_index: 1,
            length: 1,
            fb_index: 0,
            x: 0,
            y: 4,
            width: 16,
            duration_ms: 100,
            colors: &colors,
        },
    );
    let mut expected = hex(&format!(
        "{} 01 01 00 00 04 10 64000000 aaaabbbbccccac0d aaaabbbbccccac0d aaaabbbbccccac0d",
        unicast_header("2e02", "00", "10", "cb02")
    ));
    expected.resize(558, 0);
    assert_eq!(bytes, expected);
}

#[test]
fn copy_frame_buffer_layout() {
    assert_eq!(
        encode(
            &unicast(0x10),
            &Message::CopyFrameBuffer {
                tile_index: 0,
                length: 1,
                src_fb_index: 1,
                dst_fb_index: 0,
                src_x: 0,
                src_y: 0,
                dst_x: 0,
                dst_y: 0,
                width: 16,
                height: 8,
                duration_ms: 100,
            }
        ),
        hex(&format!(
            "{} 00 01 01 00 00 00 00 00 10 08 64000000 00",
            unicast_header("3300", "00", "10", "cc02")
        ))
    );
}

#[test]
fn oversized_color_slices_are_rejected() {
    let colors = vec![Hsbk::default(); 83];
    let error = encode_to_vec(
        &unicast(0),
        &Message::SetExtendedColorZones {
            duration_ms: 0,
            apply: ApplyMode::Apply,
            zone_index: 0,
            colors: &colors,
        },
    )
    .expect_err("83 colors do not fit");
    assert_eq!(
        error,
        ProtocolError::TooManyColors {
            max: 82,
            actual: 83
        }
    );

    let error = encode_to_vec(
        &unicast(0),
        &Message::Set64 {
            tile_index: 0,
            length: 1,
            fb_index: 0,
            x: 0,
            y: 0,
            width: 8,
            duration_ms: 0,
            colors: &colors[..65],
        },
    )
    .expect_err("65 colors do not fit");
    assert_eq!(
        error,
        ProtocolError::TooManyColors {
            max: 64,
            actual: 65
        }
    );
}

#[test]
fn documented_get_power_header_decodes_despite_a_reserved_bit() {
    let bytes = hex("2400001400034746d073d500133700000000000000000701000000000000000014000000");
    let packet = decode_packet(&bytes).expect("documented packet decodes");
    assert_eq!(packet.header.size, 36);
    assert!(!packet.header.tagged);
    assert_eq!(packet.header.source, 1_179_058_944);
    assert_eq!(packet.header.target, TARGET);
    assert!(packet.header.res_required);
    assert!(packet.header.ack_required);
    assert_eq!(packet.header.sequence, 1);
    assert_eq!(packet.header.message_type, 20);
    assert!(packet.payload.is_empty());
}

#[test]
fn documented_state_label_reply_parses() {
    let bytes = hex(
        "4400001487454e9ed073d5309d9e00004c49465856320101d078582cef7d010019000000637570626f617264000000000000000000000000000000000000000000000000",
    );
    let packet = decode_packet(&bytes).expect("documented reply decodes");
    assert_eq!(packet.header.source, 2_655_929_735);
    assert_eq!(packet.header.sequence, 1);
    assert_eq!(
        packet.header.target,
        Serial([0xd0, 0x73, 0xd5, 0x30, 0x9d, 0x9e])
    );
    assert_eq!(
        decode_state(&packet).expect("label decodes"),
        State::Label("cupboard".to_owned())
    );
}

/// Reply header with junk in every reserved field, like real devices send.
fn reply(message_type: u16, payload: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(
        &u16::try_from(36 + payload.len())
            .expect("fits")
            .to_le_bytes(),
    );
    bytes.extend_from_slice(&hex("0014 2a000000 d073d50013370000"));
    bytes.extend_from_slice(b"LIFXV2");
    bytes.extend_from_slice(&[0x04, 0x09]);
    bytes.extend_from_slice(&[0xee; 8]);
    bytes.extend_from_slice(&message_type.to_le_bytes());
    bytes.extend_from_slice(&[0x55, 0x55]);
    bytes.extend_from_slice(payload);
    bytes
}

fn state(message_type: u16, payload: &[u8]) -> State {
    let bytes = reply(message_type, payload);
    let packet = decode_packet(&bytes).expect("reply decodes");
    assert_eq!(packet.header.source, 42);
    assert_eq!(packet.header.sequence, 9);
    decode_state(&packet).expect("state decodes")
}

#[test]
fn state_service_parses() {
    let State::Service(service) = state(3, &hex("01 7cdd0000")) else {
        panic!("expected StateService");
    };
    assert_eq!(service.service, 1);
    assert_eq!(service.port, 56_700);
}

#[test]
fn state_host_firmware_parses() {
    let payload = hex("0011223344556677 ffffffffffffffff 4600 0300");
    let State::HostFirmware(firmware) = state(15, &payload) else {
        panic!("expected StateHostFirmware");
    };
    assert_eq!(firmware.build, 0x7766_5544_3322_1100);
    assert_eq!(firmware.version(), FirmwareVersion::new(3, 70));
    assert!(firmware.version() > FirmwareVersion::new(2, 80));
}

#[test]
fn state_version_parses() {
    let State::Version(version) = state(33, &hex("01000000 37000000 deadbeef")) else {
        panic!("expected StateVersion");
    };
    assert_eq!(version.vendor, 1);
    assert_eq!(version.product, 55);
}

#[test]
fn state_label_without_nul_uses_all_32_bytes() {
    let label = [b'x'; 32];
    assert_eq!(state(25, &label), State::Label("x".repeat(32)));
}

#[test]
fn acknowledgement_and_unhandled_parse() {
    assert_eq!(state(45, &[]), State::Acknowledgement);
    assert_eq!(state(223, &hex("ff01")), State::Unhandled(511));
}

#[test]
fn state_zone_parses() {
    let State::Zone(zone) = state(503, &hex("10 03 0100 0200 0300 0400")) else {
        panic!("expected StateZone");
    };
    assert_eq!(zone.zones_count, 16);
    assert_eq!(zone.zone_index, 3);
    assert_eq!(zone.color, Hsbk::new(1, 2, 3, 4));
}

#[test]
fn state_multi_zone_parses() {
    let mut payload = hex("10 08");
    for zone in 0..8_u16 {
        payload.extend_from_slice(&Hsbk::new(zone, 0, 0, 3_500).to_le_bytes());
    }
    let State::MultiZone(state) = state(506, &payload) else {
        panic!("expected StateMultiZone");
    };
    assert_eq!(state.zones_count, 16);
    assert_eq!(state.zone_index, 8);
    assert_eq!(state.colors[7], Hsbk::new(7, 0, 0, 3_500));
}

#[test]
fn state_extended_color_zones_keeps_only_meaningful_colors() {
    let mut payload = hex("7800 5200 03");
    for zone in 0..82_u16 {
        payload.extend_from_slice(&Hsbk::new(zone, 1, 1, 1).to_le_bytes());
    }
    let State::ExtendedColorZones(state) = state(512, &payload) else {
        panic!("expected StateExtendedColorZones");
    };
    assert_eq!(state.zones_count, 120);
    assert_eq!(state.zone_index, 82);
    assert_eq!(state.colors.len(), 3);
    assert_eq!(state.colors[2].hue, 2);
}

fn tile_record(width: u8, height: u8, user: (f32, f32), accel: (i16, i16, i16)) -> Vec<u8> {
    let mut record = vec![0_u8; 55];
    record[0..2].copy_from_slice(&accel.0.to_le_bytes());
    record[2..4].copy_from_slice(&accel.1.to_le_bytes());
    record[4..6].copy_from_slice(&accel.2.to_le_bytes());
    record[6..8].copy_from_slice(&[0x99, 0x99]);
    record[8..12].copy_from_slice(&user.0.to_le_bytes());
    record[12..16].copy_from_slice(&user.1.to_le_bytes());
    record[16] = width;
    record[17] = height;
    record[18] = 2;
    record[19..23].copy_from_slice(&1_u32.to_le_bytes());
    record[23..27].copy_from_slice(&55_u32.to_le_bytes());
    record[31..39].copy_from_slice(&0x0102_0304_0506_0708_u64.to_le_bytes());
    record[47..49].copy_from_slice(&50_u16.to_le_bytes());
    record[49..51].copy_from_slice(&3_u16.to_le_bytes());
    record
}

#[test]
fn state_device_chain_parses_tile_records() {
    let mut payload = vec![0];
    payload.extend(tile_record(8, 8, (0.0, 0.0), (0, -100, 0)));
    payload.extend(tile_record(8, 8, (1.0, 0.5), (100, 0, 0)));
    payload.resize(1 + 16 * 55, 0);
    payload.push(2);

    let State::DeviceChain(chain) = state(702, &payload) else {
        panic!("expected StateDeviceChain");
    };
    assert_eq!(chain.start_index, 0);
    assert_eq!(chain.tiles.len(), 2);
    let tile = chain.tiles[1];
    assert_eq!((tile.width, tile.height), (8, 8));
    assert_eq!((tile.user_x, tile.user_y), (1.0, 0.5));
    assert_eq!(
        (tile.accel_meas_x, tile.accel_meas_y, tile.accel_meas_z),
        (100, 0, 0)
    );
    assert_eq!(tile.supported_frame_buffers, 2);
    assert_eq!(tile.device_version_vendor, 1);
    assert_eq!(tile.device_version_product, 55);
    assert_eq!(tile.firmware_build, 0x0102_0304_0506_0708);
    assert_eq!(tile.firmware_version, FirmwareVersion::new(3, 50));
}

#[test]
fn state_device_chain_rejects_empty_tiles() {
    let mut payload = vec![0];
    payload.extend(tile_record(0, 8, (0.0, 0.0), (0, 0, 0)));
    payload.resize(1 + 16 * 55, 0);
    payload.push(1);
    let bytes = reply(702, &payload);
    let packet = decode_packet(&bytes).expect("reply decodes");
    assert_eq!(
        decode_state(&packet),
        Err(ProtocolError::EmptyTile {
            index: 0,
            width: 0,
            height: 8,
        })
    );
}

#[test]
fn malformed_packets_are_rejected() {
    assert!(matches!(
        decode_packet(&[0; 20]),
        Err(ProtocolError::Truncated { .. })
    ));

    let mut bytes = reply(45, &[]);
    bytes[0] = 40;
    assert!(matches!(
        decode_packet(&bytes),
        Err(ProtocolError::SizeMismatch { declared: 40, .. })
    ));

    let mut bytes = reply(45, &[]);
    bytes[2] = 0x01;
    bytes[3] = 0x10;
    assert_eq!(
        decode_packet(&bytes),
        Err(ProtocolError::UnsupportedProtocol(1))
    );

    let bytes = reply(33, &[1, 0, 0]);
    let packet = decode_packet(&bytes).expect("header decodes");
    assert!(matches!(
        decode_state(&packet),
        Err(ProtocolError::Truncated { expected: 12, .. })
    ));
}

#[test]
fn serial_round_trips_through_text() {
    let serial: Serial = "d0:73:d5:00:13:37".parse().expect("serial parses");
    assert_eq!(serial, TARGET);
    assert_eq!(serial.to_string(), "d073d5001337");
    assert_eq!(
        serial.to_target(),
        [0xd0, 0x73, 0xd5, 0x00, 0x13, 0x37, 0, 0]
    );
    assert!("d073d50013".parse::<Serial>().is_err());
    assert!("zz73d5001337".parse::<Serial>().is_err());
}
