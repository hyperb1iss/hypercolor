use hypercolor_hal::drivers::lianli::{
    ENE_COMMAND_DELAY, Ene6k77Protocol, LegacyUniHubProtocol, LianLiHubVariant, TlFanProtocol,
    apply_al_white_limit, apply_sum_white_limit, firmware_version_from_fine_tune,
};
use hypercolor_hal::protocol::{Protocol, TransferType};
use hypercolor_hal::transport::vendor::{VendorControlOperation, decode_operations};

#[test]
fn sl_frame_encodes_activate_color_commit_and_frame_commit() {
    let protocol =
        Ene6k77Protocol::new(LianLiHubVariant::Sl).with_fan_counts([2, 0, 0, 0, 0, 0, 0, 0]);
    let colors = vec![[10, 20, 30]; 32];

    let commands = protocol.encode_frame(&colors);
    assert_eq!(commands.len(), 4);

    assert_eq!(commands[0].transfer_type, TransferType::HidReport);
    assert_eq!(&commands[0].data[..4], &[0xE0, 0x10, 0x32, 0x02]);

    assert_eq!(commands[1].transfer_type, TransferType::Primary);
    assert_eq!(commands[1].post_delay, ENE_COMMAND_DELAY);
    assert_eq!(commands[1].data.len(), 98);
    assert_eq!(&commands[1].data[..5], &[0xE0, 0x30, 10, 30, 20]);

    assert_eq!(commands[2].transfer_type, TransferType::HidReport);
    assert_eq!(
        &commands[2].data[..6],
        &[0xE0, 0x10, 0x01, 0x02, 0x00, 0x00]
    );

    assert_eq!(commands[3].transfer_type, TransferType::HidReport);
    assert_eq!(&commands[3].data[..4], &[0xE0, 0x60, 0x00, 0x01]);
}

#[test]
fn slv2_color_packet_uses_sum_white_limit_and_fixed_353_bytes() {
    let protocol =
        Ene6k77Protocol::new(LianLiHubVariant::SlV2).with_fan_counts([6, 0, 0, 0, 0, 0, 0, 0]);
    let colors = vec![[200, 200, 200]; 96];

    let commands = protocol.encode_frame(&colors);
    assert_eq!(commands.len(), 4);
    assert_eq!(commands[1].data.len(), 353);
    assert_eq!(&commands[1].data[..5], &[0xE0, 0x30, 153, 153, 153]);
}

#[test]
fn al_frame_splits_inner_and_outer_rings_into_separate_sequences() {
    let protocol =
        Ene6k77Protocol::new(LianLiHubVariant::Al).with_fan_counts([1, 0, 0, 0, 0, 0, 0, 0]);
    let mut colors = vec![[0, 0, 0]; 20];
    colors[0] = [10, 20, 30];
    colors[8] = [200, 200, 200];

    let commands = protocol.encode_frame(&colors);
    assert_eq!(commands.len(), 7);

    assert_eq!(&commands[0].data[..5], &[0xE0, 0x10, 0x40, 0x01, 0x01]);
    assert_eq!(commands[1].data.len(), 146);
    assert_eq!(commands[1].data[1], 0x30);
    assert_eq!(&commands[1].data[2..5], &[10, 30, 20]);
    assert_eq!(commands[2].data[1], 0x10);

    assert_eq!(&commands[3].data[..5], &[0xE0, 0x10, 0x40, 0x01, 0x01]);
    assert_eq!(commands[4].data.len(), 146);
    assert_eq!(commands[4].data[1], 0x31);
    assert_eq!(&commands[4].data[2..5], &[153, 153, 153]);
    assert_eq!(commands[5].data[1], 0x11);
}

#[test]
fn sl_infinity_pairs_two_logical_channels_per_physical_group() {
    let protocol = Ene6k77Protocol::new(LianLiHubVariant::SlInfinity)
        .with_fan_counts([1, 1, 0, 0, 0, 0, 0, 0]);
    let mut colors = vec![[0, 0, 0]; 20];
    colors[0] = [1, 2, 3];
    colors[8] = [4, 5, 6];

    let commands = protocol.encode_frame(&colors);
    assert_eq!(commands.len(), 7);

    assert_eq!(&commands[0].data[..5], &[0xE0, 0x10, 0x60, 0x01, 0x01]);
    assert_eq!(commands[1].data.len(), 353);
    assert_eq!(commands[1].data[1], 0x30);
    assert_eq!(&commands[1].data[2..5], &[1, 3, 2]);
    assert_eq!(commands[2].data[1], 0x10);

    assert_eq!(&commands[3].data[..5], &[0xE0, 0x10, 0x60, 0x01, 0x01]);
    assert_eq!(commands[4].data[1], 0x31);
    assert_eq!(&commands[4].data[2..5], &[4, 6, 5]);
    assert_eq!(commands[5].data[1], 0x11);
}

#[test]
fn ene_fixed_speed_and_response_helpers_match_spec() {
    let speed_protocol = Ene6k77Protocol::new(LianLiHubVariant::SlInfinity);
    let speed = speed_protocol
        .encode_fixed_speed(2, 80)
        .expect("group 2 speed command should encode");
    assert_eq!(&speed.data[..4], &[0xE0, 0x22, 0x00, 82]);
    assert_eq!(speed.transfer_type, TransferType::HidReport);

    let rpm_protocol = Ene6k77Protocol::new(LianLiHubVariant::SlV2);
    let rpms = rpm_protocol
        .parse_rpm_response(&[0xE0, 0x00, 0x03, 0xE8, 0x04, 0xB0, 0x05, 0x78, 0x06, 0x40])
        .expect("V2 RPM response should parse");
    assert_eq!(rpms, [1_000, 1_200, 1_400, 1_600]);

    let firmware = rpm_protocol
        .parse_firmware_response(&[0xE0, 0x00, 0x00, 0x00, 0x00, 0x16])
        .expect("firmware response should parse");
    assert_eq!(firmware, "1.8");
}

#[test]
fn legacy_original_init_sequence_programs_global_setup_and_fan_counts() {
    let protocol = LegacyUniHubProtocol::original().with_fan_counts([2, 0, 1, 4]);
    let commands = protocol.init_sequence();

    assert_eq!(commands.len(), 1);
    let operations = decode_operations(&commands[0].data)
        .expect("legacy init command should decode into vendor-control operations");

    assert!(matches!(
        &operations[0],
        VendorControlOperation::Write {
            request: 0x80,
            index: 0xE021,
            data,
            ..
        } if data == &[0x34]
    ));
    assert!(matches!(
        &operations[2],
        VendorControlOperation::Write {
            index: 0xE02F,
            data,
            ..
        } if data == &[0x01]
    ));
    assert!(matches!(
        &operations[4],
        VendorControlOperation::Write {
            index: 0xE021,
            data,
            ..
        } if data == &[0x32, 0x01]
    ));
    assert!(matches!(
        &operations[8],
        VendorControlOperation::Write {
            index: 0xE021,
            data,
            ..
        } if data == &[0x32, 0x10]
    ));
    assert!(matches!(
        &operations[20],
        VendorControlOperation::Write {
            index: 0xE021,
            data,
            ..
        } if data == &[0x30, 0x00]
    ));
}

#[test]
fn legacy_original_frame_writes_rbg_color_mode_and_commit_registers() {
    let protocol = LegacyUniHubProtocol::original().with_fan_counts([1, 0, 0, 0]);
    let colors = vec![[10, 20, 30]; 16];

    let commands = protocol.encode_frame(&colors);
    assert_eq!(commands.len(), 4);

    let operations = decode_operations(&commands[0].data)
        .expect("legacy original frame should decode into vendor-control operations");

    assert!(matches!(
        &operations[0],
        VendorControlOperation::Write {
            index: 0xE300,
            data,
            ..
        } if data.len() == 192 && &data[..3] == [10, 30, 20]
    ));
    assert!(matches!(
        &operations[2],
        VendorControlOperation::Write {
            index: 0xE021,
            data,
            ..
        } if data == &[0x01]
    ));
    assert!(matches!(
        &operations[4],
        VendorControlOperation::Write {
            index: 0xE022,
            data,
            ..
        } if data == &[0x02]
    ));
    assert!(matches!(
        &operations[10],
        VendorControlOperation::Write {
            index: 0xE02F,
            data,
            ..
        } if data == &[0x01]
    ));
}

#[test]
fn legacy_al10_frame_splits_inner_and_outer_ring_packets() {
    let protocol = LegacyUniHubProtocol::al10().with_fan_counts([1, 0, 0, 0]);
    let mut colors = vec![[0, 0, 0]; 20];
    colors[0] = [10, 20, 30];
    colors[8] = [200, 200, 200];

    let commands = protocol.encode_frame(&colors);
    assert_eq!(commands.len(), 4);

    let operations = decode_operations(&commands[0].data)
        .expect("legacy AL10 frame should decode into vendor-control operations");

    assert!(matches!(
        &operations[0],
        VendorControlOperation::Write {
            index: 0xE500,
            data,
            ..
        } if data.len() == 24 && &data[..3] == [10, 30, 20]
    ));
    assert!(matches!(
        &operations[8],
        VendorControlOperation::Write {
            index: 0xE020,
            data,
            ..
        } if data[0x01] == 0x01 && data[0x02] == 0x00 && data[0x03] == 0x00 && data[0x09] == 0x00
    ));
    assert!(matches!(
        &operations[10],
        VendorControlOperation::Write {
            index: 0xE518,
            data,
            ..
        } if data.len() == 36 && &data[..3] == [153, 153, 153]
    ));
    assert!(matches!(
        &operations[18],
        VendorControlOperation::Write {
            index: 0xE030,
            data,
            ..
        } if data[0x0F] == 0x01
    ));
}

#[test]
fn legacy_connection_diagnostics_reads_firmware_register() {
    let protocol = LegacyUniHubProtocol::original();
    let commands = protocol.connection_diagnostics();

    assert_eq!(commands.len(), 1);
    assert!(commands[0].expects_response);

    let operations = decode_operations(&commands[0].data)
        .expect("legacy diagnostics command should decode into vendor-control operations");
    assert!(matches!(
        &operations[0],
        VendorControlOperation::Read {
            request: 0x81,
            index: 0xB500,
            length: 5,
            ..
        }
    ));
}

#[test]
fn tl_init_sequence_queries_handshake_and_product_info() {
    let protocol = TlFanProtocol::new();
    let commands = protocol.init_sequence();

    assert_eq!(commands.len(), 2);
    assert!(commands.iter().all(|command| command.expects_response));
    assert_eq!(commands[0].data[0], 0x01);
    assert_eq!(commands[0].data[1], 0xA1);
    assert_eq!(commands[1].data[1], 0xA6);
}

#[test]
fn tl_handshake_updates_topology_and_frame_addresses_each_fan() {
    let protocol = TlFanProtocol::new();
    protocol
        .parse_response(&[
            0x01, 0xA1, 0x00, 0x00, 0x01, 0x06, 0x80, 0x03, 0xE8, 0x90, 0x04, 0x4C,
        ])
        .expect("handshake response should parse");

    assert_eq!(protocol.port_fan_counts(), [1, 1, 0, 0]);
    assert_eq!(protocol.total_leds(), 52);
    assert_eq!(protocol.zones().len(), 2);

    let mut colors = vec![[100, 20, 10]; 26];
    colors.extend(vec![[5, 6, 7]; 26]);
    let commands = protocol.encode_frame(&colors);
    assert_eq!(commands.len(), 2);

    assert_eq!(commands[0].data[1], 0xA3);
    assert_eq!(commands[0].data[6], 0x00);
    assert_eq!(commands[0].data[7], 0x00);
    assert_eq!(&commands[0].data[11..14], &[100, 20, 10]);

    assert_eq!(commands[1].data[1], 0xA3);
    assert_eq!(commands[1].data[6], 0x10);
    assert_eq!(commands[1].data[7], 0x10);
    assert_eq!(&commands[1].data[11..14], &[5, 6, 7]);
}

#[test]
fn tl_product_info_response_caches_firmware_string() {
    let protocol = TlFanProtocol::new();
    protocol
        .parse_response(&[
            0x01, 0xA6, 0x00, 0x00, 0x02, 0x05, b'1', b'.', b'2', b'.', b'3',
        ])
        .expect("product info response should parse");

    assert_eq!(protocol.firmware().as_deref(), Some("1.2.3"));
}

#[test]
fn white_limit_and_firmware_helpers_match_spec_examples() {
    assert_eq!(apply_sum_white_limit([200, 200, 200]), [153, 153, 153]);
    assert_eq!(apply_sum_white_limit([255, 0, 0]), [255, 0, 0]);
    assert_eq!(apply_al_white_limit([200, 200, 200]), [153, 153, 153]);
    assert_eq!(apply_al_white_limit([200, 100, 50]), [200, 100, 50]);
    assert_eq!(firmware_version_from_fine_tune(0x06), "1.0");
    assert_eq!(firmware_version_from_fine_tune(0x16), "1.8");
}
