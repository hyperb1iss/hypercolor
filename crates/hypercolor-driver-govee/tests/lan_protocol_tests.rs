use hypercolor_driver_govee::lan::protocol::{LanCommand, encode_command_string};
use hypercolor_driver_govee::lan::razer::{
    RAZER_DISABLE, RAZER_ENABLE, encode_razer_frame, encode_razer_frame_base64,
    encode_razer_mode_base64,
};

#[test]
fn encodes_colorwc_with_kelvin_zero() {
    let encoded = encode_command_string(&LanCommand::ColorWc {
        red: 255,
        green: 16,
        blue: 8,
    })
    .expect("color command should encode");

    assert_eq!(
        encoded,
        r#"{"msg":{"cmd":"colorwc","data":{"color":{"r":255,"g":16,"b":8},"colorTemInKelvin":0}}}"#
    );
}

#[test]
fn encodes_scan_without_data_field() {
    let encoded = encode_command_string(&LanCommand::Scan).expect("scan should encode");

    assert_eq!(encoded, r#"{"msg":{"cmd":"scan"}}"#);
}

#[test]
fn clamps_brightness_to_govee_range() {
    let encoded =
        encode_command_string(&LanCommand::Brightness { value: 0 }).expect("brightness encodes");

    assert_eq!(
        encoded,
        r#"{"msg":{"cmd":"brightness","data":{"value":1}}}"#
    );
}

#[test]
fn encodes_razer_single_led_packet_with_known_xor() {
    let packet = encode_razer_frame(&[[255, 0, 0]]).expect("non-empty frame should encode");

    assert_eq!(
        packet,
        vec![0xBB, 0x00, 0x05, 0xB0, 0x01, 0x01, 0xFF, 0x00, 0x00, 0xF1]
    );
}

#[test]
fn encodes_razer_length_for_255_leds() {
    let colors = vec![[1, 2, 3]; 255];
    let packet = encode_razer_frame(&colors).expect("255 LED frame should encode");

    assert_eq!(packet.len(), 7 + (3 * 255));
    assert_eq!(packet[1], 0x02);
    assert_eq!(packet[2], 0xFF);
    assert_eq!(packet[5], 255);
}

#[test]
fn encodes_razer_mode_packets() {
    assert_eq!(RAZER_ENABLE, [0xBB, 0x00, 0x01, 0xB1, 0x01, 0x0A]);
    assert_eq!(RAZER_DISABLE, [0xBB, 0x00, 0x01, 0xB1, 0x00, 0x0B]);
    assert_eq!(encode_razer_mode_base64(true), "uwABsQEK");
    assert_eq!(encode_razer_mode_base64(false), "uwABsQAL");
}

#[test]
fn base64_wrap_matches_packet_length_invariant() {
    let colors = vec![[1, 2, 3]; 10];
    let packet = encode_razer_frame(&colors).expect("frame should encode");
    let encoded = encode_razer_frame_base64(&colors).expect("frame should wrap");
    let expected_len = 4 * packet.len().div_ceil(3);

    assert_eq!(encoded.len(), expected_len);
}
