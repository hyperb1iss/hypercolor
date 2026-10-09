use std::time::Duration;

use hypercolor_hal::database::ProtocolDatabase;
use hypercolor_hal::drivers::razer::{
    KRAKEN_INPUT_REPORT_ID, KRAKEN_OUTPUT_REPORT_ID, KRAKEN_REPORT_LEN, KRAKEN_REQUEST_BODY_LEN,
    KRAKEN_RESPONSE_LEN, KrakenModel, KrakenProtocol, PID_KRAKEN_ULTIMATE, RAZER_VENDOR_ID,
    build_kraken_ultimate_protocol,
};
use hypercolor_hal::protocol::{Protocol, ProtocolCommand, ProtocolError, TransferType};
use hypercolor_hal::registry::{HidRawReportMode, TransportType};
use hypercolor_hal::transport::hidapi::encode_hidapi_packet_for_testing;
use hypercolor_types::device::{DeviceColorFormat, DeviceFamily, DeviceTopologyHint};

const RAM_WRITE: u8 = 0x40;
const EEPROM_READ: u8 = 0x20;

fn ultimate() -> KrakenProtocol {
    KrakenProtocol::new(KrakenModel::Ultimate)
}

/// Expected request body: destination, length, big-endian address, then the
/// payload zero-padded to 32 bytes.
fn request_body(destination: u8, address: u16, payload: &[u8], length: u8) -> Vec<u8> {
    let mut body = vec![0_u8; KRAKEN_REQUEST_BODY_LEN];
    body[0] = destination;
    body[1] = length;
    body[2..4].copy_from_slice(&address.to_be_bytes());
    body[4..4 + payload.len()].copy_from_slice(payload);
    body
}

fn color_write(color: [u8; 3]) -> Vec<u8> {
    request_body(RAM_WRITE, 0x1189, &color, 3)
}

fn effect_write() -> Vec<u8> {
    request_body(RAM_WRITE, 0x172D, &[0x01], 1)
}

fn assert_write_only(command: &ProtocolCommand) {
    assert!(!command.expects_response);
    assert_eq!(command.response_delay, Duration::ZERO);
    assert_eq!(command.post_delay, Duration::ZERO);
    assert_eq!(command.transfer_type, TransferType::Primary);
}

#[test]
fn frame_is_a_color_write_then_an_effect_write() {
    let commands = ultimate().encode_frame(&[[0x12, 0x34, 0x56]]);

    assert_eq!(commands.len(), 2);
    assert_eq!(commands[0].data, color_write([0x12, 0x34, 0x56]));
    assert_eq!(commands[1].data, effect_write());
    for command in &commands {
        assert_eq!(command.data.len(), KRAKEN_REQUEST_BODY_LEN);
        assert_write_only(command);
    }
}

#[test]
fn color_bytes_land_in_red_green_blue_register_order() {
    let commands = ultimate().encode_frame(&[[0xFF, 0x00, 0x80]]);
    let color = &commands[0].data;

    assert_eq!(&color[2..4], &[0x11, 0x89], "custom color red register");
    assert_eq!(color[4], 0xFF, "red at 0x1189");
    assert_eq!(color[5], 0x00, "green at 0x118A");
    assert_eq!(color[6], 0x80, "blue at 0x118B");
    assert!(
        color[7..].iter().all(|byte| *byte == 0),
        "intensity register and padding stay untouched"
    );
}

#[test]
fn wire_reports_carry_output_report_id_and_match_report_length() {
    let commands = ultimate().encode_frame(&[[0x01, 0x02, 0x03]]);

    for command in &commands {
        let wire = encode_hidapi_packet_for_testing(
            &command.data,
            KRAKEN_OUTPUT_REPORT_ID,
            HidRawReportMode::OutputReport,
        );
        assert_eq!(wire.len(), KRAKEN_REPORT_LEN);
        assert_eq!(wire.len(), 37, "OpenRazer asserts a 37-byte request report");
        assert_eq!(wire[0], 0x04);
        assert_eq!(&wire[1..], command.data.as_slice());
    }
}

#[test]
fn empty_frame_pads_to_black() {
    let commands = ultimate().encode_frame(&[]);

    assert_eq!(commands.len(), 2);
    assert_eq!(commands[0].data, color_write([0, 0, 0]));
    assert_eq!(commands[1].data, effect_write());
}

#[test]
fn oversized_frame_keeps_only_the_first_color() {
    let commands = ultimate().encode_frame(&[[0x0A, 0x0B, 0x0C], [0xEE, 0xEE, 0xEE], [0x77; 3]]);

    assert_eq!(commands.len(), 2);
    assert_eq!(commands[0].data, color_write([0x0A, 0x0B, 0x0C]));
}

#[test]
fn encode_frame_into_rewrites_reused_buffers_in_place() {
    let protocol = ultimate();
    let mut commands = vec![ProtocolCommand::default(); 5];

    protocol.encode_frame_into(&[[0x11, 0x22, 0x33]], &mut commands);
    assert_eq!(
        commands.len(),
        2,
        "stale slots from a longer batch are dropped"
    );
    let first_ptrs: Vec<*const u8> = commands
        .iter()
        .map(|command| command.data.as_ptr())
        .collect();

    protocol.encode_frame_into(&[[0x44, 0x55, 0x66]], &mut commands);
    assert_eq!(commands.len(), 2);
    assert_eq!(commands[0].data, color_write([0x44, 0x55, 0x66]));
    assert_eq!(commands[1].data, effect_write());
    let second_ptrs: Vec<*const u8> = commands
        .iter()
        .map(|command| command.data.as_ptr())
        .collect();
    assert_eq!(first_ptrs, second_ptrs, "frame encoding reuses its buffers");
}

#[test]
fn encode_frame_into_matches_encode_frame() {
    let protocol = ultimate();
    let colors = [[0x90, 0xA0, 0xB0]];
    let mut commands = Vec::new();

    protocol.encode_frame_into(&colors, &mut commands);
    let allocated = protocol.encode_frame(&colors);

    assert_eq!(commands.len(), allocated.len());
    for (reused, fresh) in commands.iter().zip(&allocated) {
        assert_eq!(reused.data, fresh.data);
    }
}

#[test]
fn init_switches_the_led_to_static_on() {
    let init = ultimate().init_sequence();

    assert_eq!(init.len(), 1);
    assert_eq!(init[0].data, effect_write());
    assert_write_only(&init[0]);
}

#[test]
fn shutdown_leaves_the_last_frame_in_ram() {
    assert!(ultimate().shutdown_sequence().is_empty());
}

#[test]
fn diagnostics_read_firmware_version_from_eeprom() {
    let diagnostics = ultimate().connection_diagnostics();

    assert_eq!(diagnostics.len(), 1);
    let probe = &diagnostics[0];
    assert_eq!(probe.data, request_body(EEPROM_READ, 0x0030, &[], 2));
    assert!(probe.expects_response);
    assert_eq!(probe.transfer_type, TransferType::Primary);
    assert_eq!(probe.response.timeout, Some(Duration::from_millis(250)));
}

#[test]
fn parse_response_returns_bytes_after_input_report_id() {
    let mut report = vec![0_u8; KRAKEN_RESPONSE_LEN];
    report[0] = KRAKEN_INPUT_REPORT_ID;
    report[1] = 0x01;
    report[2] = 0x23;

    let parsed = ultimate()
        .parse_response(&report)
        .expect("input report 0x05 should parse");

    assert_eq!(parsed.data.len(), KRAKEN_RESPONSE_LEN - 1);
    assert_eq!(&parsed.data[..2], &[0x01, 0x23], "BCD firmware v1.23");
}

#[test]
fn parse_response_tolerates_padded_input_reports() {
    let mut report = vec![0_u8; KRAKEN_REPORT_LEN];
    report[0] = KRAKEN_INPUT_REPORT_ID;
    report[1] = 0x02;

    let parsed = ultimate()
        .parse_response(&report)
        .expect("a longer platform buffer should still parse");

    assert_eq!(parsed.data[0], 0x02);
}

#[test]
fn parse_response_rejects_other_input_reports() {
    let mut report = vec![0_u8; KRAKEN_RESPONSE_LEN];
    report[0] = 0x02;

    let error = ultimate()
        .parse_response(&report)
        .expect_err("a consumer-control report is not a memory read result");

    assert!(matches!(error, ProtocolError::MalformedResponse { .. }));
}

#[test]
fn parse_response_rejects_short_and_empty_reads() {
    let protocol = ultimate();

    for data in [&[][..], &[KRAKEN_INPUT_REPORT_ID, 0x01, 0x02][..]] {
        let error = protocol
            .parse_response(data)
            .expect_err("truncated reads must not parse");
        assert!(matches!(error, ProtocolError::MalformedResponse { .. }));
    }
}

#[test]
fn exposes_one_point_zone_for_both_earcups() {
    let protocol = ultimate();
    let zones = protocol.zones();

    assert_eq!(protocol.total_leds(), 1);
    assert_eq!(zones.len(), 1);
    assert_eq!(zones[0].name, "Earcups");
    assert_eq!(zones[0].led_count, 1);
    assert_eq!(zones[0].topology, DeviceTopologyHint::Point);
    assert_eq!(zones[0].color_format, DeviceColorFormat::Rgb);
    assert!(zones[0].layout_hint.is_none());
}

#[test]
fn capabilities_advertise_direct_color_without_hardware_brightness() {
    let protocol = ultimate();
    let capabilities = protocol.capabilities();

    assert_eq!(capabilities.led_count, 1);
    assert!(capabilities.supports_direct);
    assert!(!capabilities.supports_brightness);
    assert!(!capabilities.has_display);
    assert_eq!(protocol.frame_interval(), Duration::from_millis(2));
    assert_eq!(capabilities.max_fps, 500);
    assert!(protocol.encode_brightness(128).is_none());
    assert!(protocol.keepalive().is_none());
}

#[test]
fn protocol_reports_name_and_model() {
    let protocol = ultimate();

    assert_eq!(protocol.name(), "Razer Kraken");
    assert_eq!(protocol.model(), KrakenModel::Ultimate);
}

#[test]
fn database_binds_kraken_ultimate_to_output_reports_on_interface_three() {
    let descriptor = ProtocolDatabase::lookup(RAZER_VENDOR_ID, PID_KRAKEN_ULTIMATE)
        .expect("Kraken Ultimate descriptor should exist");

    assert_eq!(PID_KRAKEN_ULTIMATE, 0x0527);
    assert_eq!(descriptor.name, "Razer Kraken Ultimate");
    assert_eq!(
        descriptor.family,
        DeviceFamily::new_static("razer", "Razer")
    );
    assert_eq!(descriptor.protocol.id, "razer/kraken-ultimate");
    assert_eq!(descriptor.driver_id(), "razer");
    assert!(descriptor.firmware_predicate.is_none());
    assert_eq!(
        descriptor.transport,
        TransportType::UsbHidApi {
            interface: Some(3),
            report_id: 0x04,
            report_mode: HidRawReportMode::OutputReport,
            max_report_len: 37,
            usage_page: Some(0x000C),
            usage: Some(0x0001),
        }
    );

    let protocol = (descriptor.protocol.build)();
    assert_eq!(protocol.name(), "Razer Kraken");
    assert_eq!(protocol.total_leds(), 1);
}

#[test]
fn builder_matches_database_protocol() {
    let built = build_kraken_ultimate_protocol();
    let direct = ultimate();

    assert_eq!(
        built.encode_frame(&[[1, 2, 3]])[0].data,
        direct.encode_frame(&[[1, 2, 3]])[0].data
    );
}
