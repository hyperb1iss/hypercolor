use std::collections::BTreeSet;

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
    fn vendor_rule_line(self, vendor_id: u16) -> String {
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

    fn product_rule_line(self, vendor_id: u16, product_id: u16) -> Option<String> {
        match self {
            Self::Hidraw => Some(format!(
                "SUBSYSTEM==\"hidraw\", ATTRS{{idVendor}}==\"{vendor_id:04x}\", ATTRS{{idProduct}}==\"{product_id:04x}\", MODE=\"0660\", GROUP=\"users\", TAG+=\"uaccess\""
            )),
            Self::I2cDev => None,
            Self::Tty => Some(format!(
                "SUBSYSTEM==\"tty\", ATTRS{{idVendor}}==\"{vendor_id:04x}\", ATTRS{{idProduct}}==\"{product_id:04x}\", MODE=\"0660\", GROUP=\"users\", TAG+=\"uaccess\""
            )),
            Self::Usb => Some(format!(
                "SUBSYSTEM==\"usb\", ENV{{DEVTYPE}}==\"usb_device\", ATTR{{idVendor}}==\"{vendor_id:04x}\", ATTR{{idProduct}}==\"{product_id:04x}\", MODE=\"0660\", GROUP=\"users\", TAG+=\"uaccess\""
            )),
        }
    }
}

#[test]
fn udev_rules_cover_each_supported_vendor_transport_family() {
    let mut required_rules: BTreeSet<(u16, u16, RequiredSubsystem)> = BTreeSet::new();

    for descriptor in ProtocolDatabase::all() {
        match descriptor.transport {
            TransportType::UsbHidApi { .. } | TransportType::UsbHidRaw { .. } => {
                required_rules.insert((
                    descriptor.vendor_id,
                    descriptor.product_id,
                    RequiredSubsystem::Hidraw,
                ));
                required_rules.insert((
                    descriptor.vendor_id,
                    descriptor.product_id,
                    RequiredSubsystem::Usb,
                ));
            }
            TransportType::UsbSerial { .. } => {
                required_rules.insert((
                    descriptor.vendor_id,
                    descriptor.product_id,
                    RequiredSubsystem::Tty,
                ));
            }
            TransportType::I2cSmBus { .. } => {
                required_rules.insert((
                    descriptor.vendor_id,
                    descriptor.product_id,
                    RequiredSubsystem::I2cDev,
                ));
            }
            TransportType::UsbControl { .. }
            | TransportType::UsbHid { .. }
            | TransportType::UsbBulk { .. }
            | TransportType::UsbMidi { .. }
            | TransportType::UsbVendor => {
                required_rules.insert((
                    descriptor.vendor_id,
                    descriptor.product_id,
                    RequiredSubsystem::Usb,
                ));
            }
        }
    }

    for (vendor_id, product_id, subsystem) in required_rules {
        let vendor_rule = subsystem.vendor_rule_line(vendor_id);
        let product_rule = subsystem.product_rule_line(vendor_id, product_id);
        let has_product_rule = product_rule
            .as_ref()
            .is_some_and(|rule| UDEV_RULES.contains(rule));
        assert!(
            UDEV_RULES.contains(&vendor_rule) || has_product_rule,
            "missing udev rule for vendor {vendor_id:04x} product {product_id:04x} subsystem {subsystem:?}: expected {vendor_rule}{}",
            product_rule
                .as_ref()
                .map_or_else(String::new, |rule| format!(" or {rule}"))
        );
    }
}

#[test]
fn udev_rules_grant_generic_i2c_device_access() {
    let rule = RequiredSubsystem::I2cDev.vendor_rule_line(0);
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
