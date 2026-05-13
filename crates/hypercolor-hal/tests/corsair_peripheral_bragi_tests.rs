use std::time::Duration;

use hypercolor_hal::ProtocolDatabase;
use hypercolor_hal::drivers::corsair::peripheral::bragi::{
    CorsairBragiProtocol, build_property_get_packet_for_testing,
};
use hypercolor_hal::drivers::corsair::peripheral::devices::{
    BRAGI_INTERFACE, BRAGI_REPORT_ID, PID_K65_MINI, PID_K70_CORE_RGB_VARIANT_2,
    PID_K100_OPTICAL_V1, PID_SCIMITAR_ELITE_BRAGI,
};
use hypercolor_hal::drivers::corsair::peripheral::types::{
    BRAGI_JUMBO_PACKET_SIZE, BRAGI_LARGE_PACKET_SIZE, BRAGI_PACKET_SIZE, BragiCommand,
    BragiDeviceConfig, BragiHandle, BragiLightingFormat, BragiLightingMode, BragiProperty,
    BragiResource, CorsairPeripheralClass, CorsairPeripheralTopology,
};
use hypercolor_hal::drivers::corsair::{CORSAIR_USAGE_PAGE, CORSAIR_VID};
use hypercolor_hal::protocol::Protocol;
use hypercolor_hal::protocol::{ProtocolError, ResponseStatus};
use hypercolor_hal::registry::{HidRawReportMode, TransportType};
use hypercolor_types::device::DeviceTopologyHint;

fn bragi_config(
    packet_size: usize,
    led_count: usize,
    lighting_format: BragiLightingFormat,
) -> BragiDeviceConfig {
    let config = BragiDeviceConfig::new(
        "Test Bragi",
        CorsairPeripheralClass::Keyboard,
        packet_size,
        led_count,
        CorsairPeripheralTopology::KeyboardMatrix { rows: 1, cols: 8 },
    );

    match lighting_format {
        BragiLightingFormat::RgbPlanar => config,
        BragiLightingFormat::Monochrome => config.monochrome(),
        BragiLightingFormat::AlternateRgb => config.alternate_rgb(),
    }
}

fn bragi_protocol(
    packet_size: usize,
    led_count: usize,
    lighting_format: BragiLightingFormat,
) -> CorsairBragiProtocol {
    CorsairBragiProtocol::new(bragi_config(packet_size, led_count, lighting_format))
}

#[test]
fn property_get_packet_is_padded_to_report_size() {
    let packet = build_property_get_packet_for_testing(BRAGI_PACKET_SIZE, BragiProperty::Mode);

    assert_eq!(packet.len(), BRAGI_PACKET_SIZE);
    assert_eq!(
        &packet[..4],
        &[
            BRAGI_REPORT_ID,
            BragiCommand::Get as u8,
            BragiProperty::Mode as u8,
            0x00,
        ]
    );
    assert!(packet[4..].iter().all(|byte| *byte == 0));
}

#[test]
fn init_sets_software_mode_and_opens_lighting_handle() {
    let protocol = bragi_protocol(BRAGI_PACKET_SIZE, 8, BragiLightingFormat::RgbPlanar);
    let commands = protocol.init_sequence();

    assert_eq!(commands.len(), 2);
    assert!(commands.iter().all(|command| command.expects_response));
    assert!(commands.iter().all(|command| command.data.len() == 64));

    assert_eq!(commands[0].data[0], BRAGI_REPORT_ID);
    assert_eq!(commands[0].data[1], BragiCommand::Set as u8);
    assert_eq!(commands[0].data[2], BragiProperty::Mode as u8);
    assert_eq!(
        &commands[0].data[4..6],
        &(BragiLightingMode::Software as u16).to_le_bytes()
    );

    assert_eq!(commands[1].data[1], BragiCommand::OpenHandle as u8);
    assert_eq!(commands[1].data[2], BragiHandle::Lighting as u8);
    assert_eq!(
        &commands[1].data[3..5],
        &(BragiResource::Lighting as u16).to_le_bytes()
    );
}

#[test]
fn shutdown_closes_lighting_handle_and_restores_hardware_mode() {
    let protocol = bragi_protocol(BRAGI_PACKET_SIZE, 8, BragiLightingFormat::RgbPlanar);
    let commands = protocol.shutdown_sequence();

    assert_eq!(commands.len(), 2);
    assert_eq!(commands[0].data[1], BragiCommand::CloseHandle as u8);
    assert_eq!(commands[0].data[2], 0x01);
    assert_eq!(commands[0].data[3], BragiHandle::Lighting as u8);

    assert_eq!(commands[1].data[1], BragiCommand::Set as u8);
    assert_eq!(commands[1].data[2], BragiProperty::Mode as u8);
    assert_eq!(
        &commands[1].data[4..6],
        &(BragiLightingMode::Hardware as u16).to_le_bytes()
    );
}

#[test]
fn planar_rgb_payload_is_red_green_blue_planes() {
    let protocol = bragi_protocol(BRAGI_PACKET_SIZE, 3, BragiLightingFormat::RgbPlanar);
    let colors = [[1_u8, 2, 3], [4, 5, 6], [7, 8, 9]];
    let commands = protocol.encode_frame(&colors);

    assert_eq!(commands[0].data[1], BragiCommand::WriteData as u8);
    assert_eq!(&commands[0].data[3..7], &9_u32.to_le_bytes());
    assert_eq!(&commands[0].data[7..16], &[1, 4, 7, 2, 5, 8, 3, 6, 9]);
}

#[test]
fn standard_64_byte_frame_chunks_large_keyboard_payload() {
    let protocol = bragi_protocol(BRAGI_PACKET_SIZE, 123, BragiLightingFormat::RgbPlanar);
    let colors = vec![[0_u8, 0, 0]; 123];
    let commands = protocol.encode_frame(&colors);

    assert_eq!(commands.len(), 7);
    assert_eq!(commands[0].data.len(), BRAGI_PACKET_SIZE);
    assert_eq!(commands[0].data[1], BragiCommand::WriteData as u8);
    assert_eq!(commands[0].data[2], BragiHandle::Lighting as u8);
    assert_eq!(&commands[0].data[3..7], &369_u32.to_le_bytes());
    assert_eq!(commands[1].data[1], BragiCommand::ContinueWrite as u8);
    assert_eq!(commands[6].data[1], BragiCommand::ContinueWrite as u8);
}

#[test]
fn monochrome_payload_uses_max_channel_and_monochrome_resource() {
    let protocol = bragi_protocol(BRAGI_PACKET_SIZE, 3, BragiLightingFormat::Monochrome);
    let init = protocol.init_sequence();
    assert_eq!(
        &init[1].data[3..5],
        &(BragiResource::LightingMonochrome as u16).to_le_bytes()
    );

    let colors = [[1_u8, 20, 3], [40, 5, 6], [7, 8, 90]];
    let commands = protocol.encode_frame(&colors);
    assert_eq!(&commands[0].data[3..7], &3_u32.to_le_bytes());
    assert_eq!(&commands[0].data[7..10], &[20, 40, 90]);
}

#[test]
fn alternate_rgb_payload_has_resource_header_and_rgb_triples() {
    let protocol = bragi_protocol(BRAGI_PACKET_SIZE, 2, BragiLightingFormat::AlternateRgb);
    let init = protocol.init_sequence();
    assert_eq!(
        &init[1].data[3..5],
        &(BragiResource::AlternateLighting as u16).to_le_bytes()
    );

    let colors = [[1_u8, 2, 3], [4, 5, 6]];
    let commands = protocol.encode_frame(&colors);
    assert_eq!(&commands[0].data[3..7], &8_u32.to_le_bytes());
    assert_eq!(&commands[0].data[7..15], &[0x12, 0x00, 1, 2, 3, 4, 5, 6]);
}

#[test]
fn brightness_commands_are_emitted_after_black_state_transitions() {
    let protocol = bragi_protocol(BRAGI_PACKET_SIZE, 1, BragiLightingFormat::RgbPlanar);

    let turn_on = protocol.encode_frame(&[[1_u8, 0, 0]]);
    let on_command = turn_on
        .last()
        .expect("nonzero frame should restore brightness");
    assert_eq!(on_command.data[1], BragiCommand::Set as u8);
    assert_eq!(on_command.data[2], BragiProperty::Brightness as u8);
    assert_eq!(&on_command.data[4..6], &1_000_u16.to_le_bytes());

    let turn_off = protocol.encode_frame(&[[0_u8, 0, 0]]);
    let off_command = turn_off
        .last()
        .expect("black frame should disable brightness");
    assert_eq!(off_command.data[1], BragiCommand::Set as u8);
    assert_eq!(off_command.data[2], BragiProperty::Brightness as u8);
    assert_eq!(&off_command.data[4..6], &0_u16.to_le_bytes());
}

#[test]
fn keepalive_uses_50_second_poll_packet() {
    let protocol = bragi_protocol(BRAGI_PACKET_SIZE, 8, BragiLightingFormat::RgbPlanar);
    let keepalive = protocol.keepalive().expect("Bragi should keep alive");

    assert_eq!(keepalive.interval, Duration::from_secs(50));
    assert_eq!(keepalive.commands.len(), 1);
    assert_eq!(keepalive.commands[0].data[0], BRAGI_REPORT_ID);
    assert_eq!(keepalive.commands[0].data[1], BragiCommand::Poll as u8);
}

#[test]
fn parse_response_accepts_zero_prefixed_hidapi_ack() {
    let protocol = bragi_protocol(BRAGI_PACKET_SIZE, 8, BragiLightingFormat::RgbPlanar);
    let mut response = vec![0_u8; BRAGI_PACKET_SIZE];
    response[1] = BragiCommand::Set as u8;

    let parsed = protocol
        .parse_response(&response)
        .expect("zero-prefixed Bragi ACK should parse");

    assert_eq!(parsed.status, ResponseStatus::Ok);
    assert!(parsed.data.is_empty());
}

#[test]
fn parse_response_accepts_magic_prefixed_ack() {
    let protocol = bragi_protocol(BRAGI_PACKET_SIZE, 8, BragiLightingFormat::RgbPlanar);
    let mut response = vec![0_u8; BRAGI_PACKET_SIZE];
    response[0] = BRAGI_REPORT_ID;
    response[1] = BragiCommand::Set as u8;

    let parsed = protocol
        .parse_response(&response)
        .expect("magic-prefixed Bragi ACK should parse");

    assert_eq!(parsed.status, ResponseStatus::Ok);
    assert!(parsed.data.is_empty());
}

#[test]
fn parse_response_maps_bragi_error_statuses() {
    let protocol = bragi_protocol(BRAGI_PACKET_SIZE, 8, BragiLightingFormat::RgbPlanar);
    let mut response = vec![0_u8; BRAGI_PACKET_SIZE];
    response[1] = BragiCommand::OpenHandle as u8;

    response[2] = 0x03;
    let busy = protocol
        .parse_response(&response)
        .expect("busy Bragi status should parse");
    assert_eq!(busy.status, ResponseStatus::Busy);

    response[2] = 0x05;
    let unsupported = protocol
        .parse_response(&response)
        .expect("unsupported Bragi status should parse");
    assert_eq!(unsupported.status, ResponseStatus::Unsupported);
}

#[test]
fn parse_response_rejects_failed_and_short_replies() {
    let protocol = bragi_protocol(BRAGI_PACKET_SIZE, 8, BragiLightingFormat::RgbPlanar);
    let mut response = vec![0_u8; BRAGI_PACKET_SIZE];
    response[1] = BragiCommand::Set as u8;
    response[2] = 0x7F;

    assert!(matches!(
        protocol.parse_response(&response),
        Err(ProtocolError::DeviceError {
            status: ResponseStatus::Failed
        })
    ));
    assert!(matches!(
        protocol.parse_response(&[0_u8, BragiCommand::Set as u8]),
        Err(ProtocolError::MalformedResponse { .. })
    ));
}

#[test]
fn encode_frame_into_reuses_existing_command_buffers() {
    let protocol = bragi_protocol(BRAGI_PACKET_SIZE, 123, BragiLightingFormat::RgbPlanar);
    let colors = vec![[0_u8, 0, 0]; 123];
    let mut commands = Vec::new();

    protocol.encode_frame_into(&colors, &mut commands);
    let first_pointers = commands
        .iter()
        .map(|command| command.data.as_ptr())
        .collect::<Vec<_>>();
    let first_capacities = commands
        .iter()
        .map(|command| command.data.capacity())
        .collect::<Vec<_>>();

    protocol.encode_frame_into(&colors, &mut commands);

    assert_eq!(commands.len(), 7);
    assert_eq!(
        first_pointers,
        commands
            .iter()
            .map(|command| command.data.as_ptr())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        first_capacities,
        commands
            .iter()
            .map(|command| command.data.capacity())
            .collect::<Vec<_>>()
    );
}

#[test]
fn k70_core_variant_2_is_registered_with_safe_hidapi_transport() {
    let descriptor = ProtocolDatabase::lookup(CORSAIR_VID, PID_K70_CORE_RGB_VARIANT_2)
        .expect("Corsair K70 Core RGB descriptor should exist");

    assert_eq!(descriptor.name, "Corsair K70 Core RGB");
    assert_eq!(descriptor.protocol.id, "corsair/bragi-k70-core-rgb-v2");
    assert_eq!(
        descriptor.transport,
        TransportType::UsbHidApi {
            interface: Some(BRAGI_INTERFACE),
            report_id: BRAGI_REPORT_ID,
            report_mode: HidRawReportMode::OutputReportWithReportId,
            max_report_len: BRAGI_PACKET_SIZE,
            usage_page: Some(CORSAIR_USAGE_PAGE),
            usage: None,
        }
    );

    let protocol = (descriptor.protocol.build)();
    assert_eq!(protocol.name(), "Corsair K70 Core RGB");
    assert_eq!(protocol.total_leds(), 123);
    assert_eq!(protocol.frame_interval(), Duration::from_millis(33));
    assert_eq!(
        protocol.zones()[0].topology,
        DeviceTopologyHint::Matrix { rows: 6, cols: 21 }
    );
}

#[test]
fn k100_uses_jumbo_alternate_rgb_packets() {
    let descriptor = ProtocolDatabase::lookup(CORSAIR_VID, PID_K100_OPTICAL_V1)
        .expect("Corsair K100 RGB Optical descriptor should exist");

    assert_eq!(descriptor.name, "Corsair K100 RGB Optical");
    assert_eq!(
        descriptor.transport,
        TransportType::UsbHidApi {
            interface: Some(BRAGI_INTERFACE),
            report_id: BRAGI_REPORT_ID,
            report_mode: HidRawReportMode::OutputReportWithReportId,
            max_report_len: BRAGI_JUMBO_PACKET_SIZE,
            usage_page: Some(CORSAIR_USAGE_PAGE),
            usage: None,
        }
    );

    let protocol = (descriptor.protocol.build)();
    let colors = [[1_u8, 2, 3]; 193];
    let commands = protocol.encode_frame(&colors);

    assert_eq!(protocol.total_leds(), 193);
    assert_eq!(&commands[0].data[3..7], &581_u32.to_le_bytes());
    assert_eq!(&commands[0].data[7..9], &[0x12, 0x00]);
}

#[test]
fn jumbo_and_large_bragi_descriptors_report_their_packet_sizes() {
    let k65 = ProtocolDatabase::lookup(CORSAIR_VID, PID_K65_MINI)
        .expect("K65 Mini descriptor should exist");
    let scimitar = ProtocolDatabase::lookup(CORSAIR_VID, PID_SCIMITAR_ELITE_BRAGI)
        .expect("Scimitar Elite descriptor should exist");

    assert_eq!(
        k65.transport,
        TransportType::UsbHidApi {
            interface: Some(BRAGI_INTERFACE),
            report_id: BRAGI_REPORT_ID,
            report_mode: HidRawReportMode::OutputReportWithReportId,
            max_report_len: BRAGI_JUMBO_PACKET_SIZE,
            usage_page: Some(CORSAIR_USAGE_PAGE),
            usage: None,
        }
    );
    assert_eq!(
        scimitar.transport,
        TransportType::UsbHidApi {
            interface: Some(BRAGI_INTERFACE),
            report_id: BRAGI_REPORT_ID,
            report_mode: HidRawReportMode::OutputReportWithReportId,
            max_report_len: BRAGI_LARGE_PACKET_SIZE,
            usage_page: Some(CORSAIR_USAGE_PAGE),
            usage: None,
        }
    );
}
