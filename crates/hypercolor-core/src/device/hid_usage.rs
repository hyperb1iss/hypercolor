//! Joins the host HID stack's top-level collections onto USB observations.
//!
//! nusb describes a USB device down to its interface classes but not to the
//! usage pages of its HID collections, and those pages are what separate a
//! media-key function from a lighting controller. hidapi reports a usage
//! pair per top-level collection; this module decides which collections
//! belong to which nusb device. Every doubt resolves to "unknown", which
//! leaves the observation's page list empty, and an empty list never hides
//! a device.

use hypercolor_hal::transport::hidapi::{HidCollectionInfo, enumerate_usb_hid_collections};
use tracing::debug;

use super::unclaimed::UsbObservation;

/// Every USB HID top-level collection the host exposes, enumerated once and
/// joined onto as many observations as need it.
#[derive(Debug, Clone, Default)]
pub struct HidUsageIndex {
    collections: Vec<HidCollectionInfo>,
}

impl HidUsageIndex {
    /// An index over already-enumerated collections.
    #[must_use]
    pub fn new(collections: Vec<HidCollectionInfo>) -> Self {
        Self { collections }
    }

    /// Enumerate the host HID stack.
    ///
    /// This blocks on the platform HID API, so async callers run it on the
    /// blocking pool. A failed enumeration yields an empty index, and every
    /// join against it comes back unknown.
    #[must_use]
    pub fn enumerate() -> Self {
        match enumerate_usb_hid_collections() {
            Ok(collections) => Self::new(collections),
            Err(error) => {
                debug!(%error, "HID enumeration failed; usage pages stay unknown");
                Self::default()
            }
        }
    }

    /// The top-level usage pages of `observation`'s HID collections, sorted
    /// and deduplicated, or empty when the join cannot vouch for them.
    ///
    /// `hid_interfaces` lists the interface numbers USB enumeration reports
    /// with the HID class. Each of them must be covered by at least one
    /// collection, or the pages would describe only part of the device (a
    /// device that just arrived may still be binding its HID functions).
    ///
    /// Collections join by USB path where the platform resolves one, which
    /// separates identical units. Elsewhere they join by vendor, product,
    /// and serial when the device reports one. A device without a serial
    /// joins on vendor and product alone, because Windows invents a serial
    /// for it on the HID side. `has_twin` says whether another attached
    /// device [could own the same collections](UsbObservation::is_hid_twin_of);
    /// when it could and no path settles it, the answer is unknown. Two
    /// identical units without serials therefore both stay visible rather
    /// than one borrowing the other's collections.
    #[must_use]
    pub fn usage_pages_for(
        &self,
        observation: &UsbObservation,
        hid_interfaces: &[u8],
        has_twin: bool,
    ) -> Vec<u16> {
        if hid_interfaces.is_empty() {
            return Vec::new();
        }
        let joined = self.joined_collections(observation, has_twin);
        let mut covered = vec![false; hid_interfaces.len()];
        let mut pages = Vec::with_capacity(joined.len());
        for collection in joined {
            let interface = match (collection.interface_number, hid_interfaces) {
                (Some(interface), _) => interface,
                // hidapi could not name the interface; that is only safe to
                // resolve when the device has a single HID interface.
                (None, [only]) => *only,
                (None, _) => return Vec::new(),
            };
            let Some(slot) = hid_interfaces.iter().position(|&hid| hid == interface) else {
                // A collection on an interface USB enumeration does not call
                // HID means the join matched something it should not have.
                return Vec::new();
            };
            covered[slot] = true;
            pages.push(collection.usage_page);
        }
        if covered.contains(&false) {
            return Vec::new();
        }
        pages.sort_unstable();
        pages.dedup();
        pages
    }

    fn joined_collections(
        &self,
        observation: &UsbObservation,
        has_twin: bool,
    ) -> Vec<&HidCollectionInfo> {
        let same_model = |collection: &&HidCollectionInfo| {
            collection.vendor_id == observation.vendor_id
                && collection.product_id == observation.product_id
        };

        if let Some(bus_path) = observation.bus_path.as_deref() {
            let by_path: Vec<_> = self
                .collections
                .iter()
                .filter(same_model)
                .filter(|collection| collection.is_at_usb_path(bus_path))
                .collect();
            if !by_path.is_empty() {
                return by_path;
            }
        }

        if has_twin {
            return Vec::new();
        }
        let serial = observation.serial.as_deref().map(serial_key);
        self.collections
            .iter()
            .filter(same_model)
            // A collection whose path was resolved and did not match above
            // belongs to some other unit.
            .filter(|collection| observation.bus_path.is_none() || collection.usb_path.is_none())
            .filter(|collection| {
                serial.is_none_or(|serial| collection.serial.as_deref().map(serial_key) == Some(serial))
            })
            .collect()
    }
}

/// Enumerate the host HID stack on the blocking pool so the executor thread
/// running a scan or the hotplug loop never waits on the platform HID API.
pub(crate) async fn enumerate_off_executor() -> HidUsageIndex {
    tokio::task::spawn_blocking(HidUsageIndex::enumerate)
        .await
        .unwrap_or_else(|error| {
            debug!(%error, "HID enumeration task failed; usage pages stay unknown");
            HidUsageIndex::default()
        })
}

/// Serials compare without the padding some firmware leaves in the
/// descriptor.
fn serial_key(serial: &str) -> &str {
    serial.trim_matches(|c: char| c.is_whitespace() || c == '\0')
}
