use std::collections::{BTreeMap, BTreeSet};

use hypercolor_hal::database::ProtocolDatabase;
use hypercolor_hal::registry::TransportType;

const UDEV_RULES: &str = include_str!("../../../udev/99-hypercolor.rules");

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum RequiredSubsystem {
    Hidraw,
    Tty,
    Usb,
}

impl RequiredSubsystem {
    fn rule_line(self, vendor_id: u16) -> String {
        match self {
            Self::Hidraw => {
                format!(
                    "SUBSYSTEM==\"hidraw\", ATTRS{{idVendor}}==\"{vendor_id:04x}\", TAG+=\"uaccess\""
                )
            }
            Self::Tty => {
                format!(
                    "SUBSYSTEM==\"tty\", ATTRS{{idVendor}}==\"{vendor_id:04x}\", TAG+=\"uaccess\""
                )
            }
            Self::Usb => format!(
                "SUBSYSTEM==\"usb\", ENV{{DEVTYPE}}==\"usb_device\", ATTR{{idVendor}}==\"{vendor_id:04x}\", TAG+=\"uaccess\""
            ),
        }
    }
}

#[test]
fn udev_rules_cover_each_supported_vendor_transport_family() {
    let mut required_rules: BTreeMap<u16, BTreeSet<RequiredSubsystem>> = BTreeMap::new();

    for descriptor in ProtocolDatabase::all() {
        let required = required_rules.entry(descriptor.vendor_id).or_default();
        match descriptor.transport {
            TransportType::UsbHidRaw { .. } => {
                required.insert(RequiredSubsystem::Hidraw);
            }
            TransportType::UsbSerial { .. } => {
                required.insert(RequiredSubsystem::Tty);
            }
            TransportType::UsbControl { .. }
            | TransportType::UsbHid { .. }
            | TransportType::UsbBulk { .. }
            | TransportType::UsbVendor => {
                required.insert(RequiredSubsystem::Usb);
            }
        }
    }

    for (vendor_id, subsystems) in required_rules {
        for subsystem in subsystems {
            let expected_rule = subsystem.rule_line(vendor_id);
            assert!(
                UDEV_RULES.contains(&expected_rule),
                "missing vendor-wide udev rule for vendor {vendor_id:04x} subsystem {subsystem:?}: {expected_rule}"
            );
        }
    }
}
