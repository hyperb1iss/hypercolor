//! Wooting descriptor registration and input-safety tests (Spec 90 §2, §8).

use std::collections::{BTreeMap, BTreeSet};

use hypercolor_hal::database::ProtocolDatabase;
use hypercolor_hal::drivers::wooting::{
    CONFIG_USAGE_PAGE, MODELS, MULTI_REPORT_USAGE_PAGE, WOOTING_FAMILY, WOOTING_LEGACY_VID,
    WOOTING_VID, WootingInterface, descriptors, transport_for,
};
use hypercolor_hal::registry::{HidRawReportMode, TransportType};

/// (VID, PID, protocol ID) for every registered Wooting product (Spec 90 §2).
const EXPECTED: &[(u16, u16, &str)] = &[
    (0x03EB, 0xFF01, "wooting/one-v1"),
    (0x03EB, 0xFF02, "wooting/two-v1"),
    (0x31E3, 0x1100, "wooting/one"),
    (0x31E3, 0x1101, "wooting/one"),
    (0x31E3, 0x1102, "wooting/one"),
    (0x31E3, 0x1200, "wooting/two"),
    (0x31E3, 0x1201, "wooting/two"),
    (0x31E3, 0x1202, "wooting/two"),
    (0x31E3, 0x1210, "wooting/two-lekker"),
    (0x31E3, 0x1211, "wooting/two-lekker"),
    (0x31E3, 0x1212, "wooting/two-lekker"),
    (0x31E3, 0x1220, "wooting/two-he"),
    (0x31E3, 0x1221, "wooting/two-he"),
    (0x31E3, 0x1222, "wooting/two-he"),
    (0x31E3, 0x1230, "wooting/two-he-arm"),
    (0x31E3, 0x1231, "wooting/two-he-arm"),
    (0x31E3, 0x1232, "wooting/two-he-arm"),
    (0x31E3, 0x1300, "wooting/60he"),
    (0x31E3, 0x1301, "wooting/60he"),
    (0x31E3, 0x1302, "wooting/60he"),
    (0x31E3, 0x1310, "wooting/60he-arm"),
    (0x31E3, 0x1311, "wooting/60he-arm"),
    (0x31E3, 0x1312, "wooting/60he-arm"),
    (0x31E3, 0x1320, "wooting/60he-plus"),
    (0x31E3, 0x1321, "wooting/60he-plus"),
    (0x31E3, 0x1322, "wooting/60he-plus"),
    (0x31E3, 0x1340, "wooting/60he-v2"),
    (0x31E3, 0x1341, "wooting/60he-v2"),
    (0x31E3, 0x1342, "wooting/60he-v2"),
    (0x31E3, 0x1400, "wooting/80he"),
    (0x31E3, 0x1401, "wooting/80he"),
    (0x31E3, 0x1402, "wooting/80he"),
    (0x31E3, 0x1510, "wooting/uwu-rgb"),
    (0x31E3, 0x1511, "wooting/uwu-rgb"),
    (0x31E3, 0x1512, "wooting/uwu-rgb"),
];

/// Products deliberately left unregistered (Spec 90 §2.3).
const NOT_REGISTERED: &[(u16, u16)] = &[
    // UwU without RGB: no addressable LEDs.
    (0x31E3, 0x1500),
    (0x31E3, 0x1501),
    (0x31E3, 0x1502),
    // Update-mode (bootloader) PIDs: never drive lighting there.
    (0x03EB, 0x2402),
    (0x03EB, 0x2403),
];

#[test]
fn every_wooting_product_has_exactly_one_descriptor() {
    let wooting = descriptors();
    assert_eq!(wooting.len(), EXPECTED.len());

    for &(vendor_id, product_id, protocol_id) in EXPECTED {
        let matches: Vec<_> = wooting
            .iter()
            .filter(|descriptor| {
                descriptor.vendor_id == vendor_id && descriptor.product_id == product_id
            })
            .collect();
        assert_eq!(matches.len(), 1, "{vendor_id:04X}:{product_id:04X}");
        let descriptor = matches[0];
        assert_eq!(descriptor.protocol.id, protocol_id);
        assert_eq!(descriptor.driver_id(), "wooting");
        assert_eq!(descriptor.family, WOOTING_FAMILY);
        assert!(descriptor.name.starts_with("Wooting "));
        assert!(descriptor.firmware_predicate.is_none());
        assert!(descriptor.serial_quirk.is_none());
    }
}

#[test]
fn database_resolves_every_wooting_product() {
    for &(vendor_id, product_id, protocol_id) in EXPECTED {
        let descriptor = ProtocolDatabase::lookup(vendor_id, product_id)
            .unwrap_or_else(|| panic!("{vendor_id:04X}:{product_id:04X} must resolve"));
        assert_eq!(descriptor.protocol.id, protocol_id);
        let protocol = (descriptor.protocol.build)();
        assert!(protocol.total_leds() > 0);
        assert!(!protocol.init_sequence().is_empty());
        assert!(!protocol.shutdown_sequence().is_empty());
    }
    for &(vendor_id, product_id) in NOT_REGISTERED {
        assert!(
            ProtocolDatabase::lookup(vendor_id, product_id).is_none(),
            "{vendor_id:04X}:{product_id:04X} must stay unregistered"
        );
    }
}

#[test]
fn gamepad_variants_share_one_protocol_and_transport() {
    let mut by_protocol: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    for descriptor in descriptors() {
        by_protocol
            .entry(descriptor.protocol.id)
            .or_default()
            .insert(format!("{:?}|{}", descriptor.transport, descriptor.name));
    }
    for (protocol_id, shapes) in by_protocol {
        assert_eq!(
            shapes.len(),
            1,
            "{protocol_id} variants must agree on transport and name"
        );
    }
}

#[test]
fn generations_bind_the_collection_their_firmware_exposes() {
    for model in MODELS {
        let expected_page = match model.interface {
            WootingInterface::V1 | WootingInterface::V2 { .. } => CONFIG_USAGE_PAGE,
            WootingInterface::V3 => MULTI_REPORT_USAGE_PAGE,
        };
        let TransportType::UsbHidApi {
            usage_page,
            report_mode,
            max_report_len,
            ..
        } = transport_for(model.interface)
        else {
            panic!("{} must use the HID stack", model.name);
        };
        assert_eq!(usage_page, Some(expected_page), "{}", model.name);
        assert_eq!(report_mode, HidRawReportMode::OutputReportWithReportId);
        let expected_len = match model.interface {
            WootingInterface::V1 => 129,
            WootingInterface::V2 { .. } => 257,
            WootingInterface::V3 => 2047,
        };
        assert_eq!(max_report_len, expected_len, "{}", model.name);

        let legacy = model.vendor_id == WOOTING_LEGACY_VID;
        assert_eq!(legacy, model.interface == WootingInterface::V1);
        assert!(legacy || model.vendor_id == WOOTING_VID);
    }
}

// ── Input safety (Spec 90 §8.2) ──────────────────────────────────────────

/// Transports that reach a device through the host HID stack. Neither
/// opens the USB device node or claims an interface, so the kernel's
/// keyboard driver stays bound.
fn rides_the_host_hid_stack(transport: TransportType) -> bool {
    matches!(
        transport,
        TransportType::UsbHidApi { .. } | TransportType::UsbHidRaw { .. }
    )
}

#[test]
fn wooting_transports_cannot_detach_a_kernel_driver() {
    for descriptor in descriptors() {
        assert!(
            rides_the_host_hid_stack(descriptor.transport),
            "{} ({:04X}:{:04X}) must not use a transport that claims or detaches \
             a USB interface: {:?}",
            descriptor.name,
            descriptor.vendor_id,
            descriptor.product_id,
            descriptor.transport
        );
        let TransportType::UsbHidApi {
            interface,
            usage_page,
            usage,
            ..
        } = descriptor.transport
        else {
            unreachable!("checked above");
        };
        assert!(
            matches!(usage_page, Some(page) if page >= 0xFF00 || page == CONFIG_USAGE_PAGE),
            "{} must select the vendor configuration collection, never a \
             generic desktop or consumer page",
            descriptor.name
        );
        assert_eq!(interface, None, "{} selects by usage page", descriptor.name);
        assert_eq!(usage, None);
    }
}

#[test]
fn the_hid_stack_transports_contain_no_interface_claim() {
    // The descriptors above only ever reach these two modules. Neither may
    // gain a claim or detach call, since a detached keyboard stops typing.
    // This is a textual guard on the source, not behavioral proof: it
    // catches the direct nusb calls, not a detach reached some other way.
    for (name, source) in [
        (
            "transport/hidapi.rs",
            include_str!("../src/transport/hidapi.rs"),
        ),
        (
            "transport/hidraw.rs",
            include_str!("../src/transport/hidraw.rs"),
        ),
    ] {
        for forbidden in [
            "detach_and_claim_interface",
            "claim_interface",
            "detach_kernel_driver",
        ] {
            assert!(
                !source.contains(forbidden),
                "{name} must not call {forbidden}"
            );
        }
    }
}
