#![cfg(target_os = "linux")]

use hypercolor_hal::transport::hidraw::{
    encode_feature_report_packet, encode_feature_report_request_buffer,
    hidraw_usb_identity_for_testing, usb_paths_match_for_testing,
};

#[test]
fn feature_report_packet_prefixes_report_id() {
    let packet = encode_feature_report_packet(&[0x00, 0x1F, 0xAA], 0x07);

    assert_eq!(packet, vec![0x07, 0x00, 0x1F, 0xAA]);
}

#[test]
fn feature_report_request_buffer_includes_transaction_id_hint() {
    let buffer = encode_feature_report_request_buffer(0x00, 90, Some(0x1F));

    assert_eq!(buffer.len(), 91);
    assert_eq!(buffer[0], 0x00);
    assert_eq!(buffer[1], 0x00);
    assert_eq!(buffer[2], 0x1F);
}

#[test]
fn feature_report_request_buffer_leaves_hint_empty_when_unknown() {
    let buffer = encode_feature_report_request_buffer(0x07, 63, None);

    assert_eq!(buffer.len(), 64);
    assert_eq!(buffer[0], 0x07);
    assert_eq!(buffer[1], 0x00);
    assert_eq!(buffer[2], 0x00);
}

#[test]
fn hidraw_sysfs_identity_extracts_usb_path_and_interface() {
    let path =
        "/sys/devices/pci0000:00/0000:00:14.0/usb1/1-3/1-3:1.2/0003:1532:0099.0007/hidraw/hidraw3";
    let (usb_path, interface) = hidraw_usb_identity_for_testing(path);

    assert_eq!(usb_path.as_deref(), Some("1-3"));
    assert_eq!(interface, Some(2));
}

#[test]
fn usb_path_match_normalizes_bus_numbers() {
    assert!(usb_paths_match_for_testing("01-3.4", "1-3.4"));
    assert!(usb_paths_match_for_testing("1-3.4", "01-3.4"));
    assert!(!usb_paths_match_for_testing("1-3.4", "1-3.5"));
}
