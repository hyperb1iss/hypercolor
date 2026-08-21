//! USB scanner backed by the HAL protocol database.

use std::collections::{BTreeSet, HashMap};

use anyhow::{Context, Result};
use hypercolor_hal::database::{DeviceDescriptor, ProtocolDatabase};
use hypercolor_hal::protocol::{Protocol, ProtocolZone};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFeatures, DeviceIdentifier,
    DeviceInfo, DeviceOrigin, DeviceTopologyHint, USB_OUTPUT_BACKEND_ID,
};
use hypercolor_types::portable::{PortableIdentityClaim, SerialNormalizerRegistry};

use super::{DiscoveredDevice, DiscoveryConnectBehavior};

/// The serial normalizations reviewed for cross-OS stability.
///
/// Empty is the correct starting state, not a stub: registering a
/// `(vendor, product)` pair asserts that its serial reporting has been
/// checked byte-for-byte across the OS stacks we support, and no pair has
/// that evidence yet. Until one does, USB devices re-bind per machine,
/// which is the designed fallback rather than a failure.
#[must_use]
pub fn reviewed_serial_normalizers() -> SerialNormalizerRegistry {
    SerialNormalizerRegistry::new()
}

/// USB transport scanner that discovers HAL-backed devices by VID/PID.
pub struct UsbScanner {
    enabled_driver_ids: Option<BTreeSet<String>>,
    serial_normalizers: SerialNormalizerRegistry,
}

impl UsbScanner {
    /// Create a USB scanner.
    #[must_use]
    pub fn new() -> Self {
        Self {
            enabled_driver_ids: None,
            serial_normalizers: reviewed_serial_normalizers(),
        }
    }

    /// Create a USB scanner limited to enabled HAL driver module IDs.
    #[must_use]
    pub fn with_enabled_driver_ids(enabled_driver_ids: BTreeSet<String>) -> Self {
        Self {
            enabled_driver_ids: Some(enabled_driver_ids),
            serial_normalizers: reviewed_serial_normalizers(),
        }
    }

    fn build_device_info(
        usb: &nusb::DeviceInfo,
        descriptor: &'static DeviceDescriptor,
        protocol: Option<&dyn Protocol>,
        device_id: hypercolor_types::device::DeviceId,
    ) -> DeviceInfo {
        let (segments, capabilities) = if let Some(protocol) = protocol {
            let segments = protocol
                .zones()
                .into_iter()
                .map(protocol_zone_to_segment_info)
                .collect::<Vec<_>>();
            (segments, protocol.capabilities())
        } else {
            let fallback_led_count = 1_u32;
            (
                vec![hypercolor_types::device::SegmentInfo {
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
            origin: DeviceOrigin::native(
                descriptor.driver_id(),
                USB_OUTPUT_BACKEND_ID,
                ConnectionType::Usb,
            )
            .with_protocol_id(descriptor.protocol.id),
            segments,
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

impl UsbScanner {
    /// Discover USB devices supported by the enabled HAL driver set.
    pub async fn scan(&mut self) -> Result<Vec<DiscoveredDevice>> {
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
            let fingerprint = identifier.fingerprint(&descriptor.driver_id());
            let info = Self::build_device_info(
                &usb,
                descriptor,
                Some(protocol.as_ref()),
                fingerprint.stable_device_id(),
            );

            // Claimed here, where serial-vs-path is still a known fact; the
            // fingerprint string discards it. Refusal (no registered
            // normalizer, placeholder serial, no serial at all) yields None
            // and the device re-binds per machine.
            let claim = usb.serial_number().and_then(|serial| {
                PortableIdentityClaim::usb_serial(
                    vendor_id,
                    product_id,
                    serial,
                    path.clone(),
                    &self.serial_normalizers,
                )
            });

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
                fingerprint,
                connect_behavior: DiscoveryConnectBehavior::AutoConnect,
                info,
                metadata,
                claim,
            });
        }

        Ok(discovered)
    }
}

fn protocol_zone_to_segment_info(zone: ProtocolZone) -> hypercolor_types::device::SegmentInfo {
    hypercolor_types::device::SegmentInfo {
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
