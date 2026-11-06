use std::collections::BTreeSet;

use hypercolor_hal::database::ProtocolDatabase;
use hypercolor_hal::drivers::corsair::CORSAIR_VID;
use hypercolor_hal::drivers::corsair::peripheral::devices as corsair_peripherals;
use hypercolor_hal::drivers::wooting::{
    WOOTING_FAMILY, WOOTING_LEGACY_VID, WOOTING_VID, descriptors as wooting_descriptors,
};
use hypercolor_hal::registry::TransportType;

const UDEV_RULES: &str = include_str!("../../../udev/99-hypercolor.rules");
const UDEV_INPUT_RULES: &str = include_str!("../../../udev/70-hypercolor-input.rules");
const UDEV_INPUT_ALL_RULES: &str = include_str!("../../../udev/70-hypercolor-input-all.rules");
/// udev property the Corsair peripheral rules use to tag a product before
/// granting its lighting-interface hidraw node.
const CORSAIR_LIGHTING_IF_KEY: &str = "HYPERCOLOR_CORSAIR_LIGHTING_IF";
const I2C_UDEV_RULE: &str = "SUBSYSTEM==\"i2c-dev\", KERNEL==\"i2c-[0-9]*\", MODE=\"0660\", GROUP=\"users\", TAG+=\"uaccess\"";

fn input_rule_lines(rules: &str) -> Vec<&str> {
    rules
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('#') && line.contains("SUBSYSTEM==\"input\""))
        .collect()
}

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
        // Wooting keyboards follow the interface-scoped policy checked in
        // `wooting_udev_rules_open_only_evidenced_configuration_interfaces`.
        if descriptor.family == WOOTING_FAMILY {
            continue;
        }
        match descriptor.transport {
            // Host-managed HID opens /dev/hidraw* (directly or through
            // HIDAPI's hidraw backend) and never the USB device node.
            TransportType::UsbHidApi { .. } | TransportType::UsbHidRaw { .. } => {
                required_rules.insert((
                    descriptor.vendor_id,
                    descriptor.product_id,
                    RequiredSubsystem::Hidraw,
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
            | TransportType::DriverUsb { .. }
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
        let has_interface_rule = subsystem == RequiredSubsystem::Hidraw
            && has_interface_scoped_hidraw_rule(vendor_id, product_id);
        assert!(
            UDEV_RULES.contains(&vendor_rule) || has_product_rule || has_interface_rule,
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

#[test]
fn udev_rules_grant_uaccess_on_input_event_nodes() {
    let input_rules = input_rule_lines(UDEV_INPUT_RULES);
    assert!(
        !input_rules.is_empty(),
        "expected at least one SUBSYSTEM==\"input\" rule for supported input vendors"
    );
    for rule in input_rules {
        assert!(
            rule.contains("KERNEL==\"event*\""),
            "input rule must scope to event nodes: {rule}"
        );
        assert!(
            rule.contains("TAG+=\"uaccess\""),
            "input rule must grant uaccess: {rule}"
        );
    }
}

#[test]
fn udev_input_rules_never_carry_group_assignments() {
    for rule in input_rule_lines(UDEV_INPUT_RULES)
        .into_iter()
        .chain(input_rule_lines(UDEV_INPUT_ALL_RULES))
    {
        assert!(
            !rule.contains("GROUP="),
            "input event rules must be uaccess-only; a group-readable event node \
             grants keystroke access to every group member: {rule}"
        );
    }
}

#[test]
fn optional_input_all_rules_guard_by_input_class() {
    let keyboard_guard = "ENV{ID_INPUT_KEYBOARD}==\"1\"";
    let mouse_guard = "ENV{ID_INPUT_MOUSE}==\"1\"";
    assert!(
        UDEV_INPUT_ALL_RULES
            .lines()
            .any(|line| line.contains("SUBSYSTEM==\"input\"") && line.contains(keyboard_guard)),
        "input-all rules must scope keyboard access with {keyboard_guard}"
    );
    assert!(
        UDEV_INPUT_ALL_RULES
            .lines()
            .any(|line| line.contains("SUBSYSTEM==\"input\"") && line.contains(mouse_guard)),
        "input-all rules must scope mouse access with {mouse_guard}"
    );
}

#[test]
fn input_uaccess_rules_sort_before_systemd_seat_late() {
    // `TAG+="uaccess"` only grants the per-user ACL when it is set before
    // systemd's 73-seat-late.rules runs the uaccess builtin. A file numbered
    // at or above 73 tags the node too late and the ACL is silently skipped,
    // leaving supported keyboards root-only. Guard the prefix so a rename can
    // never regress host input capture.
    for name in ["70-hypercolor-input.rules", "70-hypercolor-input-all.rules"] {
        let prefix: u32 = name
            .split('-')
            .next()
            .and_then(|value| value.parse().ok())
            .expect("input rules file must start with a numeric priority");
        assert!(
            prefix < 73,
            "{name} must sort before systemd 73-seat-late.rules for uaccess to apply"
        );
    }
}

/// A product's lighting-interface tag value, from
/// `ATTRS{idVendor}=="vvvv", ATTRS{idProduct}=="pppp", ENV{KEY}="value"`.
fn lighting_tag(vendor_id: u16, product_id: u16) -> Option<&'static str> {
    let prefix = format!(
        "SUBSYSTEM==\"hidraw\", ATTRS{{idVendor}}==\"{vendor_id:04x}\", ATTRS{{idProduct}}==\"{product_id:04x}\", ENV{{{CORSAIR_LIGHTING_IF_KEY}}}=\""
    );
    rule_lines(UDEV_RULES).find_map(|line| {
        line.strip_prefix(prefix.as_str())
            .and_then(|rest| rest.strip_suffix('"'))
    })
}

/// What the grant rule for one tag value admits.
#[derive(Debug)]
struct LightingGrant {
    interface: u8,
    requires_in_and_out_endpoints: bool,
}

fn lighting_grant(tag: &str) -> Option<LightingGrant> {
    let prefix = format!(
        "SUBSYSTEM==\"hidraw\", ENV{{{CORSAIR_LIGHTING_IF_KEY}}}==\"{tag}\", ATTRS{{bInterfaceNumber}}==\""
    );
    rule_lines(UDEV_RULES).find_map(|line| {
        let rest = line.strip_prefix(prefix.as_str())?;
        if !line.ends_with("MODE=\"0660\", GROUP=\"users\", TAG+=\"uaccess\"") {
            return None;
        }
        let interface = u8::from_str_radix(rest.get(..2)?, 16).ok()?;
        Some(LightingGrant {
            interface,
            requires_in_and_out_endpoints: line.contains("ATTRS{bNumEndpoints}==\"02\""),
        })
    })
}

fn has_interface_scoped_hidraw_rule(vendor_id: u16, product_id: u16) -> bool {
    lighting_tag(vendor_id, product_id).is_some_and(|tag| lighting_grant(tag).is_some())
}

fn rule_lines(rules: &str) -> impl Iterator<Item = &str> {
    rules
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
}

/// Corsair keyboards and mice share their USB device with boot-keyboard,
/// NKRO, and pointer collections. A vendor-wide hidraw or USB-node grant
/// would expose those to the whole fallback group.
#[test]
fn corsair_rules_are_never_vendor_wide() {
    let vendor = format!("{{idVendor}}==\"{CORSAIR_VID:04x}\"");
    for line in rule_lines(UDEV_RULES) {
        if !line.contains(&vendor) {
            continue;
        }
        assert!(
            line.contains("{idProduct}=="),
            "Corsair udev rule must name a product ID: {line}"
        );
    }
}

/// Each Corsair peripheral grants only the hidraw node on the interface its
/// transport opens, and never the USB device node.
#[test]
fn corsair_peripheral_rules_grant_only_the_lighting_node() {
    for descriptor in corsair_peripherals::descriptors() {
        let TransportType::UsbHidApi {
            interface: Some(interface),
            ..
        } = descriptor.transport
        else {
            panic!(
                "{} must pin the interface of its host-managed HID transport",
                descriptor.name
            );
        };
        let product = descriptor.product_id;

        let tag = lighting_tag(CORSAIR_VID, product).unwrap_or_else(|| {
            panic!(
                "{} ({product:04x}) needs a lighting-interface tag",
                descriptor.name
            )
        });
        let grant = lighting_grant(tag).unwrap_or_else(|| {
            panic!(
                "{} ({product:04x}) tag {tag} has no grant rule",
                descriptor.name
            )
        });
        assert_eq!(
            grant.interface, interface,
            "{} grant must match the interface its transport opens",
            descriptor.name
        );

        let usb_rule = RequiredSubsystem::Usb
            .product_rule_line(CORSAIR_VID, product)
            .expect("usb rules have a product form");
        assert!(
            !UDEV_RULES.contains(&usb_rule),
            "{} ({product:04x}) must not get a USB device node grant: {usb_rule}",
            descriptor.name
        );
    }
}

/// Older NXP firmware puts an IN-only keyboard interface where current
/// firmware puts the IN+OUT command interface. The grant must tell them
/// apart.
#[test]
fn nxp_input_device_grants_require_the_in_and_out_command_interface() {
    for descriptor in corsair_peripherals::descriptors() {
        if descriptor.transport != corsair_peripherals::NXP_INPUT_DEVICE_TRANSPORT {
            continue;
        }
        let tag = lighting_tag(CORSAIR_VID, descriptor.product_id)
            .unwrap_or_else(|| panic!("{} needs a lighting-interface tag", descriptor.name));
        let grant = lighting_grant(tag)
            .unwrap_or_else(|| panic!("{} tag {tag} has no grant rule", descriptor.name));
        assert!(
            grant.requires_in_and_out_endpoints,
            "{} grant must require both endpoints: {grant:?}",
            descriptor.name
        );
    }
}

/// The interface-scoped grant only means something when no other Corsair
/// hidraw rule grants the same product wholesale.
#[test]
fn corsair_peripherals_have_no_whole_product_hidraw_grant() {
    for descriptor in corsair_peripherals::descriptors() {
        let product = format!("ATTRS{{idProduct}}==\"{:04x}\"", descriptor.product_id);
        for line in rule_lines(UDEV_RULES) {
            if line.starts_with("SUBSYSTEM==\"hidraw\"") && line.contains(&product) {
                assert!(
                    !line.contains("MODE="),
                    "{} must not get a whole-product hidraw grant: {line}",
                    descriptor.name
                );
            }
        }
    }
}

/// Rule lines that could reach a Wooting keyboard, comments excluded: any
/// line naming Wooting's VID, and any line naming the Atmel VID that v1
/// firmware borrowed without pinning a non-Wooting product.
fn wooting_rule_lines() -> Vec<&'static str> {
    // The v1 firmware PIDs on the borrowed Atmel VID (Spec 90 section 2.2).
    let legacy_pids: Vec<String> = [0xFF01_u16, 0xFF02]
        .iter()
        .flat_map(|pid| [format!("{pid:04x}"), format!("{pid:04X}")])
        .collect();
    let wooting_vid = [format!("{WOOTING_VID:04x}"), format!("{WOOTING_VID:04X}")];
    let legacy_vid = [
        format!("{WOOTING_LEGACY_VID:04x}"),
        format!("{WOOTING_LEGACY_VID:04X}"),
    ];

    UDEV_RULES
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('#') && !line.is_empty())
        .filter(|line| {
            let names_wooting = wooting_vid.iter().any(|vid| line.contains(vid.as_str()));
            let names_atmel = legacy_vid.iter().any(|vid| line.contains(vid.as_str()));
            let pins_product = line.contains("idProduct") || line.contains("modalias");
            let pins_legacy_pid = legacy_pids.iter().any(|pid| line.contains(pid.as_str()));
            names_wooting || (names_atmel && (!pins_product || pins_legacy_pid))
        })
        .collect()
}

/// Wooting products whose configuration interface a published descriptor
/// dump pins down, with the source (Spec 90 section 8.3). This table is the
/// evidence on its own, kept out of the driver so no production change can
/// widen a grant: adding a row means citing a new dump.
const WOOTING_EVIDENCED_CONFIG_INTERFACES: &[(u16, u8, &str)] = &[
    (
        0x1232,
        2,
        "Two HE (ARM), no gamepad, firmware 2.12+: usage page 0xFF55 alone on \
         mi_02 (github.com/hrvach/deskhop/issues/335)",
    ),
    (
        0x1312,
        2,
        "60HE (ARM), no gamepad: interface 2 is the only one with an OUT \
         endpoint (forum.level1techs.com/t/wooting-60he-and-hid-ports/210911)",
    ),
];

fn wooting_config_rule(product_id: u16, interface: u8) -> String {
    format!(
        "SUBSYSTEM==\"hidraw\", ATTRS{{modalias}}==\"usb:v{WOOTING_VID:04X}p{product_id:04X}d*ic03isc00ip00in{interface:02X}\", MODE=\"0660\", GROUP=\"users\", TAG+=\"uaccess\""
    )
}

#[test]
fn wooting_udev_rules_open_only_evidenced_configuration_interfaces() {
    let lines = wooting_rule_lines();
    let expected: BTreeSet<String> = WOOTING_EVIDENCED_CONFIG_INTERFACES
        .iter()
        .map(|&(product_id, interface, _)| wooting_config_rule(product_id, interface))
        .collect();

    for &(product_id, _, source) in WOOTING_EVIDENCED_CONFIG_INTERFACES {
        assert!(
            wooting_descriptors()
                .iter()
                .any(|descriptor| descriptor.vendor_id == WOOTING_VID
                    && descriptor.product_id == product_id),
            "evidence row {product_id:04x} ({source}) names no registered Wooting product"
        );
    }
    for rule in &expected {
        assert!(
            lines.contains(&rule.as_str()),
            "missing evidenced Wooting rule: {rule}"
        );
    }
    for line in &lines {
        assert!(
            expected.contains(*line),
            "Wooting udev rule without cited descriptor evidence (a wider grant can expose \
             a keyboard, consumer, or analog input node): {line}"
        );
    }

    for descriptor in wooting_descriptors() {
        let evidenced = descriptor.vendor_id == WOOTING_VID
            && WOOTING_EVIDENCED_CONFIG_INTERFACES
                .iter()
                .any(|&(product_id, _, _)| product_id == descriptor.product_id);
        let pid_token = format!("p{:04X}d", descriptor.product_id);
        let idproduct_token = format!("idProduct}}==\"{:04x}\"", descriptor.product_id);
        let mentions = lines
            .iter()
            .filter(|line| line.contains(&pid_token) || line.contains(&idproduct_token))
            .count();
        assert_eq!(
            mentions,
            usize::from(evidenced),
            "{} ({:04x}:{:04x}) must get {} udev rule",
            descriptor.name,
            descriptor.vendor_id,
            descriptor.product_id,
            if evidenced { "exactly one" } else { "no" }
        );
    }
}

#[test]
fn wooting_udev_rules_never_open_the_usb_device_node() {
    for line in wooting_rule_lines() {
        assert!(
            !line.contains("SUBSYSTEM==\"usb\""),
            "a usb_device grant lets a group member detach the keyboard's input driver: {line}"
        );
        assert!(
            line.contains("ic03isc00ip00"),
            "Wooting rules must exclude the boot keyboard interface by class: {line}"
        );
    }
}
