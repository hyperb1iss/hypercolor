#![cfg(target_os = "linux")]

use hypercolor_hal::transport::hidraw::{
    encode_feature_report_packet, encode_feature_report_request_buffer,
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
