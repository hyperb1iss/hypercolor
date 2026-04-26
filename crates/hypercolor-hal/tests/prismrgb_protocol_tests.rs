use std::time::Duration;

use hypercolor_hal::drivers::prismrgb::{
    LOW_POWER_THRESHOLD, PrismRgbModel, PrismRgbProtocol, PrismSConfig, PrismSGpuCable,
    apply_low_power_saver, build_prism_mini_protocol, build_prism_s_protocol, compress_color_pair,
};
use hypercolor_hal::protocol::{Protocol, ResponseStatus};
use hypercolor_types::device::DeviceTopologyHint;

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
fn prism_s_dual_gpu_only_uses_packet_20_and_dual_led_count() {
    let protocol = PrismRgbProtocol::new(PrismRgbModel::PrismS).with_prism_s_config(PrismSConfig {
        atx_present: false,
        gpu_cable: Some(PrismSGpuCable::Dual8Pin),
    });
    let colors = vec![[1, 2, 3]; 108];

    let commands = protocol.encode_frame(&colors);
    assert_eq!(commands.len(), 6);
    assert_eq!(commands[0].data[1], 0x05);
    assert_eq!(commands[5].data[1], 0x14);

    let zones = protocol.zones();
    assert_eq!(zones.len(), 1);
    assert_eq!(zones[0].name, "GPU Strimer");
    assert_eq!(zones[0].led_count, 108);
    assert_eq!(
        zones[0].topology,
        DeviceTopologyHint::Matrix { rows: 4, cols: 27 }
    );
    assert_eq!(protocol.total_leds(), 108);
}

#[test]
fn prism_s_atx_only_keeps_final_atx_packet_and_reports_single_zone() {
    let protocol = PrismRgbProtocol::new(PrismRgbModel::PrismS).with_prism_s_config(PrismSConfig {
        atx_present: true,
        gpu_cable: None,
    });
    let colors = vec![[1, 2, 3]; 120];

    let commands = protocol.encode_frame(&colors);
    assert_eq!(commands.len(), 6);
    assert_eq!(commands[5].data[1], 0x0F);

    let zones = protocol.zones();
    assert_eq!(zones.len(), 1);
    assert_eq!(zones[0].name, "ATX Strimer");
    assert_eq!(zones[0].led_count, 120);
    assert_eq!(protocol.total_leds(), 120);
}

#[test]
fn prism_s_dual_mode_sets_settings_byte_to_one() {
    let protocol = PrismRgbProtocol::new(PrismRgbModel::PrismS).with_prism_s_config(PrismSConfig {
        atx_present: true,
        gpu_cable: Some(PrismSGpuCable::Dual8Pin),
    });

    let commands = protocol.init_sequence();
    assert_eq!(commands[0].data[6], 0x01);
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
    let mini = PrismRgbProtocol::new(PrismRgbModel::PrismMini);

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
