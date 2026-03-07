use std::time::Duration;

use hypercolor_hal::drivers::corsair::framing::{LINK_WRITE_BUF_SIZE, build_link_packet};
use hypercolor_hal::drivers::corsair::{
    CORSAIR_KEEPALIVE_INTERVAL, CorsairLightingNodeProtocol, CorsairLinkProtocol, EP_GET_DEVICES,
};
use hypercolor_hal::protocol::{Protocol, ResponseStatus};
use hypercolor_types::device::DeviceTopologyHint;

fn link_enumeration_response(records: &[(u8, u8, &str)]) -> Vec<u8> {
    let mut data = vec![0x00, 0x00, 0x00, 0x00];
    data.extend_from_slice(&EP_GET_DEVICES.data_type);
    data.push(u8::try_from(records.len()).expect("record count should fit in u8"));

    for (device_type, model, serial) in records {
        data.extend_from_slice(&[
            0x00,
            0x00,
            *device_type,
            *model,
            0x00,
            0x00,
            0x00,
            u8::try_from(serial.len()).expect("serial length should fit in u8"),
        ]);
        data.extend_from_slice(serial.as_bytes());
    }

    data
}

#[test]
fn build_link_packet_sets_command_prefix_and_zero_padding() {
    let packet = build_link_packet(&[0x06, 0x00], &[0xAA, 0xBB]);

    assert_eq!(packet.len(), LINK_WRITE_BUF_SIZE);
    assert_eq!(packet[2], 0x01);
    assert_eq!(&packet[3..7], &[0x06, 0x00, 0xAA, 0xBB]);
    assert!(packet[7..].iter().all(|&byte| byte == 0));
}

#[test]
fn link_parse_response_populates_children_and_capabilities() {
    let protocol = CorsairLinkProtocol::new();
    let response = protocol
        .parse_response(&link_enumeration_response(&[
            (0x01, 0x00, "FAN1"),
            (0x05, 0x02, "CASE"),
            (0x0E, 0x00, "LCD1"),
        ]))
        .expect("enumeration response should parse");

    assert_eq!(response.status, ResponseStatus::Ok);

    let children = protocol.children();
    assert_eq!(children.len(), 2);
    assert_eq!(children[0].serial, "FAN1");
    assert_eq!(children[0].led_count, 34);
    assert_eq!(children[0].color_offset, 0);
    assert_eq!(children[1].serial, "CASE");
    assert_eq!(children[1].led_count, 160);
    assert_eq!(children[1].color_offset, 34);

    let zones = protocol.zones();
    assert_eq!(zones.len(), 2);
    assert_eq!(zones[0].name, "iCUE LINK QX RGB (FAN1)");
    assert_eq!(zones[0].topology, DeviceTopologyHint::Ring { count: 34 });
    assert_eq!(zones[1].name, "iCUE LINK Case Adapter (CASE)");
    assert_eq!(zones[1].topology, DeviceTopologyHint::Strip);

    let capabilities = protocol.capabilities();
    assert_eq!(capabilities.led_count, 194);
    assert!(capabilities.supports_direct);
    assert!(!capabilities.supports_brightness);
    assert!(!capabilities.has_display);
    assert_eq!(capabilities.max_fps, 30);
    assert_eq!(protocol.total_leds(), 194);
    assert_eq!(protocol.frame_interval(), Duration::from_millis(33));
}

#[test]
fn link_encode_frame_chunks_payload_and_reuses_it_for_keepalive() {
    let protocol = CorsairLinkProtocol::new();
    protocol
        .parse_response(&link_enumeration_response(&[
            (0x01, 0x00, "FAN1"),
            (0x05, 0x02, "CASE"),
        ]))
        .expect("enumeration response should parse");

    let colors = (0_u8..194_u8)
        .map(|value| [value, value.saturating_add(1), value.saturating_add(2)])
        .collect::<Vec<_>>();
    let commands = protocol.encode_frame(&colors);

    assert_eq!(commands.len(), 5);
    assert_eq!(&commands[0].data[3..7], &[0x05, 0x01, 0x01, 0x22]);
    assert_eq!(&commands[1].data[3..6], &[0x0D, 0x00, 0x22]);
    assert_eq!(&commands[2].data[3..5], &[0x06, 0x00]);
    assert_eq!(&commands[3].data[3..5], &[0x07, 0x00]);
    assert_eq!(&commands[4].data[3..7], &[0x05, 0x01, 0x01, 0x22]);
    assert_eq!(
        &commands[2].data[5..11],
        &[0x48, 0x02, 0x00, 0x00, 0x12, 0x00]
    );
    assert_eq!(&commands[2].data[11..14], &[0x00, 0x01, 0x02]);

    let keepalive = protocol
        .keepalive()
        .expect("LINK protocol should expose keepalive");
    assert!(keepalive.commands.is_empty());
    assert_eq!(keepalive.interval, CORSAIR_KEEPALIVE_INTERVAL);

    let replay = protocol.keepalive_commands();
    assert_eq!(replay.len(), commands.len());
    assert_eq!(replay[0].data, commands[0].data);
    assert_eq!(replay[3].data, commands[3].data);
}

#[test]
fn lighting_node_encode_frame_uses_planar_chunks_and_commit() {
    let protocol = CorsairLightingNodeProtocol::new("Test Lighting Node", 1);
    let colors = (0_u8..204_u8)
        .map(|value| [value, 255_u8.saturating_sub(value), value / 2])
        .collect::<Vec<_>>();

    let commands = protocol.encode_frame(&colors);

    assert_eq!(commands.len(), 17);
    assert_eq!(commands[0].data[1], 0x38);
    assert_eq!(&commands[0].data[2..4], &[0x00, 0x02]);

    assert_eq!(commands[1].data[1], 0x32);
    assert_eq!(&commands[1].data[2..6], &[0x00, 0x00, 50, 0x00]);
    assert_eq!(&commands[1].data[6..9], &[0x00, 0x01, 0x02]);

    assert_eq!(commands[2].data[1], 0x32);
    assert_eq!(&commands[2].data[2..6], &[0x00, 0x00, 50, 0x01]);
    assert_eq!(&commands[2].data[6..9], &[0xFF, 0xFE, 0xFD]);

    assert_eq!(commands[3].data[1], 0x32);
    assert_eq!(&commands[3].data[2..6], &[0x00, 0x00, 50, 0x02]);
    assert_eq!(&commands[3].data[6..9], &[0x00, 0x00, 0x01]);

    assert_eq!(&commands[13].data[2..6], &[0x00, 200, 4, 0x00]);
    assert_eq!(&commands[16].data[1..3], &[0x33, 0xFF]);
}

#[test]
fn lighting_node_brightness_keepalive_and_shutdown_are_supported() {
    let protocol = CorsairLightingNodeProtocol::new("Test Lighting Node Pro", 2);

    let brightness = protocol
        .encode_brightness(128)
        .expect("brightness should be supported");
    assert_eq!(brightness.len(), 2);
    assert_eq!(&brightness[0].data[1..4], &[0x39, 0x00, 50]);
    assert_eq!(&brightness[1].data[1..4], &[0x39, 0x01, 50]);

    let keepalive = protocol
        .keepalive()
        .expect("lighting node should expose keepalive");
    assert_eq!(keepalive.interval, CORSAIR_KEEPALIVE_INTERVAL);
    assert_eq!(keepalive.commands.len(), 1);
    assert_eq!(&keepalive.commands[0].data[1..3], &[0x33, 0xFF]);

    let shutdown = protocol.shutdown_sequence();
    assert_eq!(shutdown.len(), 2);
    assert_eq!(&shutdown[0].data[1..4], &[0x38, 0x00, 0x01]);
    assert_eq!(&shutdown[1].data[1..4], &[0x38, 0x01, 0x01]);

    let zones = protocol.zones();
    assert_eq!(zones.len(), 2);
    assert_eq!(zones[0].led_count, 204);
    assert_eq!(zones[1].topology, DeviceTopologyHint::Strip);

    let capabilities = protocol.capabilities();
    assert_eq!(capabilities.led_count, 408);
    assert!(capabilities.supports_direct);
    assert!(capabilities.supports_brightness);
    assert_eq!(protocol.total_leds(), 408);
}
