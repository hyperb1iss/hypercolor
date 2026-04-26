//! Static HAL protocol descriptor database.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::LazyLock;

use hypercolor_types::device::{
    DriverCapabilitySet, DriverModuleDescriptor, DriverModuleKind, DriverTransportKind,
};

pub use crate::registry::{DeviceDescriptor, ProtocolBinding, ProtocolFactory, TransportType};

use crate::drivers::{asus, corsair, dygma, lianli, nollie, prismrgb, push2, qmk, razer};

static DEVICE_DESCRIPTORS: LazyLock<Vec<DeviceDescriptor>> = LazyLock::new(|| {
    let mut descriptors = Vec::new();
    descriptors.extend_from_slice(asus::devices::descriptors());
    descriptors.extend_from_slice(corsair::devices::descriptors());
    descriptors.extend_from_slice(dygma::devices::descriptors());
    descriptors.extend_from_slice(lianli::devices::descriptors());
    descriptors.extend_from_slice(nollie::devices::descriptors());
    descriptors.extend_from_slice(prismrgb::devices::descriptors());
    descriptors.extend_from_slice(push2::descriptors());
    descriptors.extend_from_slice(qmk::devices::descriptors());
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

static MODULE_DESCRIPTORS: LazyLock<Vec<DriverModuleDescriptor>> = LazyLock::new(|| {
    let mut modules = BTreeMap::<String, HalModuleAccumulator>::new();
    for descriptor in DEVICE_DESCRIPTORS.iter() {
        let id = descriptor.family.id().into_owned();
        let entry = modules.entry(id.clone()).or_insert_with(|| {
            HalModuleAccumulator::new(id, descriptor.family.vendor_name().to_owned())
        });
        entry.add_transport(transport_kind(descriptor.transport));
    }

    modules
        .into_values()
        .map(HalModuleAccumulator::into_descriptor)
        .collect()
});

struct HalModuleAccumulator {
    id: String,
    display_name: String,
    transports: Vec<DriverTransportKind>,
}

impl HalModuleAccumulator {
    fn new(id: String, display_name: String) -> Self {
        Self {
            id,
            display_name,
            transports: Vec::new(),
        }
    }

    fn add_transport(&mut self, transport: DriverTransportKind) {
        if !self.transports.contains(&transport) {
            self.transports.push(transport);
        }
    }

    fn into_descriptor(self) -> DriverModuleDescriptor {
        let mut transports = self.transports;
        transports.sort_by_key(transport_sort_key);

        DriverModuleDescriptor {
            id: self.id,
            display_name: self.display_name,
            vendor_name: None,
            module_kind: DriverModuleKind::Hal,
            transports,
            capabilities: DriverCapabilitySet {
                config: false,
                discovery: false,
                pairing: false,
                backend_factory: false,
                protocol_catalog: true,
                runtime_cache: false,
                credentials: false,
                presentation: false,
                controls: false,
            },
            api_schema_version: 1,
            config_version: 1,
            default_enabled: true,
        }
    }
}

const fn transport_sort_key(transport: &DriverTransportKind) -> u8 {
    match transport {
        DriverTransportKind::Network => 0,
        DriverTransportKind::Usb => 1,
        DriverTransportKind::Smbus => 2,
        DriverTransportKind::Midi => 3,
        DriverTransportKind::Serial => 4,
        DriverTransportKind::Virtual => 5,
        DriverTransportKind::Custom(_) => 6,
    }
}

const fn transport_kind(transport: TransportType) -> DriverTransportKind {
    match transport {
        TransportType::UsbControl { .. }
        | TransportType::UsbHidApi { .. }
        | TransportType::UsbHidRaw { .. }
        | TransportType::UsbHid { .. }
        | TransportType::UsbBulk { .. }
        | TransportType::UsbVendor => DriverTransportKind::Usb,
        TransportType::UsbMidi { .. } => DriverTransportKind::Midi,
        TransportType::UsbSerial { .. } => DriverTransportKind::Serial,
        TransportType::I2cSmBus { .. } => DriverTransportKind::Smbus,
    }
}

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
        Self::lookup_with_firmware_for_driver_ids(vendor_id, product_id, firmware, None)
    }

    /// Lookup a descriptor by `(VID, PID)`, firmware string, and optional
    /// enabled driver module IDs.
    ///
    /// When `enabled_driver_ids` is provided, descriptors whose family ID is
    /// absent are ignored.
    #[must_use]
    pub fn lookup_with_firmware_for_driver_ids(
        vendor_id: u16,
        product_id: u16,
        firmware: Option<&str>,
        enabled_driver_ids: Option<&BTreeSet<String>>,
    ) -> Option<&'static DeviceDescriptor> {
        let candidates = MAP_BY_VID_PID.get(&(vendor_id, product_id))?;
        let driver_enabled = |descriptor: &DeviceDescriptor| {
            enabled_driver_ids.is_none_or(|ids| ids.contains(descriptor.family.id().as_ref()))
        };

        if let Some(firmware) = firmware {
            for descriptor in candidates
                .iter()
                .copied()
                .filter(|descriptor| driver_enabled(descriptor))
            {
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
            .copied()
            .filter(|descriptor| driver_enabled(descriptor))
            .find(|descriptor| descriptor.firmware_predicate.is_none())
            .or_else(|| {
                candidates
                    .iter()
                    .copied()
                    .find(|descriptor| driver_enabled(descriptor))
            })
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

    /// All HAL protocol modules in deterministic order.
    #[must_use]
    pub fn module_descriptors() -> &'static [DriverModuleDescriptor] {
        MODULE_DESCRIPTORS.as_slice()
    }
}
