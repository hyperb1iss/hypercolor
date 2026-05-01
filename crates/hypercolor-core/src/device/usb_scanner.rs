//! USB scanner backed by the HAL protocol database.

use std::collections::{BTreeSet, HashMap};

use anyhow::{Context, Result};
use hypercolor_hal::database::{DeviceDescriptor, ProtocolDatabase};
use hypercolor_hal::protocol::{Protocol, ProtocolZone};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFeatures, DeviceIdentifier,
    DeviceInfo, DeviceOrigin, DeviceTopologyHint,
};

use super::{DiscoveredDevice, DiscoveryConnectBehavior, TransportScanner};

/// USB transport scanner that discovers HAL-backed devices by VID/PID.
pub struct UsbScanner {
    enabled_driver_ids: Option<BTreeSet<String>>,
}

impl UsbScanner {
    /// Create a USB scanner.
    #[must_use]
    pub fn new() -> Self {
        Self {
            enabled_driver_ids: None,
        }
    }

    /// Create a USB scanner limited to enabled HAL driver module IDs.
    #[must_use]
    pub fn with_enabled_driver_ids(enabled_driver_ids: BTreeSet<String>) -> Self {
        Self {
            enabled_driver_ids: Some(enabled_driver_ids),
        }
    }

    fn build_device_info(
        usb: &nusb::DeviceInfo,
        descriptor: &'static DeviceDescriptor,
        protocol: Option<&dyn Protocol>,
        device_id: hypercolor_types::device::DeviceId,
    ) -> DeviceInfo {
        let (zones, capabilities) = if let Some(protocol) = protocol {
            let zones = protocol
                .zones()
                .into_iter()
                .map(protocol_zone_to_zone_info)
                .collect::<Vec<_>>();
            (zones, protocol.capabilities())
        } else {
            let fallback_led_count = 1_u32;
            (
                vec![hypercolor_types::device::ZoneInfo {
                    name: "Lighting".to_owned(),
                    led_count: fallback_led_count,
                    topology: DeviceTopologyHint::Point,
                    color_format: DeviceColorFormat::Rgb,
                    layout_hint: None,
                }],
                DeviceCapabilities {
                    led_count: fallback_led_count,
                    supports_direct: true,
                    supports_brightness: true,
                    has_display: false,
                    display_resolution: None,
                    max_fps: 60,
                    color_space: hypercolor_types::device::DeviceColorSpace::default(),
                    features: DeviceFeatures::default(),
                },
            )
        };

        let vendor = usb.manufacturer_string().map_or_else(
            || descriptor.family.vendor_name().to_owned(),
            ToOwned::to_owned,
        );

        DeviceInfo {
            id: device_id,
            name: descriptor.name.to_owned(),
            vendor,
            family: descriptor.family.clone(),
            model: descriptor_model_id(descriptor),
            connection_type: ConnectionType::Usb,
            origin: DeviceOrigin::native(descriptor.driver_id(), "usb", ConnectionType::Usb)
                .with_protocol_id(descriptor.protocol.id),
            zones,
            firmware_version: Some(hex_version(usb.device_version())),
            capabilities,
        }
    }
}

impl Default for UsbScanner {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl TransportScanner for UsbScanner {
    fn name(&self) -> &'static str {
        "USB HAL"
    }

    async fn scan(&mut self) -> Result<Vec<DiscoveredDevice>> {
        let devices = nusb::list_devices()
            .await
            .context("failed to enumerate USB devices")?;

        let mut discovered = Vec::new();
        for usb in devices {
            let vendor_id = usb.vendor_id();
            let product_id = usb.product_id();
            let firmware_hint = usb.product_string();

            let Some(descriptor) = ProtocolDatabase::lookup_with_firmware_for_driver_ids(
                vendor_id,
                product_id,
                firmware_hint,
                self.enabled_driver_ids.as_ref(),
            ) else {
                continue;
            };

            let protocol = (descriptor.protocol.build)();
            let path = usb_path(&usb);
            let identifier = DeviceIdentifier::UsbHid {
                vendor_id,
                product_id,
                serial: usb.serial_number().map(ToOwned::to_owned),
                usb_path: (!path.is_empty()).then_some(path.clone()),
            };
            let fingerprint = identifier.fingerprint();
            let info = Self::build_device_info(
                &usb,
                descriptor,
                Some(protocol.as_ref()),
                fingerprint.stable_device_id(),
            );

            let mut metadata = HashMap::new();
            metadata.insert("vendor_id".to_owned(), format!("0x{vendor_id:04X}"));
            metadata.insert("product_id".to_owned(), format!("0x{product_id:04X}"));
            if let Some(serial) = usb.serial_number() {
                metadata.insert("serial".to_owned(), serial.to_owned());
            }
            if let Some(product_string) = usb.product_string() {
                metadata.insert("product_string".to_owned(), product_string.to_owned());
            }
            if !path.is_empty() {
                metadata.insert("usb_path".to_owned(), path);
            }

            discovered.push(DiscoveredDevice {
                origin: info.origin.clone(),
                name: descriptor.name.to_owned(),
                family: descriptor.family.clone(),
                fingerprint,
                connect_behavior: DiscoveryConnectBehavior::AutoConnect,
                info,
                metadata,
            });
        }

        Ok(discovered)
    }
}

fn protocol_zone_to_zone_info(zone: ProtocolZone) -> hypercolor_types::device::ZoneInfo {
    hypercolor_types::device::ZoneInfo {
        name: zone.name,
        led_count: zone.led_count,
        topology: zone.topology,
        color_format: zone.color_format,
        layout_hint: zone.layout_hint,
    }
}

fn hex_version(version: u16) -> String {
    format!("{version:#06X}")
}

fn descriptor_model_id(descriptor: &DeviceDescriptor) -> Option<String> {
    let (_, raw_model) = descriptor.protocol.id.split_once('/')?;
    Some(raw_model.replace('-', "_"))
}

fn usb_path(usb: &nusb::DeviceInfo) -> String {
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    {
        let ports = usb
            .port_chain()
            .iter()
            .map(u8::to_string)
            .collect::<Vec<_>>()
            .join(".");

        if ports.is_empty() {
            usb.bus_id().to_owned()
        } else {
            format!("{}-{ports}", usb.bus_id())
        }
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = usb;
        String::new()
    }
}
