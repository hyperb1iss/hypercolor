//! Static USB protocol descriptor database.

use std::collections::{BTreeSet, HashMap};
use std::sync::LazyLock;

use hypercolor_types::device::DeviceFamily;

use crate::drivers::razer::{
    LED_ID_BACKLIGHT, LED_ID_ZERO, PID_BASILISK_V3, PID_HUNTSMAN_V2, PID_SEIREN_EMOTE,
    RAZER_VENDOR_ID, RazerMatrixType, RazerProtocolVersion,
};

/// Static metadata for a known USB device.
#[derive(Debug, Clone)]
pub struct DeviceDescriptor {
    /// USB vendor ID (`VID`).
    pub vendor_id: u16,

    /// USB product ID (`PID`).
    pub product_id: u16,

    /// Human-readable device name.
    pub name: &'static str,

    /// Device family classification.
    pub family: DeviceFamily,

    /// Transport type required by this device.
    pub transport: TransportType,

    /// Protocol-specific construction parameters.
    pub params: ProtocolParams,

    /// Optional firmware-based disambiguation predicate.
    pub firmware_predicate: Option<fn(&str) -> bool>,
}

/// USB transport mechanism for a descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportType {
    /// HID feature reports over USB control transfers.
    UsbControl {
        /// Interface number to claim.
        interface: u8,
        /// HID report ID.
        report_id: u8,
    },

    /// HID interrupt endpoint transport.
    UsbHid {
        /// Interface number to claim.
        interface: u8,
    },

    /// Vendor-specific control transfer transport.
    UsbVendor,
}

/// Razer protocol parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RazerParams {
    /// Razer protocol version.
    pub version: RazerProtocolVersion,

    /// Matrix addressing mode.
    pub matrix_type: RazerMatrixType,

    /// Matrix dimensions in `(rows, cols)`.
    pub matrix_size: (u8, u8),

    /// Primary LED ID.
    pub led_id: u8,
}

/// Lian Li hub variant placeholder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LianLiHubVariant {
    /// Placeholder for future variants.
    Unknown,
}

/// PrismRGB model placeholder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrismRgbModel {
    /// Placeholder for future models.
    Unknown,
}

/// Protocol-family-specific parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolParams {
    /// Razer protocol parameters.
    Razer(RazerParams),

    /// Lian Li protocol parameters.
    LianLi {
        /// Hub variant.
        variant: LianLiHubVariant,
    },

    /// PrismRGB protocol parameters.
    PrismRgb {
        /// Controller model.
        model: PrismRgbModel,
    },

    /// Native Corsair protocol parameters (future phase).
    Corsair {
        /// USB interface number.
        interface: u8,
        /// HID usage page.
        usage_page: u16,
        /// HID usage.
        usage: u16,
    },
}

macro_rules! razer_device_descriptor {
    (
        $pid:expr,
        $name:expr,
        $version:ident,
        $matrix:ident,
        ($rows:expr, $cols:expr),
        $interface:expr,
        $led_id:expr
    ) => {
        DeviceDescriptor {
            vendor_id: RAZER_VENDOR_ID,
            product_id: $pid,
            name: $name,
            family: DeviceFamily::Razer,
            transport: TransportType::UsbControl {
                interface: $interface,
                report_id: 0x00,
            },
            params: ProtocolParams::Razer(RazerParams {
                version: RazerProtocolVersion::$version,
                matrix_type: RazerMatrixType::$matrix,
                matrix_size: ($rows, $cols),
                led_id: $led_id,
            }),
            firmware_predicate: None,
        }
    };
}

static DEVICE_DESCRIPTORS: &[DeviceDescriptor] = &[
    razer_device_descriptor!(
        PID_HUNTSMAN_V2,
        "Razer Huntsman V2",
        Extended,
        Extended,
        (6, 22),
        3,
        LED_ID_BACKLIGHT
    ),
    razer_device_descriptor!(
        PID_BASILISK_V3,
        "Razer Basilisk V3",
        Modern,
        Extended,
        (1, 11),
        3,
        LED_ID_ZERO
    ),
    razer_device_descriptor!(
        PID_SEIREN_EMOTE,
        "Razer Seiren Emote",
        Extended,
        Extended,
        (8, 8),
        3,
        LED_ID_ZERO
    ),
];

static MAP_BY_VID_PID: LazyLock<HashMap<(u16, u16), Vec<&'static DeviceDescriptor>>> =
    LazyLock::new(|| {
        let mut map: HashMap<(u16, u16), Vec<&'static DeviceDescriptor>> = HashMap::new();
        for descriptor in DEVICE_DESCRIPTORS {
            map.entry((descriptor.vendor_id, descriptor.product_id))
                .or_default()
                .push(descriptor);
        }
        map
    });

static KNOWN_VID_PIDS: LazyLock<Vec<(u16, u16)>> = LazyLock::new(|| {
    let mut set = BTreeSet::new();
    for descriptor in DEVICE_DESCRIPTORS {
        set.insert((descriptor.vendor_id, descriptor.product_id));
    }
    set.into_iter().collect()
});

/// Static protocol database.
pub struct ProtocolDatabase;

impl ProtocolDatabase {
    /// Lookup a descriptor by `(VID, PID)`.
    #[must_use]
    pub fn lookup(vendor_id: u16, product_id: u16) -> Option<&'static DeviceDescriptor> {
        Self::lookup_with_firmware(vendor_id, product_id, None)
    }

    /// Lookup a descriptor by `(VID, PID)` and optional firmware string.
    ///
    /// When multiple descriptors share a PID, firmware predicates are applied.
    #[must_use]
    pub fn lookup_with_firmware(
        vendor_id: u16,
        product_id: u16,
        firmware: Option<&str>,
    ) -> Option<&'static DeviceDescriptor> {
        let candidates = MAP_BY_VID_PID.get(&(vendor_id, product_id))?;

        if let Some(firmware) = firmware {
            for descriptor in candidates {
                if descriptor
                    .firmware_predicate
                    .is_some_and(|predicate| predicate(firmware))
                {
                    return Some(descriptor);
                }
            }
        }

        candidates
            .iter()
            .find(|descriptor| descriptor.firmware_predicate.is_none())
            .copied()
            .or_else(|| candidates.first().copied())
    }

    /// All known VID/PID pairs used by the scanner filter.
    #[must_use]
    pub fn known_vid_pids() -> &'static [(u16, u16)] {
        KNOWN_VID_PIDS.as_slice()
    }

    /// All descriptors for a vendor.
    #[must_use]
    pub fn by_vendor(vendor_id: u16) -> Vec<&'static DeviceDescriptor> {
        DEVICE_DESCRIPTORS
            .iter()
            .filter(|descriptor| descriptor.vendor_id == vendor_id)
            .collect()
    }

    /// Total registered descriptor count.
    #[must_use]
    pub fn count() -> usize {
        DEVICE_DESCRIPTORS.len()
    }

    /// All descriptors in registration order.
    #[must_use]
    pub fn all() -> &'static [DeviceDescriptor] {
        DEVICE_DESCRIPTORS
    }
}
