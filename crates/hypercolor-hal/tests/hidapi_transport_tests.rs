use hypercolor_hal::registry::HidRawReportMode;
use hypercolor_hal::transport::hidapi::encode_hidapi_packet_for_testing;

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
