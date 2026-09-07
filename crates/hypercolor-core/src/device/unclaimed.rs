//! Inventory of USB devices the host can see but no enabled native driver
//! claims.
//!
//! The USB scanner and the hotplug watcher used to drop such devices on the
//! floor, which made "why does Hypercolor not see my thing" unanswerable
//! from the API. Both now report every observation here: the scanner
//! replaces the whole snapshot per scan, the hotplug loop patches it per
//! event. The store decides what counts as unclaimed from the set of
//! enabled driver ids the daemon hands it, so a device whose descriptor
//! exists but whose driver is disabled shows up with `claimable_by` set.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, RwLock};

use hypercolor_types::api::devices::UnclaimedDevice;
use hypercolor_types::event::HypercolorEvent;

use crate::bus::HypercolorBus;

/// One USB device as the scanner or hotplug watcher saw it.
///
/// `descriptor_driver_id` is the native driver whose protocol database
/// matches the vendor/product pair, when one exists; the store consults
/// its enabled-driver set to decide whether that makes the device claimed
/// or merely claimable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsbObservation {
    pub vendor_id: u16,
    pub product_id: u16,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
    pub serial: Option<String>,
    pub bus_path: Option<String>,
    pub interface_classes: Vec<u8>,
    pub descriptor_driver_id: Option<String>,
}

impl UsbObservation {
    /// Stable key for patching one observation in and out of the snapshot.
    ///
    /// Bus path first because two identical units on different ports must
    /// stay distinct; vendor/product/serial round it out for platforms
    /// that report no path.
    #[must_use]
    pub fn key(&self) -> String {
        format!(
            "{}|{:04x}:{:04x}|{}",
            self.bus_path.as_deref().unwrap_or(""),
            self.vendor_id,
            self.product_id,
            self.serial.as_deref().unwrap_or("")
        )
    }

    fn into_unclaimed(self, claimable_by: Option<String>) -> UnclaimedDevice {
        let mut interface_classes = self.interface_classes;
        interface_classes.sort_unstable();
        interface_classes.dedup();
        UnclaimedDevice {
            vendor_id: self.vendor_id,
            product_id: self.product_id,
            manufacturer: self.manufacturer,
            product: self.product,
            serial: self.serial,
            bus_path: self.bus_path,
            interface_classes,
            claimable_by,
        }
    }
}

#[derive(Debug, Default)]
struct UnclaimedInner {
    /// Every observation keyed by [`UsbObservation::key`], claimed or not,
    /// so a later change to the enabled driver set can re-derive the
    /// unclaimed view without another scan.
    observations: BTreeMap<String, UsbObservation>,
    /// Driver ids the daemon currently runs; `None` treats every
    /// descriptor-backed device as claimed.
    enabled_driver_ids: Option<BTreeSet<String>>,
    /// The last published view, for change detection.
    published: Vec<UnclaimedDevice>,
}

/// What the enabled driver set says about one observation.
enum Ownership {
    /// An enabled driver's protocol database matches it.
    Claimed,
    /// Nobody enabled drives it; the payload is the disabled driver that
    /// could, when a descriptor exists.
    Unclaimed(Option<String>),
}

impl UnclaimedInner {
    fn ownership(&self, observation: &UsbObservation) -> Ownership {
        match &observation.descriptor_driver_id {
            None => Ownership::Unclaimed(None),
            Some(driver_id) => match &self.enabled_driver_ids {
                Some(enabled) if !enabled.contains(driver_id) => {
                    Ownership::Unclaimed(Some(driver_id.clone()))
                }
                // No enabled set means "trust the descriptor": the device
                // is somebody's, so it is not unclaimed.
                _ => Ownership::Claimed,
            },
        }
    }

    fn view(&self) -> Vec<UnclaimedDevice> {
        let mut view: Vec<UnclaimedDevice> = self
            .observations
            .values()
            .filter_map(|observation| match self.ownership(observation) {
                Ownership::Claimed => None,
                Ownership::Unclaimed(claimable_by) => {
                    Some(observation.clone().into_unclaimed(claimable_by))
                }
            })
            .collect();
        view.sort_by(|left, right| {
            (
                left.vendor_id,
                left.product_id,
                &left.bus_path,
                &left.serial,
            )
                .cmp(&(
                    right.vendor_id,
                    right.product_id,
                    &right.bus_path,
                    &right.serial,
                ))
        });
        view
    }
}

/// Shared inventory of unclaimed USB devices.
///
/// Cloning shares the same inventory, mirroring
/// [`UsbProtocolConfigStore`](super::UsbProtocolConfigStore) so the driver
/// bundle, the scanner, the hotplug watcher, and the API all see one list.
#[derive(Clone, Default)]
pub struct UnclaimedDeviceStore {
    inner: Arc<RwLock<UnclaimedInner>>,
    event_bus: Option<Arc<HypercolorBus>>,
}

impl std::fmt::Debug for UnclaimedDeviceStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UnclaimedDeviceStore")
            .field("devices", &self.len())
            .finish_non_exhaustive()
    }
}

impl UnclaimedDeviceStore {
    /// An empty inventory that publishes nothing.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Publish [`HypercolorEvent::UnclaimedDevicesChanged`] on `bus`
    /// whenever the unclaimed view changes.
    #[must_use]
    pub fn with_event_bus(mut self, bus: Arc<HypercolorBus>) -> Self {
        self.event_bus = Some(bus);
        self
    }

    /// Tell the store which native drivers are running so descriptor-backed
    /// devices of disabled drivers surface as claimable.
    pub fn set_enabled_driver_ids(&self, enabled_driver_ids: Option<BTreeSet<String>>) {
        let changed = {
            let mut inner = self.write();
            if inner.enabled_driver_ids == enabled_driver_ids {
                return;
            }
            inner.enabled_driver_ids = enabled_driver_ids;
            Self::refresh_published(&mut inner)
        };
        self.publish_if(changed);
    }

    /// The enabled driver set the store is currently filtering with.
    #[must_use]
    pub fn enabled_driver_ids(&self) -> Option<BTreeSet<String>> {
        self.read().enabled_driver_ids.clone()
    }

    /// Replace every observation with the result of a full scan.
    pub fn replace_snapshot(&self, observations: impl IntoIterator<Item = UsbObservation>) {
        let changed = {
            let mut inner = self.write();
            inner.observations = observations
                .into_iter()
                .map(|observation| (observation.key(), observation))
                .collect();
            Self::refresh_published(&mut inner)
        };
        self.publish_if(changed);
    }

    /// Record one arrival from the hotplug watcher.
    pub fn upsert(&self, observation: UsbObservation) {
        let changed = {
            let mut inner = self.write();
            inner.observations.insert(observation.key(), observation);
            Self::refresh_published(&mut inner)
        };
        self.publish_if(changed);
    }

    /// Forget one observation by its [`UsbObservation::key`].
    pub fn remove(&self, key: &str) {
        let changed = {
            let mut inner = self.write();
            if inner.observations.remove(key).is_none() {
                return;
            }
            Self::refresh_published(&mut inner)
        };
        self.publish_if(changed);
    }

    /// The current unclaimed view, sorted by vendor, product, path, serial.
    #[must_use]
    pub fn snapshot(&self) -> Vec<UnclaimedDevice> {
        self.read().published.clone()
    }

    /// How many devices the unclaimed view lists.
    #[must_use]
    pub fn len(&self) -> usize {
        self.read().published.len()
    }

    /// Whether the unclaimed view is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn refresh_published(inner: &mut UnclaimedInner) -> Option<usize> {
        let view = inner.view();
        if view == inner.published {
            return None;
        }
        let count = view.len();
        inner.published = view;
        Some(count)
    }

    fn publish_if(&self, changed: Option<usize>) {
        if let (Some(count), Some(bus)) = (changed, self.event_bus.as_ref()) {
            bus.publish(HypercolorEvent::UnclaimedDevicesChanged { count });
        }
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, UnclaimedInner> {
        self.inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, UnclaimedInner> {
        self.inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}
