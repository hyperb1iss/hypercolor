//! Golden-byte coverage for the Razer accessories registered from Spec 17
//! section 6.3: Tomahawk ATX and Raptor 27 (`0x1F`, OpenRazer facts) and
//! Core X Chroma (`0x3F`, OpenRGB facts).

use hypercolor_hal::database::ProtocolDatabase;
use hypercolor_hal::drivers::razer::{RAZER_REPORT_LEN, RAZER_VENDOR_ID, RazerReport, razer_crc};
use hypercolor_hal::protocol::{Protocol, ProtocolCommand};
use hypercolor_hal::registry::{HidRawReportMode, TransportType};
use hypercolor_types::device::{DeviceFamily, DeviceTopologyHint};
use zerocopy::FromBytes;

const PID_TOMAHAWK_ATX: u16 = 0x0F17;
const PID_RAPTOR_27: u16 = 0x0F12;
const PID_CORE_X_CHROMA: u16 = 0x0F1A;

struct Accessory {
    product_id: u16,
    name: &'static str,
    protocol_id: &'static str,
    transaction_id: u8,
    leds: u8,
    transport: TransportType,
}

fn shared_feature_transport(
    interface: u8,
    usage_page: Option<u16>,
    usage: Option<u16>,
) -> TransportType {
    TransportType::UsbHidApi {
        interface: Some(interface),
        report_id: 0x00,
        report_mode: HidRawReportMode::FeatureReport,
        max_report_len: RAZER_REPORT_LEN + 1,
        usage_page,
        usage,
    }
}

fn accessories() -> [Accessory; 3] {
    [
        Accessory {
            product_id: PID_TOMAHAWK_ATX,
            name: "Razer Tomahawk ATX",
            protocol_id: "razer/tomahawk-atx",
            transaction_id: 0x1F,
            leds: 20,
            transport: shared_feature_transport(0, None, None),
        },
        Accessory {
            product_id: PID_RAPTOR_27,
            name: "Razer Raptor 27",
            protocol_id: "razer/raptor-27",
            transaction_id: 0x1F,
            leds: 12,
            transport: shared_feature_transport(0, None, None),
        },
        Accessory {
            product_id: PID_CORE_X_CHROMA,
            name: "Razer Core X Chroma",
            protocol_id: "razer/core-x-chroma",
            transaction_id: 0x3F,
            leds: 16,
            transport: shared_feature_transport(2, Some(0x0001), Some(0x0002)),
        },
    ]
}

fn protocol_for(product_id: u16) -> Box<dyn Protocol> {
    let descriptor = ProtocolDatabase::lookup(RAZER_VENDOR_ID, product_id)
        .expect("registered Razer accessory should resolve");
    (descriptor.protocol.build)()
}

/// `[index, 0x40 + index, 0x80 + index]` per LED, so byte order and column
/// placement are both visible in the golden packets.
fn ramp(leds: u8) -> Vec<[u8; 3]> {
    (0..leds)
        .map(|index| [index, 0x40 + index, 0x80 + index])
        .collect()
}

/// Expected 90-byte report: header, arguments, then the checksum at offset
/// 88 (XOR of bytes 2 through 87, so the transaction ID is not covered). The
/// checksum is passed in as a literal so the test pins the wire value rather
/// than re-deriving it with the code under test.
fn report(transaction_id: u8, header: [u8; 3], args: &[u8], crc: u8) -> Vec<u8> {
    let mut bytes = vec![0_u8; RAZER_REPORT_LEN];
    bytes[1] = transaction_id;
    bytes[5] = header[0];
    bytes[6] = header[1];
    bytes[7] = header[2];
    bytes[8..8 + args.len()].copy_from_slice(args);
    bytes[88] = crc;
    bytes
}

fn frame_args(leds: u8) -> Vec<u8> {
    let mut args = vec![0x00, 0x00, 0x00, 0x00, leds - 1];
    for color in ramp(leds) {
        args.extend_from_slice(&color);
    }
    args
}

fn assert_write_only(commands: &[ProtocolCommand]) {
    assert!(commands.iter().all(|command| !command.expects_response));
}

#[test]
fn tomahawk_atx_frame_matches_golden_bytes() {
    let commands = protocol_for(PID_TOMAHAWK_ATX).encode_frame(&ramp(20));

    assert_eq!(commands.len(), 2, "one row upload plus the apply packet");
    assert_eq!(
        commands[0].data,
        report(0x1F, [0x41, 0x0F, 0x03], &frame_args(20), 0x5E)
    );
    assert_eq!(
        commands[1].data,
        report(0x1F, [0x0C, 0x0F, 0x02], &[0x00, 0x00, 0x08], 0x09)
    );
    assert_write_only(&commands);
}

#[test]
fn raptor_27_frame_matches_golden_bytes() {
    let commands = protocol_for(PID_RAPTOR_27).encode_frame(&ramp(12));

    assert_eq!(commands.len(), 2, "one row upload plus the apply packet");
    assert_eq!(
        commands[0].data,
        report(0x1F, [0x29, 0x0F, 0x03], &frame_args(12), 0x2E)
    );
    assert_eq!(
        commands[1].data,
        report(0x1F, [0x0C, 0x0F, 0x02], &[0x00, 0x00, 0x08], 0x09)
    );
    assert_write_only(&commands);
}

#[test]
fn core_x_chroma_frame_matches_golden_bytes() {
    let commands = protocol_for(PID_CORE_X_CHROMA).encode_frame(&ramp(16));

    assert_eq!(commands.len(), 2, "one row upload plus the apply packet");
    assert_eq!(
        commands[0].data,
        report(0x3F, [0x35, 0x0F, 0x03], &frame_args(16), 0x36)
    );
    assert_eq!(
        commands[1].data,
        report(0x3F, [0x0C, 0x0F, 0x02], &[0x00, 0x00, 0x08], 0x09)
    );
    assert_write_only(&commands);
}

#[test]
fn every_packet_is_a_full_report_with_a_valid_checksum() {
    for accessory in accessories() {
        let protocol = protocol_for(accessory.product_id);
        let commands = protocol.encode_frame(&ramp(accessory.leds));

        for command in &commands {
            assert_eq!(command.data.len(), RAZER_REPORT_LEN, "{}", accessory.name);
            let (parsed, _) = RazerReport::read_from_prefix(&command.data)
                .expect("encoded packet should parse as a Razer report");
            assert_eq!(parsed.transaction_id, accessory.transaction_id);
            assert_eq!(parsed.crc, razer_crc(&parsed), "{}", accessory.name);
        }
    }
}

#[test]
fn color_bytes_land_in_red_green_blue_order() {
    for accessory in accessories() {
        let mut colors = vec![[0_u8; 3]; usize::from(accessory.leds)];
        colors[1] = [0xAA, 0xBB, 0xCC];
        let commands = protocol_for(accessory.product_id).encode_frame(&colors);

        // Arguments start at offset 8, RGB data after the 5-byte row header.
        let second_led = 8 + 5 + 3;
        assert_eq!(
            &commands[0].data[second_led..second_led + 3],
            &[0xAA, 0xBB, 0xCC],
            "{}",
            accessory.name
        );
    }
}

#[test]
fn short_frames_pad_with_black_and_long_frames_truncate() {
    for accessory in accessories() {
        let protocol = protocol_for(accessory.product_id);
        let leds = usize::from(accessory.leds);
        let stop_col = accessory.leds - 1;

        let short = protocol.encode_frame(&[[0x10, 0x20, 0x30]]);
        assert_eq!(short[0].data[12], stop_col, "{}", accessory.name);
        assert_eq!(&short[0].data[13..16], &[0x10, 0x20, 0x30]);
        assert!(
            short[0].data[16..13 + leds * 3]
                .iter()
                .all(|&byte| byte == 0),
            "{} pads missing LEDs with black",
            accessory.name
        );

        let long = protocol.encode_frame(&vec![[0xFF, 0xFF, 0xFF]; leds + 5]);
        assert_eq!(long.len(), 2, "{}", accessory.name);
        assert_eq!(long[0].data[12], stop_col, "{}", accessory.name);
        assert_eq!(
            usize::from(long[0].data[5]),
            leds * 3 + 5,
            "{} declares exactly one row",
            accessory.name
        );
    }
}

#[test]
fn encode_frame_into_reuses_buffers_and_matches_encode_frame() {
    for accessory in accessories() {
        let protocol = protocol_for(accessory.product_id);
        let colors = ramp(accessory.leds);

        let mut commands = Vec::new();
        protocol.encode_frame_into(&colors, &mut commands);
        protocol.encode_frame_into(&colors, &mut commands);

        let fresh = protocol.encode_frame(&colors);
        assert_eq!(commands.len(), fresh.len(), "{}", accessory.name);
        for (reused, fresh) in commands.iter().zip(&fresh) {
            assert_eq!(reused.data, fresh.data, "{}", accessory.name);
        }
    }
}

#[test]
fn accessories_skip_device_mode_and_probe_the_serial_on_connect() {
    for accessory in accessories() {
        let protocol = protocol_for(accessory.product_id);

        assert!(protocol.init_sequence().is_empty(), "{}", accessory.name);
        assert!(
            protocol.shutdown_sequence().is_empty(),
            "{}",
            accessory.name
        );

        let diagnostics = protocol.connection_diagnostics();
        assert_eq!(diagnostics.len(), 1, "{}", accessory.name);
        assert!(diagnostics[0].expects_response);
        assert_eq!(diagnostics[0].data[1], accessory.transaction_id);
        assert_eq!(diagnostics[0].data[6], 0x00);
        assert_eq!(diagnostics[0].data[7], 0x82);
    }
}

#[test]
fn brightness_uses_the_extended_command_on_the_zero_led() {
    let tomahawk = protocol_for(PID_TOMAHAWK_ATX)
        .encode_brightness(0x80)
        .expect("Tomahawk ATX supports brightness");
    assert_eq!(tomahawk.len(), 1);
    assert!(tomahawk[0].expects_response);
    assert_eq!(
        tomahawk[0].data,
        report(0x1F, [0x03, 0x0F, 0x04], &[0x00, 0x00, 0x80], 0x88)
    );

    let core_x = protocol_for(PID_CORE_X_CHROMA)
        .encode_brightness(0x80)
        .expect("Core X Chroma supports brightness");
    assert_eq!(
        core_x[0].data,
        report(0x3F, [0x03, 0x0F, 0x04], &[0x00, 0x00, 0x80], 0x88)
    );
}

#[test]
fn accessories_expose_one_row_matrix_zones() {
    for accessory in accessories() {
        let protocol = protocol_for(accessory.product_id);
        let zones = protocol.zones();

        assert_eq!(protocol.total_leds(), u32::from(accessory.leds));
        assert_eq!(zones.len(), 1, "{}", accessory.name);
        assert_eq!(zones[0].led_count, u32::from(accessory.leds));
        assert_eq!(
            zones[0].topology,
            DeviceTopologyHint::Matrix {
                rows: 1,
                cols: u32::from(accessory.leds),
            },
            "{}",
            accessory.name
        );
        assert!(protocol.capabilities().supports_brightness);
    }
}

#[test]
fn database_binds_accessories_to_their_reference_transports() {
    for accessory in accessories() {
        let descriptor = ProtocolDatabase::lookup(RAZER_VENDOR_ID, accessory.product_id)
            .expect("registered Razer accessory should resolve");

        assert_eq!(descriptor.name, accessory.name);
        assert_eq!(descriptor.protocol.id, accessory.protocol_id);
        assert_eq!(descriptor.driver_id(), "razer");
        assert_eq!(
            descriptor.family,
            DeviceFamily::new_static("razer", "Razer")
        );
        assert!(descriptor.firmware_predicate.is_none());
        assert_eq!(
            descriptor.transport, accessory.transport,
            "{}",
            accessory.name
        );
    }
}
