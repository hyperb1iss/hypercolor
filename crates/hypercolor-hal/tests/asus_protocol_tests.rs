#![cfg(target_os = "linux")]

use hypercolor_hal::drivers::asus::{
    AURA_REPORT_ID, AuraColorOrder, AuraControllerGen, AuraUsbProtocol,
    build_aura_addressable_gen1_protocol, build_aura_terminal_protocol, build_effect_color_payload,
};
use hypercolor_hal::protocol::{Protocol, ResponseStatus};
use hypercolor_hal::transport::hidraw::encode_feature_report_packet;
use hypercolor_types::device::{DeviceColorFormat, DeviceTopologyHint};

fn wire_packet(payload: &[u8]) -> Vec<u8> {
    encode_feature_report_packet(payload, AURA_REPORT_ID)
}

fn firmware_response(firmware: &str) -> Vec<u8> {
    let mut response = vec![0_u8; 65];
    response[0] = AURA_REPORT_ID;
    response[1] = 0x02;
    let bytes = firmware.as_bytes();
    let copy_len = bytes.len().min(16);
    response[2..2 + copy_len].copy_from_slice(&bytes[..copy_len]);
    response
}

fn config_response(argb_channels: u8, mainboard_leds: u8, rgb_headers: u8) -> Vec<u8> {
    let mut response = vec![0_u8; 65];
    response[0] = AURA_REPORT_ID;
    response[1] = 0x30;
    response[4 + 0x02] = argb_channels;
    response[4 + 0x1B] = mainboard_leds;
    response[4 + 0x1D] = rgb_headers;
    response
}

#[test]
fn motherboard_queries_are_byte_exact_on_the_wire() {
    let protocol = AuraUsbProtocol::new(AuraControllerGen::Motherboard);
    let init = protocol.init_sequence();

    assert_eq!(init.len(), 7, "firmware + config + 5 direct-mode commands");
    assert!(init[0].expects_response);
    assert!(init[1].expects_response);

    let firmware = wire_packet(&init[0].data);
    assert_eq!(firmware.len(), 65);
    assert_eq!(firmware[0], AURA_REPORT_ID);
    assert_eq!(firmware[1], 0x82);

    let config = wire_packet(&init[1].data);
    assert_eq!(config.len(), 65);
    assert_eq!(config[0], AURA_REPORT_ID);
    assert_eq!(config[1], 0xB0);
}

#[test]
fn gen1_addressable_init_includes_disable_and_direct_mode_setup() {
    let protocol = build_aura_addressable_gen1_protocol();
    let init = protocol.init_sequence();

    assert_eq!(
        init.len(),
        7,
        "firmware + config + gen1 disable + 4 channels"
    );

    let disable = wire_packet(&init[2].data);
    assert_eq!(&disable[..5], &[AURA_REPORT_ID, 0x52, 0x53, 0x00, 0x01]);

    for (index, command) in init[3..].iter().enumerate() {
        let packet = wire_packet(&command.data);
        assert_eq!(packet[1], 0x35);
        assert_eq!(
            packet[2],
            u8::try_from(index).expect("channel index should fit in u8")
        );
        assert_eq!(packet[5], 0xFF);
    }
}

#[test]
fn terminal_init_uses_addressable_mode_for_all_five_channels() {
    let protocol = build_aura_terminal_protocol();
    let init = protocol.init_sequence();

    assert_eq!(init.len(), 5);

    for (index, command) in init.iter().enumerate() {
        let packet = wire_packet(&command.data);
        assert_eq!(packet[1], 0x3B);
        assert_eq!(
            packet[2],
            u8::try_from(index).expect("channel index should fit in u8")
        );
        assert_eq!(packet[4], 0xFF);
    }
}

#[test]
fn parse_firmware_and_config_updates_runtime_topology() {
    let protocol =
        AuraUsbProtocol::new(AuraControllerGen::Motherboard).with_argb_led_counts(vec![60, 30, 15]);

    let firmware = protocol
        .parse_response(&firmware_response("HCOLOR-TEST-0001"))
        .expect("firmware response should parse");
    assert_eq!(firmware.status, ResponseStatus::Ok);
    assert_eq!(protocol.firmware().as_deref(), Some("HCOLOR-TEST-0001"));

    let config = protocol
        .parse_response(&config_response(3, 2, 1))
        .expect("config response should parse");
    assert_eq!(config.status, ResponseStatus::Ok);

    let zones = protocol.zones();
    assert_eq!(zones.len(), 4);
    assert_eq!(zones[0].name, "Mainboard");
    assert_eq!(zones[0].led_count, 2);
    assert_eq!(zones[1].led_count, 60);
    assert_eq!(zones[2].led_count, 30);
    assert_eq!(zones[3].led_count, 15);
    assert_eq!(protocol.total_leds(), 107);
}

#[test]
fn firmware_override_takes_precedence_over_config_table() {
    let protocol = AuraUsbProtocol::new(AuraControllerGen::Motherboard);

    protocol
        .parse_response(&firmware_response("AULA3-AR32-0218"))
        .expect("firmware response should parse");
    protocol
        .parse_response(&config_response(1, 1, 0))
        .expect("config response should parse");

    let zones = protocol.zones();
    assert_eq!(zones[0].led_count, 5);
    assert_eq!(zones.len(), 4);
    assert!(zones[1..].iter().all(|zone| zone.led_count == 120));
    assert_eq!(protocol.total_leds(), 365);
}

#[test]
fn board_name_override_applies_when_firmware_has_no_match() {
    let protocol =
        AuraUsbProtocol::new(AuraControllerGen::Motherboard).with_board_name("PRIME Z790-A WIFI");

    protocol
        .parse_response(&firmware_response("UNKNOWN-FW-000001"))
        .expect("firmware response should parse");
    protocol
        .parse_response(&config_response(1, 1, 0))
        .expect("config response should parse");

    let zones = protocol.zones();
    assert_eq!(zones[0].led_count, 4);
    assert_eq!(zones.len(), 4);
    assert!(zones[1..].iter().all(|zone| zone.led_count == 120));
}

#[test]
fn encode_frame_splits_mainboard_and_argb_packets_with_apply_flags() {
    let protocol = AuraUsbProtocol::new(AuraControllerGen::Motherboard).with_topology(3, vec![30]);
    let colors = vec![[0x10, 0x20, 0x30]; 33];

    let commands = protocol.encode_frame(&colors);
    assert_eq!(commands.len(), 3);

    let mainboard = wire_packet(&commands[0].data);
    assert_eq!(mainboard[1], 0x40);
    assert_eq!(mainboard[2], 0x84);
    assert_eq!(mainboard[3], 0x00);
    assert_eq!(mainboard[4], 0x03);

    let argb_first = wire_packet(&commands[1].data);
    assert_eq!(argb_first[2], 0x00);
    assert_eq!(argb_first[3], 0x00);
    assert_eq!(argb_first[4], 20);

    let argb_last = wire_packet(&commands[2].data);
    assert_eq!(argb_last[2], 0x80);
    assert_eq!(argb_last[3], 20);
    assert_eq!(argb_last[4], 10);
}

#[test]
fn color_order_permutations_affect_direct_payload_bytes() {
    let cases = [
        (AuraColorOrder::Rgb, [0x10, 0x20, 0x30]),
        (AuraColorOrder::Rbg, [0x10, 0x30, 0x20]),
        (AuraColorOrder::Grb, [0x20, 0x10, 0x30]),
        (AuraColorOrder::Gbr, [0x20, 0x30, 0x10]),
        (AuraColorOrder::Brg, [0x30, 0x10, 0x20]),
        (AuraColorOrder::Bgr, [0x30, 0x20, 0x10]),
    ];

    for (order, expected) in cases {
        let protocol = AuraUsbProtocol::new(AuraControllerGen::Motherboard)
            .with_color_order(order)
            .with_topology(1, Vec::new());
        let commands = protocol.encode_frame(&[[0x10, 0x20, 0x30]]);
        let packet = wire_packet(&commands[0].data);
        assert_eq!(&packet[5..8], &expected);
    }
}

#[test]
fn mixed_argb_lengths_map_to_expected_zones() {
    let protocol =
        AuraUsbProtocol::new(AuraControllerGen::AddressableOnly).with_topology(0, vec![2, 1]);
    let zones = protocol.zones();

    assert_eq!(zones.len(), 2);
    assert_eq!(zones[0].name, "ARGB Header 1");
    assert_eq!(zones[0].led_count, 2);
    assert_eq!(zones[0].topology, DeviceTopologyHint::Strip);
    assert_eq!(zones[0].color_format, DeviceColorFormat::Rgb);
    assert_eq!(zones[1].name, "ARGB Header 2");
    assert_eq!(zones[1].led_count, 1);
}

#[test]
fn effect_color_payload_uses_masked_offsets() {
    let payload = build_effect_color_payload(
        2,
        &[[0x12, 0x34, 0x56], [0xAA, 0xBB, 0xCC], [0xDE, 0xAD, 0xBE]],
        false,
        AuraColorOrder::Rgb,
    )
    .expect("effect-color payload should build");

    let packet = wire_packet(&payload);
    assert_eq!(packet[1], 0x36);
    assert_eq!(packet[2], 0x00);
    assert_eq!(packet[3], 0x1C);
    assert_eq!(&packet[11..14], &[0x12, 0x34, 0x56]);
    assert_eq!(&packet[14..17], &[0xAA, 0xBB, 0xCC]);
    assert_eq!(&packet[17..20], &[0xDE, 0xAD, 0xBE]);
}
