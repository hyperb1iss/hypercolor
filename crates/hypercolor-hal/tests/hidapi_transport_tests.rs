use hypercolor_hal::registry::HidRawReportMode;
use hypercolor_hal::transport::TransportError;
use hypercolor_hal::transport::hidapi::{
    HidCollectionInfo, check_output_write_for_testing, decode_feature_report_packet_for_testing,
    encode_feature_report_request_buffer_for_testing, encode_hidapi_packet_for_testing,
    enumerate_usb_hid_collections,
};

fn consumer_collection_at(usb_path: Option<&str>) -> HidCollectionInfo {
    HidCollectionInfo {
        vendor_id: 0x1532,
        product_id: 0x48F0,
        serial: None,
        usb_path: usb_path.map(ToOwned::to_owned),
        interface_number: Some(0),
        usage_page: 0x000C,
        usage: 0x0001,
    }
}

#[test]
fn a_collection_matches_its_usb_path_whatever_the_bus_padding() {
    let collection = consumer_collection_at(Some("1-2.3"));

    assert!(collection.is_at_usb_path("1-2.3"));
    assert!(
        collection.is_at_usb_path("001-2.3"),
        "nusb pads the bus number on Linux, sysfs does not"
    );
    assert!(!collection.is_at_usb_path("1-2.4"));
    assert!(!collection.is_at_usb_path("2-2.3"));
}

#[test]
fn a_collection_without_a_resolved_path_matches_no_device() {
    assert!(!consumer_collection_at(None).is_at_usb_path("1-2.3"));
}

/// Hardware-dependent by nature: a CI host usually exposes no HID devices,
/// and a sandbox may refuse the HID stack outright. What the function owns
/// is the shape of what it reports, so that is what this checks.
#[test]
fn enumeration_reports_trimmed_serials_when_the_hid_stack_answers() {
    let Ok(collections) = enumerate_usb_hid_collections() else {
        return;
    };
    for collection in &collections {
        assert!(
            collection
                .serial
                .as_deref()
                .is_none_or(|serial| !serial.is_empty() && serial.trim() == serial),
            "serials arrive trimmed and never blank: {collection:?}"
        );
    }
}

#[test]
fn hidapi_output_write_accepts_windows_padded_length() {
    // Nollie 16 v3 on Windows: a 1024-byte packet on a collection whose
    // longest output report is 1025 bytes including the report ID.
    assert!(check_output_write_for_testing(1025, 1024).is_ok());
    assert!(check_output_write_for_testing(1024, 1024).is_ok());
}

#[test]
fn hidapi_output_write_accepts_synchronous_completion() {
    assert!(check_output_write_for_testing(0, 1024).is_ok());
}

#[test]
fn hidapi_output_write_rejects_truncated_packets() {
    let error = check_output_write_for_testing(513, 1024).expect_err("truncated write");

    assert!(matches!(
        error,
        TransportError::IoError { ref detail } if detail == "short hidapi output write: wrote 513 of 1024 bytes"
    ));
}

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
