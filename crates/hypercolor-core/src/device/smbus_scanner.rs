//! `SMBus` scanner for ASUS Aura ENE controllers.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use hypercolor_types::device::{ConnectionType, DeviceFingerprint, DeviceInfo};

use super::discovery::{DiscoveredDevice, DiscoveryConnectBehavior, TransportScanner};

#[cfg(target_os = "linux")]
use anyhow::anyhow;
#[cfg(target_os = "linux")]
use hypercolor_hal::drivers::asus::{
    AuraSmBusProtocol, encode_ene_transaction, ene_dram_remap_sequence,
};
#[cfg(target_os = "linux")]
use hypercolor_hal::protocol::{Protocol, ProtocolZone, ResponseStatus};
#[cfg(target_os = "linux")]
use hypercolor_hal::transport::Transport;
#[cfg(target_os = "linux")]
use hypercolor_hal::transport::smbus::SmBusTransport;
#[cfg(target_os = "linux")]
use hypercolor_types::device::DeviceOrigin;
#[cfg(target_os = "linux")]
use hypercolor_types::device::{DeviceFamily, DeviceIdentifier};
#[cfg(target_os = "linux")]
use std::collections::HashSet;
#[cfg(target_os = "linux")]
use tracing::{debug, trace};

#[cfg(target_os = "linux")]
const ASUS_SMBUS_BACKEND_ID: &str = "smbus";

#[cfg(target_os = "linux")]
const ASUS_MOTHERBOARD_SMBUS_ADDRESSES: &[(u16, SmBusControllerKind)] = &[
    (0x40, SmBusControllerKind::Motherboard),
    (0x4E, SmBusControllerKind::Motherboard),
    (0x4F, SmBusControllerKind::Motherboard),
];

#[cfg(target_os = "linux")]
const ASUS_GPU_SMBUS_ADDRESSES: &[(u16, SmBusControllerKind)] = &[
    (0x29, SmBusControllerKind::Gpu),
    (0x2A, SmBusControllerKind::Gpu),
    (0x67, SmBusControllerKind::Gpu),
];

#[cfg(target_os = "linux")]
const ASUS_DRAM_REMAP_HUB_ADDRESS: u16 = 0x77;
#[cfg(target_os = "linux")]
const ASUS_DRAM_REMAP_SLOT_COUNT: usize = 8;
#[cfg(target_os = "linux")]
const ASUS_DRAM_SMBUS_ADDRESSES: &[u16] = &[
    0x70, 0x71, 0x72, 0x73, 0x74, 0x75, 0x76, 0x78, 0x79, 0x7A, 0x7B, 0x7C, 0x7D, 0x7E, 0x7F, 0x4F,
    0x66, 0x67, 0x39, 0x3A, 0x3B, 0x3C, 0x3D,
];
const INTEL_VENDOR_ID: u16 = 0x8086;
const AMD_VENDOR_ID: u16 = 0x1022;
#[cfg(target_os = "linux")]
const AMD_GPU_VENDOR_ID: u16 = 0x1002;
#[cfg(target_os = "linux")]
const NVIDIA_VENDOR_ID: u16 = 0x10DE;
const SYSTEM_SMBUS_ADAPTER_IDS: &[(u16, u16)] = &[
    (AMD_VENDOR_ID, 0x790B),
    (INTEL_VENDOR_ID, 0x3A30),
    (INTEL_VENDOR_ID, 0xA123),
    (INTEL_VENDOR_ID, 0x2085),
    (INTEL_VENDOR_ID, 0xA2A3),
    (INTEL_VENDOR_ID, 0xA323),
    (INTEL_VENDOR_ID, 0x06A3),
    (INTEL_VENDOR_ID, 0xA3A3),
    (INTEL_VENDOR_ID, 0x43A3),
    (INTEL_VENDOR_ID, 0x7AA3),
    (INTEL_VENDOR_ID, 0x7A23),
    (INTEL_VENDOR_ID, 0x7F23),
];

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SmBusControllerKind {
    Motherboard,
    Gpu,
    Dram,
}

#[cfg(target_os = "linux")]
impl SmBusControllerKind {
    pub(crate) const fn display_name(self) -> &'static str {
        match self {
            Self::Motherboard => "Motherboard",
            Self::Gpu => "GPU",
            Self::Dram => "DRAM",
        }
    }

    pub(crate) const fn model_id(self) -> &'static str {
        match self {
            Self::Motherboard => "asus_aura_smbus_motherboard",
            Self::Gpu => "asus_aura_smbus_gpu",
            Self::Dram => "asus_aura_smbus_dram",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct SmBusProbe {
    pub(crate) fingerprint: DeviceFingerprint,
    pub(crate) info: DeviceInfo,
    pub(crate) metadata: HashMap<String, String>,
}

/// `SMBus` transport scanner for ASUS ENE controllers.
pub struct SmBusScanner {
    dev_root: PathBuf,
}

impl SmBusScanner {
    /// Create an `SMBus` scanner.
    #[must_use]
    pub fn new() -> Self {
        Self::with_dev_root("/dev")
    }

    /// Create an `SMBus` scanner with a custom device-node root.
    #[must_use]
    pub fn with_dev_root<P: Into<PathBuf>>(dev_root: P) -> Self {
        Self {
            dev_root: dev_root.into(),
        }
    }
}

impl Default for SmBusScanner {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl TransportScanner for SmBusScanner {
    fn name(&self) -> &'static str {
        "SMBus HAL"
    }

    async fn scan(&mut self) -> Result<Vec<DiscoveredDevice>> {
        let probes = probe_asus_smbus_devices_in_root(&self.dev_root).await?;
        Ok(probes
            .into_iter()
            .map(|probe| DiscoveredDevice {
                connection_type: ConnectionType::SmBus,
                origin: probe.info.origin.clone(),
                name: probe.info.name.clone(),
                family: probe.info.family.clone(),
                fingerprint: probe.fingerprint,
                connect_behavior: DiscoveryConnectBehavior::AutoConnect,
                info: probe.info,
                metadata: probe.metadata,
            })
            .collect())
    }
}

#[cfg(target_os = "linux")]
pub(crate) async fn probe_asus_smbus_devices_in_root(dev_root: &Path) -> Result<Vec<SmBusProbe>> {
    let mut discovered = Vec::new();

    for bus_path in i2c_bus_paths_in(dev_root)? {
        let pci_id = bus_pci_id(&bus_path);

        if motherboard_capable_bus_pci_id(pci_id) {
            for &(address, controller_kind) in ASUS_MOTHERBOARD_SMBUS_ADDRESSES {
                if let Some(probe) = probe_bus_address(&bus_path, address, controller_kind).await? {
                    discovered.push(probe);
                }
            }
        } else {
            trace!(
                bus_path,
                pci_id = ?pci_id,
                "skipping ASUS Aura motherboard SMBus probe on incompatible i2c adapter"
            );
        }

        if gpu_capable_bus_pci_id(pci_id) {
            for &(address, controller_kind) in ASUS_GPU_SMBUS_ADDRESSES {
                if let Some(probe) = probe_bus_address(&bus_path, address, controller_kind).await? {
                    discovered.push(probe);
                }
            }
        } else {
            trace!(
                bus_path,
                pci_id = ?pci_id,
                "skipping ASUS Aura GPU SMBus probe on non-GPU i2c adapter"
            );
        }

        if dram_capable_bus_pci_id(pci_id) {
            discovered.extend(probe_dram_bus(&bus_path).await?);
        } else {
            trace!(
                bus_path,
                pci_id = ?pci_id,
                "skipping ASUS Aura DRAM probe on non-chipset i2c adapter"
            );
        }
    }

    Ok(discovered)
}

#[cfg(not(target_os = "linux"))]
#[allow(
    clippy::unused_async,
    reason = "the scanner uses one async probe interface across supported and excluded platforms"
)]
pub(crate) async fn probe_asus_smbus_devices_in_root(dev_root: &Path) -> Result<Vec<SmBusProbe>> {
    let _ = dev_root;
    Ok(Vec::new())
}

#[cfg(target_os = "linux")]
async fn probe_dram_bus(bus_path: &str) -> Result<Vec<SmBusProbe>> {
    let hub_present = bus_address_responds(bus_path, ASUS_DRAM_REMAP_HUB_ADDRESS);
    if hub_present {
        debug!(
            bus_path,
            hub_address = format_args!("0x{ASUS_DRAM_REMAP_HUB_ADDRESS:02X}"),
            "detected ASUS Aura DRAM remap hub"
        );
    } else {
        trace!(
            bus_path,
            hub_address = format_args!("0x{ASUS_DRAM_REMAP_HUB_ADDRESS:02X}"),
            "no ASUS Aura DRAM remap hub detected on bus; probing known ENE address pool directly"
        );
    }

    let occupied_addresses = probe_occupied_dram_addresses(bus_path);
    trace!(
        bus_path,
        occupied_addresses = ?occupied_addresses,
        "snapshotted occupied SMBus addresses before DRAM remap"
    );

    let mut remapped_addresses = Vec::new();
    if hub_present {
        let Ok(hub_transport) = SmBusTransport::open(bus_path, ASUS_DRAM_REMAP_HUB_ADDRESS) else {
            return Ok(Vec::new());
        };

        let mut next_address_index = 0_usize;

        for slot_index in 0..ASUS_DRAM_REMAP_SLOT_COUNT {
            if !bus_address_responds(bus_path, ASUS_DRAM_REMAP_HUB_ADDRESS) {
                break;
            }

            let Some((selected_index, remap_address)) =
                next_available_dram_address(next_address_index, &occupied_addresses)
            else {
                break;
            };
            next_address_index = selected_index + 1;

            let target_address = u8::try_from(remap_address).map_err(|_| {
                anyhow!("DRAM remap address 0x{remap_address:02X} exceeds u8 range")
            })?;
            let slot_index = u8::try_from(slot_index)
                .map_err(|_| anyhow!("DRAM slot index {slot_index} exceeds u8 range"))?;
            let remap_command =
                encode_ene_transaction(&ene_dram_remap_sequence(slot_index, target_address))?;

            if let Err(error) = hub_transport.send(&remap_command).await {
                debug!(
                    bus_path,
                    slot_index,
                    remap_address = format_args!("0x{remap_address:02X}"),
                    error = %error,
                    "failed to program ASUS Aura DRAM remap slot"
                );
                break;
            }

            debug!(
                bus_path,
                slot_index,
                remap_address = format_args!("0x{remap_address:02X}"),
                "programmed ASUS Aura DRAM remap slot"
            );
            remapped_addresses.push(remap_address);
        }

        let _ = hub_transport.close().await;
    }

    let mut discovered = Vec::new();
    let probe_addresses = dram_probe_addresses(&occupied_addresses, &remapped_addresses);
    trace!(
        bus_path,
        probe_addresses = ?probe_addresses,
        "probing ASUS Aura DRAM address pool for ENE controllers"
    );
    for remapped_address in probe_addresses {
        if let Some(probe) =
            probe_bus_address(bus_path, remapped_address, SmBusControllerKind::Dram).await?
        {
            discovered.push(probe);
        }
    }

    debug!(
        bus_path,
        discovered_count = discovered.len(),
        hub_present,
        "completed ASUS Aura DRAM probe"
    );

    Ok(discovered)
}

#[cfg(target_os = "linux")]
async fn probe_bus_address(
    bus_path: &str,
    address: u16,
    controller_kind: SmBusControllerKind,
) -> Result<Option<SmBusProbe>> {
    trace!(
        bus_path,
        address = format_args!("0x{address:02X}"),
        controller_kind = controller_kind.display_name(),
        "probing ASUS Aura SMBus candidate"
    );

    let transport = match SmBusTransport::open(bus_path, address) {
        Ok(transport) => transport,
        Err(error) => {
            trace!(
                bus_path,
                address = format_args!("0x{address:02X}"),
                controller_kind = controller_kind.display_name(),
                error = %error,
                "failed to open ASUS Aura SMBus candidate"
            );
            return Ok(None);
        }
    };

    let probed = probe_with_transport(&transport, bus_path, address, controller_kind).await;
    let _ = transport.close().await;
    probed
}

#[cfg(target_os = "linux")]
#[allow(
    clippy::too_many_lines,
    reason = "SMBus probe walks sequential register reads with per-vendor branching"
)]
async fn probe_with_transport(
    transport: &SmBusTransport,
    bus_path: &str,
    address: u16,
    controller_kind: SmBusControllerKind,
) -> Result<Option<SmBusProbe>> {
    let protocol = AuraSmBusProtocol::new();
    let init = protocol.init_sequence();

    let Some(firmware_command) = init.first() else {
        return Ok(None);
    };
    let firmware_response = match transport
        .send_receive(&firmware_command.data, protocol.response_timeout())
        .await
    {
        Ok(response) => response,
        Err(error) => {
            trace!(
                bus_path,
                address = format_args!("0x{address:02X}"),
                controller_kind = controller_kind.display_name(),
                error = %error,
                "ASUS Aura SMBus firmware query failed"
            );
            return Ok(None);
        }
    };
    let firmware_status = match protocol.parse_response(&firmware_response) {
        Ok(response) => response.status,
        Err(error) => {
            trace!(
                bus_path,
                address = format_args!("0x{address:02X}"),
                controller_kind = controller_kind.display_name(),
                error = %error,
                "ASUS Aura SMBus firmware response parse failed"
            );
            return Ok(None);
        }
    };
    if firmware_status != ResponseStatus::Ok || protocol.firmware_variant().is_none() {
        trace!(
            bus_path,
            address = format_args!("0x{address:02X}"),
            controller_kind = controller_kind.display_name(),
            firmware_status = ?firmware_status,
            firmware_variant = ?protocol.firmware_variant(),
            "ASUS Aura SMBus firmware probe rejected candidate"
        );
        return Ok(None);
    }

    let Some(config_command) = init.get(1) else {
        return Ok(None);
    };
    let config_response = match transport
        .send_receive(&config_command.data, protocol.response_timeout())
        .await
    {
        Ok(response) => response,
        Err(error) => {
            trace!(
                bus_path,
                address = format_args!("0x{address:02X}"),
                controller_kind = controller_kind.display_name(),
                error = %error,
                "ASUS Aura SMBus config query failed"
            );
            return Ok(None);
        }
    };
    let config_status = match protocol.parse_response(&config_response) {
        Ok(response) => response.status,
        Err(error) => {
            trace!(
                bus_path,
                address = format_args!("0x{address:02X}"),
                controller_kind = controller_kind.display_name(),
                error = %error,
                "ASUS Aura SMBus config response parse failed"
            );
            return Ok(None);
        }
    };
    if config_status != ResponseStatus::Ok || protocol.total_leds() == 0 {
        trace!(
            bus_path,
            address = format_args!("0x{address:02X}"),
            controller_kind = controller_kind.display_name(),
            config_status = ?config_status,
            total_leds = protocol.total_leds(),
            "ASUS Aura SMBus config probe rejected candidate"
        );
        return Ok(None);
    }

    debug!(
        bus_path,
        address = format_args!("0x{address:02X}"),
        controller_kind = controller_kind.display_name(),
        firmware_name = protocol.firmware_name(),
        total_leds = protocol.total_leds(),
        "discovered ASUS Aura SMBus controller"
    );

    let identifier = DeviceIdentifier::SmBus {
        bus_path: bus_path.to_owned(),
        address,
    };
    let fingerprint = identifier.fingerprint();
    let firmware_name = protocol.firmware_name();
    let info = build_device_info(
        controller_kind,
        &protocol,
        firmware_name.clone(),
        address,
        fingerprint.stable_device_id(),
    );

    let mut metadata = HashMap::new();
    metadata.insert("backend_id".to_owned(), ASUS_SMBUS_BACKEND_ID.to_owned());
    metadata.insert("bus_path".to_owned(), bus_path.to_owned());
    metadata.insert("smbus_address".to_owned(), format!("0x{address:02X}"));
    metadata.insert(
        "controller_kind".to_owned(),
        controller_kind.display_name().to_ascii_lowercase(),
    );
    if let Some(firmware_name) = firmware_name {
        metadata.insert("firmware_name".to_owned(), firmware_name);
    }

    Ok(Some(SmBusProbe {
        fingerprint,
        info,
        metadata,
    }))
}

#[cfg(target_os = "linux")]
fn i2c_bus_paths_in(dev_root: &Path) -> Result<Vec<String>> {
    let mut paths = std::fs::read_dir(dev_root)?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let name = entry.file_name().into_string().ok()?;
            if !name.starts_with("i2c-") {
                return None;
            }
            Some(dev_root.join(name).display().to_string())
        })
        .collect::<Vec<_>>();
    paths.sort_unstable();
    Ok(paths)
}

#[cfg(target_os = "linux")]
fn bus_pci_id(bus_path: &str) -> Option<(u16, u16)> {
    let bus_name = Path::new(bus_path).file_name()?;
    let sysfs_root = Path::new("/sys/class/i2c-dev")
        .join(bus_name)
        .join("device");
    resolve_parent_pci_id_from_sysfs_path(&sysfs_root)
}

fn read_sysfs_hex_u16(path: &Path) -> Option<u16> {
    let raw = std::fs::read_to_string(path).ok()?;
    let trimmed = raw.trim();
    let trimmed = trimmed.strip_prefix("0x").unwrap_or(trimmed);
    u16::from_str_radix(trimmed, 16).ok()
}

#[doc(hidden)]
#[must_use]
pub fn dram_capable_pci_id(vendor_id: u16, device_id: u16) -> bool {
    SYSTEM_SMBUS_ADAPTER_IDS.contains(&(vendor_id, device_id))
}

#[cfg(target_os = "linux")]
fn dram_capable_bus_pci_id(pci_id: Option<(u16, u16)>) -> bool {
    let Some((vendor_id, device_id)) = pci_id else {
        return false;
    };

    dram_capable_pci_id(vendor_id, device_id)
}

#[cfg(target_os = "linux")]
fn motherboard_capable_bus_pci_id(pci_id: Option<(u16, u16)>) -> bool {
    let Some((vendor_id, device_id)) = pci_id else {
        return false;
    };

    SYSTEM_SMBUS_ADAPTER_IDS.contains(&(vendor_id, device_id))
}

#[cfg(target_os = "linux")]
fn gpu_capable_bus_pci_id(pci_id: Option<(u16, u16)>) -> bool {
    let Some((vendor_id, _)) = pci_id else {
        return false;
    };

    matches!(vendor_id, AMD_GPU_VENDOR_ID | NVIDIA_VENDOR_ID)
}

#[doc(hidden)]
#[must_use]
pub fn resolve_parent_pci_id_from_sysfs_path(path: &Path) -> Option<(u16, u16)> {
    let mut current = std::fs::canonicalize(path).ok()?;

    loop {
        let vendor_path = current.join("vendor");
        let device_path = current.join("device");
        if let (Some(vendor_id), Some(device_id)) = (
            read_sysfs_hex_u16(&vendor_path),
            read_sysfs_hex_u16(&device_path),
        ) {
            return Some((vendor_id, device_id));
        }

        let parent = current.parent()?;
        if parent == current {
            return None;
        }
        current = parent.to_path_buf();
    }
}

#[cfg(target_os = "linux")]
fn probe_occupied_dram_addresses(bus_path: &str) -> HashSet<u16> {
    ASUS_DRAM_SMBUS_ADDRESSES
        .iter()
        .copied()
        .filter(|&address| bus_address_quick_responds(bus_path, address))
        .collect()
}

#[cfg(target_os = "linux")]
fn bus_address_responds(bus_path: &str, address: u16) -> bool {
    SmBusTransport::probe_presence(bus_path, address).unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn bus_address_quick_responds(bus_path: &str, address: u16) -> bool {
    SmBusTransport::probe_quick_write(bus_path, address).unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn next_available_dram_address(
    start_index: usize,
    occupied_addresses: &HashSet<u16>,
) -> Option<(usize, u16)> {
    ASUS_DRAM_SMBUS_ADDRESSES
        .iter()
        .copied()
        .enumerate()
        .skip(start_index)
        .find(|(_, address)| !occupied_addresses.contains(address))
}

#[cfg(target_os = "linux")]
fn dram_probe_addresses(occupied_addresses: &HashSet<u16>, remapped_addresses: &[u16]) -> Vec<u16> {
    let mut probe_addresses = Vec::new();
    let mut seen = HashSet::new();

    for address in ASUS_DRAM_SMBUS_ADDRESSES
        .iter()
        .copied()
        .filter(|address| occupied_addresses.contains(address))
        .chain(remapped_addresses.iter().copied())
        .chain(ASUS_DRAM_SMBUS_ADDRESSES.iter().copied())
    {
        if seen.insert(address) {
            probe_addresses.push(address);
        }
    }

    probe_addresses
}

#[cfg(target_os = "linux")]
fn build_device_info(
    controller_kind: SmBusControllerKind,
    protocol: &AuraSmBusProtocol,
    firmware_name: Option<String>,
    address: u16,
    device_id: hypercolor_types::device::DeviceId,
) -> DeviceInfo {
    let zones = protocol
        .zones()
        .into_iter()
        .map(protocol_zone_to_zone_info)
        .collect::<Vec<_>>();

    DeviceInfo {
        id: device_id,
        name: format!(
            "ASUS Aura {} (SMBus 0x{address:02X})",
            controller_kind.display_name()
        ),
        vendor: "ASUS".to_owned(),
        family: DeviceFamily::Asus,
        model: Some(controller_kind.model_id().to_owned()),
        connection_type: ConnectionType::SmBus,
        origin: DeviceOrigin::native("asus", "smbus", ConnectionType::SmBus)
            .with_protocol_id("asus/aura-smbus"),
        zones,
        firmware_version: firmware_name,
        capabilities: protocol.capabilities(),
    }
}

#[cfg(target_os = "linux")]
fn protocol_zone_to_zone_info(zone: ProtocolZone) -> hypercolor_types::device::ZoneInfo {
    hypercolor_types::device::ZoneInfo {
        name: zone.name,
        led_count: zone.led_count,
        topology: zone.topology,
        color_format: zone.color_format,
    }
}
