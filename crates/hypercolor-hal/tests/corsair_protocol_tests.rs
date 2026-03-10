use std::time::Duration;

use hypercolor_hal::drivers::corsair::framing::{
    LCD_PACKET_SIZE, LINK_WRITE_BUF_SIZE, build_link_packet,
};
use hypercolor_hal::drivers::corsair::{
    CORSAIR_KEEPALIVE_INTERVAL, CorsairLcdProtocol, CorsairLightingNodeProtocol,
    CorsairLinkProtocol, EP_GET_DEVICES, build_icue_link_lcd_protocol,
    build_xd6_elite_lcd_protocol,
};
use hypercolor_hal::protocol::{Protocol, ProtocolCommand, ResponseStatus, TransferType};
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

#[test]
fn lcd_init_sequence_uses_hid_reports_and_reports_display_capabilities() {
    let protocol = CorsairLcdProtocol::new("Test LCD", 480, 480, 0x40, 0x40, true, 0);
    let commands = protocol.init_sequence();

    assert_eq!(commands.len(), 4);
    assert!(commands.iter().all(|command| command.expects_response));
    assert!(commands.iter().all(|command| command.data.len() == 32));
    assert!(
        commands
            .iter()
            .all(|command| command.transfer_type == TransferType::HidReport)
    );
    assert_eq!(&commands[0].data[..4], &[0x03, 0x1D, 0x01, 0x00]);
    assert_eq!(&commands[1].data[..2], &[0x03, 0x19]);
    assert_eq!(
        &commands[2].data[..6],
        &[0x03, 0x20, 0x00, 0x19, 0x79, 0xE7]
    );
    assert_eq!(
        &commands[3].data[..6],
        &[0x03, 0x0B, 0x40, 0x01, 0x79, 0xE7]
    );

    let zones = protocol.zones();
    assert_eq!(zones.len(), 1);
    assert_eq!(zones[0].led_count, 0);
    assert_eq!(
        zones[0].topology,
        DeviceTopologyHint::Display {
            width: 480,
            height: 480,
            circular: true,
        }
    );

    let capabilities = protocol.capabilities();
    assert_eq!(capabilities.led_count, 0);
    assert!(!capabilities.supports_direct);
    assert!(capabilities.has_display);
    assert_eq!(capabilities.display_resolution, Some((480, 480)));
}

#[test]
fn lcd_encode_display_frame_chunks_bulk_packets_and_appends_keepalive() {
    let protocol = CorsairLcdProtocol::new("Test LCD", 480, 480, 0x40, 0x40, true, 0);
    let jpeg = (0_u16..1_500_u16)
        .map(|value| u8::try_from(value % 251).unwrap_or_default())
        .collect::<Vec<_>>();

    let commands = protocol
        .encode_display_frame(&jpeg)
        .expect("display frames should be supported");

    assert_eq!(commands.len(), 3);
    assert_eq!(commands[0].transfer_type, TransferType::Bulk);
    assert_eq!(commands[1].transfer_type, TransferType::Bulk);
    assert_eq!(commands[2].transfer_type, TransferType::HidReport);
    assert_eq!(commands[0].data.len(), LCD_PACKET_SIZE);
    assert_eq!(
        &commands[0].data[..8],
        &[0x02, 0x05, 0x40, 0x00, 0x00, 0x00, 0xF8, 0x03]
    );
    assert_eq!(&commands[0].data[8..11], &[0x00, 0x01, 0x02]);
    assert_eq!(
        &commands[1].data[..8],
        &[0x02, 0x05, 0x40, 0x01, 0x01, 0x00, 0xF8, 0x03]
    );
    assert_eq!(commands[1].data[492], 0);
    assert_eq!(
        &commands[2].data[..8],
        &[0x03, 0x19, 0x40, 0x01, 0x02, 0x00, 0xF8, 0x03]
    );
}

#[test]
fn lcd_encode_display_frame_into_reuses_command_buffer() {
    let protocol = CorsairLcdProtocol::new("Test LCD", 480, 480, 0x40, 0x40, true, 0);
    let jpeg = vec![0x55; 32];
    let mut commands = vec![ProtocolCommand {
        data: vec![0xAA; LCD_PACKET_SIZE],
        expects_response: true,
        response_delay: Duration::from_millis(1),
        post_delay: Duration::from_millis(1),
        transfer_type: TransferType::Primary,
    }];

    protocol
        .encode_display_frame_into(&jpeg, &mut commands)
        .expect("display frames should be supported");
    assert_eq!(commands.len(), 2);
    assert_eq!(commands[0].transfer_type, TransferType::Bulk);
    assert_eq!(commands[1].transfer_type, TransferType::HidReport);
    assert_eq!(
        &commands[0].data[..8],
        &[0x02, 0x05, 0x40, 0x01, 0x00, 0x00, 0xF8, 0x03]
    );

    protocol
        .encode_display_frame_into(&jpeg[..8], &mut commands)
        .expect("display frames should still be supported on buffer reuse");
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].transfer_type, TransferType::Bulk);
    assert_eq!(
        &commands[0].data[..8],
        &[0x02, 0x05, 0x40, 0x01, 0x00, 0x00, 0xF8, 0x03]
    );
}

#[test]
fn lcd_keepalive_and_shutdown_use_hid_reports() {
    let protocol = CorsairLcdProtocol::new("Test LCD", 480, 480, 0x01, 0x40, true, 0);

    let keepalive = protocol
        .keepalive()
        .expect("LCD protocol should expose keepalive");
    assert_eq!(keepalive.interval, Duration::from_secs(30));
    assert!(keepalive.commands.is_empty());

    let keepalive_commands = protocol.keepalive_commands();
    assert_eq!(keepalive_commands.len(), 1);
    assert_eq!(keepalive_commands[0].transfer_type, TransferType::HidReport);
    assert_eq!(
        &keepalive_commands[0].data[..8],
        &[0x03, 0x19, 0x40, 0x01, 0x00, 0x00, 0x00, 0x00]
    );

    let shutdown = protocol.shutdown_sequence();
    assert_eq!(shutdown.len(), 1);
    assert_eq!(shutdown[0].transfer_type, TransferType::HidReport);
    assert_eq!(
        &shutdown[0].data[..8],
        &[0x03, 0x1E, 0x40, 0x01, 0x43, 0x00, 0x69, 0x00]
    );
}

#[test]
fn xc7_lcd_uses_short_init_and_model_specific_shutdown() {
    let protocol = CorsairLcdProtocol::new_xc7("Corsair XC7 RGB Elite LCD");

    let init = protocol.init_sequence();
    assert_eq!(init.len(), 2);
    assert_eq!(&init[0].data[..4], &[0x03, 0x1D, 0x01, 0x00]);
    assert_eq!(&init[1].data[..2], &[0x03, 0x19]);
    assert!(init.iter().all(|command| command.expects_response));
    assert!(
        init.iter()
            .all(|command| command.transfer_type == TransferType::HidReport)
    );

    let shutdown = protocol.shutdown_sequence();
    assert_eq!(shutdown.len(), 2);
    assert_eq!(shutdown[0].transfer_type, TransferType::HidReport);
    assert_eq!(shutdown[1].transfer_type, TransferType::HidReport);
    assert_eq!(
        &shutdown[0].data[..7],
        &[0x03, 0x1E, 0x19, 0x01, 0x04, 0x00, 0xA3]
    );
    assert_eq!(
        &shutdown[1].data[..7],
        &[0x03, 0x1D, 0x00, 0x01, 0x04, 0x00, 0xA3]
    );
}

#[test]
fn xc7_lcd_supports_ring_zone_and_model_specific_keepalive() {
    let protocol = CorsairLcdProtocol::new_xc7("Corsair XC7 RGB Elite LCD");
    let ring_colors = (0_u8..31_u8)
        .map(|value| [value, value.saturating_add(1), value.saturating_add(2)])
        .collect::<Vec<_>>();

    let ring_commands = protocol.encode_frame(&ring_colors);
    assert_eq!(ring_commands.len(), 1);
    assert_eq!(ring_commands[0].transfer_type, TransferType::Bulk);
    assert_eq!(ring_commands[0].data.len(), LCD_PACKET_SIZE);
    assert_eq!(
        &ring_commands[0].data[..6],
        &[0x02, 0x07, 0x1F, 0x00, 0x01, 0x02]
    );

    let jpeg = vec![0x55; 32];
    let display_commands = protocol
        .encode_display_frame(&jpeg)
        .expect("XC7 should support display frames");
    assert_eq!(display_commands.len(), 2);
    assert_eq!(display_commands[0].transfer_type, TransferType::Bulk);
    assert_eq!(display_commands[1].transfer_type, TransferType::HidReport);
    assert_eq!(
        &display_commands[0].data[..8],
        &[0x02, 0x05, 0x1F, 0x01, 0x00, 0x00, 0xF8, 0x03]
    );
    assert_eq!(
        &display_commands[1].data[..8],
        &[0x03, 0x19, 0x1C, 0x01, 0x01, 0x00, 0xF8, 0x03]
    );

    let zones = protocol.zones();
    assert_eq!(zones.len(), 2);
    assert_eq!(zones[0].name, "Display");
    assert_eq!(zones[1].name, "RGB Ring");
    assert_eq!(zones[1].topology, DeviceTopologyHint::Ring { count: 31 });

    let capabilities = protocol.capabilities();
    assert_eq!(capabilities.led_count, 31);
    assert!(capabilities.supports_direct);
    assert!(capabilities.has_display);
    assert_eq!(protocol.total_leds(), 31);
}

#[test]
fn icue_link_lcd_matches_signalrgb_standard_lcd_flow() {
    let protocol = build_icue_link_lcd_protocol();

    let init = protocol.init_sequence();
    assert_eq!(init.len(), 4);
    assert_eq!(&init[3].data[..6], &[0x03, 0x0B, 0x40, 0x01, 0x79, 0xE7]);

    let jpeg = vec![0x55; 32];
    let commands = protocol
        .encode_display_frame(&jpeg)
        .expect("iCUE LINK LCD should support display frames");

    assert_eq!(commands.len(), 2);
    assert_eq!(commands[0].transfer_type, TransferType::Bulk);
    assert_eq!(commands[1].transfer_type, TransferType::HidReport);
    assert_eq!(
        &commands[0].data[..8],
        &[0x02, 0x05, 0x40, 0x01, 0x00, 0x00, 0xF8, 0x03]
    );
    assert_eq!(
        &commands[1].data[..8],
        &[0x03, 0x19, 0x40, 0x01, 0x01, 0x00, 0xF8, 0x03]
    );
}

#[test]
fn xd6_lcd_uses_standard_init_with_model_specific_zone_byte() {
    let protocol = build_xd6_elite_lcd_protocol();

    let init = protocol.init_sequence();
    assert_eq!(init.len(), 4);
    assert_eq!(&init[3].data[..6], &[0x03, 0x0B, 0x40, 0x01, 0x79, 0xE7]);

    let jpeg = vec![0xAA; 32];
    let commands = protocol
        .encode_display_frame(&jpeg)
        .expect("XD6 LCD should support display frames");

    assert_eq!(commands.len(), 2);
    assert_eq!(commands[0].transfer_type, TransferType::Bulk);
    assert_eq!(commands[1].transfer_type, TransferType::HidReport);
    assert_eq!(
        &commands[0].data[..8],
        &[0x02, 0x05, 0x01, 0x01, 0x00, 0x00, 0xF8, 0x03]
    );
    assert_eq!(
        &commands[1].data[..8],
        &[0x03, 0x19, 0x40, 0x01, 0x01, 0x00, 0xF8, 0x03]
    );
}
