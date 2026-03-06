use hypercolor_hal::drivers::razer::{
    LED_ID_BACKLIGHT, RAZER_REPORT_LEN, RazerLightingCommandSet, RazerMatrixType, RazerProtocol,
    RazerProtocolVersion, razer_crc,
};
use hypercolor_hal::protocol::{Protocol, ProtocolError, ResponseStatus};
use hypercolor_types::device::DeviceTopologyHint;

#[test]
fn crc_uses_expected_byte_window() {
    let mut packet = [0_u8; RAZER_REPORT_LEN];
    packet[1] = 0x12;
    packet[86] = 0x34;
    packet[87] = 0xFF; // Out of CRC range

    assert_eq!(razer_crc(&packet), 0x12 ^ 0x34);
}

#[test]
fn protocol_version_transaction_id_mapping() {
    assert_eq!(RazerProtocolVersion::Legacy.transaction_id(), 0xFF);
    assert_eq!(RazerProtocolVersion::Extended.transaction_id(), 0x3F);
    assert_eq!(RazerProtocolVersion::Modern.transaction_id(), 0x1F);
    assert_eq!(RazerProtocolVersion::WirelessKb.transaction_id(), 0x9F);
}

#[test]
fn encode_extended_matrix_splits_row_chunks_and_adds_activation() {
    let protocol = RazerProtocol::new(
        RazerProtocolVersion::Extended,
        RazerLightingCommandSet::Extended,
        RazerMatrixType::Extended,
        (1, 26),
        LED_ID_BACKLIGHT,
    );

    let colors = (0_u8..26)
        .map(|index| [index, index, index])
        .collect::<Vec<_>>();

    let commands = protocol.encode_frame(&colors);
    assert_eq!(commands.len(), 3, "2 row chunks + activation command");

    // First frame packet (25 LEDs)
    let first = &commands[0].data;
    assert_eq!(first[1], 0x3F);
    assert_eq!(first[6], 0x0F);
    assert_eq!(first[7], 0x03);
    assert_eq!(first[5], 80); // 5-byte header + 25 * RGB

    // Second frame packet (1 LED)
    let second = &commands[1].data;
    assert_eq!(second[5], 8); // 5-byte header + 1 * RGB

    // Activation packet
    let activation = &commands[2].data;
    assert_eq!(activation[6], 0x0F);
    assert_eq!(activation[7], 0x02);
    assert_eq!(activation[8], 0x00);
    assert_eq!(activation[9], 0x00);
    assert_eq!(activation[10], 0x08);
}

#[test]
fn encode_standard_matrix_supports_modern_transaction_ids() {
    let protocol = RazerProtocol::new(
        RazerProtocolVersion::Modern,
        RazerLightingCommandSet::Standard,
        RazerMatrixType::Standard,
        (1, 2),
        LED_ID_BACKLIGHT,
    );

    let commands = protocol.encode_frame(&[[1, 2, 3], [4, 5, 6]]);
    assert_eq!(commands.len(), 2, "frame packet + activation");
    assert_eq!(protocol.name(), "Razer 0x1F Standard");

    let frame = &commands[0].data;
    assert_eq!(frame[1], 0x1F);
    assert_eq!(frame[6], 0x03);
    assert_eq!(frame[7], 0x0B);
    assert_eq!(frame[8], 0xFF);

    let activation = &commands[1].data;
    assert_eq!(activation[1], 0x1F);
    assert_eq!(activation[6], 0x03);
    assert_eq!(activation[7], 0x0A);
}

#[test]
fn encode_brightness_uses_command_family_specific_packets() {
    let standard = RazerProtocol::new(
        RazerProtocolVersion::Modern,
        RazerLightingCommandSet::Standard,
        RazerMatrixType::Standard,
        (1, 1),
        LED_ID_BACKLIGHT,
    );
    let extended = RazerProtocol::new(
        RazerProtocolVersion::Extended,
        RazerLightingCommandSet::Extended,
        RazerMatrixType::Extended,
        (1, 1),
        LED_ID_BACKLIGHT,
    );

    let standard_cmd = standard
        .encode_brightness(0x7F)
        .expect("standard brightness should encode");
    assert_eq!(standard_cmd.len(), 1);
    assert_eq!(standard_cmd[0].data[1], 0x1F);
    assert_eq!(standard_cmd[0].data[6], 0x03);
    assert_eq!(standard_cmd[0].data[7], 0x03);

    let extended_cmd = extended
        .encode_brightness(0x55)
        .expect("extended brightness should encode");
    assert_eq!(extended_cmd.len(), 1);
    assert_eq!(extended_cmd[0].data[1], 0x3F);
    assert_eq!(extended_cmd[0].data[6], 0x0F);
    assert_eq!(extended_cmd[0].data[7], 0x04);
}

#[test]
fn reported_matrix_size_overrides_user_visible_topology() {
    let protocol = RazerProtocol::new(
        RazerProtocolVersion::Extended,
        RazerLightingCommandSet::Extended,
        RazerMatrixType::Extended,
        (4, 16),
        LED_ID_BACKLIGHT,
    )
    .with_reported_matrix_size((8, 8));

    let zones = protocol.zones();
    assert_eq!(zones.len(), 1);
    assert_eq!(protocol.total_leds(), 64);
    match &zones[0].topology {
        DeviceTopologyHint::Matrix { rows, cols } => assert_eq!((*rows, *cols), (8, 8)),
        other => panic!("expected matrix topology, got {other:?}"),
    }
}

#[test]
fn parse_response_reads_payload_on_success() {
    let protocol = RazerProtocol::new(
        RazerProtocolVersion::Extended,
        RazerLightingCommandSet::Extended,
        RazerMatrixType::Extended,
        (1, 1),
        LED_ID_BACKLIGHT,
    );

    let mut report = [0_u8; RAZER_REPORT_LEN];
    report[0] = 0x02; // Ok
    report[1] = 0x3F;
    report[5] = 3;
    report[6] = 0x00;
    report[7] = 0x81;
    report[8] = 0xAA;
    report[9] = 0xBB;
    report[10] = 0xCC;
    report[88] = razer_crc(&report);

    let parsed = protocol
        .parse_response(&report)
        .expect("response should parse");

    assert_eq!(parsed.status, ResponseStatus::Ok);
    assert_eq!(parsed.data, vec![0xAA, 0xBB, 0xCC]);
}

#[test]
fn parse_response_rejects_crc_mismatch() {
    let protocol = RazerProtocol::new(
        RazerProtocolVersion::Extended,
        RazerLightingCommandSet::Extended,
        RazerMatrixType::Extended,
        (1, 1),
        LED_ID_BACKLIGHT,
    );

    let mut report = [0_u8; RAZER_REPORT_LEN];
    report[0] = 0x02;
    report[1] = 0x3F;
    report[5] = 1;
    report[8] = 0xAA;
    report[88] = 0x00;

    let error = protocol
        .parse_response(&report)
        .expect_err("crc mismatch should fail");

    match error {
        ProtocolError::CrcMismatch { .. } => {}
        other => panic!("expected CRC mismatch, got {other:?}"),
    }
}

#[test]
fn parse_response_propagates_device_failure() {
    let protocol = RazerProtocol::new(
        RazerProtocolVersion::Extended,
        RazerLightingCommandSet::Extended,
        RazerMatrixType::Extended,
        (1, 1),
        LED_ID_BACKLIGHT,
    );

    let mut report = [0_u8; RAZER_REPORT_LEN];
    report[0] = 0x03; // Fail
    report[1] = 0x3F;
    report[88] = razer_crc(&report);

    let error = protocol
        .parse_response(&report)
        .expect_err("failed status should error");

    match error {
        ProtocolError::DeviceError {
            status: ResponseStatus::Failed,
        } => {}
        other => panic!("expected device failure status, got {other:?}"),
    }
}
