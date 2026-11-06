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
use hypercolor_hal::drivers::corsair::peripheral::devices::{
    NXP_ACCESSORY_INTERFACE, NXP_COMMAND_USAGE, NXP_INTERFACE, NXP_MAX_REPORT_LEN,
    NXP_POLARIS_USAGE, NXP_REPORT_ID, NXP_USAGE_PAGE, PID_K55_RGB, PID_K65_LUX_RGB, PID_K65_RGB,
    PID_K70_LUX_RGB, PID_K70_RGB, PID_K70_RGB_MK2, PID_K95_RGB, PID_K95_RGB_PLATINUM, PID_M65_RGB,
    PID_MM800_POLARIS, PID_SCIMITAR_RGB, PID_ST100_RGB, PID_STRAFE_RGB,
};
use hypercolor_hal::drivers::corsair::peripheral::types::{
    BRAGI_JUMBO_PACKET_SIZE, BRAGI_LARGE_PACKET_SIZE, BRAGI_PACKET_SIZE, NXP_KEYBOARD_LED_SLOTS,
    NXP_MOUSE_ZONE_SLOTS, NXP_MOUSEPAD_ZONE_SLOTS,
};
use hypercolor_hal::drivers::corsair::{CORSAIR_USAGE_PAGE, CORSAIR_VID};
use hypercolor_hal::registry::{HidRawReportMode, TransportType};
use hypercolor_types::device::DeviceTopologyHint;

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
    assert!(ProtocolDatabase::lookup(CORSAIR_VID, 0x1B7F).is_none());
    assert!(ProtocolDatabase::lookup(CORSAIR_VID, 0x1B81).is_none());
    assert!(ProtocolDatabase::lookup(CORSAIR_VID, 0x1BA6).is_none());
}

#[test]
fn nxp_devices_register_on_the_vendor_lighting_collection() {
    let input_devices = [
        (PID_K55_RGB, 3),
        (PID_K65_RGB, NXP_KEYBOARD_LED_SLOTS),
        (PID_K65_LUX_RGB, NXP_KEYBOARD_LED_SLOTS),
        (PID_K70_RGB, NXP_KEYBOARD_LED_SLOTS),
        (PID_K70_LUX_RGB, NXP_KEYBOARD_LED_SLOTS),
        (PID_K70_RGB_MK2, NXP_KEYBOARD_LED_SLOTS),
        (PID_K95_RGB, NXP_KEYBOARD_LED_SLOTS),
        (PID_K95_RGB_PLATINUM, NXP_KEYBOARD_LED_SLOTS + 19),
        (PID_STRAFE_RGB, NXP_KEYBOARD_LED_SLOTS),
        (PID_M65_RGB, NXP_MOUSE_ZONE_SLOTS),
        (PID_SCIMITAR_RGB, NXP_MOUSE_ZONE_SLOTS),
    ];
    let expected_transport = |interface, usage| TransportType::UsbHidApi {
        interface: Some(interface),
        report_id: NXP_REPORT_ID,
        report_mode: HidRawReportMode::OutputReport,
        max_report_len: NXP_MAX_REPORT_LEN,
        usage_page: Some(NXP_USAGE_PAGE),
        usage,
    };
    let input_transport = expected_transport(NXP_INTERFACE, Some(NXP_COMMAND_USAGE));
    let mut expected = input_devices
        .iter()
        .map(|&(pid, leds)| (pid, leds, input_transport))
        .collect::<Vec<_>>();
    expected.push((
        PID_MM800_POLARIS,
        NXP_MOUSEPAD_ZONE_SLOTS,
        expected_transport(NXP_ACCESSORY_INTERFACE, Some(NXP_POLARIS_USAGE)),
    ));
    expected.push((
        PID_ST100_RGB,
        NXP_MOUSEPAD_ZONE_SLOTS,
        expected_transport(NXP_ACCESSORY_INTERFACE, None),
    ));

    for (pid, led_count, transport) in expected {
        let descriptor = ProtocolDatabase::lookup(CORSAIR_VID, pid)
            .unwrap_or_else(|| panic!("missing NXP descriptor for PID {pid:#06X}"));
        let protocol = (descriptor.protocol.build)();

        assert_eq!(descriptor.transport, transport, "{}", descriptor.name);
        assert!(
            descriptor.protocol.id.starts_with("corsair/nxp-"),
            "{}",
            descriptor.name
        );
        assert_eq!(
            protocol.total_leds(),
            u32::try_from(led_count).expect("LED count fits u32"),
            "{}",
            descriptor.name
        );
        assert!(
            protocol.capabilities().supports_direct,
            "{}",
            descriptor.name
        );
    }
}

#[test]
fn nxp_keyboards_report_their_hardware_index_space_as_custom_topology() {
    let descriptor = ProtocolDatabase::lookup(CORSAIR_VID, PID_K70_RGB)
        .expect("K70 RGB descriptor should exist");
    let zones = (descriptor.protocol.build)().zones();

    assert_eq!(zones.len(), 1);
    assert_eq!(zones[0].led_count, 144);
    assert_eq!(zones[0].topology, DeviceTopologyHint::Custom);
}

#[test]
fn k95_platinum_exposes_its_light_bar_as_a_left_to_right_zone() {
    let descriptor = ProtocolDatabase::lookup(CORSAIR_VID, PID_K95_RGB_PLATINUM)
        .expect("K95 Platinum descriptor should exist");
    let protocol = (descriptor.protocol.build)();
    let zones = protocol.zones();

    assert_eq!(protocol.total_leds(), 163);
    assert_eq!(zones.len(), 2);
    assert_eq!(zones[0].led_count, 144);
    assert_eq!(zones[0].topology, DeviceTopologyHint::Custom);
    assert_eq!(zones[1].name, "Light Bar");
    assert_eq!(zones[1].led_count, 19);
    assert_eq!(
        zones[1].topology,
        DeviceTopologyHint::Matrix { rows: 1, cols: 19 }
    );
}

#[test]
fn packed_512_keyboards_keep_the_144_slot_space() {
    for pid in [PID_K65_RGB, PID_K70_RGB, PID_K95_RGB] {
        let descriptor =
            ProtocolDatabase::lookup(CORSAIR_VID, pid).expect("packed keyboard should exist");
        let protocol = (descriptor.protocol.build)();

        assert_eq!(protocol.total_leds(), 144, "{}", descriptor.name);
        assert_eq!(protocol.zones().len(), 1, "{}", descriptor.name);
    }
}

#[test]
fn held_peripherals_stay_cataloged_but_unregistered() {
    let held = [
        (0x1B41, "K66"),
        (0x1B07, "K65 Legacy"),
        (0x1B09, "K70 Legacy"),
        (0x1B02, "K90 Legacy"),
        (0x1B08, "K95 Legacy"),
        (0x1B06, "M95"),
    ];

    for (pid, name) in held {
        assert!(
            CORSAIR_TOML.contains(&format!("pid = 0x{pid:04X}\nname = \"{name}\"")),
            "{name} should stay in the vendor database"
        );
        assert!(
            ProtocolDatabase::lookup(CORSAIR_VID, pid).is_none(),
            "{name} must not be registered"
        );
    }
}

/// Registration and the vendor database must agree: every registered
/// peripheral is `supported`, and nothing still marked `researched` is
/// registered.
#[test]
fn vendor_database_status_matches_peripheral_registration() {
    for entry in CORSAIR_TOML.split("[[devices]]").skip(1) {
        let Some(pid) = entry
            .lines()
            .find_map(|line| line.trim().strip_prefix("pid = 0x"))
            .and_then(|hex| u16::from_str_radix(hex.trim(), 16).ok())
        else {
            continue;
        };
        let registered = ProtocolDatabase::lookup(CORSAIR_VID, pid).is_some();
        if entry.contains("status = \"researched\"") {
            assert!(
                !registered,
                "researched PID {pid:#06X} must not be registered"
            );
        }
    }

    for descriptor in descriptors() {
        let marker = format!("pid = 0x{:04X}\n", descriptor.product_id);
        let entry = CORSAIR_TOML
            .split("[[devices]]")
            .find(|entry| entry.contains(&marker))
            .unwrap_or_else(|| panic!("{} is missing from corsair.toml", descriptor.name));
        assert!(
            entry.contains("status = \"supported\""),
            "{} is registered but not marked supported",
            descriptor.name
        );
    }
}
