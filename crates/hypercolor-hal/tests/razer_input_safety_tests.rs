//! Input-safety contract for every Razer device.
//!
//! Spec 53 section 8.4 and Spec 17 section 7.2: Hypercolor must never
//! break a device's keyboard or pointer input. A transport that claims a
//! USB interface detaches the kernel driver bound to it, and on Razer
//! hardware the interface that carries the lighting feature report is
//! itself a HID input interface (the mouse collection on keyboards, mice,
//! and laptops alike). So every Razer descriptor must reach its device
//! through the host HID stack, which leaves `usbhid` bound.
//!
//! Blade and Book laptops get an explicit table below: their interface 2
//! is a class 3, mouse-protocol interface bound by `usbhid`, next to the
//! keyboard interfaces 0 and 1, and they once claimed it.

use std::collections::BTreeSet;

use hypercolor_hal::ProtocolDatabase;
use hypercolor_hal::drivers::razer::{self, RAZER_REPORT_LEN, RAZER_VENDOR_ID};
use hypercolor_hal::registry::{DeviceDescriptor, HidRawReportMode, TransportType};

/// Every registered Blade and Book laptop, by PID. Written out on purpose,
/// so adding or moving a laptop descriptor has to touch this table.
const LAPTOP_PIDS: &[u16] = &[
    0x0205, 0x020F, 0x0210, 0x0220, 0x0224, 0x0225, 0x022D, 0x022F, 0x0232, 0x0233, 0x0234, 0x0239,
    0x023A, 0x023B, 0x0240, 0x0245, 0x0246, 0x024A, 0x024C, 0x024D, 0x0252, 0x0253, 0x0255, 0x0256,
    0x0259, 0x0268, 0x026A, 0x026D, 0x026F, 0x0270, 0x0276, 0x0279, 0x027A, 0x028A, 0x028C, 0x029D,
];

/// The lighting path every laptop must use: the 90-byte feature report on
/// the mouse collection of interface 2, report ID 0, through HIDAPI.
const LAPTOP_TRANSPORT: TransportType = TransportType::UsbHidApi {
    interface: Some(2),
    report_id: 0x00,
    report_mode: HidRawReportMode::FeatureReport,
    max_report_len: RAZER_REPORT_LEN + 1,
    usage_page: Some(0x0001),
    usage: Some(0x0002),
};

/// Why a transport can take a device's input away, or `None` when it rides
/// the host HID stack.
///
/// The match is exhaustive on purpose: a new transport variant fails to
/// compile here until someone decides whether it can detach a kernel
/// driver.
fn detach_hazard(transport: &TransportType) -> Option<&'static str> {
    match transport {
        TransportType::UsbHidApi { .. } | TransportType::UsbHidRaw { .. } => None,
        TransportType::UsbControl { .. } => {
            Some("claims a USB interface for control transfers, detaching its kernel driver")
        }
        TransportType::UsbHid { .. } => {
            Some("claims the HID interface for interrupt endpoints, detaching usbhid")
        }
        TransportType::UsbBulk { .. } => {
            Some("claims a USB interface for bulk transfers, detaching its kernel driver")
        }
        TransportType::DriverUsb { .. } => {
            Some("is driver-owned and opens the USB device directly")
        }
        TransportType::UsbVendor => {
            Some("opens the whole USB device node for vendor control transfers")
        }
        TransportType::UsbSerial { .. } => Some("is a serial transport, not a HID lighting path"),
        TransportType::I2cSmBus { .. } => Some("is an SMBus transport, not a HID lighting path"),
    }
}

fn registered_razer_descriptors() -> Vec<&'static DeviceDescriptor> {
    ProtocolDatabase::all()
        .iter()
        .filter(|descriptor| descriptor.vendor_id == RAZER_VENDOR_ID)
        .collect()
}

fn is_laptop(descriptor: &DeviceDescriptor) -> bool {
    descriptor.name.starts_with("Razer Blade") || descriptor.name.starts_with("Razer Book")
}

#[test]
fn every_razer_descriptor_rides_the_host_hid_stack() {
    let descriptors = registered_razer_descriptors();
    assert!(
        descriptors.len() >= razer::descriptors().len(),
        "the database must carry every Razer module descriptor"
    );

    for descriptor in descriptors {
        if let Some(hazard) = detach_hazard(&descriptor.transport) {
            panic!(
                "{} ({:04X}:{:04X}) uses a transport that {hazard}: {:?}",
                descriptor.name, descriptor.vendor_id, descriptor.product_id, descriptor.transport
            );
        }
    }
}

#[test]
fn every_laptop_binds_the_interface_two_feature_report() {
    let laptops = registered_razer_descriptors()
        .into_iter()
        .filter(|descriptor| is_laptop(descriptor))
        .collect::<Vec<_>>();
    let registered = laptops
        .iter()
        .map(|descriptor| descriptor.product_id)
        .collect::<BTreeSet<_>>();
    let expected = LAPTOP_PIDS.iter().copied().collect::<BTreeSet<_>>();
    assert_eq!(
        expected.len(),
        LAPTOP_PIDS.len(),
        "LAPTOP_PIDS must not repeat a PID"
    );
    assert_eq!(
        registered, expected,
        "LAPTOP_PIDS must list exactly the registered Blade and Book laptops"
    );

    for descriptor in laptops {
        assert_eq!(
            descriptor.transport, LAPTOP_TRANSPORT,
            "{} ({:04X}) must reach its lighting through the interface 2 feature report",
            descriptor.name, descriptor.product_id
        );
        assert!(
            descriptor.firmware_predicate.is_none(),
            "{} must not select its transport by firmware",
            descriptor.name
        );
    }
}

/// A firmware-predicated alternate could smuggle a claiming transport back
/// in for one PID. Every descriptor the database holds for a laptop PID is
/// checked above, and this pins that there is only one per PID.
#[test]
fn every_laptop_pid_resolves_to_one_descriptor() {
    for &pid in LAPTOP_PIDS {
        let matches = registered_razer_descriptors()
            .into_iter()
            .filter(|descriptor| descriptor.product_id == pid)
            .count();
        assert_eq!(matches, 1, "{RAZER_VENDOR_ID:04X}:{pid:04X}");
    }
}

#[test]
fn detach_hazard_rejects_every_interface_claiming_transport() {
    for transport in [
        TransportType::UsbControl {
            interface: 2,
            report_id: 0,
        },
        TransportType::UsbHid { interface: 2 },
        TransportType::UsbBulk {
            interface: 2,
            report_id: 0,
        },
        TransportType::UsbVendor,
    ] {
        assert!(
            detach_hazard(&transport).is_some(),
            "{transport:?} must be rejected for Razer devices"
        );
    }
    assert!(detach_hazard(&LAPTOP_TRANSPORT).is_none());
}

#[test]
fn the_host_hid_transports_contain_no_interface_claim() {
    // Every Razer descriptor reaches one of these two modules. Neither may
    // gain a claim or detach call. This is a textual guard on the source,
    // not behavioral proof: it catches the direct nusb calls, not a detach
    // reached some other way.
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
