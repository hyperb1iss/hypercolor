//! Generic descriptor and transport types shared by driver registries.

use std::borrow::Cow;
use std::fmt;

use hypercolor_types::device::DeviceFamily;

use crate::protocol::Protocol;

/// Function pointer used to construct a protocol instance.
pub type ProtocolFactory = fn() -> Box<dyn Protocol>;

/// Generic protocol binding attached to a descriptor.
#[derive(Clone, Copy)]
pub struct ProtocolBinding {
    /// Stable protocol identifier.
    pub id: &'static str,

    /// Protocol constructor.
    pub build: ProtocolFactory,
}

impl fmt::Debug for ProtocolBinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProtocolBinding")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

/// Static metadata for a known HAL-managed device.
#[derive(Debug, Clone)]
pub struct DeviceDescriptor {
    /// USB vendor ID (`VID`).
    pub vendor_id: u16,

    /// USB product ID (`PID`).
    pub product_id: u16,

    /// Human-readable device name.
    pub name: &'static str,

    /// Device family classification.
    pub family: DeviceFamily,

    /// Transport type required by this device.
    pub transport: TransportType,

    /// Generic protocol binding.
    pub protocol: ProtocolBinding,

    /// Optional firmware-based disambiguation predicate.
    pub firmware_predicate: Option<fn(&str) -> bool>,
}

impl DeviceDescriptor {
    /// Driver module that owns this protocol descriptor.
    #[must_use]
    pub fn driver_id(&self) -> Cow<'_, str> {
        if matches!(self.family, DeviceFamily::Custom(_)) {
            return self.family.id();
        }

        self.protocol.id.split_once('/').map_or_else(
            || self.family.id(),
            |(driver_id, _)| Cow::Borrowed(driver_id),
        )
    }

    /// Human-readable driver module name.
    #[must_use]
    pub fn driver_display_name(&self) -> Cow<'_, str> {
        let driver_id = self.driver_id();
        if driver_id.as_ref() == self.family.id().as_ref() {
            Cow::Borrowed(self.family.vendor_name())
        } else {
            Cow::Owned(titleize_driver_id(driver_id.as_ref()))
        }
    }
}

fn titleize_driver_id(driver_id: &str) -> String {
    driver_id
        .split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            chars.next().map_or_else(String::new, |first| {
                first
                    .to_uppercase()
                    .chain(chars.flat_map(char::to_lowercase))
                    .collect()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Transport mechanism for a descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportType {
    /// HID feature reports over USB control transfers.
    UsbControl {
        /// Interface number to claim.
        interface: u8,
        /// HID report ID.
        report_id: u8,
    },

    /// HID feature/output reports over `hidapi`.
    ///
    /// This keeps the OS HID stack attached and avoids claiming the USB
    /// interface directly, which is important for live input devices.
    UsbHidApi {
        /// Optional HID interface number. `None` matches any interface that
        /// satisfies the remaining identity and usage filters.
        interface: Option<u8>,
        /// HID report ID.
        report_id: u8,
        /// Whether HID I/O should use feature reports or output/input reports.
        report_mode: HidRawReportMode,
        /// Optional HID usage page filter for devices that expose multiple
        /// collections on the same interface.
        usage_page: Option<u16>,
        /// Optional HID usage filter for devices that expose multiple
        /// collections on the same interface.
        usage: Option<u16>,
    },

    /// HID feature/output reports over Linux `/dev/hidraw*` nodes.
    ///
    /// This keeps `usbhid` attached and avoids claiming the USB interface.
    UsbHidRaw {
        /// HID interface number.
        interface: u8,
        /// HID report ID.
        report_id: u8,
        /// Whether hidraw I/O should use HID feature ioctls or raw report
        /// read/write semantics.
        report_mode: HidRawReportMode,
        /// Optional HID usage page filter for devices that expose multiple
        /// collections on the same interface.
        usage_page: Option<u16>,
        /// Optional HID usage filter for devices that expose multiple
        /// collections on the same interface.
        usage: Option<u16>,
    },

    /// HID interrupt endpoint transport.
    UsbHid {
        /// Interface number to claim.
        interface: u8,
    },

    /// USB bulk-transfer transport with HID feature-report sideband control.
    UsbBulk {
        /// Interface number to claim.
        interface: u8,
        /// HID report ID used for feature-report init/keepalive commands.
        report_id: u8,
    },

    /// Composite transport with USB MIDI for control and USB bulk for display.
    UsbMidi {
        /// MIDI interface number associated with the device's user port.
        midi_interface: u8,
        /// Bulk display interface number to claim.
        display_interface: u8,
        /// Bulk OUT endpoint used for display frames.
        display_endpoint: u8,
    },

    /// USB CDC-ACM serial transport.
    UsbSerial {
        /// Serial port baud rate hint.
        baud_rate: u32,
    },

    /// Linux I2C/`SMBus` transport.
    I2cSmBus {
        /// 7-bit `SMBus` slave address.
        address: u16,
    },

    /// Vendor-specific control transfer transport.
    UsbVendor,
}

/// HID report path used by HIDAPI and Linux hidraw transports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HidRawReportMode {
    /// Send and receive HID feature reports via native feature-report APIs.
    FeatureReport,
    /// Send output reports and receive input reports through the platform HID
    /// stack.
    OutputReport,
}
