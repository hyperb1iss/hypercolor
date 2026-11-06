#[path = "support/udev_model.rs"]
mod udev_model;

use std::collections::{BTreeMap, BTreeSet};

use hypercolor_hal::database::ProtocolDatabase;
use hypercolor_hal::drivers::corsair::CORSAIR_VID;
use hypercolor_hal::drivers::corsair::peripheral::devices as corsair_peripherals;
use hypercolor_hal::drivers::lianli::LIANLI_TL_VENDOR_ID;
use hypercolor_hal::drivers::lianli::wireless::transport::PID_WIRELESS_RX;
use hypercolor_hal::drivers::wooting::{
    WOOTING_FAMILY, WOOTING_LEGACY_VID, WOOTING_VID, descriptors as wooting_descriptors,
};
use hypercolor_hal::registry::TransportType;
use udev_model::{
    HYPERCOLOR_RULES, INTERFACE_SHAPES, InterfaceShape, PROBED_INTERFACES, RuleSet, hidraw_node,
    named_product_ids, named_vendor_ids, tty_node, usb_device_node,
};

const UDEV_RULES: &str = include_str!("../../../udev/99-hypercolor.rules");
const UDEV_INPUT_RULES: &str = include_str!("../../../udev/70-hypercolor-input.rules");
const UDEV_INPUT_ALL_RULES: &str = include_str!("../../../udev/70-hypercolor-input-all.rules");
/// udev property the Corsair peripheral rules use to tag a product before
/// granting its lighting-interface hidraw node.
const CORSAIR_LIGHTING_IF_KEY: &str = "HYPERCOLOR_CORSAIR_LIGHTING_IF";
const I2C_UDEV_RULE: &str = "SUBSYSTEM==\"i2c-dev\", KERNEL==\"i2c-[0-9]*\", MODE=\"0660\", GROUP=\"users\", TAG+=\"uaccess\"";

/// Vendor IDs whose products include keyboards, mice, or headsets. On these
/// a hidraw node outside the lighting interface can carry keystrokes or
/// pointer input, and write access to a USB device node is enough to detach
/// usbhid from an input interface and read it directly.
const INPUT_DEVICE_VENDORS: &[(u16, &str)] = &[
    (0x1532, "Razer"),
    (0x1b1c, "Corsair"),
    (0x0b05, "ASUS"),
    (0x35ef, "Dygma"),
    (0x3434, "Keychron"),
    (0x3297, "ZSA"),
    (0xfeed, "QMK's default vendor ID"),
    (0x320f, "Glorious"),
];

/// Input-vendor products whose descriptor does not pin the lighting
/// interface and whose rule grants every HID interface of the product's own
/// USB device. Each entry needs a reason that holds for the hardware: no
/// keys, no pointer, and a rule that never reaches devices plugged in
/// behind it.
const WHOLE_DEVICE_HIDRAW_GRANTS: &[(u16, u16, &str)] = &[
    (
        0x1532,
        0x0f21,
        "Razer Thunderbolt 4 Dock Chroma: a dock without keys or pointer; no \
         descriptor dump pins its lighting interface, and the modalias rule \
         never matches the devices plugged into the dock",
    ),
    (
        0x1532,
        0x0f52,
        "Razer Thunderbolt 5 Dock Chroma: same as the Thunderbolt 4 dock",
    ),
];

fn input_rule_lines(rules: &str) -> Vec<&str> {
    rules
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('#') && line.contains("SUBSYSTEM==\"input\""))
        .collect()
}

fn rule_lines(rules: &str) -> impl Iterator<Item = &str> {
    rules
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
}

/// What a host-managed HID transport opens on hidraw.
#[derive(Debug, Clone, Copy)]
enum HidrawTarget {
    /// The descriptor names the USB interface.
    Interface(u8),
    /// The descriptor selects a collection without naming an interface.
    Unpinned,
}

/// The hidraw target of a transport, or `None` if it never opens hidraw.
fn hidraw_target(transport: TransportType) -> Option<HidrawTarget> {
    match transport {
        TransportType::UsbHidApi {
            interface: Some(interface),
            ..
        }
        | TransportType::UsbHidRaw { interface, .. } => Some(HidrawTarget::Interface(interface)),
        TransportType::UsbHidApi {
            interface: None, ..
        } => Some(HidrawTarget::Unpinned),
        _ => None,
    }
}

/// Whether a transport opens the USB device node (`/dev/bus/usb/*`).
fn opens_usb_device_node(transport: TransportType) -> bool {
    matches!(
        transport,
        TransportType::UsbControl { .. }
            | TransportType::UsbHid { .. }
            | TransportType::UsbBulk { .. }
            | TransportType::DriverUsb { .. }
            | TransportType::UsbVendor
    )
}

/// Which device nodes Hypercolor opens for one product, across every
/// descriptor registered for it.
#[derive(Debug, Default)]
struct ProductAccess {
    names: Vec<&'static str>,
    hidraw_interfaces: BTreeSet<u8>,
    unpinned_hidraw: bool,
    usb_device: bool,
    claims_hid_over_usb: bool,
}

fn product_access() -> BTreeMap<(u16, u16), ProductAccess> {
    let mut products: BTreeMap<(u16, u16), ProductAccess> = BTreeMap::new();
    for descriptor in ProtocolDatabase::all() {
        let access = products
            .entry((descriptor.vendor_id, descriptor.product_id))
            .or_default();
        access.names.push(descriptor.name);
        match hidraw_target(descriptor.transport) {
            Some(HidrawTarget::Interface(interface)) => {
                access.hidraw_interfaces.insert(interface);
            }
            Some(HidrawTarget::Unpinned) => access.unpinned_hidraw = true,
            None => {}
        }
        access.usb_device |= opens_usb_device_node(descriptor.transport);
        access.claims_hid_over_usb |= matches!(descriptor.transport, TransportType::UsbHid { .. });
    }
    products
}

fn is_input_device_vendor(vendor_id: u16) -> bool {
    INPUT_DEVICE_VENDORS
        .iter()
        .any(|&(vendor, _)| vendor == vendor_id)
}

/// A product ID of `vendor_id` that no rule names and no descriptor
/// registers, for probing whether a rule matches more than it names.
fn unnamed_product(rules: &RuleSet, vendor_id: u16) -> u16 {
    let named = named_product_ids(rules, vendor_id);
    let registered: Vec<u16> = ProtocolDatabase::by_vendor(vendor_id)
        .iter()
        .map(|descriptor| descriptor.product_id)
        .collect();
    (0..=u16::MAX)
        .rev()
        .find(|product| !named.contains(product) && !registered.contains(product))
        .expect("some product ID is neither named nor registered")
}

fn suggested_hidraw_rule(vendor_id: u16, product_id: u16, interface: Option<u8>) -> String {
    let interface = interface.map_or_else(|| "*".to_owned(), |value| format!("{value:02X}"));
    format!(
        "SUBSYSTEM==\"hidraw\", ATTRS{{modalias}}==\"usb:v{vendor_id:04X}p{product_id:04X}d*ic03isc*ip*in{interface}\", MODE=\"0660\", GROUP=\"users\", TAG+=\"uaccess\""
    )
}

fn hidraw_granted_on(vendor_id: u16, product_id: u16, interface: u8) -> bool {
    INTERFACE_SHAPES.iter().any(|&shape| {
        HYPERCOLOR_RULES.grants(&hidraw_node(vendor_id, product_id, interface, shape))
    })
}

#[test]
fn every_hidraw_descriptor_is_granted_on_the_interface_it_opens() {
    for descriptor in ProtocolDatabase::all() {
        // Wooting boards rely on Wooting's own udev rules except where a
        // public descriptor dump pins the interface.
        if descriptor.family == WOOTING_FAMILY {
            continue;
        }
        let Some(target) = hidraw_target(descriptor.transport) else {
            continue;
        };
        let (vendor, product) = (descriptor.vendor_id, descriptor.product_id);
        let (granted, interface) = match target {
            HidrawTarget::Interface(interface) => (
                hidraw_granted_on(vendor, product, interface),
                Some(interface),
            ),
            HidrawTarget::Unpinned => (
                PROBED_INTERFACES
                    .into_iter()
                    .any(|interface| hidraw_granted_on(vendor, product, interface)),
                None,
            ),
        };
        assert!(
            granted,
            "{} ({vendor:04x}:{product:04x}) opens hidraw on interface {} but no rule \
             grants that node; add a rule such as {}",
            descriptor.name,
            interface.map_or_else(|| "<any>".to_owned(), |value| value.to_string()),
            suggested_hidraw_rule(vendor, product, interface)
        );
    }
}

#[test]
fn every_usb_device_and_serial_descriptor_is_granted() {
    let mut usb_products = BTreeSet::new();
    let mut tty_products = BTreeSet::new();
    for descriptor in ProtocolDatabase::all() {
        let product = (descriptor.vendor_id, descriptor.product_id);
        if opens_usb_device_node(descriptor.transport) {
            usb_products.insert(product);
        }
        if matches!(descriptor.transport, TransportType::UsbSerial { .. }) {
            tty_products.insert(product);
        }
    }
    // The L-Wireless controller's protocol also opens its RX sibling, which
    // has no descriptor of its own.
    usb_products.insert((LIANLI_TL_VENDOR_ID, PID_WIRELESS_RX));

    for (vendor, product) in usb_products {
        assert!(
            HYPERCOLOR_RULES.grants(&usb_device_node(vendor, product)),
            "missing usb_device rule for {vendor:04x}:{product:04x}: expected \
             SUBSYSTEM==\"usb\", ENV{{DEVTYPE}}==\"usb_device\", ATTR{{idVendor}}==\"{vendor:04x}\", \
             ATTR{{idProduct}}==\"{product:04x}\", MODE=\"0660\", GROUP=\"users\", TAG+=\"uaccess\""
        );
    }
    for (vendor, product) in tty_products {
        assert!(
            HYPERCOLOR_RULES.grants(&tty_node(vendor, product)),
            "missing tty rule for serial device {vendor:04x}:{product:04x}"
        );
    }
}

#[test]
fn udev_rules_include_i2c_access_for_smbus_devices() {
    assert!(
        UDEV_RULES.contains(I2C_UDEV_RULE),
        "missing SMBus i2c-dev udev rule: {I2C_UDEV_RULE}"
    );
}

/// A hidraw rule that matches a product it does not name reaches every
/// keyboard and mouse that shares the vendor ID, and a vendor-wide
/// `ATTRS{idVendor}` match also reaches devices plugged in behind a hub
/// that reports that vendor.
#[test]
fn hidraw_grants_are_never_vendor_wide() {
    let rules = &*HYPERCOLOR_RULES;
    let mut vendors = named_vendor_ids(rules);
    // A vendor no rule names catches a grant with no vendor match at all.
    vendors.push(
        (0..=u16::MAX)
            .rev()
            .find(|vendor| !vendors.contains(vendor))
            .expect("some vendor ID is unnamed"),
    );
    for vendor in vendors {
        let probe = unnamed_product(rules, vendor);
        for interface in PROBED_INTERFACES {
            for shape in INTERFACE_SHAPES {
                assert!(
                    !rules.grants(&hidraw_node(vendor, probe, interface, shape)),
                    "a hidraw rule grants product {probe:04x} of vendor {vendor:04x}, which no \
                     rule names (interface {interface}, {shape:?}); scope the grant to the \
                     products and interfaces Hypercolor opens"
                );
            }
        }
    }
}

/// On an input-device vendor, each product's hidraw grant admits only the
/// interfaces its descriptors open. Controllers that claim their HID
/// interface through the USB device node (`TransportType::UsbHid`) keep a
/// whole-product grant: Corsair's fan, LCD, and Lighting Node controllers
/// have no input collections.
#[test]
fn input_device_hidraw_grants_stop_at_the_interfaces_hypercolor_opens() {
    for ((vendor, product), access) in product_access() {
        if !is_input_device_vendor(vendor) || access.unpinned_hidraw || access.claims_hid_over_usb {
            continue;
        }
        for interface in PROBED_INTERFACES {
            if access.hidraw_interfaces.contains(&interface) {
                continue;
            }
            for shape in INTERFACE_SHAPES {
                assert!(
                    !HYPERCOLOR_RULES.grants(&hidraw_node(vendor, product, interface, shape)),
                    "{:?} ({vendor:04x}:{product:04x}) has interface {interface} ({shape:?}) \
                     granted, but Hypercolor opens hidraw only on {:?}; that interface can \
                     carry keystrokes or pointer input",
                    access.names,
                    access.hidraw_interfaces
                );
            }
        }
    }
}

/// Without an interface number, a rule has to pick the lighting interface
/// out by its shape. An IN and an OUT endpoint on a non-boot interface is
/// the only shape udev can see that keyboard, NKRO, mouse, and media-key
/// interfaces do not share, so an unpinned grant must require it.
#[test]
fn unpinned_input_device_grants_never_reach_an_input_only_interface() {
    for ((vendor, product), access) in product_access() {
        let whole_device = WHOLE_DEVICE_HIDRAW_GRANTS
            .iter()
            .any(|&(v, p, _)| (v, p) == (vendor, product));
        if !is_input_device_vendor(vendor) || !access.unpinned_hidraw || whole_device {
            continue;
        }
        for interface in PROBED_INTERFACES {
            for shape in INTERFACE_SHAPES {
                let input_shaped = shape.endpoints < 2 || shape.subclass == 1;
                if !input_shaped {
                    continue;
                }
                assert!(
                    !HYPERCOLOR_RULES.grants(&hidraw_node(vendor, product, interface, shape)),
                    "{:?} ({vendor:04x}:{product:04x}) does not pin its lighting interface, \
                     and its rule grants interface {interface} with {shape:?}, which keyboard \
                     and mouse interfaces share",
                    access.names
                );
            }
        }
    }
}

#[test]
fn whole_device_hidraw_grants_name_unpinned_input_device_products() {
    let products = product_access();
    for &(vendor, product, reason) in WHOLE_DEVICE_HIDRAW_GRANTS {
        let access = products
            .get(&(vendor, product))
            .unwrap_or_else(|| panic!("{vendor:04x}:{product:04x} is not registered ({reason})"));
        assert!(
            is_input_device_vendor(vendor) && access.unpinned_hidraw,
            "{vendor:04x}:{product:04x} pins its interface or is not an input-device vendor, \
             so it needs no whole-device entry ({reason})"
        );
    }
}

/// Write access to a keyboard's or mouse's USB device node is enough to
/// detach usbhid and read its input interfaces, so input-device vendors get
/// a usb_device grant only for products whose transport opens that node.
#[test]
fn input_device_products_get_a_usb_device_grant_only_when_it_is_opened() {
    for ((vendor, product), access) in product_access() {
        if !is_input_device_vendor(vendor) || access.usb_device {
            continue;
        }
        assert!(
            !HYPERCOLOR_RULES.grants(&usb_device_node(vendor, product)),
            "{:?} ({vendor:04x}:{product:04x}) gets a usb_device grant, but Hypercolor \
             reaches it without opening the USB device node",
            access.names
        );
    }
    for &(vendor, name) in INPUT_DEVICE_VENDORS {
        let probe = unnamed_product(&HYPERCOLOR_RULES, vendor);
        assert!(
            !HYPERCOLOR_RULES.grants(&usb_device_node(vendor, probe)),
            "{name} ({vendor:04x}) has a vendor-wide usb_device grant"
        );
    }
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

        assert!(
            !HYPERCOLOR_RULES.grants(&usb_device_node(CORSAIR_VID, product)),
            "{} ({product:04x}) must not get a USB device node grant",
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

/// The model reads the rule forms the file uses the way udev does: a
/// modalias pattern pins vendor, product, and interface on one parent, and
/// the Corsair two-step form tags a product before granting its interface.
#[test]
fn udev_model_reads_modalias_and_tagged_interface_rules() {
    const RULES: &str = concat!(
        "SUBSYSTEM==\"hidraw\", ATTRS{modalias}==\"usb:v1532p026Cd*ic03isc*ip*in03\", MODE=\"0660\", GROUP=\"users\"\n",
        "SUBSYSTEM==\"hidraw\", ATTRS{idVendor}==\"1b1c\", ATTRS{idProduct}==\"1b3d\", ENV{TAGGED}=\"01-io\"\n",
        "SUBSYSTEM==\"hidraw\", ENV{TAGGED}==\"01-io\", ATTRS{bInterfaceNumber}==\"01\", ATTRS{bNumEndpoints}==\"02\", MODE=\"0660\"\n",
        "SUBSYSTEM==\"hidraw\", ATTRS{idVendor}==\"1b1c\", ATTRS{bInterfaceNumber}==\"00\", MODE=\"0600\"\n",
        "SUBSYSTEM==\"usb\", ENV{DEVTYPE}==\"usb_device\", ATTR{idVendor}==\"2982\", ATTR{idProduct}==\"1967|19[0-9]8\", MODE=\"0660\"\n",
        "SUBSYSTEM==\"tty\", ATTRS{idVendor}==\"35ef\", MODE=\"0660\"\n",
    );
    let rules = RuleSet::parse(RULES);
    let open = InterfaceShape {
        subclass: 0,
        protocol: 0,
        endpoints: 2,
    };
    let in_only = InterfaceShape {
        endpoints: 1,
        ..open
    };

    assert!(rules.grants(&hidraw_node(0x1532, 0x026c, 3, in_only)));
    assert!(!rules.grants(&hidraw_node(0x1532, 0x026c, 0, in_only)));
    assert!(
        !rules.grants(&hidraw_node(0x1532, 0x026d, 3, in_only)),
        "modalias hex is uppercase, and the product is part of the pattern"
    );

    assert!(rules.grants(&hidraw_node(0x1b1c, 0x1b3d, 1, open)));
    assert!(!rules.grants(&hidraw_node(0x1b1c, 0x1b3d, 1, in_only)));
    assert!(!rules.grants(&hidraw_node(0x1b1c, 0x1b11, 1, open)));
    assert!(
        !rules.grants(&hidraw_node(0x1b1c, 0x1b3d, 0, open)),
        "ATTRS keys of one rule must match on the same parent: idVendor sits on the \
         USB device and bInterfaceNumber on the interface"
    );

    assert!(rules.grants(&usb_device_node(0x2982, 0x1967)));
    assert!(rules.grants(&usb_device_node(0x2982, 0x1958)));
    assert!(!rules.grants(&usb_device_node(0x2982, 0x1969)));
    assert!(!rules.grants(&hidraw_node(0x2982, 0x1967, 0, open)));
    assert!(rules.grants(&tty_node(0x35ef, 0x0012)));
    assert!(!rules.grants(&tty_node(0x35ee, 0x0012)));
}
