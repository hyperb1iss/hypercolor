use hypercolor_hal::registry::HidRawReportMode;
use hypercolor_hal::transport::hidapi::{
    decode_feature_report_packet_for_testing, encode_feature_report_request_buffer_for_testing,
    encode_hidapi_packet_for_testing,
};

#[test]
fn hidapi_prepends_report_id_for_payload_only_modes() {
    let packet =
        encode_hidapi_packet_for_testing(&[0xA0, 0x01], 0x00, HidRawReportMode::OutputReport);

    assert_eq!(packet, [0x00, 0xA0, 0x01]);
}

#[test]
fn hidapi_preserves_packets_that_already_include_report_id() {
    let packet = encode_hidapi_packet_for_testing(
        &[0x00, 0xFC, 0x01],
        0x00,
        HidRawReportMode::OutputReportWithReportId,
    );

    assert_eq!(packet, [0x00, 0xFC, 0x01]);
}

#[test]
fn hidapi_emits_report_id_for_empty_report_id_payload_packets() {
    let packet =
        encode_hidapi_packet_for_testing(&[], 0x00, HidRawReportMode::FeatureReportWithReportId);

    assert_eq!(packet, [0x00]);
}

#[test]
fn hidapi_feature_report_request_uses_full_report_len() {
    let buffer = encode_feature_report_request_buffer_for_testing(0x00, 91, Some(0x1F));

    assert_eq!(buffer.len(), 91);
    assert_eq!(buffer[0], 0x00);
    assert_eq!(buffer[2], 0x1F);
}

#[test]
fn hidapi_decode_strips_report_id_only_for_payload_only_modes() {
    let report = [0x00, 0x1F, 0x00, 0xAA];

    assert_eq!(
        decode_feature_report_packet_for_testing(&report, 0x00, report.len(), false),
        [0x1F, 0x00, 0xAA]
    );
    assert_eq!(
        decode_feature_report_packet_for_testing(&report, 0x00, report.len(), true),
        report
    );
}
