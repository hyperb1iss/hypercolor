//! Static USB protocol descriptor database.

use std::collections::{BTreeSet, HashMap};
use std::sync::LazyLock;

pub use crate::registry::{DeviceDescriptor, ProtocolBinding, ProtocolFactory, TransportType};

use crate::drivers::{corsair, dygma, prismrgb, razer};

static DEVICE_DESCRIPTORS: LazyLock<Vec<DeviceDescriptor>> = LazyLock::new(|| {
    let mut descriptors = Vec::new();
    descriptors.extend_from_slice(corsair::devices::descriptors());
    descriptors.extend_from_slice(dygma::devices::descriptors());
    descriptors.extend_from_slice(prismrgb::devices::descriptors());
    descriptors.extend_from_slice(razer::devices::descriptors());
    descriptors
});

static MAP_BY_VID_PID: LazyLock<HashMap<(u16, u16), Vec<&'static DeviceDescriptor>>> =
    LazyLock::new(|| {
        let mut map: HashMap<(u16, u16), Vec<&'static DeviceDescriptor>> = HashMap::new();
        for descriptor in DEVICE_DESCRIPTORS.iter() {
            map.entry((descriptor.vendor_id, descriptor.product_id))
                .or_default()
                .push(descriptor);
        }
        map
    });

static KNOWN_VID_PIDS: LazyLock<Vec<(u16, u16)>> = LazyLock::new(|| {
    let mut set = BTreeSet::new();
    for descriptor in DEVICE_DESCRIPTORS.iter() {
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
        DEVICE_DESCRIPTORS.as_slice()
    }
}
