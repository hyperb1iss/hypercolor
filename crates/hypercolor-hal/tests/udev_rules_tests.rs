use std::collections::{BTreeMap, BTreeSet};

use hypercolor_hal::database::ProtocolDatabase;
use hypercolor_hal::registry::TransportType;

const UDEV_RULES: &str = include_str!("../../../udev/99-hypercolor.rules");
const I2C_UDEV_RULE: &str = "SUBSYSTEM==\"i2c-dev\", KERNEL==\"i2c-[0-9]*\", MODE=\"0660\", GROUP=\"users\", TAG+=\"uaccess\"";

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum RequiredSubsystem {
    Hidraw,
    I2cDev,
    Tty,
    Usb,
}

impl RequiredSubsystem {
    fn rule_line(self, vendor_id: u16) -> String {
        match self {
            Self::Hidraw => {
                format!(
                    "SUBSYSTEM==\"hidraw\", ATTRS{{idVendor}}==\"{vendor_id:04x}\", MODE=\"0660\", GROUP=\"users\", TAG+=\"uaccess\""
                )
            }
            Self::I2cDev => {
                "SUBSYSTEM==\"i2c-dev\", KERNEL==\"i2c-[0-9]*\", MODE=\"0660\", GROUP=\"users\", TAG+=\"uaccess\""
                    .to_owned()
            }
            Self::Tty => {
                format!(
                    "SUBSYSTEM==\"tty\", ATTRS{{idVendor}}==\"{vendor_id:04x}\", MODE=\"0660\", GROUP=\"users\", TAG+=\"uaccess\""
                )
            }
            Self::Usb => format!(
                "SUBSYSTEM==\"usb\", ENV{{DEVTYPE}}==\"usb_device\", ATTR{{idVendor}}==\"{vendor_id:04x}\", MODE=\"0660\", GROUP=\"users\", TAG+=\"uaccess\""
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
            TransportType::UsbHidApi { .. } | TransportType::UsbHidRaw { .. } => {
                required.insert(RequiredSubsystem::Hidraw);
                required.insert(RequiredSubsystem::Usb);
            }
            TransportType::UsbSerial { .. } => {
                required.insert(RequiredSubsystem::Tty);
            }
            TransportType::I2cSmBus { .. } => {
                required.insert(RequiredSubsystem::I2cDev);
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

#[test]
fn udev_rules_grant_generic_i2c_device_access() {
    let rule = RequiredSubsystem::I2cDev.rule_line(0);
    assert!(
        UDEV_RULES.contains(&rule),
        "missing generic i2c-dev access rule: {rule}"
    );
}

#[test]
fn udev_rules_include_i2c_access_for_smbus_devices() {
    assert!(
        UDEV_RULES.contains(I2C_UDEV_RULE),
        "missing SMBus i2c-dev udev rule: {I2C_UDEV_RULE}"
    );
}
