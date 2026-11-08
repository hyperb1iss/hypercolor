//! Static HAL protocol descriptor database.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::LazyLock;

use hypercolor_types::device::{
    BLOCKS_OUTPUT_BACKEND_ID, DRIVER_MODULE_API_SCHEMA_VERSION, DriverCapabilitySet,
    DriverModuleDescriptor, DriverModuleKind, DriverProtocolDescriptor, DriverStability,
    DriverTransportDescriptor, DriverTransportKind, SMBUS_OUTPUT_BACKEND_ID, USB_OUTPUT_BACKEND_ID,
};

pub use crate::registry::{
    DeviceDescriptor, ProtocolBinding, ProtocolFactory, TransportType, UsbTransportKind,
};

use crate::drivers::{
    asus, corsair, dygma, lianli, nollie, nzxt, prismrgb, push2, qmk, razer, wooting,
};
use crate::smbus_registry::ASUS_AURA_SMBUS_PROTOCOL_ID;
use crate::transport::{TransportError, TransportIntent, TransportPlatform, resolve_transport};

static DEVICE_DESCRIPTORS: LazyLock<Vec<DeviceDescriptor>> = LazyLock::new(|| {
    let mut descriptors = Vec::new();
    descriptors.extend_from_slice(asus::devices::descriptors());
    descriptors.extend_from_slice(corsair::devices::descriptors());
    descriptors.extend_from_slice(dygma::devices::descriptors());
    descriptors.extend_from_slice(lianli::devices::descriptors());
    descriptors.extend_from_slice(nollie::devices::descriptors());
    descriptors.extend_from_slice(nzxt::devices::descriptors());
    descriptors.extend_from_slice(prismrgb::devices::descriptors());
    descriptors.extend_from_slice(push2::descriptors());
    descriptors.extend_from_slice(qmk::devices::descriptors());
    descriptors.extend_from_slice(razer::devices::descriptors());
    descriptors.extend_from_slice(wooting::devices::descriptors());
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

static MODULE_DESCRIPTORS: LazyLock<Vec<DriverModuleDescriptor>> =
    LazyLock::new(|| derive_module_descriptors(DEVICE_DESCRIPTORS.as_slice()));

static PROTOCOL_DESCRIPTORS: LazyLock<Vec<DriverProtocolDescriptor>> = LazyLock::new(|| {
    let mut descriptors = DEVICE_DESCRIPTORS
        .iter()
        .map(protocol_descriptor)
        .collect::<Vec<_>>();
    descriptors.push(DriverProtocolDescriptor {
        driver_id: "asus".to_owned(),
        protocol_id: ASUS_AURA_SMBUS_PROTOCOL_ID.to_owned(),
        display_name: "ASUS Aura SMBus".to_owned(),
        vendor_id: None,
        product_id: None,
        family_id: "asus".to_owned(),
        model_id: None,
        transport: DriverTransportKind::Smbus,
        route_backend_id: SMBUS_OUTPUT_BACKEND_ID.to_owned(),
        presentation: None,
        stability: DriverStability::Stable,
    });
    descriptors.sort_by(|left, right| {
        left.driver_id
            .cmp(&right.driver_id)
            .then_with(|| left.protocol_id.cmp(&right.protocol_id))
            .then_with(|| left.product_id.cmp(&right.product_id))
    });
    descriptors
});

fn derive_module_descriptors(descriptors: &[DeviceDescriptor]) -> Vec<DriverModuleDescriptor> {
    let mut modules = BTreeMap::<String, HalModuleAccumulator>::new();
    for descriptor in descriptors {
        let id = descriptor.driver_id().into_owned();
        let entry = modules.entry(id.clone()).or_insert_with(|| {
            HalModuleAccumulator::new(id, descriptor.driver_display_name().into_owned())
        });
        entry.add_transport(DriverTransportDescriptor::available(transport_kind(
            descriptor.transport,
        )));
        entry.add_stability(descriptor.stability);
    }
    if let Some(module) = modules.get_mut("asus") {
        module.add_transport(smbus_transport_descriptor());
        module.add_stability(DriverStability::Stable);
    }

    modules
        .into_values()
        .map(HalModuleAccumulator::into_descriptor)
        .collect()
}

/// HAL families a fresh install leaves off until the user turns them on.
///
/// A family belongs here when its devices are registered but cannot light
/// yet. `dygma`: stock Defy firmware has no live LED path for the Focus
/// protocol to drive, so frame writes are dropped.
const DEFAULT_DISABLED_FAMILIES: &[&str] = &["dygma"];

struct HalModuleAccumulator {
    id: String,
    display_name: String,
    transports: Vec<DriverTransportDescriptor>,
    /// Whether any descriptor of the family is stable. A family made only
    /// of experimental descriptors gets the experimental module label.
    has_stable_descriptor: bool,
}

impl HalModuleAccumulator {
    fn new(id: String, display_name: String) -> Self {
        Self {
            id,
            display_name,
            transports: Vec::new(),
            has_stable_descriptor: false,
        }
    }

    fn add_stability(&mut self, stability: DriverStability) {
        self.has_stable_descriptor |= !stability.is_experimental();
    }

    fn add_transport(&mut self, transport: DriverTransportDescriptor) {
        if !self
            .transports
            .iter()
            .any(|existing| existing.kind == transport.kind)
        {
            self.transports.push(transport);
        }
    }

    fn into_descriptor(self) -> DriverModuleDescriptor {
        let mut transports = self.transports;
        transports.sort_by_key(|transport| transport_sort_key(&transport.kind));

        let stability = if self.has_stable_descriptor {
            DriverStability::Stable
        } else {
            DriverStability::Experimental
        };
        let default_enabled = !DEFAULT_DISABLED_FAMILIES.contains(&self.id.as_str());

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
                output_backend: false,
                protocol_catalog: true,
                runtime_cache: false,
                credentials: false,
                presentation: false,
                controls: false,
            },
            api_schema_version: DRIVER_MODULE_API_SCHEMA_VERSION,
            config_version: 1,
            default_enabled,
            stability,
        }
    }
}

fn smbus_transport_descriptor() -> DriverTransportDescriptor {
    match resolve_transport(
        TransportIntent::I2cSmBus { address: 0x40 },
        TransportPlatform::CURRENT,
    ) {
        Ok(_) => DriverTransportDescriptor::available(DriverTransportKind::Smbus),
        Err(TransportError::UnsupportedPlatform { platform, .. }) => {
            DriverTransportDescriptor::unsupported_platform(
                DriverTransportKind::Smbus,
                platform.to_string(),
            )
        }
        Err(_) => unreachable!("transport resolution only reports platform support"),
    }
}

const fn transport_sort_key(transport: &DriverTransportKind) -> u8 {
    match transport {
        DriverTransportKind::Network => 0,
        DriverTransportKind::Usb => 1,
        DriverTransportKind::Smbus => 2,
        DriverTransportKind::Midi => 3,
        DriverTransportKind::Serial => 4,
        DriverTransportKind::Bridge => 5,
        DriverTransportKind::Virtual => 6,
        DriverTransportKind::Custom(_) => 7,
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
        TransportType::DriverUsb { binding } => match binding.kind {
            UsbTransportKind::Usb => DriverTransportKind::Usb,
            UsbTransportKind::Midi => DriverTransportKind::Midi,
        },
        TransportType::UsbSerial { .. } => DriverTransportKind::Serial,
        TransportType::I2cSmBus { .. } => DriverTransportKind::Smbus,
    }
}

fn protocol_descriptor(descriptor: &DeviceDescriptor) -> DriverProtocolDescriptor {
    let transport = transport_kind(descriptor.transport);
    DriverProtocolDescriptor {
        driver_id: descriptor.driver_id().into_owned(),
        protocol_id: descriptor.protocol.id.to_owned(),
        display_name: descriptor.name.to_owned(),
        vendor_id: Some(descriptor.vendor_id),
        product_id: Some(descriptor.product_id),
        family_id: descriptor.family.id().into_owned(),
        model_id: None,
        route_backend_id: route_backend_id(&transport).to_owned(),
        transport,
        presentation: None,
        stability: descriptor.stability,
    }
}

const fn route_backend_id(transport: &DriverTransportKind) -> &'static str {
    match transport {
        DriverTransportKind::Network => "network",
        DriverTransportKind::Smbus => SMBUS_OUTPUT_BACKEND_ID,
        DriverTransportKind::Usb | DriverTransportKind::Midi | DriverTransportKind::Serial => {
            USB_OUTPUT_BACKEND_ID
        }
        DriverTransportKind::Bridge => BLOCKS_OUTPUT_BACKEND_ID,
        DriverTransportKind::Virtual => "virtual",
        DriverTransportKind::Custom(_) => "custom",
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
    /// When `enabled_driver_ids` is provided, descriptors whose driver ID is
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
            enabled_driver_ids.is_none_or(|ids| ids.contains(descriptor.driver_id().as_ref()))
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

    /// All HAL protocol descriptors in deterministic order.
    #[must_use]
    pub fn protocol_descriptors() -> &'static [DriverProtocolDescriptor] {
        PROTOCOL_DESCRIPTORS.as_slice()
    }

    /// HAL protocol descriptors owned by one driver module.
    #[must_use]
    pub fn protocol_descriptors_for_driver(driver_id: &str) -> Vec<DriverProtocolDescriptor> {
        PROTOCOL_DESCRIPTORS
            .iter()
            .filter(|descriptor| descriptor.driver_id == driver_id)
            .cloned()
            .collect()
    }

    /// Derive driver module descriptors from `descriptors` exactly as the
    /// static database derives its own.
    ///
    /// A family whose every descriptor is experimental gets the
    /// experimental module label; one stable descriptor keeps the module
    /// stable, and its experimental protocols carry the label on their own
    /// protocol descriptors (Spec 89).
    #[must_use]
    pub fn derive_module_descriptors(
        descriptors: &[DeviceDescriptor],
    ) -> Vec<DriverModuleDescriptor> {
        derive_module_descriptors(descriptors)
    }

    /// The protocol descriptor the catalog publishes for one device
    /// descriptor, stability label included.
    #[must_use]
    pub fn protocol_descriptor_for(descriptor: &DeviceDescriptor) -> DriverProtocolDescriptor {
        protocol_descriptor(descriptor)
    }

    /// Protocol ids in `descriptors` whose descriptors disagree on their
    /// stability label.
    ///
    /// A connected device is labeled through the protocol id in its
    /// origin, so every descriptor sharing a protocol id must carry one
    /// label; give an experimental variant its own protocol id.
    #[must_use]
    pub fn stability_conflicts(descriptors: &[DeviceDescriptor]) -> Vec<&'static str> {
        let mut by_protocol = BTreeMap::<&'static str, BTreeSet<DriverStability>>::new();
        for descriptor in descriptors {
            by_protocol
                .entry(descriptor.protocol.id)
                .or_default()
                .insert(descriptor.stability);
        }
        by_protocol
            .into_iter()
            .filter(|(_, labels)| labels.len() > 1)
            .map(|(protocol_id, _)| protocol_id)
            .collect()
    }
}
