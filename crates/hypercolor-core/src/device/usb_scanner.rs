//! USB scanner backed by the HAL protocol database.

use std::collections::{BTreeSet, HashMap};

use anyhow::{Context, Result};
use hypercolor_driver_api::{DiscoveredDevice, DiscoveryConnectBehavior};
use hypercolor_hal::database::{DeviceDescriptor, ProtocolDatabase};
use hypercolor_hal::protocol::Protocol;
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFeatures, DeviceIdentifier,
    DeviceInfo, DeviceOrigin, DeviceTopologyHint, USB_OUTPUT_BACKEND_ID,
};
#[cfg(doc)]
use hypercolor_types::portable::ReviewedSerial;
use hypercolor_types::portable::{PortableIdentityClaim, SerialNormalizerRegistry};

/// The serial normalizations reviewed for cross-OS stability.
///
/// Empty is the correct starting state, not a stub: registering a
/// `(vendor, product)` pair asserts that its serial reporting has been
/// checked byte-for-byte across the OS stacks we support, and no pair has
/// that evidence yet. Until one does, USB devices re-bind per machine,
/// which is the designed fallback rather than a failure.
///
/// A review that earns an entry answers three things, and a
/// [`ReviewedSerial`] records all three so a later reader can re-check
/// them:
///
/// 1. Does the pair report an `iSerialNumber` string descriptor at all?
///    Plenty do not, and those devices key on USB topology instead. The
///    PrismRGB Prism S (`16D0:1294`) is the documented in-tree example.
/// 2. Is the value per unit rather than per model? A constant is well
///    formed and passes every generic placeholder check, so nothing but a
///    review catches it. The wired Lian Li Uni Fan TL LCD panel
///    (`04FC:7393`) ships `TL_LCDV0.1` on every panel, which is exactly
///    the trap: registering it with a plain normalization would merge a
///    whole fan stack into one account-wide device. Such constants belong
///    in the entry's `refused` list.
/// 3. Do the OS stacks agree on case? They agree on the bytes, because
///    all three read the same descriptor, but the reviewer has to say so
///    rather than assume it. Padding is handled for every pair already:
///    canonicalization trims ASCII whitespace and NUL.
///
/// The `receipt` on each entry names where that evidence lives. An
/// assertion nobody can re-check is indistinguishable from a guess, and
/// this registry exists precisely because guessing here silently merges
/// two people's hardware into one identity.
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
            let segments = protocol.zones();
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
            // A placeholder serial is a model string every unit reports, so it
            // must not reach identity: the fingerprint prefers serial over
            // path, and a chain of panels would collapse into one device.
            let identity_serial = identity_serial(descriptor, usb.serial_number());
            let identifier = DeviceIdentifier::UsbHid {
                vendor_id,
                product_id,
                serial: identity_serial.map(ToOwned::to_owned),
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
            // fingerprint string discards it.
            let claim = portable_claim(
                descriptor,
                vendor_id,
                product_id,
                usb.serial_number(),
                &path,
                &self.serial_normalizers,
            );

            let mut metadata = HashMap::new();
            metadata.insert("vendor_id".to_owned(), format!("0x{vendor_id:04X}"));
            metadata.insert("product_id".to_owned(), format!("0x{product_id:04X}"));
            match (usb.serial_number(), identity_serial) {
                // Kept out of the "serial" key so nothing downstream treats a
                // model string as an identity: device labels fall back to the
                // USB path, which is what actually tells two panels apart.
                (Some(observed), None) => {
                    metadata.insert("placeholder_serial".to_owned(), observed.to_owned());
                }
                (_, Some(serial)) => {
                    metadata.insert("serial".to_owned(), serial.to_owned());
                }
                (None, None) => {}
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

/// The serial a device may be identified by, or `None` when its descriptor
/// says the reported value is a factory placeholder.
///
/// Placeholder serials are worse than no serial: `DeviceIdentifier` prefers
/// serial over USB path, so a family that reports one fixed string would map
/// every unit onto a single fingerprint.
fn identity_serial<'a>(
    descriptor: &DeviceDescriptor,
    observed: Option<&'a str>,
) -> Option<&'a str> {
    observed.filter(|serial| !descriptor.is_placeholder_serial(serial))
}

/// The portable identity claim for one observed device, or `None` when it
/// has no claimable serial.
///
/// Refusal has three causes and they are all normal: the descriptor says the
/// serial is a placeholder, the `(vendor, product)` pair has no reviewed
/// normalizer, or the device reports no serial at all. A refused device
/// re-binds per machine rather than claiming an identity it cannot back.
fn portable_claim(
    descriptor: &DeviceDescriptor,
    vendor_id: u16,
    product_id: u16,
    observed_serial: Option<&str>,
    usb_path: &str,
    normalizers: &SerialNormalizerRegistry,
) -> Option<PortableIdentityClaim> {
    identity_serial(descriptor, observed_serial).and_then(|serial| {
        PortableIdentityClaim::usb_serial(
            vendor_id,
            product_id,
            serial,
            usb_path.to_owned(),
            normalizers,
        )
    })
}

#[cfg(test)]
mod tests {
    use hypercolor_hal::registry::SerialQuirk;
    use hypercolor_types::device::DeviceIdentifier;

    use super::*;

    const PLACEHOLDER: &str = "TL_LCDV0.1";

    fn descriptor_with_quirk(quirk: Option<SerialQuirk>) -> DeviceDescriptor {
        let mut descriptor = ProtocolDatabase::all()
            .first()
            .cloned()
            .expect("the device database should not be empty");
        descriptor.serial_quirk = quirk;
        descriptor
    }

    fn fingerprint_for(
        descriptor: &DeviceDescriptor,
        serial: &str,
        path: &str,
    ) -> hypercolor_types::device::DeviceId {
        let identifier = DeviceIdentifier::UsbHid {
            vendor_id: 0x04FC,
            product_id: 0x7393,
            serial: identity_serial(descriptor, Some(serial)).map(ToOwned::to_owned),
            usb_path: Some(path.to_owned()),
        };
        identifier
            .fingerprint(&descriptor.driver_id())
            .stable_device_id()
    }

    #[test]
    fn a_placeholder_serial_keys_identity_on_the_usb_path() {
        let descriptor =
            descriptor_with_quirk(Some(SerialQuirk::PlaceholderValues(&[PLACEHOLDER])));

        let first = fingerprint_for(&descriptor, PLACEHOLDER, "1-1.2");
        let second = fingerprint_for(&descriptor, PLACEHOLDER, "1-1.3");

        assert_ne!(
            first, second,
            "two panels on different ports must be two devices"
        );
    }

    /// The regression this quirk exists to prevent.
    #[test]
    fn without_the_quirk_a_shared_serial_collapses_two_panels_into_one() {
        let descriptor = descriptor_with_quirk(None);

        let first = fingerprint_for(&descriptor, PLACEHOLDER, "1-1.2");
        let second = fingerprint_for(&descriptor, PLACEHOLDER, "1-1.3");

        assert_eq!(
            first, second,
            "serial wins over path, so a shared serial is one fingerprint"
        );
    }

    /// A placeholder must not become a portable identity even when its pair
    /// has a reviewed normalizer that would happily accept the string. The
    /// registry cannot know the serial is a model name; the descriptor does.
    #[test]
    fn a_placeholder_serial_is_withheld_from_the_portable_claim() {
        let mut normalizers = SerialNormalizerRegistry::new();
        normalizers.register(
            0x04FC,
            0x7393,
            hypercolor_types::portable::ReviewedSerial::new(
                hypercolor_types::portable::SerialNormalization::TrimmedAscii,
                "test fixture: a review that missed the placeholder",
            ),
        );

        let quirked = descriptor_with_quirk(Some(SerialQuirk::PlaceholderValues(&[PLACEHOLDER])));
        assert!(
            portable_claim(
                &quirked,
                0x04FC,
                0x7393,
                Some(PLACEHOLDER),
                "1-1.2",
                &normalizers
            )
            .is_none(),
            "a model string must never be claimed as a portable identity"
        );

        let unquirked = descriptor_with_quirk(None);
        assert!(
            portable_claim(
                &unquirked,
                0x04FC,
                0x7393,
                Some(PLACEHOLDER),
                "1-1.2",
                &normalizers
            )
            .is_some(),
            "without the quirk the registry claims it, which is the bug"
        );

        assert!(
            portable_claim(
                &quirked,
                0x04FC,
                0x7393,
                Some("A1B2C3D4"),
                "1-1.2",
                &normalizers
            )
            .is_some(),
            "a real serial still claims an identity"
        );
    }

    #[test]
    fn a_real_serial_still_identifies_the_device() {
        let descriptor =
            descriptor_with_quirk(Some(SerialQuirk::PlaceholderValues(&[PLACEHOLDER])));

        assert_eq!(
            identity_serial(&descriptor, Some("A1B2C3D4")),
            Some("A1B2C3D4"),
            "the quirk only suppresses the placeholder values it names"
        );
        assert_eq!(identity_serial(&descriptor, None), None);
    }

    #[test]
    fn placeholder_matching_ignores_case_and_padding() {
        let descriptor =
            descriptor_with_quirk(Some(SerialQuirk::PlaceholderValues(&[PLACEHOLDER])));

        assert!(descriptor.is_placeholder_serial("  TL_LCDV0.1 "));
        assert!(descriptor.is_placeholder_serial("tl_lcdv0.1"));
        assert!(!descriptor.is_placeholder_serial("TL_LCDV0.2"));
    }

    #[test]
    fn the_reviewed_registry_ships_empty_until_a_pair_carries_evidence() {
        // This is a gate, not a tautology. Adding an entry means editing
        // this test too, which is the point: the diff that registers a
        // pair has to show the receipt beside it, and a reviewer who
        // cannot name one has nothing to write here.
        let registry = reviewed_serial_normalizers();
        let reviewed: Vec<_> = registry.reviewed().collect();

        assert!(
            reviewed.is_empty(),
            "registered pairs without receipts recorded here: {reviewed:?}"
        );
        assert_eq!(
            registry.normalize(0x1532, 0x0226, "PM2332H12345678"),
            None,
            "an unregistered pair refuses even a well-formed serial"
        );
    }
}
