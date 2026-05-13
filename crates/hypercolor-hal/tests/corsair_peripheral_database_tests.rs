use hypercolor_hal::ProtocolDatabase;
use hypercolor_hal::drivers::corsair::peripheral::devices::{
    BRAGI_INTERFACE, BRAGI_REPORT_ID, PID_K65_MINI, PID_SCIMITAR_ELITE_BRAGI, descriptors,
};
use hypercolor_hal::drivers::corsair::peripheral::types::{
    BRAGI_JUMBO_PACKET_SIZE, BRAGI_LARGE_PACKET_SIZE,
};
use hypercolor_hal::drivers::corsair::{CORSAIR_USAGE_PAGE, CORSAIR_VID};
use hypercolor_hal::registry::{HidRawReportMode, TransportType};

const CORSAIR_TOML: &str = include_str!("../../../data/drivers/vendors/corsair.toml");

#[test]
fn all_supported_peripheral_descriptors_use_corsair_vid() {
    for descriptor in descriptors() {
        assert_eq!(descriptor.vendor_id, CORSAIR_VID, "{}", descriptor.name);
        assert_eq!(descriptor.family.id(), "corsair");
    }
}

#[test]
fn bragi_jumbo_and_large_descriptors_use_expected_report_lengths() {
    let k65 = ProtocolDatabase::lookup(CORSAIR_VID, PID_K65_MINI)
        .expect("K65 Mini descriptor should exist");
    let scimitar = ProtocolDatabase::lookup(CORSAIR_VID, PID_SCIMITAR_ELITE_BRAGI)
        .expect("Scimitar Elite descriptor should exist");

    assert_eq!(
        k65.transport,
        TransportType::UsbHidApi {
            interface: Some(BRAGI_INTERFACE),
            report_id: BRAGI_REPORT_ID,
            report_mode: HidRawReportMode::OutputReportWithReportId,
            max_report_len: BRAGI_JUMBO_PACKET_SIZE,
            usage_page: Some(CORSAIR_USAGE_PAGE),
            usage: None,
        }
    );
    assert_eq!(
        scimitar.transport,
        TransportType::UsbHidApi {
            interface: Some(BRAGI_INTERFACE),
            report_id: BRAGI_REPORT_ID,
            report_mode: HidRawReportMode::OutputReportWithReportId,
            max_report_len: BRAGI_LARGE_PACKET_SIZE,
            usage_page: Some(CORSAIR_USAGE_PAGE),
            usage: None,
        }
    );
}

#[test]
fn bragi_dongles_are_researched_but_not_registered_as_rgb_devices() {
    assert!(CORSAIR_TOML.contains("pid = 0x1B62\nname = \"K57 Wireless Dongle\""));
    assert!(CORSAIR_TOML.contains("pid = 0x1BA6\nname = \"Generic Bragi Dongle\""));

    assert!(ProtocolDatabase::lookup(CORSAIR_VID, 0x1B62).is_none());
    assert!(ProtocolDatabase::lookup(CORSAIR_VID, 0x1BA6).is_none());
}

#[test]
fn nxp_researched_devices_are_cataloged_but_not_registered() {
    assert!(CORSAIR_TOML.contains("pid = 0x1B3D\nname = \"K55\""));
    assert!(CORSAIR_TOML.contains("pid = 0x1B13\nname = \"K70 RGB\""));
    assert!(CORSAIR_TOML.contains("pid = 0x1B3B\nname = \"Polaris\""));

    assert!(ProtocolDatabase::lookup(CORSAIR_VID, 0x1B3D).is_none());
    assert!(ProtocolDatabase::lookup(CORSAIR_VID, 0x1B13).is_none());
    assert!(ProtocolDatabase::lookup(CORSAIR_VID, 0x1B3B).is_none());
}

#[test]
fn legacy_researched_devices_are_cataloged_but_not_registered() {
    assert!(CORSAIR_TOML.contains("pid = 0x1B09\nname = \"K70 Legacy\""));
    assert!(CORSAIR_TOML.contains("pid = 0x1B06\nname = \"M95\""));

    assert!(ProtocolDatabase::lookup(CORSAIR_VID, 0x1B09).is_none());
    assert!(ProtocolDatabase::lookup(CORSAIR_VID, 0x1B06).is_none());
}
