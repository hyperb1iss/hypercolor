use std::time::Duration;

use hypercolor_hal::drivers::dygma::{
    DygmaProtocol, DygmaVariant, FocusColorMode, build_defy_wired_protocol,
    build_defy_wireless_protocol, rgb_to_rgbw,
};
use hypercolor_hal::protocol::{Protocol, ProtocolError};
use hypercolor_types::device::{DeviceColorFormat, DeviceTopologyHint};

#[test]
fn rgb_to_rgbw_matches_reference_conversion() {
    assert_eq!(rgb_to_rgbw(255, 255, 255), (0, 0, 0, 255));
    assert_eq!(rgb_to_rgbw(255, 0, 0), (255, 0, 0, 0));
    assert_eq!(rgb_to_rgbw(200, 100, 50), (150, 50, 0, 50));
}

#[test]
fn init_sequence_queries_identity_probe_and_blackout() {
    let protocol = build_defy_wired_protocol();
    let commands = protocol.init_sequence();

    assert_eq!(commands.len(), 6);
    assert_eq!(commands[0].data, b"hardware.chip_id\n");
    assert_eq!(commands[1].data, b"hardware.firmware\n");
    assert_eq!(commands[2].data, b"led.at 0\n");
    assert_eq!(commands[3].data, b"led.fade 0\n");
    assert_eq!(commands[4].data, b"led.mode 0\n");
    assert_eq!(commands[5].data, b"led.setAll 0 0 0\n");
    assert_eq!(commands[5].post_delay, Duration::from_millis(50));
    assert!(commands.iter().all(|command| command.expects_response));
}

#[test]
fn shutdown_sequence_restores_palette_mode() {
    let protocol = build_defy_wired_protocol();
    let commands = protocol.shutdown_sequence();

    assert_eq!(commands.len(), 3);
    assert_eq!(commands[0].data, b"led.setAll 0 0 0\n");
    assert_eq!(commands[1].data, b"led.fade 1\n");
    assert_eq!(commands[2].data, b"led.mode 0\n");
}

#[test]
fn probe_response_switches_protocol_to_rgb_mode() {
    let protocol = DygmaProtocol::new(DygmaVariant::DefyWired);
    assert_eq!(protocol.color_mode(), FocusColorMode::Rgbw);

    protocol
        .parse_response(b"12 34 56")
        .expect("RGB probe should parse");

    assert_eq!(protocol.color_mode(), FocusColorMode::Rgb);
    assert_eq!(protocol.zones()[0].color_format, DeviceColorFormat::Rgb);
}

#[test]
fn rgbw_frame_extracts_white_channel_for_every_led() {
    let protocol = DygmaProtocol::new(DygmaVariant::DefyWired);
    let colors = vec![[255, 255, 255]; 176];

    let commands = protocol.encode_frame(&colors);
    assert_eq!(commands.len(), 1);
    let payload = String::from_utf8(commands[0].data.clone()).expect("frame should be utf-8");
    let parts = payload.split_whitespace().collect::<Vec<_>>();

    assert_eq!(parts[0], "led.theme");
    assert_eq!(parts.len(), 1 + 176 * 4);
    assert_eq!(&parts[1..5], ["0", "0", "0", "255"]);
}

#[test]
fn rgb_frame_uses_three_channels_after_probe() {
    let protocol = DygmaProtocol::new(DygmaVariant::DefyWired);
    protocol
        .parse_response(b"5 10 15")
        .expect("RGB probe should parse");

    let colors = vec![[1, 2, 3]; 176];
    let commands = protocol.encode_frame(&colors);
    let payload = String::from_utf8(commands[0].data.clone()).expect("frame should be utf-8");
    let parts = payload.split_whitespace().collect::<Vec<_>>();

    assert_eq!(parts.len(), 1 + 176 * 3);
    assert_eq!(&parts[1..4], ["1", "2", "3"]);
}

#[test]
fn brightness_commands_follow_variant() {
    let wired = build_defy_wired_protocol();
    let wireless = build_defy_wireless_protocol();

    let wired_commands = wired
        .encode_brightness(128)
        .expect("wired brightness should be supported");
    let wireless_commands = wireless
        .encode_brightness(128)
        .expect("wireless brightness should be supported");

    assert_eq!(wired_commands[0].data, b"led.brightness 128\n");
    assert_eq!(wired_commands[1].data, b"led.brightnessUG 128\n");
    assert_eq!(wireless_commands[0].data, b"led.brightness.wireless 128\n");
    assert_eq!(
        wireless_commands[1].data,
        b"led.brightnessUG.wireless 128\n"
    );
}

#[test]
fn response_timeout_is_two_seconds() {
    let protocol = build_defy_wired_protocol();
    assert_eq!(protocol.response_timeout(), Duration::from_millis(2_000));
}

#[test]
fn parse_response_rejects_invalid_utf8() {
    let protocol = build_defy_wired_protocol();
    let error = protocol
        .parse_response(&[0xFF])
        .expect_err("invalid UTF-8 should fail");

    assert!(matches!(error, ProtocolError::MalformedResponse { .. }));
}

#[test]
fn zones_and_capabilities_match_defy_layout() {
    let protocol = build_defy_wired_protocol();
    let zones = protocol.zones();

    assert_eq!(zones.len(), 4);
    assert_eq!(zones[0].led_count, 35);
    assert_eq!(zones[1].led_count, 35);
    assert_eq!(zones[2].led_count, 53);
    assert_eq!(zones[3].led_count, 53);
    assert_eq!(zones[0].topology, DeviceTopologyHint::Custom);
    assert_eq!(zones[2].topology, DeviceTopologyHint::Strip);

    let capabilities = protocol.capabilities();
    assert_eq!(capabilities.led_count, 176);
    assert!(capabilities.supports_direct);
    assert!(capabilities.supports_brightness);
    assert_eq!(capabilities.max_fps, 10);
    assert_eq!(protocol.frame_interval(), Duration::from_millis(100));
}
