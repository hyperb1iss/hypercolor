use hypercolor_hal::ProtocolDatabase;
use hypercolor_hal::drivers::corsair::peripheral::devices::{
    BRAGI_INTERFACE, BRAGI_REPORT_ID, PID_DARK_CORE_RGB_PRO_SE_WIRED, PID_DARK_CORE_RGB_SE_WIRED,
    PID_HARPOON_WIRELESS_WIRED, PID_IRONCLAW_WIRELESS_WIRED, PID_K55_RGB_PRO, PID_K60_PRO_MONO,
    PID_K60_PRO_RGB, PID_K60_PRO_RGB_LOW_PROFILE, PID_K60_PRO_RGB_SE, PID_K60_PRO_TKL,
    PID_K60_PRO_TKL_WHITE, PID_K65_MINI, PID_K70_CORE_RGB, PID_K70_CORE_RGB_TKL,
    PID_K70_CORE_RGB_VARIANT_2, PID_K70_CORE_RGB_VARIANT_3, PID_K70_PRO, PID_K70_PRO_OPTICAL,
    PID_K70_RGB_PRO, PID_K70_TKL, PID_K70_TKL_CHAMPION_OPTICAL, PID_K95_PLATINUM_XT,
    PID_K100_MX_RED, PID_K100_OPTICAL_V1, PID_K100_OPTICAL_V2, PID_KATAR_PRO, PID_KATAR_PRO_V2,
    PID_KATAR_PRO_XT, PID_M55_RGB_PRO, PID_M65_RGB_ULTRA_WIRED, PID_M65_RGB_ULTRA_WIRELESS_WIRED,
    PID_M75_GAMING_MOUSE, PID_MM700, PID_MM700_3XL, PID_SCIMITAR_ELITE_BRAGI, descriptors,
};
use hypercolor_hal::drivers::corsair::peripheral::types::{
    BRAGI_JUMBO_PACKET_SIZE, BRAGI_LARGE_PACKET_SIZE, BRAGI_PACKET_SIZE,
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
fn supported_bragi_usb_ids_are_registered_with_expected_led_counts() {
    let expected = [
        (PID_K55_RGB_PRO, 6, BRAGI_PACKET_SIZE),
        (PID_K60_PRO_RGB, 123, BRAGI_PACKET_SIZE),
        (PID_K60_PRO_RGB_LOW_PROFILE, 123, BRAGI_PACKET_SIZE),
        (PID_K60_PRO_RGB_SE, 123, BRAGI_PACKET_SIZE),
        (PID_K60_PRO_MONO, 123, BRAGI_PACKET_SIZE),
        (PID_K60_PRO_TKL, 123, BRAGI_PACKET_SIZE),
        (PID_K60_PRO_TKL_WHITE, 123, BRAGI_PACKET_SIZE),
        (PID_K65_MINI, 123, BRAGI_JUMBO_PACKET_SIZE),
        (PID_K70_TKL, 193, BRAGI_JUMBO_PACKET_SIZE),
        (PID_K70_TKL_CHAMPION_OPTICAL, 193, BRAGI_JUMBO_PACKET_SIZE),
        (PID_K70_RGB_PRO, 193, BRAGI_JUMBO_PACKET_SIZE),
        (PID_K70_PRO, 193, BRAGI_JUMBO_PACKET_SIZE),
        (PID_K70_PRO_OPTICAL, 193, BRAGI_JUMBO_PACKET_SIZE),
        (PID_K70_CORE_RGB, 123, BRAGI_PACKET_SIZE),
        (PID_K70_CORE_RGB_VARIANT_2, 123, BRAGI_PACKET_SIZE),
        (PID_K70_CORE_RGB_VARIANT_3, 123, BRAGI_PACKET_SIZE),
        (PID_K70_CORE_RGB_TKL, 102, BRAGI_PACKET_SIZE),
        (PID_K95_PLATINUM_XT, 156, BRAGI_PACKET_SIZE),
        (PID_K100_OPTICAL_V1, 193, BRAGI_JUMBO_PACKET_SIZE),
        (PID_K100_OPTICAL_V2, 193, BRAGI_JUMBO_PACKET_SIZE),
        (PID_K100_MX_RED, 193, BRAGI_JUMBO_PACKET_SIZE),
        (PID_DARK_CORE_RGB_SE_WIRED, 12, BRAGI_PACKET_SIZE),
        (PID_DARK_CORE_RGB_PRO_SE_WIRED, 12, BRAGI_PACKET_SIZE),
        (PID_HARPOON_WIRELESS_WIRED, 2, BRAGI_PACKET_SIZE),
        (PID_IRONCLAW_WIRELESS_WIRED, 6, BRAGI_PACKET_SIZE),
        (PID_M55_RGB_PRO, 2, BRAGI_PACKET_SIZE),
        (PID_KATAR_PRO, 2, BRAGI_PACKET_SIZE),
        (PID_KATAR_PRO_V2, 2, BRAGI_PACKET_SIZE),
        (PID_KATAR_PRO_XT, 2, BRAGI_PACKET_SIZE),
        (PID_M65_RGB_ULTRA_WIRED, 3, BRAGI_PACKET_SIZE),
        (PID_M65_RGB_ULTRA_WIRELESS_WIRED, 2, BRAGI_PACKET_SIZE),
        (PID_M75_GAMING_MOUSE, 2, BRAGI_PACKET_SIZE),
        (PID_SCIMITAR_ELITE_BRAGI, 5, BRAGI_LARGE_PACKET_SIZE),
        (PID_MM700, 3, BRAGI_PACKET_SIZE),
        (PID_MM700_3XL, 3, BRAGI_PACKET_SIZE),
    ];

    for (pid, led_count, packet_size) in expected {
        let descriptor = ProtocolDatabase::lookup(CORSAIR_VID, pid)
            .unwrap_or_else(|| panic!("missing descriptor for PID {pid:#06X}"));
        let TransportType::UsbHidApi { max_report_len, .. } = descriptor.transport else {
            panic!("expected HIDAPI transport for {}", descriptor.name);
        };
        let protocol = (descriptor.protocol.build)();

        assert_eq!(max_report_len, packet_size, "{}", descriptor.name);
        assert_eq!(protocol.total_leds(), led_count, "{}", descriptor.name);
    }
}

#[test]
fn bragi_dongles_are_researched_but_not_registered_as_rgb_devices() {
    assert!(CORSAIR_TOML.contains("pid = 0x1B62\nname = \"K57 Wireless Dongle\""));
    assert!(CORSAIR_TOML.contains("pid = 0x1BA6\nname = \"Generic Bragi Dongle\""));

    assert!(ProtocolDatabase::lookup(CORSAIR_VID, 0x1B62).is_none());
    assert!(ProtocolDatabase::lookup(CORSAIR_VID, 0x1B65).is_none());
    assert!(ProtocolDatabase::lookup(CORSAIR_VID, 0x1B66).is_none());
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
