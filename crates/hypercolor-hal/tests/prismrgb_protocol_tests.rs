use std::time::Duration;

use hypercolor_hal::drivers::prismrgb::{
    LOW_POWER_THRESHOLD, PrismRgbModel, PrismRgbProtocol, apply_low_power_saver,
    build_nollie_8_v2_protocol, build_prism_8_protocol, build_prism_mini_protocol,
    build_prism_s_protocol, compress_color_pair,
};
use hypercolor_hal::protocol::{Protocol, ResponseStatus};
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
    let protocol = PrismRgbProtocol::new(PrismRgbModel::Prism8);
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
fn nollie_frame_uses_full_brightness_grb_encoding() {
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
fn prism_s_uses_combined_buffer_chunk_ids() {
    let protocol = build_prism_s_protocol();
    let colors = vec![[1, 2, 3]; 282];

    let commands = protocol.encode_frame(&colors);
    assert_eq!(commands.len(), 14);
    assert_eq!(commands[0].data[1], 0x00);
    assert_eq!(commands[1].data[1], 0x01);
    assert_eq!(commands[4].data[1], 0x04);
    assert_eq!(commands[5].data[1], 0x05);
    assert_eq!(commands[6].data[1], 0x06);
    assert_eq!(commands[13].data[1], 0x0D);
}

#[test]
fn prism_s_init_sequence_encodes_settings_delay() {
    let protocol = build_prism_s_protocol();
    let commands = protocol.init_sequence();

    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].data[1], 0xFE);
    assert_eq!(commands[0].data[2], 0x01);
    assert_eq!(commands[0].post_delay, Duration::from_millis(50));
}

#[test]
fn prism_mini_frame_uses_numbered_packets_and_marker() {
    let protocol = build_prism_mini_protocol();
    let colors = vec![[255, 0, 0]; 128];

    let commands = protocol.encode_frame(&colors);
    assert_eq!(commands.len(), 7);

    let first = &commands[0].data;
    assert_eq!(first[1], 1);
    assert_eq!(first[2], 7);
    assert_eq!(first[4], 0xAA);

    let last = &commands[6].data;
    assert_eq!(last[1], 7);
    assert_eq!(last[2], 7);
}

#[test]
fn prism_mini_low_power_saver_caps_rgb_sum() {
    let (r, g, b) = apply_low_power_saver(255, 255, 255);
    assert!(u16::from(r) + u16::from(g) + u16::from(b) <= LOW_POWER_THRESHOLD);

    let unchanged = apply_low_power_saver(60, 50, 40);
    assert_eq!(unchanged, (60, 50, 40));
}

#[test]
fn prism_mini_compression_matches_reference_packing() {
    let compressed = compress_color_pair((0xAB, 0xCD, 0xEF), (0x12, 0x34, 0x56));
    assert_eq!(compressed, [0xCA, 0x1E, 0x53]);
}

#[test]
fn prismrgb_parse_response_accepts_model_specific_shapes() {
    let prism8 = PrismRgbProtocol::new(PrismRgbModel::Prism8);
    let mini = PrismRgbProtocol::new(PrismRgbModel::PrismMini);

    let parsed_prism8 = prism8
        .parse_response(&[0x00, 0x00, 0x02])
        .expect("Prism 8 response should parse");
    assert_eq!(parsed_prism8.status, ResponseStatus::Ok);

    let parsed_mini = mini
        .parse_response(&[0x00, 0x01, 0x00, 0x00])
        .expect("Prism Mini response should parse");
    assert_eq!(parsed_mini.status, ResponseStatus::Ok);
}

#[test]
fn prismrgb_zones_report_expected_topologies() {
    let prism_s = PrismRgbProtocol::new(PrismRgbModel::PrismS);
    let prism_mini = PrismRgbProtocol::new(PrismRgbModel::PrismMini);

    let prism_s_zones = prism_s.zones();
    assert_eq!(prism_s_zones.len(), 2);
    assert_eq!(
        prism_s_zones[0].topology,
        DeviceTopologyHint::Matrix { rows: 6, cols: 20 }
    );
    assert_eq!(
        prism_s_zones[1].topology,
        DeviceTopologyHint::Matrix { rows: 6, cols: 27 }
    );

    let prism_mini_zones = prism_mini.zones();
    assert_eq!(prism_mini_zones.len(), 1);
    assert_eq!(prism_mini_zones[0].topology, DeviceTopologyHint::Strip);
}
