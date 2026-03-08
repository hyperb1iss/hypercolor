use hypercolor_hal::drivers::razer::{
    LED_ID_BACKLIGHT, LED_ID_LOGO, RAZER_REPORT_LEN, RazerLightingCommandSet, RazerMatrixType,
    RazerProtocol, RazerProtocolVersion, build_basilisk_v3_protocol, build_blade_14_2021_protocol,
    build_blade_15_late_2021_advanced_protocol, build_huntsman_v2_protocol,
    build_seiren_v3_protocol, razer_crc,
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
    assert_eq!(RazerProtocolVersion::Special08.transaction_id(), 0x08);
    assert_eq!(RazerProtocolVersion::KrakenV4.transaction_id(), 0x60);
}

#[test]
fn encode_extended_matrix_splits_row_chunks_and_adds_activation() {
    let protocol = RazerProtocol::new(
        RazerProtocolVersion::Extended,
        RazerLightingCommandSet::Extended,
        RazerMatrixType::Extended,
        (1, 23),
        LED_ID_BACKLIGHT,
    );

    let colors = (0_u8..23)
        .map(|index| [index, index, index])
        .collect::<Vec<_>>();

    let commands = protocol.encode_frame(&colors);
    assert_eq!(commands.len(), 3, "2 row chunks + activation command");
    assert!(commands.iter().all(|command| command.expects_response));

    // First frame packet (22 LEDs)
    let first = &commands[0].data;
    assert_eq!(first[1], 0x3F);
    assert_eq!(first[6], 0x0F);
    assert_eq!(first[7], 0x03);
    assert_eq!(first[5], 0x47);
    assert_eq!(first[10], 0x00);
    assert_eq!(first[11], 0x00);
    assert_eq!(first[12], 0x15);

    // Second frame packet (1 LED)
    let second = &commands[1].data;
    assert_eq!(second[5], 0x08);
    assert_eq!(second[10], 0x00);
    assert_eq!(second[11], 0x16);
    assert_eq!(second[12], 0x16);

    // Activation packet
    let activation = &commands[2].data;
    assert_eq!(activation[5], 0x06);
    assert_eq!(activation[6], 0x0F);
    assert_eq!(activation[7], 0x02);
    assert_eq!(activation[8], 0x00);
    assert_eq!(activation[9], 0x00);
    assert_eq!(activation[10], 0x08);
    assert_eq!(activation[11], 0x00);
    assert_eq!(activation[12], 0x01);
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
    assert!(commands.iter().all(|command| command.expects_response));

    let frame = &commands[0].data;
    assert_eq!(frame[1], 0x1F);
    assert_eq!(frame[5], 0x46);
    assert_eq!(frame[6], 0x03);
    assert_eq!(frame[7], 0x0B);
    assert_eq!(frame[8], 0xFF);

    let activation = &commands[1].data;
    assert_eq!(activation[1], 0x1F);
    assert_eq!(activation[6], 0x03);
    assert_eq!(activation[7], 0x0A);
}

#[test]
fn basilisk_v3_protocol_uses_fixed_length_matrix_packets() {
    let protocol = build_basilisk_v3_protocol();
    let colors = (0_u8..11)
        .map(|index| [index, index.saturating_add(1), index.saturating_add(2)])
        .collect::<Vec<_>>();

    let init = protocol.init_sequence();
    assert_eq!(init.len(), 1, "custom effect activation only");
    assert_eq!(init[0].data[5], 0x06);
    assert_eq!(init[0].data[6], 0x0F);
    assert_eq!(init[0].data[7], 0x02);
    assert!(!init[0].expects_response);
    assert_eq!(init[0].post_delay, std::time::Duration::from_millis(10));
    assert_eq!(init[0].data[11], 0x00);
    assert_eq!(init[0].data[12], 0x01);

    let commands = protocol.encode_frame(&colors);
    assert_eq!(commands.len(), 1, "single frame chunk");
    assert!(commands.iter().all(|command| !command.expects_response));
    assert!(
        commands
            .iter()
            .all(|command| command.response_delay == std::time::Duration::ZERO)
    );

    let frame = &commands[0].data;
    assert_eq!(frame[1], 0x1F);
    assert_eq!(frame[5], 0x26);
    assert_eq!(frame[6], 0x0F);
    assert_eq!(frame[7], 0x03);
    assert_eq!(frame[10], 0x00);
    assert_eq!(frame[11], 0x00);
    assert_eq!(frame[12], 0x0A);
}

#[test]
fn huntsman_v2_protocol_initializes_custom_mode_once_and_streams_write_only_frames() {
    let protocol = build_huntsman_v2_protocol();
    let colors = vec![[0x12, 0x34, 0x56]; 6 * 22];

    let init = protocol.init_sequence();
    assert_eq!(init.len(), 2, "device mode + custom effect activation");
    assert_eq!(init[0].data[6], 0x00);
    assert_eq!(init[0].data[7], 0x04);
    assert_eq!(init[1].data[6], 0x0F);
    assert_eq!(init[1].data[7], 0x02);

    let commands = protocol.encode_frame(&colors);
    assert_eq!(commands.len(), 6, "one write-only row packet per row");
    assert!(commands.iter().all(|command| !command.expects_response));
    assert!(commands.iter().all(|command| command.data[6] == 0x0F));
    assert!(commands.iter().all(|command| command.data[7] == 0x03));
}

#[test]
fn seiren_v3_protocol_uses_report_id_07_payload_shape() {
    let protocol = build_seiren_v3_protocol();
    let init = protocol.init_sequence();
    assert_eq!(init.len(), 2);
    assert!(init.iter().all(|command| !command.expects_response));
    assert_eq!(init[0].data.len(), 63);
    assert_eq!(init[0].data[1], 0x1F);
    assert_eq!(init[0].data[5], 0x02);
    assert_eq!(init[0].data[6], 0x00);
    assert_eq!(init[0].data[7], 0x04);
    assert_eq!(init[0].data[8], 0x03);
    assert_eq!(init[1].data[5], 0x06);
    assert_eq!(init[1].data[6], 0x0F);
    assert_eq!(init[1].data[7], 0x02);
    assert_eq!(init[1].data[10], 0x08);
    assert_eq!(init[1].data[12], 0x01);

    let colors = vec![
        [1, 0, 0],
        [2, 0, 0],
        [3, 0, 0],
        [4, 0, 0],
        [5, 0, 0],
        [6, 0, 0],
        [7, 0, 0],
        [8, 0, 0],
        [9, 0, 0],
        [10, 0, 0],
    ];
    let frame = protocol.encode_frame(&colors);
    assert_eq!(frame.len(), 1);
    assert_eq!(frame[0].data.len(), 63);
    assert_eq!(frame[0].data[5], 0x23);
    assert_eq!(frame[0].data[6], 0x0F);
    assert_eq!(frame[0].data[7], 0x03);
    assert_eq!(&frame[0].data[8..12], &[0x00, 0x00, 0x00, 0x09]);
    assert_eq!(&frame[0].data[12..15], &[8, 0, 0]);
    assert_eq!(&frame[0].data[15..18], &[6, 0, 0]);
    assert_eq!(&frame[0].data[18..21], &[5, 0, 0]);
    assert_eq!(&frame[0].data[39..42], &[4, 0, 0]);
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
    assert!(standard_cmd[0].expects_response);
    assert_eq!(standard_cmd[0].data[1], 0x1F);
    assert_eq!(standard_cmd[0].data[6], 0x03);
    assert_eq!(standard_cmd[0].data[7], 0x03);

    let extended_cmd = extended
        .encode_brightness(0x55)
        .expect("extended brightness should encode");
    assert_eq!(extended_cmd.len(), 1);
    assert!(extended_cmd[0].expects_response);
    assert_eq!(extended_cmd[0].data[1], 0x3F);
    assert_eq!(extended_cmd[0].data[6], 0x0F);
    assert_eq!(extended_cmd[0].data[7], 0x04);
}

#[test]
fn encode_brightness_can_target_a_different_led_id() {
    let protocol = RazerProtocol::new(
        RazerProtocolVersion::Legacy,
        RazerLightingCommandSet::Standard,
        RazerMatrixType::Standard,
        (6, 22),
        LED_ID_BACKLIGHT,
    )
    .with_brightness_led_id(LED_ID_LOGO);

    let commands = protocol
        .encode_brightness(0x33)
        .expect("brightness should encode");

    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].data[1], 0xFF);
    assert_eq!(commands[0].data[6], 0x03);
    assert_eq!(commands[0].data[7], 0x03);
    assert_eq!(commands[0].data[9], LED_ID_LOGO);
    assert_eq!(commands[0].data[10], 0x33);
}

#[test]
fn blade_protocol_matches_uchroma_laptop_path() {
    let protocol = build_blade_15_late_2021_advanced_protocol();

    assert_eq!(protocol.name(), "Razer 0x1F Standard");
    assert!(protocol.init_sequence().is_empty());
    assert!(protocol.shutdown_sequence().is_empty());

    let colors = vec![[0xFF, 0x06, 0xB5]; 96];
    let commands = protocol.encode_frame(&colors);
    assert_eq!(commands.len(), 7, "6 rows + custom-mode activation");
    assert!(
        commands[..6]
            .iter()
            .all(|command| !command.expects_response)
    );
    assert!(commands[6].expects_response);
    assert!(
        commands
            .iter()
            .all(|command| command.response_delay == std::time::Duration::ZERO)
    );

    let first_row = &commands[0].data;
    assert_eq!(first_row[1], 0xFF);
    assert_eq!(first_row[6], 0x03);
    assert_eq!(first_row[7], 0x0B);
    assert_eq!(first_row[8], 0xFF);
    assert_eq!(first_row[9], 0x00);
    assert_eq!(first_row[10], 0x00);
    assert_eq!(first_row[11], 0x0F);

    let activation = &commands[6].data;
    assert_eq!(activation[1], 0x1F);
    assert_eq!(activation[6], 0x03);
    assert_eq!(activation[7], 0x0A);
    assert_eq!(activation[8], 0x05);
    assert_eq!(activation[9], 0x01);

    let brightness = protocol
        .encode_brightness(0x7F)
        .expect("blade brightness should encode");
    assert_eq!(brightness.len(), 1);
    assert_eq!(brightness[0].data[1], 0x1F);
    assert_eq!(brightness[0].data[6], 0x03);
    assert_eq!(brightness[0].data[7], 0x03);
    assert_eq!(brightness[0].data[8], 0x01);
    assert_eq!(brightness[0].data[9], 0x05);
    assert_eq!(brightness[0].data[10], 0x7F);
}

#[test]
fn blade_14_2021_protocol_emits_keepalive_device_mode_query() {
    let protocol = build_blade_14_2021_protocol();
    let keepalive = protocol
        .keepalive()
        .expect("Blade 14 keepalive should exist");

    assert_eq!(keepalive.interval, std::time::Duration::from_millis(2_500));
    assert_eq!(keepalive.commands.len(), 1);

    let command = &keepalive.commands[0];
    assert!(command.expects_response);
    assert_eq!(command.data[1], 0x3F);
    assert_eq!(command.data[5], 0x02);
    assert_eq!(command.data[6], 0x00);
    assert_eq!(command.data[7], 0x84);
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
fn parse_response_accepts_crc_mismatch() {
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

    let parsed = protocol
        .parse_response(&report)
        .expect("CRC mismatch should not reject otherwise valid response");

    assert_eq!(parsed.status, ResponseStatus::Ok);
    assert_eq!(parsed.data, vec![0xAA]);
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
