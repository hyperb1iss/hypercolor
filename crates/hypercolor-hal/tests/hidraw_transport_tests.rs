#![cfg(target_os = "linux")]

use hypercolor_hal::transport::hidraw::{
    decode_feature_report_packet, encode_feature_report_packet, usb_paths_match_for_testing,
};

#[test]
fn encode_feature_report_packet_prepends_report_id_for_zero_report_devices() {
    let payload = [0x00, 0x3F, 0x02, 0x00];

    let packet = encode_feature_report_packet(&payload, 0x00);

    assert_eq!(packet, vec![0x00, 0x00, 0x3F, 0x02, 0x00]);
}

#[test]
fn encode_feature_report_packet_prepends_report_id_for_numbered_reports() {
    let payload = [0x02, 0x60, 0x01];

    let packet = encode_feature_report_packet(&payload, 0x07);

    assert_eq!(packet, vec![0x07, 0x02, 0x60, 0x01]);
}

#[test]
fn decode_feature_report_packet_strips_explicit_leading_report_id() {
    let buffer = [0x00, 0x02, 0x3F, 0x00];

    let payload = decode_feature_report_packet(&buffer, 0x00, 3);

    assert_eq!(payload, vec![0x02, 0x3F, 0x00]);
}

#[test]
fn decode_feature_report_packet_preserves_payload_without_explicit_report_id() {
    let buffer = [0x00, 0x3F, 0x00];

    let payload = decode_feature_report_packet(&buffer, 0x00, 3);

    assert_eq!(payload, buffer);
}

#[test]
fn usb_paths_match_handles_padded_bus_numbers() {
    assert!(usb_paths_match_for_testing("3-7", "003-7"));
    assert!(usb_paths_match_for_testing("003-7", "3-7"));
    assert!(usb_paths_match_for_testing("03-7.2", "3-7.2"));
}

#[test]
fn usb_paths_match_rejects_different_ports() {
    assert!(!usb_paths_match_for_testing("3-7", "3-8"));
}
