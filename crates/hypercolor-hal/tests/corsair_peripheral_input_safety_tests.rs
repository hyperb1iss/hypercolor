//! Input-safety contract for Corsair keyboards, mice, and desk accessories.
//!
//! Spec 53 sections 8.4 and 8.5: Hypercolor must never break a peripheral's
//! keyboard or pointer input. A transport that claims a USB interface
//! detaches the kernel driver bound to it, and on a keyboard or mouse that
//! driver is `usbhid`, so the device stops typing or pointing. The only
//! transports a Corsair peripheral may use are the host-managed HID paths
//! (HIDAPI and hidraw), bound to a vendor-defined lighting collection.

use std::collections::BTreeMap;

use hypercolor_hal::ProtocolDatabase;
use hypercolor_hal::drivers::corsair::CORSAIR_VID;
use hypercolor_hal::drivers::corsair::peripheral::devices as peripherals;
use hypercolor_hal::registry::{DeviceDescriptor, TransportType};

const CORSAIR_TOML: &str = include_str!("../../../data/drivers/vendors/corsair.toml");

/// Device types in the vendor database that carry live input collections or
/// share a USB device with them.
const PERIPHERAL_TYPES: &[&str] = &["keyboard", "mouse", "mousepad", "headset"];

/// First vendor-defined HID usage page.
const VENDOR_USAGE_PAGE_START: u16 = 0xFF00;

/// Why a transport is unsafe for a peripheral, or `None` when it is safe.
///
/// The match is exhaustive on purpose: a new transport variant fails to
/// compile here until someone decides whether it can detach a kernel
/// driver.
fn input_hazard(transport: &TransportType) -> Option<&'static str> {
    match transport {
        TransportType::UsbHidApi { usage_page, .. }
        | TransportType::UsbHidRaw { usage_page, .. } => match usage_page {
            Some(page) if *page >= VENDOR_USAGE_PAGE_START => None,
            Some(_) => Some("binds a non-vendor HID collection"),
            None => Some("has no usage page filter, so it may bind an input collection"),
        },
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

/// `pid -> type` for every device in the Corsair vendor database.
fn corsair_device_types() -> BTreeMap<u16, String> {
    CORSAIR_TOML
        .split("[[devices]]")
        .skip(1)
        .filter_map(|entry| {
            let pid = entry
                .lines()
                .find_map(|line| line.trim().strip_prefix("pid = 0x"))
                .and_then(|hex| u16::from_str_radix(hex.trim(), 16).ok())?;
            let kind = entry
                .lines()
                .find_map(|line| line.trim().strip_prefix("type = "))
                .map(|value| value.trim().trim_matches('"').to_owned())?;
            Some((pid, kind))
        })
        .collect()
}

fn registered_corsair_descriptors() -> impl Iterator<Item = &'static DeviceDescriptor> {
    ProtocolDatabase::all()
        .iter()
        .filter(|descriptor| descriptor.vendor_id == CORSAIR_VID)
}

#[test]
fn every_corsair_peripheral_descriptor_is_input_safe() {
    let peripheral_pids = peripherals::descriptors()
        .iter()
        .map(|descriptor| descriptor.product_id)
        .collect::<Vec<_>>();
    let device_types = corsair_device_types();

    let mut checked = 0_usize;
    for descriptor in registered_corsair_descriptors() {
        let is_peripheral = peripheral_pids.contains(&descriptor.product_id)
            || device_types
                .get(&descriptor.product_id)
                .is_some_and(|kind| PERIPHERAL_TYPES.contains(&kind.as_str()));
        if !is_peripheral {
            continue;
        }

        checked += 1;
        if let Some(hazard) = input_hazard(&descriptor.transport) {
            panic!(
                "{} ({:04X}:{:04X}) uses a transport that {hazard}: {:?}",
                descriptor.name, descriptor.vendor_id, descriptor.product_id, descriptor.transport
            );
        }
    }

    assert!(
        checked >= peripheral_pids.len(),
        "every peripheral descriptor should have been checked"
    );
}

/// Every descriptor the database holds for a peripheral PID is checked
/// above, so a firmware-predicated alternate cannot smuggle in a
/// direct-endpoint fallback. This pins that each peripheral PID resolves to
/// exactly one descriptor, so there is no alternate to fall back to at all.
#[test]
fn corsair_peripherals_have_no_alternate_transport_descriptor() {
    for descriptor in peripherals::descriptors() {
        let matches = ProtocolDatabase::all()
            .iter()
            .filter(|candidate| {
                candidate.vendor_id == descriptor.vendor_id
                    && candidate.product_id == descriptor.product_id
            })
            .count();
        assert_eq!(
            matches, 1,
            "{} ({:04X}) must have exactly one descriptor",
            descriptor.name, descriptor.product_id
        );
        assert!(
            descriptor.firmware_predicate.is_none(),
            "{} must not select its transport by firmware",
            descriptor.name
        );
    }
}

#[test]
fn input_hazard_rejects_every_interface_claiming_transport() {
    for transport in [
        TransportType::UsbControl {
            interface: 1,
            report_id: 0,
        },
        TransportType::UsbHid { interface: 1 },
        TransportType::UsbBulk {
            interface: 1,
            report_id: 0,
        },
        TransportType::UsbVendor,
    ] {
        assert!(
            input_hazard(&transport).is_some(),
            "{transport:?} must be rejected for peripherals"
        );
    }
}

/// A usage page alone is not enough: NXP keyboards and mice expose an
/// input-only `0xFFC2` collection on their composite input interface, and
/// Bragi spreads `0xFF42` over two interfaces. Every peripheral pins the
/// interface that carries its command collection.
#[test]
fn corsair_peripherals_pin_their_lighting_interface() {
    for descriptor in peripherals::descriptors() {
        let pinned = match descriptor.transport {
            TransportType::UsbHidApi { interface, .. } => interface.is_some(),
            TransportType::UsbHidRaw { .. } => true,
            _ => false,
        };
        assert!(
            pinned,
            "{} must pin the interface of its lighting collection",
            descriptor.name
        );
    }
}

#[test]
fn nxp_keyboards_and_mice_bind_only_the_command_collection() {
    let TransportType::UsbHidApi {
        interface,
        usage_page,
        usage,
        ..
    } = peripherals::NXP_INPUT_DEVICE_TRANSPORT
    else {
        panic!("NXP input devices must use HIDAPI");
    };

    assert_eq!(interface, Some(peripherals::NXP_INTERFACE));
    assert_eq!(usage_page, Some(peripherals::NXP_USAGE_PAGE));
    assert_eq!(usage, Some(peripherals::NXP_COMMAND_USAGE));
}
