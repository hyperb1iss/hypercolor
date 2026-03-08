//! Generic descriptor and transport types shared by driver registries.

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

/// Static metadata for a known USB device.
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

/// USB transport mechanism for a descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportType {
    /// HID feature reports over USB control transfers.
    UsbControl {
        /// Interface number to claim.
        interface: u8,
        /// HID report ID.
        report_id: u8,
    },

    /// HID feature reports over Linux `/dev/hidraw*` nodes.
    ///
    /// This keeps `usbhid` attached and avoids claiming the USB interface.
    UsbHidRaw {
        /// HID interface number.
        interface: u8,
        /// HID report ID.
        report_id: u8,
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

    /// USB CDC-ACM serial transport.
    UsbSerial {
        /// Serial port baud rate hint.
        baud_rate: u32,
    },

    /// Vendor-specific control transfer transport.
    UsbVendor,
}
