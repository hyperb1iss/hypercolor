use std::time::Duration;

use hypercolor_hal::drivers::nollie::{
    CDC_SERIAL_REPORT_SIZE, GpuCableType, Nollie32Config, NollieModel, NollieProtocol,
    ProtocolVersion, build_nollie_8_v2_protocol, build_prism_8_protocol,
};
use hypercolor_hal::protocol::Protocol;
use hypercolor_types::device::DeviceTopologyHint;

#[test]
fn prism8_init_sequence_queries_firmware_and_channel_counts() {
    let protocol = build_prism_8_protocol();
    let commands = protocol.init_sequence();

    assert_eq!(commands.len(), 3);
    assert!(commands[0].expects_response);
    assert_eq!(commands[0].data[1], 0xFC);
    assert_eq!(commands[0].data[2], 0x01);
    assert!(commands[1].expects_response);
    assert_eq!(commands[1].data[1], 0xFC);
    assert_eq!(commands[1].data[2], 0x03);
    assert!(!commands[2].expects_response);
    assert_eq!(commands[2].data[1], 0xFE);
    assert_eq!(commands[2].data[2], 0x02);
}

#[test]
fn prism8_frame_encodes_scaled_grb_packets_and_commit() {
    let protocol = NollieProtocol::new(NollieModel::Prism8);
    let mut colors = vec![[0_u8, 0_u8, 0_u8]; 1_008];
    colors[0] = [100, 40, 20];

    let commands = protocol.encode_frame(&colors);
    assert_eq!(commands.len(), 49);

    let first = &commands[0].data;
    assert_eq!(first[1], 0x00);
    assert_eq!(first[2], 30);
    assert_eq!(first[3], 75);
    assert_eq!(first[4], 15);

    let last = &commands[48].data;
    assert_eq!(last[1], 0xFF);
}

#[test]
fn nollie8_frame_uses_full_brightness_grb_encoding() {
    let protocol = build_nollie_8_v2_protocol();
    let mut colors = vec![[0_u8, 0_u8, 0_u8]; 1_008];
    colors[0] = [100, 40, 20];

    let commands = protocol.encode_frame(&colors);
    let first = &commands[0].data;
    assert_eq!(first[2], 40);
    assert_eq!(first[3], 100);
    assert_eq!(first[4], 20);
}

#[test]
fn nollie1_uses_dense_packet_ids_and_omits_render_commit() {
    let protocol = NollieProtocol::new(NollieModel::Nollie1);
    let commands = protocol.encode_frame(&vec![[1, 2, 3]; 630]);

    assert_eq!(commands.len(), 30);
    for (index, command) in commands.iter().enumerate() {
        assert_eq!(
            command.data[1],
            u8::try_from(index).expect("packet id fits")
        );
    }

    let shutdown = protocol.shutdown_sequence();
    assert_eq!(shutdown[0].data[1], 0xFF);
}

#[test]
fn nollie28_12_uses_interval_two_rgb_packets_and_commit() {
    let protocol = NollieProtocol::new(NollieModel::Nollie28_12);
    let mut colors = vec![[0_u8, 0_u8, 0_u8]; 504];
    colors[0] = [100, 40, 20];

    let commands = protocol.encode_frame(&colors);
    assert_eq!(commands.len(), 25);
    assert_eq!(commands[0].data[1], 0);
    assert_eq!(commands[1].data[1], 1);
    assert_eq!(commands[2].data[1], 2);
    assert_eq!(commands[23].data[1], 23);
    assert_eq!(commands[24].data[1], 0xFF);
    assert_eq!(commands[0].data[2], 100);
    assert_eq!(commands[0].data[3], 40);
    assert_eq!(commands[0].data[4], 20);
}

#[test]
fn nollie_cdc_uses_64_byte_serial_blocks_and_show_packet() {
    let protocol = NollieProtocol::new(NollieModel::Nollie8Cdc);
    let mut colors = vec![[0_u8, 0_u8, 0_u8]; 1_008];
    colors[126] = [10, 20, 30];

    let commands = protocol.encode_frame(&colors);
    assert_eq!(commands.len(), 49);
    assert!(
        commands
            .iter()
            .all(|command| command.data.len() == CDC_SERIAL_REPORT_SIZE)
    );
    assert_eq!(commands[0].data[0], 0);
    assert_eq!(commands[6].data[0], 6);
    assert_eq!(commands[6].data[1], 20);
    assert_eq!(commands[6].data[2], 10);
    assert_eq!(commands[6].data[3], 30);
    assert_eq!(commands[48].data[0], 0xFF);
}

#[test]
fn nollie16v3_nos2_uses_direct_1024_byte_packets_and_alt_remap() {
    let protocol = NollieProtocol::new(NollieModel::Nollie16v3Nos2);
    let commands = protocol.encode_frame(&vec![[1, 2, 3]; 4_096]);

    assert_eq!(commands.len(), 16);
    assert!(commands.iter().all(|command| command.data.len() == 1_024));
    assert_eq!(commands[0].data[1], 0);
    assert_eq!(commands[0].data[2], 0);
    assert_eq!(commands[0].data[5], 2);
    assert_eq!(commands[0].data[6], 1);
    assert_eq!(commands[0].data[7], 3);

    let flag = commands
        .iter()
        .find(|command| command.data[1] == 15)
        .expect("NOS2 marker channel should be present");
    assert_eq!(flag.data[2], 1);
}

#[test]
fn nollie32_nos2_marks_low_and_high_physical_groups_with_strimers() {
    let protocol =
        NollieProtocol::new(NollieModel::Nollie32Nos2).with_nollie32_config(Nollie32Config {
            atx_cable_present: true,
            gpu_cable_type: GpuCableType::Triple8Pin,
        });
    let commands = protocol.encode_frame(&vec![[1, 2, 3]; 5_402]);

    assert_eq!(commands.len(), 32);
    let low = commands
        .iter()
        .find(|command| command.data[1] == 15)
        .expect("low marker channel should be present");
    assert_eq!(low.data[2], 1);
    let high = commands
        .iter()
        .find(|command| command.data[1] == 31)
        .expect("high marker channel should be present");
    assert_eq!(high.data[2], 2);
}

#[test]
fn discontinued_legacy_header_models_use_official_five_byte_header() {
    let protocol = NollieProtocol::new(NollieModel::NollieLegacy2);
    let mut colors = vec![[0_u8, 0_u8, 0_u8]; 1_024];
    colors[0] = [100, 40, 20];

    let commands = protocol.encode_frame(&colors);
    assert_eq!(commands.len(), 52);
    assert_eq!(commands[0].data[1], 1);
    assert_eq!(commands[0].data[2], 0);
    assert_eq!(commands[0].data[3], 26);
    assert_eq!(commands[0].data[4], 1);
    assert_eq!(commands[0].data[5], 100);
    assert_eq!(commands[0].data[6], 40);
    assert_eq!(commands[0].data[7], 20);
}

#[test]
fn nollie4_stream65_prepends_led_count_config_and_concatenates_channels() {
    let protocol = NollieProtocol::new(NollieModel::Nollie4);
    let commands = protocol.encode_frame(&vec![[1, 2, 3]; 2_544]);

    assert_eq!(commands[0].data[1], 0x86);
    assert_eq!(commands[0].data[3], 0x02);
    assert_eq!(commands[0].data[4], 0x7C);
    assert_eq!(commands[1].data[1], 0);
    assert_eq!(commands[2].data[1], 1);
    assert_eq!(commands[1].data[2], 2);
    assert_eq!(commands[1].data[3], 1);
    assert_eq!(commands[1].data[4], 3);
}

#[test]
fn nollie16v3_v2_emits_count_config_then_upper_half_groups() {
    let protocol = NollieProtocol::new(NollieModel::Nollie16v3);
    let commands = protocol.encode_frame(&vec![[1, 2, 3]; 4_096]);

    assert_eq!(commands.len(), 17);
    assert_eq!(commands[0].data[1], 0x88);
    assert_eq!(commands[1].data[1], 0x40);
    assert_eq!(commands[1].data[2], 16);
    assert_eq!(commands[1].data[3], 16);
    assert_eq!(commands[16].data[4], 2);
}

#[test]
fn nollie32_v2_marks_flag1_and_final_groups() {
    let protocol = NollieProtocol::new(NollieModel::Nollie32 {
        protocol_version: ProtocolVersion::V2,
    });
    let commands = protocol.encode_frame(&vec![[1, 2, 3]; 5_120]);

    assert_eq!(commands[0].data[1], 0x88);
    let flag1 = commands
        .iter()
        .find(|command| command.data[1] == 0x40 && command.data[2] == 15)
        .expect("FLAG1 group should be emitted");
    assert_eq!(flag1.data[4], 1);

    let final_group = commands
        .iter()
        .rev()
        .find(|command| command.data[1] == 0x40)
        .expect("final group should be emitted");
    assert_eq!(final_group.data[2], 31);
    assert_eq!(final_group.data[4], 2);
}

#[test]
fn nollie32_v1_uses_standalone_channel_packets_and_boundary_delay() {
    let protocol = NollieProtocol::new(NollieModel::Nollie32 {
        protocol_version: ProtocolVersion::V1,
    });
    let commands = protocol.encode_frame(&vec![[1, 2, 3]; 5_120]);

    assert_eq!(commands[0].data[1], 0x88);
    assert!(
        commands
            .iter()
            .skip(1)
            .all(|command| command.data[1] != 0x40)
    );
    let flag1 = commands
        .iter()
        .find(|command| command.data[1] == 15)
        .expect("FLAG1 packet should be emitted");
    assert_eq!(flag1.data[2], 1);
    assert_eq!(flag1.post_delay, Duration::from_millis(8));
}

#[test]
fn nollie32_strimer_config_adds_matrix_zones_and_mos_byte() {
    let protocol = NollieProtocol::new(NollieModel::Nollie32 {
        protocol_version: ProtocolVersion::V2,
    })
    .with_nollie32_config(Nollie32Config {
        atx_cable_present: true,
        gpu_cable_type: GpuCableType::Dual8Pin,
    });

    let zones = protocol.zones();
    assert_eq!(zones.len(), 22);
    assert_eq!(
        zones[20].topology,
        DeviceTopologyHint::Matrix { rows: 6, cols: 20 }
    );
    assert_eq!(
        zones[21].topology,
        DeviceTopologyHint::Matrix { rows: 4, cols: 27 }
    );
    assert_eq!(protocol.total_leds(), 5_348);

    let init = protocol.init_sequence();
    assert_eq!(init[1].data[1], 0x80);
    assert_eq!(init[1].data[2], 0x01);
}
