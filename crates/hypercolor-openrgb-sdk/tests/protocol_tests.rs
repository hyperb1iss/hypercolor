use hypercolor_openrgb_sdk::packet::{
    PacketId, client_name_payload, request_controller_data_payload,
    request_protocol_version_payload, update_leds_payload, update_mode_payload,
    update_zone_leds_payload, validate_protocol_version,
};
use hypercolor_openrgb_sdk::{
    CLIENT_MAX_PROTOCOL_VERSION, ColorMode, ControllerMode, DeviceType, HEADER_LEN, ModeFlag,
    ModeFlagPolicy, OpenRgbError, Packet, PacketDecoder, PacketHeader, PacketId as PublicPacketId,
    RgbColor, parse_controller_data,
};

#[test]
fn packet_header_round_trips_little_endian() {
    let header = PacketHeader {
        device_index: 7,
        packet_id: PacketId::UpdateLeds,
        size: 42,
    };

    let bytes = header.encode();
    assert_eq!(&bytes[0..4], b"ORGB");
    assert_eq!(&bytes[4..8], &7_u32.to_le_bytes());
    assert_eq!(&bytes[8..12], &1050_u32.to_le_bytes());
    assert_eq!(&bytes[12..16], &42_u32.to_le_bytes());
    assert_eq!(
        PacketHeader::decode(&bytes).expect("header should decode"),
        header
    );
}

#[test]
fn packet_decode_rejects_bad_magic() {
    let mut bytes = PacketHeader {
        device_index: 0,
        packet_id: PacketId::RequestControllerCount,
        size: 0,
    }
    .encode();
    bytes[0] = b'X';

    assert_eq!(
        PacketHeader::decode(&bytes),
        Err(OpenRgbError::InvalidMagic(*b"XRGB"))
    );
}

#[test]
fn stream_decoder_waits_for_fragmented_packet() {
    let packet = Packet {
        header: PacketHeader {
            device_index: 3,
            packet_id: PacketId::SetClientName,
            size: 4,
        },
        payload: b"abc\0".to_vec(),
    }
    .encode();
    let split_at = HEADER_LEN + 2;

    let mut decoder = PacketDecoder::new();
    decoder.push(&packet[..split_at]);
    assert!(
        decoder
            .next_packet()
            .expect("partial packet should be valid")
            .is_none()
    );
    decoder.push(&packet[split_at..]);

    let decoded = decoder
        .next_packet()
        .expect("packet should decode")
        .expect("packet should be complete");
    assert_eq!(decoded.header.packet_id, PacketId::SetClientName);
    assert_eq!(decoded.payload, b"abc\0");
}

#[test]
fn truncated_packet_returns_needed_length() {
    let packet = Packet {
        header: PacketHeader {
            device_index: 0,
            packet_id: PacketId::RequestControllerData,
            size: 8,
        },
        payload: vec![1, 2, 3],
    }
    .encode();

    assert_eq!(
        Packet::decode(&packet),
        Err(OpenRgbError::Truncated {
            needed: HEADER_LEN + 8,
            remaining: HEADER_LEN + 3,
        })
    );
}

#[test]
fn forbidden_packets_are_not_encoded_for_client_use() {
    for packet_id in [PublicPacketId::SaveMode, PublicPacketId::ResizeZone] {
        assert_eq!(
            hypercolor_openrgb_sdk::encode_client_packet(0, packet_id, Vec::new()),
            Err(OpenRgbError::ForbiddenPacket(packet_id))
        );
    }
}

#[test]
fn client_payload_helpers_encode_documented_values() {
    assert_eq!(
        request_protocol_version_payload(CLIENT_MAX_PROTOCOL_VERSION),
        5_u32.to_le_bytes()
    );
    assert_eq!(client_name_payload("Hypercolor"), b"Hypercolor\0");
    assert_eq!(
        request_controller_data_payload(5),
        5_u32.to_le_bytes().to_vec()
    );
    assert_eq!(validate_protocol_version(0).is_err(), true);
    assert_eq!(validate_protocol_version(5), Ok(5));
}

#[test]
fn update_leds_payload_uses_rgbcolor_wire_order() {
    let payload = update_leds_payload(&[RgbColor::new(1, 2, 3), RgbColor::new(4, 5, 6)])
        .expect("payload should encode");

    assert_eq!(&payload[0..4], &14_u32.to_le_bytes());
    assert_eq!(&payload[4..6], &2_u16.to_le_bytes());
    assert_eq!(&payload[6..10], &[1, 2, 3, 0]);
    assert_eq!(&payload[10..14], &[4, 5, 6, 0]);
}

#[test]
fn update_zone_leds_payload_includes_zone_index() {
    let payload =
        update_zone_leds_payload(9, &[RgbColor::new(10, 20, 30)]).expect("payload should encode");

    assert_eq!(&payload[0..4], &14_u32.to_le_bytes());
    assert_eq!(&payload[4..8], &9_u32.to_le_bytes());
    assert_eq!(&payload[8..10], &1_u16.to_le_bytes());
    assert_eq!(&payload[10..14], &[10, 20, 30, 0]);
}

#[test]
fn mode_policy_uses_public_per_led_flag_and_rejects_persistent_mask() {
    let mode = sample_mode(ModeFlag::PerLedColor.mask(), ColorMode::PerLed);
    assert!(mode.is_realtime_writable(ModeFlagPolicy::default()));

    let policy = ModeFlagPolicy {
        persistent_mask: ModeFlag::PerLedColor.mask(),
        ..ModeFlagPolicy::default()
    };
    assert!(!mode.is_realtime_writable(policy));

    let random = sample_mode(ModeFlag::PerLedColor.mask(), ColorMode::Random);
    assert!(!random.is_realtime_writable(ModeFlagPolicy::default()));
}

#[test]
fn update_mode_payload_encodes_mode_block_without_savemode() {
    let mode = sample_mode(ModeFlag::PerLedColor.mask(), ColorMode::PerLed);
    let payload = update_mode_payload(2, &mode).expect("mode payload should encode");

    assert_eq!(&payload[4..8], &2_u32.to_le_bytes());
}

#[test]
fn parse_protocol_v5_controller_data() {
    let payload = controller_payload_v5();
    let controller = parse_controller_data(&payload, 5).expect("controller should parse");

    assert_eq!(controller.device_type, DeviceType::Keyboard);
    assert_eq!(controller.name, "Board");
    assert_eq!(controller.vendor, "Acme");
    assert_eq!(controller.active_mode, 0);
    assert_eq!(controller.modes.len(), 1);
    assert_eq!(controller.zones.len(), 1);
    assert_eq!(controller.zones[0].segments.len(), 1);
    assert_eq!(controller.leds.len(), 2);
    assert_eq!(controller.colors[1], RgbColor::new(4, 5, 6));
    assert_eq!(controller.led_alt_names, vec!["Alt 0".to_owned()]);
    assert_eq!(controller.flags, Some(0xA5));
}

#[test]
fn parser_rejects_lacking_length_body() {
    let mut payload = controller_payload_v5();
    payload.truncate(payload.len() - 3);

    assert!(parse_controller_data(&payload, 5).is_err());
}

#[test]
fn parser_rejects_losing_nul_byte() {
    let mut payload = controller_payload_v5();
    let name_content_offset = 10;
    payload[name_content_offset + "Board".len()] = b'!';

    assert_eq!(
        parse_controller_data(&payload, 5),
        Err(OpenRgbError::StringMissingNul)
    );
}

fn sample_mode(flags: u32, color_mode: ColorMode) -> ControllerMode {
    ControllerMode {
        name: "Direct".to_owned(),
        value: 0,
        flags,
        speed_min: 0,
        speed_max: 100,
        brightness_min: Some(0),
        brightness_max: Some(100),
        colors_min: 0,
        colors_max: 0,
        speed: 0,
        brightness: Some(100),
        direction: 0,
        color_mode,
        colors: Vec::new(),
    }
}

fn controller_payload_v5() -> Vec<u8> {
    let mut body = Vec::new();
    push_u32(&mut body, 0);
    push_i32(&mut body, 5);
    push_str(&mut body, "Board");
    push_str(&mut body, "Acme");
    push_str(&mut body, "Keyboard controller");
    push_str(&mut body, "1.2.3");
    push_str(&mut body, "SER123");
    push_str(&mut body, "hidraw0");
    push_u16(&mut body, 1);
    push_i32(&mut body, 0);
    push_mode(&mut body);
    push_u16(&mut body, 1);
    push_zone(&mut body);
    push_u16(&mut body, 2);
    push_str(&mut body, "LED 0");
    push_u32(&mut body, 0);
    push_str(&mut body, "LED 1");
    push_u32(&mut body, 1);
    push_u16(&mut body, 2);
    body.extend_from_slice(&RgbColor::new(1, 2, 3).to_wire_bytes());
    body.extend_from_slice(&RgbColor::new(4, 5, 6).to_wire_bytes());
    push_u16(&mut body, 1);
    push_str(&mut body, "Alt 0");
    push_u32(&mut body, 0xA5);
    let size = u32::try_from(body.len()).expect("fixture should fit u32");
    body[0..4].copy_from_slice(&size.to_le_bytes());
    body
}

fn push_mode(body: &mut Vec<u8>) {
    push_str(body, "Direct");
    push_i32(body, 0);
    push_u32(body, ModeFlag::PerLedColor.mask());
    push_u32(body, 0);
    push_u32(body, 100);
    push_u32(body, 0);
    push_u32(body, 100);
    push_u32(body, 0);
    push_u32(body, 0);
    push_u32(body, 0);
    push_u32(body, 100);
    push_u32(body, 0);
    push_u32(body, ColorMode::PerLed.raw());
    push_u16(body, 0);
}

fn push_zone(body: &mut Vec<u8>) {
    push_str(body, "Main");
    push_i32(body, 1);
    push_u32(body, 2);
    push_u32(body, 2);
    push_u32(body, 2);
    push_u16(body, 0);
    push_u16(body, 1);
    push_str(body, "Half");
    push_i32(body, 1);
    push_u32(body, 0);
    push_u32(body, 2);
    push_u32(body, 0);
}

fn push_str(body: &mut Vec<u8>, value: &str) {
    let len = u16::try_from(value.len() + 1).expect("fixture string should fit u16");
    push_u16(body, len);
    body.extend_from_slice(value.as_bytes());
    body.push(0);
}

fn push_u16(body: &mut Vec<u8>, value: u16) {
    body.extend_from_slice(&value.to_le_bytes());
}

fn push_u32(body: &mut Vec<u8>, value: u32) {
    body.extend_from_slice(&value.to_le_bytes());
}

fn push_i32(body: &mut Vec<u8>, value: i32) {
    body.extend_from_slice(&value.to_le_bytes());
}
