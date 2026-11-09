//! Input-safety contract for Corsair keyboards, mice, and desk accessories.
//!
//! Spec 53 sections 8.4 and 8.5: Hypercolor must never break a peripheral's
//! keyboard or pointer input. A transport that claims a USB interface
//! detaches the kernel driver bound to it, and on a keyboard or mouse that
//! driver is `usbhid`, so the device stops typing or pointing. The only
//! transports a Corsair peripheral may use are the host-managed HID paths
//! (HIDAPI and hidraw), bound to the vendor lighting collection and never to
//! any of the input collections that share its USB device.
//!
//! The expected lighting collection of every registered peripheral is
//! written out below, independently of the descriptor constants, so a
//! descriptor change has to update this table on purpose.

use std::collections::{BTreeMap, BTreeSet};

use hypercolor_hal::ProtocolDatabase;
use hypercolor_hal::drivers::corsair::CORSAIR_VID;
use hypercolor_hal::drivers::corsair::peripheral::devices as peripherals;
use hypercolor_hal::registry::{DeviceDescriptor, HidRawReportMode, TransportType};

const CORSAIR_TOML: &str = include_str!("../../../data/drivers/vendors/corsair.toml");

/// Device types in the vendor database that carry live input collections or
/// share a USB device with them.
const PERIPHERAL_TYPES: &[&str] = &["keyboard", "mouse", "mousepad", "headset"];

/// First vendor-defined HID usage page.
const VENDOR_USAGE_PAGE_START: u16 = 0xFF00;

/// One HID top-level collection: the interface it lives on and its usage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Collection {
    interface: u8,
    usage_page: u16,
    usage: u16,
}

const fn collection(interface: u8, usage_page: u16, usage: u16) -> Collection {
    Collection {
        interface,
        usage_page,
        usage,
    }
}

/// Collections on NXP/CUE keyboards and mice that are not the lighting
/// command collection, from HIDAPI enumerations of a K70 RGB (OpenRGB work
/// item 4944) and a Scimitar RGB Elite (OpenRGB issue 4016). Interface 0 is
/// the composite input interface; its `0xFFC2`/`0x0003` collection is
/// input-only and carries vendor key reports.
const NXP_INPUT_COLLECTIONS: &[Collection] = &[
    collection(0, 0x0001, 0x0006),
    collection(0, 0x0001, 0x0002),
    collection(0, 0x0001, 0x0001),
    collection(0, 0x000C, 0x0001),
    collection(0, 0xFFC0, 0x0002),
    collection(0, 0xFFC1, 0x0002),
    collection(0, 0xFFC2, 0x0003),
];

/// The Polaris keyboard collection on interface 1 (OpenRGB issue 4016).
const POLARIS_INPUT_COLLECTIONS: &[Collection] = &[collection(1, 0x0001, 0x0006)];

/// Collections on Bragi devices besides the interface-1 lighting
/// collection, from a K100 enumeration (OpenRGB issue 4016). The second
/// `0xFF42` collection on interface 2 is IN-only and not a lighting target.
const BRAGI_INPUT_COLLECTIONS: &[Collection] = &[
    collection(0, 0x0001, 0x0006),
    collection(0, 0x000C, 0x0001),
    collection(2, 0xFF42, 0x0002),
    collection(3, 0x0001, 0x0002),
];

/// Which input collections share a device with its lighting collection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Family {
    Bragi,
    NxpInputDevice,
    Polaris,
    /// One vendor-only interface and nothing else.
    St100,
}

impl Family {
    const fn input_collections(self) -> &'static [Collection] {
        match self {
            Self::Bragi => BRAGI_INPUT_COLLECTIONS,
            Self::NxpInputDevice => NXP_INPUT_COLLECTIONS,
            Self::Polaris => POLARIS_INPUT_COLLECTIONS,
            Self::St100 => &[],
        }
    }
}

/// The HID filter a registered peripheral's transport must carry.
#[derive(Clone, Copy, Debug)]
struct ExpectedLighting {
    family: Family,
    interface: u8,
    usage_page: u16,
    usage: Option<u16>,
}

const BRAGI: ExpectedLighting = ExpectedLighting {
    family: Family::Bragi,
    interface: 1,
    usage_page: 0xFF42,
    usage: None,
};
const NXP_INPUT_DEVICE: ExpectedLighting = ExpectedLighting {
    family: Family::NxpInputDevice,
    interface: 1,
    usage_page: 0xFFC2,
    usage: Some(0x0004),
};
const POLARIS: ExpectedLighting = ExpectedLighting {
    family: Family::Polaris,
    interface: 0,
    usage_page: 0xFFC2,
    usage: Some(0x0001),
};
const ST100: ExpectedLighting = ExpectedLighting {
    family: Family::St100,
    interface: 0,
    usage_page: 0xFFC2,
    usage: None,
};

/// Every registered Corsair peripheral, by PID.
const EXPECTED_LIGHTING: &[(u16, ExpectedLighting)] = &[
    // Bragi keyboards
    (0x1BA4, BRAGI),
    (0x1BA0, BRAGI),
    (0x1BAD, BRAGI),
    (0x1B8D, BRAGI),
    (0x1B83, BRAGI),
    (0x1BC7, BRAGI),
    (0x1BED, BRAGI),
    (0x1BAF, BRAGI),
    (0x1B73, BRAGI),
    (0x1BB9, BRAGI),
    (0x1BC4, BRAGI),
    (0x1BB3, BRAGI),
    (0x1BD4, BRAGI),
    (0x2B0A, BRAGI),
    (0x1BFF, BRAGI),
    (0x1BFD, BRAGI),
    (0x2B01, BRAGI),
    (0x1B89, BRAGI),
    (0x1B7C, BRAGI),
    (0x1BC5, BRAGI),
    (0x1B7D, BRAGI),
    // Bragi mice and mousepads
    (0x1B4B, BRAGI),
    (0x1B7E, BRAGI),
    (0x1B5E, BRAGI),
    (0x1B4C, BRAGI),
    (0x1B70, BRAGI),
    (0x1B93, BRAGI),
    (0x1BBA, BRAGI),
    (0x1BAC, BRAGI),
    (0x1B9E, BRAGI),
    (0x1BB5, BRAGI),
    (0x1BF0, BRAGI),
    (0x1BE3, BRAGI),
    (0x1B9B, BRAGI),
    (0x1BC9, BRAGI),
    // NXP/CUE keyboards and mice
    (0x1B3D, NXP_INPUT_DEVICE),
    (0x1B17, NXP_INPUT_DEVICE),
    (0x1B37, NXP_INPUT_DEVICE),
    (0x1B13, NXP_INPUT_DEVICE),
    (0x1B33, NXP_INPUT_DEVICE),
    (0x1B49, NXP_INPUT_DEVICE),
    (0x1B11, NXP_INPUT_DEVICE),
    (0x1B2D, NXP_INPUT_DEVICE),
    (0x1B20, NXP_INPUT_DEVICE),
    (0x1B12, NXP_INPUT_DEVICE),
    (0x1B1E, NXP_INPUT_DEVICE),
    // NXP/CUE desk accessories
    (0x1B3B, POLARIS),
    (0x0A34, ST100),
];

fn expected_lighting(pid: u16) -> Option<ExpectedLighting> {
    EXPECTED_LIGHTING
        .iter()
        .find(|(expected_pid, _)| *expected_pid == pid)
        .map(|(_, expected)| *expected)
}

/// The HID filter a host-managed transport opens with, or `None` for any
/// other transport.
fn hid_filter(transport: &TransportType) -> Option<(Option<u8>, Option<u16>, Option<u16>)> {
    match *transport {
        TransportType::UsbHidApi {
            interface,
            usage_page,
            usage,
            ..
        } => Some((interface, usage_page, usage)),
        TransportType::UsbHidRaw {
            interface,
            usage_page,
            usage,
            ..
        } => Some((Some(interface), usage_page, usage)),
        _ => None,
    }
}

/// Whether a transport's filter would accept this collection.
fn selects(transport: &TransportType, target: Collection) -> bool {
    hid_filter(transport).is_some_and(|(interface, usage_page, usage)| {
        interface.is_none_or(|interface| interface == target.interface)
            && usage_page.is_none_or(|page| page == target.usage_page)
            && usage.is_none_or(|usage| usage == target.usage)
    })
}

/// Why a transport is unsafe for a peripheral with these input collections,
/// or `None` when it is safe.
///
/// The match is exhaustive on purpose: a new transport variant fails to
/// compile here until someone decides whether it can detach a kernel
/// driver.
fn input_hazard(transport: &TransportType, input_collections: &[Collection]) -> Option<String> {
    let reason = match transport {
        TransportType::UsbHidApi { usage_page, .. }
        | TransportType::UsbHidRaw { usage_page, .. } => match usage_page {
            Some(page) if *page >= VENDOR_USAGE_PAGE_START => {
                return input_collections
                    .iter()
                    .find(|input| selects(transport, **input))
                    .map(|input| format!("can select the input collection {input:?}"));
            }
            Some(_) => "binds a non-vendor HID collection",
            None => "has no usage page filter, so it may bind an input collection",
        },
        TransportType::UsbControl { .. } => {
            "claims a USB interface for control transfers, detaching its kernel driver"
        }
        TransportType::UsbHid { .. } => {
            "claims the HID interface for interrupt endpoints, detaching usbhid"
        }
        TransportType::UsbBulk { .. } => {
            "claims a USB interface for bulk transfers, detaching its kernel driver"
        }
        TransportType::DriverUsb { .. } => "is driver-owned and opens the USB device directly",
        TransportType::UsbVendor => "opens the whole USB device node for vendor control transfers",
        TransportType::UsbSerial { .. } => "is a serial transport, not a HID lighting path",
        TransportType::I2cSmBus { .. } => "is an SMBus transport, not a HID lighting path",
    };
    Some(reason.to_owned())
}

const fn hidapi(interface: Option<u8>, usage_page: u16, usage: Option<u16>) -> TransportType {
    TransportType::UsbHidApi {
        interface,
        report_id: 0,
        report_mode: HidRawReportMode::OutputReport,
        max_report_len: 65,
        usage_page: Some(usage_page),
        usage,
    }
}

const fn hidraw(interface: u8, usage_page: u16, usage: Option<u16>) -> TransportType {
    TransportType::UsbHidRaw {
        interface,
        report_id: 0,
        report_mode: HidRawReportMode::OutputReport,
        usage_page: Some(usage_page),
        usage,
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

/// Registered Corsair descriptors that are peripherals: in the peripheral
/// module, or typed as one in the vendor database.
fn registered_peripheral_descriptors() -> Vec<&'static DeviceDescriptor> {
    let module_pids = peripherals::descriptors()
        .iter()
        .map(|descriptor| descriptor.product_id)
        .collect::<BTreeSet<_>>();
    let device_types = corsair_device_types();

    ProtocolDatabase::all()
        .iter()
        .filter(|descriptor| descriptor.vendor_id == CORSAIR_VID)
        .filter(|descriptor| {
            module_pids.contains(&descriptor.product_id)
                || device_types
                    .get(&descriptor.product_id)
                    .is_some_and(|kind| PERIPHERAL_TYPES.contains(&kind.as_str()))
        })
        .collect()
}

#[test]
fn every_corsair_peripheral_descriptor_is_input_safe() {
    let descriptors = registered_peripheral_descriptors();
    assert!(descriptors.len() >= peripherals::descriptors().len());

    for descriptor in descriptors {
        let expected = expected_lighting(descriptor.product_id).unwrap_or_else(|| {
            panic!(
                "{} ({:04X}) is a registered peripheral missing from EXPECTED_LIGHTING; add its \
                 lighting collection and the input collections it shares a device with",
                descriptor.name, descriptor.product_id
            )
        });
        if let Some(hazard) =
            input_hazard(&descriptor.transport, expected.family.input_collections())
        {
            panic!(
                "{} ({:04X}:{:04X}) uses a transport that {hazard}: {:?}",
                descriptor.name, descriptor.vendor_id, descriptor.product_id, descriptor.transport
            );
        }
    }
}

#[test]
fn every_registered_peripheral_binds_its_expected_lighting_collection() {
    let registered = registered_peripheral_descriptors();
    let registered_pids = registered
        .iter()
        .map(|descriptor| descriptor.product_id)
        .collect::<BTreeSet<_>>();
    let expected_pids = EXPECTED_LIGHTING
        .iter()
        .map(|(pid, _)| *pid)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        registered_pids, expected_pids,
        "EXPECTED_LIGHTING must list exactly the registered peripherals"
    );
    assert_eq!(
        expected_pids.len(),
        EXPECTED_LIGHTING.len(),
        "EXPECTED_LIGHTING must not repeat a PID"
    );

    for descriptor in registered {
        let expected =
            expected_lighting(descriptor.product_id).expect("the PID sets were just compared");
        assert_eq!(
            hid_filter(&descriptor.transport),
            Some((
                Some(expected.interface),
                Some(expected.usage_page),
                expected.usage
            )),
            "{} ({:04X}) must bind interface {}, page {:#06X}, usage {:?}",
            descriptor.name,
            descriptor.product_id,
            expected.interface,
            expected.usage_page,
            expected.usage
        );
    }
}

/// Every known input collection, bound by an exact filter, a page-only
/// filter on its interface, or a filter with no interface, is refused.
#[test]
fn known_input_collections_are_rejected() {
    for family in [
        Family::Bragi,
        Family::NxpInputDevice,
        Family::Polaris,
        Family::St100,
    ] {
        let inputs = family.input_collections();
        for input in inputs {
            for transport in [
                hidapi(Some(input.interface), input.usage_page, Some(input.usage)),
                hidapi(Some(input.interface), input.usage_page, None),
                hidapi(None, input.usage_page, Some(input.usage)),
                hidapi(None, input.usage_page, None),
                hidraw(input.interface, input.usage_page, Some(input.usage)),
            ] {
                assert!(
                    input_hazard(&transport, inputs).is_some(),
                    "{family:?}: {transport:?} must be refused because it reaches {input:?}"
                );
            }
        }
    }
}

/// The two collections a page-only NXP filter cannot tell apart.
#[test]
fn nxp_input_only_vendor_collection_is_refused() {
    for transport in [
        hidapi(Some(0), 0xFFC2, Some(0x0003)),
        hidapi(Some(0), 0xFFC2, None),
        hidapi(None, 0xFFC2, None),
        hidapi(None, 0xFFC2, Some(0x0003)),
        hidraw(0, 0xFFC2, Some(0x0003)),
    ] {
        assert!(
            input_hazard(&transport, NXP_INPUT_COLLECTIONS).is_some(),
            "{transport:?} can reach the NXP input-only vendor collection"
        );
    }
    assert!(
        input_hazard(
            &hidapi(Some(1), 0xFFC2, Some(0x0004)),
            NXP_INPUT_COLLECTIONS
        )
        .is_none()
    );
}

/// A Bragi filter without its interface would also match the IN-only
/// second `0xFF42` collection.
#[test]
fn bragi_unpinned_vendor_page_is_refused() {
    assert!(input_hazard(&hidapi(None, 0xFF42, None), BRAGI_INPUT_COLLECTIONS).is_some());
    assert!(input_hazard(&hidapi(Some(1), 0xFF42, None), BRAGI_INPUT_COLLECTIONS).is_none());
}

/// Each family's expected lighting filter must not overlap its own input
/// collections, or the table above would contradict itself.
#[test]
fn expected_lighting_filters_avoid_their_input_collections() {
    for (pid, expected) in EXPECTED_LIGHTING {
        let transport = hidapi(
            Some(expected.interface),
            expected.usage_page,
            expected.usage,
        );
        assert!(
            input_hazard(&transport, expected.family.input_collections()).is_none(),
            "{pid:04X}: the expected filter reaches an input collection"
        );
    }
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
            input_hazard(&transport, &[]).is_some(),
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
        let pinned =
            hid_filter(&descriptor.transport).is_some_and(|(interface, _, _)| interface.is_some());
        assert!(
            pinned,
            "{} must pin the interface of its lighting collection",
            descriptor.name
        );
    }
}
