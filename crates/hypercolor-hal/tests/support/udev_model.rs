//! A small model of udev rule matching, enough to ask what a rules file
//! grants a given device node.
//!
//! Text checks cannot answer that question once rules pin a product's
//! interface through `ATTRS{modalias}` or tag a product in one rule and
//! grant its interface in the next. This model evaluates the rules the way
//! udev does for the keys the Hypercolor files use:
//!
//! - rules run in file order, and an `ENV{KEY}="value"` assignment is
//!   visible to every later rule;
//! - `SUBSYSTEM`, `KERNEL`, `ATTR`, and `ENV` match the event device;
//! - every `ATTRS`, `SUBSYSTEMS`, and `KERNELS` key of one rule must match
//!   on the same device, which is the event device or any of its parents;
//! - a missing attribute fails its key whatever the operator, while a
//!   missing environment value matches as the empty string;
//! - patterns take `*`, `?`, `[...]` classes, and `|` alternatives.
//!
//! A key outside that set panics, so a rules change the model cannot read
//! fails loudly instead of being judged wrong.
//!
//! Devices are modeled as sysfs chains: a hidraw node sits under its HID
//! device, its USB interface, its USB device, and a root hub, with the
//! attributes the kernel exports at each level, in the kernel's formats
//! (lowercase hex for `idVendor`, uppercase hex inside `modalias`).

use std::collections::BTreeMap;
use std::ops::RangeInclusive;
use std::sync::LazyLock;

/// Interface numbers probed when a check needs "any interface".
pub const PROBED_INTERFACES: RangeInclusive<u8> = 0..=7;

/// One `KEY{attr}OP"value"` term of a rule.
#[derive(Debug)]
pub struct Term {
    key: &'static str,
    attr: Option<&'static str>,
    op: Op,
    value: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Match,
    Mismatch,
    Assign,
    Add,
}

/// A parsed rules file.
pub struct RuleSet {
    rules: Vec<Vec<Term>>,
}

impl RuleSet {
    /// Parse every rule line of `text`, skipping comments and blank lines.
    pub fn parse(text: &'static str) -> Self {
        let rules = text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(parse_rule)
            .collect();
        Self { rules }
    }

    /// Run every rule against `chain` (event device first) and report what
    /// the matching rules assigned.
    pub fn evaluate(&self, chain: &[SysDevice]) -> Outcome {
        let mut outcome = Outcome::default();
        for terms in &self.rules {
            if rule_matches(terms, chain, &outcome.env) {
                apply(terms, &mut outcome);
            }
        }
        outcome
    }

    /// Whether the rules give `chain`'s event node any access: a mode, a
    /// group, or the `uaccess` tag.
    pub fn grants(&self, chain: &[SysDevice]) -> bool {
        self.evaluate(chain).grants_access()
    }
}

/// What the matching rules assigned to one device node.
#[derive(Debug, Default)]
pub struct Outcome {
    pub mode: Option<&'static str>,
    pub group: Option<&'static str>,
    pub tags: Vec<&'static str>,
    env: BTreeMap<&'static str, &'static str>,
}

impl Outcome {
    pub fn grants_access(&self) -> bool {
        self.mode.is_some() || self.group.is_some() || self.tags.contains(&"uaccess")
    }
}

/// One device in a sysfs chain.
#[derive(Debug, Clone)]
pub struct SysDevice {
    subsystem: &'static str,
    devtype: Option<&'static str>,
    kernel: String,
    attrs: Vec<(&'static str, String)>,
}

impl SysDevice {
    fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(attr, _)| *attr == name)
            .map(|(_, value)| value.as_str())
    }
}

/// The shape of a USB HID interface: boot subclass and protocol, and how
/// many interrupt endpoints it has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterfaceShape {
    pub subclass: u8,
    pub protocol: u8,
    pub endpoints: u8,
}

/// Every interface shape the checks probe: non-boot, non-boot with a
/// keyboard or mouse protocol, boot keyboard, and boot mouse, each with an
/// IN endpoint alone and with an IN and an OUT endpoint.
pub const INTERFACE_SHAPES: [InterfaceShape; 10] = {
    const fn shape(subclass: u8, protocol: u8, endpoints: u8) -> InterfaceShape {
        InterfaceShape {
            subclass,
            protocol,
            endpoints,
        }
    }
    [
        shape(0, 0, 1),
        shape(0, 0, 2),
        shape(0, 1, 1),
        shape(0, 1, 2),
        shape(0, 2, 1),
        shape(0, 2, 2),
        shape(1, 1, 1),
        shape(1, 1, 2),
        shape(1, 2, 1),
        shape(1, 2, 2),
    ]
};

/// The sysfs chain of a hidraw node on one interface of a USB HID device.
pub fn hidraw_node(
    vendor_id: u16,
    product_id: u16,
    interface: u8,
    shape: InterfaceShape,
) -> Vec<SysDevice> {
    let InterfaceShape {
        subclass,
        protocol,
        endpoints,
    } = shape;
    vec![
        SysDevice {
            subsystem: "hidraw",
            devtype: None,
            kernel: "hidraw0".to_owned(),
            attrs: Vec::new(),
        },
        SysDevice {
            subsystem: "hid",
            devtype: None,
            kernel: format!("0003:{vendor_id:04X}:{product_id:04X}.0001"),
            attrs: vec![(
                "modalias",
                format!("hid:b0003g0001v0000{vendor_id:04X}p0000{product_id:04X}"),
            )],
        },
        SysDevice {
            subsystem: "usb",
            devtype: Some("usb_interface"),
            kernel: format!("1-1:1.{interface}"),
            attrs: vec![
                ("bInterfaceNumber", format!("{interface:02x}")),
                ("bAlternateSetting", " 0".to_owned()),
                ("bInterfaceClass", "03".to_owned()),
                ("bInterfaceSubClass", format!("{subclass:02x}")),
                ("bInterfaceProtocol", format!("{protocol:02x}")),
                ("bNumEndpoints", format!("{endpoints:02x}")),
                (
                    "modalias",
                    format!(
                        "usb:v{vendor_id:04X}p{product_id:04X}d0100dc00dsc00dp00ic03isc{subclass:02X}ip{protocol:02X}in{interface:02X}"
                    ),
                ),
            ],
        },
        usb_device(vendor_id, product_id, "1-1"),
        root_hub(),
    ]
}

/// The sysfs chain of a CDC-ACM serial node (`/dev/ttyACM*`).
pub fn tty_node(vendor_id: u16, product_id: u16) -> Vec<SysDevice> {
    vec![
        SysDevice {
            subsystem: "tty",
            devtype: None,
            kernel: "ttyACM0".to_owned(),
            attrs: Vec::new(),
        },
        SysDevice {
            subsystem: "usb",
            devtype: Some("usb_interface"),
            kernel: "1-1:1.0".to_owned(),
            attrs: vec![
                ("bInterfaceNumber", "00".to_owned()),
                ("bInterfaceClass", "02".to_owned()),
                (
                    "modalias",
                    format!(
                        "usb:v{vendor_id:04X}p{product_id:04X}d0100dc02dsc00dp00ic02isc02ip01in00"
                    ),
                ),
            ],
        },
        usb_device(vendor_id, product_id, "1-1"),
        root_hub(),
    ]
}

/// The sysfs chain of a USB device node (`/dev/bus/usb/BBB/DDD`).
pub fn usb_device_node(vendor_id: u16, product_id: u16) -> Vec<SysDevice> {
    vec![usb_device(vendor_id, product_id, "1-1"), root_hub()]
}

fn usb_device(vendor_id: u16, product_id: u16, kernel: &str) -> SysDevice {
    SysDevice {
        subsystem: "usb",
        devtype: Some("usb_device"),
        kernel: kernel.to_owned(),
        attrs: vec![
            ("idVendor", format!("{vendor_id:04x}")),
            ("idProduct", format!("{product_id:04x}")),
            ("bcdDevice", "0100".to_owned()),
            ("bDeviceClass", "00".to_owned()),
        ],
    }
}

fn root_hub() -> SysDevice {
    usb_device(0x1d6b, 0x0002, "usb1")
}

/// The parsed main rules file, shared by every check.
pub static HYPERCOLOR_RULES: LazyLock<RuleSet> =
    LazyLock::new(|| RuleSet::parse(include_str!("../../../../udev/99-hypercolor.rules")));

fn parse_rule(line: &'static str) -> Vec<Term> {
    let mut terms = Vec::new();
    let mut rest = line;
    while !rest.is_empty() {
        let key_len = rest
            .find(|c: char| !(c.is_ascii_uppercase() || c == '_'))
            .unwrap_or_else(|| panic!("udev rule ends inside a key: {line}"));
        let (key, after_key) = rest.split_at(key_len);
        let (attr, after_attr) = match after_key.strip_prefix('{') {
            Some(inner) => {
                let (attr, tail) = inner
                    .split_once('}')
                    .unwrap_or_else(|| panic!("unterminated attribute name: {line}"));
                (Some(attr), tail)
            }
            None => (None, after_key),
        };
        let (op, after_op) = [
            ("==", Op::Match),
            ("!=", Op::Mismatch),
            ("+=", Op::Add),
            ("=", Op::Assign),
        ]
        .into_iter()
        .find_map(|(token, op)| after_attr.strip_prefix(token).map(|tail| (op, tail)))
        .unwrap_or_else(|| panic!("unsupported operator after {key}: {line}"));
        let quoted = after_op
            .strip_prefix('"')
            .unwrap_or_else(|| panic!("unquoted value for {key}: {line}"));
        let (value, tail) = quoted
            .split_once('"')
            .unwrap_or_else(|| panic!("unterminated value for {key}: {line}"));
        terms.push(Term {
            key,
            attr,
            op,
            value,
        });
        rest = tail.trim_start();
        rest = rest.strip_prefix(',').unwrap_or(rest).trim_start();
    }
    terms
}

fn rule_matches(
    terms: &[Term],
    chain: &[SysDevice],
    env: &BTreeMap<&'static str, &'static str>,
) -> bool {
    let event = chain
        .first()
        .expect("a device chain starts at its event device");
    let mut parent_terms = Vec::new();
    for term in terms
        .iter()
        .filter(|term| matches!(term.op, Op::Match | Op::Mismatch))
    {
        let actual = match (term.key, term.attr) {
            ("SUBSYSTEM", None) => Some(event.subsystem),
            ("KERNEL", None) => Some(event.kernel.as_str()),
            ("ATTR", Some(name)) => event.attr(name),
            ("ENV", Some("DEVTYPE")) => Some(event.devtype.unwrap_or("")),
            ("ENV", Some(name)) => Some(env.get(name).copied().unwrap_or("")),
            ("ATTRS" | "SUBSYSTEMS" | "KERNELS", _) => {
                parent_terms.push(term);
                continue;
            }
            _ => panic!("the udev model does not understand match key {term:?}; extend it"),
        };
        if !term_holds(term, actual) {
            return false;
        }
    }
    parent_terms.is_empty()
        || chain.iter().any(|device| {
            parent_terms.iter().all(|term| {
                let actual = match term.key {
                    "ATTRS" => device.attr(term.attr.expect("ATTRS names an attribute")),
                    "SUBSYSTEMS" => Some(device.subsystem),
                    _ => Some(device.kernel.as_str()),
                };
                term_holds(term, actual)
            })
        })
}

fn term_holds(term: &Term, actual: Option<&str>) -> bool {
    let Some(actual) = actual else {
        return false;
    };
    let matched = pattern_matches(term.value, actual);
    if term.op == Op::Match {
        matched
    } else {
        !matched
    }
}

fn apply(terms: &[Term], outcome: &mut Outcome) {
    for term in terms
        .iter()
        .filter(|term| matches!(term.op, Op::Assign | Op::Add))
    {
        match (term.key, term.attr, term.op) {
            ("ENV", Some(name), Op::Assign) => {
                outcome.env.insert(name, term.value);
            }
            ("MODE", None, Op::Assign) => outcome.mode = Some(term.value),
            ("GROUP", None, Op::Assign) => outcome.group = Some(term.value),
            ("TAG", None, Op::Add) => outcome.tags.push(term.value),
            _ => panic!("the udev model does not understand assignment {term:?}; extend it"),
        }
    }
}

/// udev's pattern match: `|` separates alternatives, each a shell glob.
fn pattern_matches(pattern: &str, value: &str) -> bool {
    pattern
        .split('|')
        .any(|alternative| glob_matches(alternative.as_bytes(), value.as_bytes()))
}

fn glob_matches(pattern: &[u8], value: &[u8]) -> bool {
    match pattern.split_first() {
        None => value.is_empty(),
        Some((b'*', rest)) => (0..=value.len()).any(|skip| {
            value
                .get(skip..)
                .is_some_and(|tail| glob_matches(rest, tail))
        }),
        Some((b'?', rest)) => value
            .split_first()
            .is_some_and(|(_, tail)| glob_matches(rest, tail)),
        Some((b'[', rest)) => {
            // A `]` right after `[` is a class member, not the close.
            let Some(offset) = rest.iter().skip(1).position(|&byte| byte == b']') else {
                return value
                    .split_first()
                    .is_some_and(|(&byte, tail)| byte == b'[' && glob_matches(rest, tail));
            };
            let (class, after) = rest.split_at(offset + 1);
            let after = after.get(1..).unwrap_or_default();
            value.split_first().is_some_and(|(&byte, tail)| {
                class_matches(class, byte) && glob_matches(after, tail)
            })
        }
        Some((&literal, rest)) => value
            .split_first()
            .is_some_and(|(&byte, tail)| byte == literal && glob_matches(rest, tail)),
    }
}

fn class_matches(class: &[u8], byte: u8) -> bool {
    let (negated, members) = match class.split_first() {
        Some((b'!' | b'^', members)) => (true, members),
        _ => (false, class),
    };
    let mut found = false;
    let mut index = 0;
    while let Some(&low) = members.get(index) {
        if members.get(index + 1) == Some(&b'-')
            && let Some(&high) = members.get(index + 2)
        {
            found |= (low..=high).contains(&byte);
            index += 3;
        } else {
            found |= low == byte;
            index += 1;
        }
    }
    found != negated
}
