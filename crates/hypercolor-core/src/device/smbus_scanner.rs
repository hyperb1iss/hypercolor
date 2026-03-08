//! SMBus scanner for ASUS Aura ENE controllers.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use hypercolor_hal::drivers::asus::AuraSmBusProtocol;
use hypercolor_hal::protocol::{Protocol, ProtocolZone, ResponseStatus};
use hypercolor_hal::transport::Transport;
use hypercolor_hal::transport::smbus::SmBusTransport;
use hypercolor_types::device::{
    ConnectionType, DeviceFamily, DeviceFingerprint, DeviceIdentifier, DeviceInfo,
};

use super::discovery::{DiscoveredDevice, TransportScanner};

const ASUS_SMBUS_BACKEND_ID: &str = "smbus";

const ASUS_MOTHERBOARD_SMBUS_ADDRESSES: &[(u16, SmBusControllerKind)] = &[
    (0x40, SmBusControllerKind::Motherboard),
    (0x4E, SmBusControllerKind::Motherboard),
    (0x4F, SmBusControllerKind::Motherboard),
];

const ASUS_GPU_SMBUS_ADDRESSES: &[(u16, SmBusControllerKind)] = &[
    (0x29, SmBusControllerKind::Gpu),
    (0x2A, SmBusControllerKind::Gpu),
    (0x67, SmBusControllerKind::Gpu),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SmBusControllerKind {
    Motherboard,
    Gpu,
}

impl SmBusControllerKind {
    pub(crate) const fn display_name(self) -> &'static str {
        match self {
            Self::Motherboard => "Motherboard",
            Self::Gpu => "GPU",
        }
    }

    pub(crate) const fn model_id(self) -> &'static str {
        match self {
            Self::Motherboard => "asus_aura_smbus_motherboard",
            Self::Gpu => "asus_aura_smbus_gpu",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct SmBusProbe {
    pub(crate) fingerprint: DeviceFingerprint,
    pub(crate) info: DeviceInfo,
    pub(crate) metadata: HashMap<String, String>,
}

/// SMBus transport scanner for ASUS ENE controllers.
pub struct SmBusScanner {
    dev_root: PathBuf,
}

impl SmBusScanner {
    /// Create an SMBus scanner.
    #[must_use]
    pub fn new() -> Self {
        Self::with_dev_root("/dev")
    }

    /// Create an SMBus scanner with a custom device-node root.
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
                name: probe.info.name.clone(),
                family: probe.info.family.clone(),
                fingerprint: probe.fingerprint,
                info: probe.info,
                metadata: probe.metadata,
            })
            .collect())
    }
}

pub(crate) async fn probe_asus_smbus_devices_in_root(dev_root: &Path) -> Result<Vec<SmBusProbe>> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = dev_root;
        Ok(Vec::new())
    }

    #[cfg(target_os = "linux")]
    {
        let mut discovered = Vec::new();

        for bus_path in i2c_bus_paths_in(dev_root)? {
            for &(address, controller_kind) in ASUS_MOTHERBOARD_SMBUS_ADDRESSES
                .iter()
                .chain(ASUS_GPU_SMBUS_ADDRESSES.iter())
            {
                if let Some(probe) = probe_bus_address(&bus_path, address, controller_kind).await? {
                    discovered.push(probe);
                }
            }
        }

        Ok(discovered)
    }
}

#[cfg(target_os = "linux")]
async fn probe_bus_address(
    bus_path: &str,
    address: u16,
    controller_kind: SmBusControllerKind,
) -> Result<Option<SmBusProbe>> {
    let transport = match SmBusTransport::open(bus_path, address) {
        Ok(transport) => transport,
        Err(_) => return Ok(None),
    };

    let probed = probe_with_transport(&transport, bus_path, address, controller_kind).await;
    let _ = transport.close().await;
    probed
}

#[cfg(target_os = "linux")]
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
        Err(_) => return Ok(None),
    };
    let firmware_status = match protocol.parse_response(&firmware_response) {
        Ok(response) => response.status,
        Err(_) => return Ok(None),
    };
    if firmware_status != ResponseStatus::Ok || protocol.firmware_variant().is_none() {
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
        Err(_) => return Ok(None),
    };
    let config_status = match protocol.parse_response(&config_response) {
        Ok(response) => response.status,
        Err(_) => return Ok(None),
    };
    if config_status != ResponseStatus::Ok || protocol.total_leds() == 0 {
        return Ok(None);
    }

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
        zones,
        firmware_version: firmware_name,
        capabilities: protocol.capabilities(),
    }
}

fn protocol_zone_to_zone_info(zone: ProtocolZone) -> hypercolor_types::device::ZoneInfo {
    hypercolor_types::device::ZoneInfo {
        name: zone.name,
        led_count: zone.led_count,
        topology: zone.topology,
        color_format: zone.color_format,
    }
}
