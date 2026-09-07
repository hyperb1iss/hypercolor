//! Generic descriptor and transport types shared by driver registries.

use std::borrow::Cow;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use hypercolor_types::device::DeviceFamily;

use crate::protocol::Protocol;
use crate::transport::{Transport, TransportError};

/// Function pointer used to construct a protocol instance.
pub type ProtocolFactory = fn() -> Box<dyn Protocol>;

/// Future returned by a driver-owned USB transport factory.
pub type UsbTransportFuture =
    Pin<Box<dyn Future<Output = Result<Box<dyn Transport>, TransportError>> + Send + 'static>>;

/// Function pointer used to open a driver-owned USB transport.
pub type UsbTransportFactory = fn(UsbTransportOpenRequest) -> UsbTransportFuture;

/// Identity and native handle passed to a driver-owned USB transport factory.
pub struct UsbTransportOpenRequest {
    /// Open native USB device handle.
    pub device: nusb::Device,
    /// USB vendor identifier.
    pub vendor_id: u16,
    /// USB product identifier.
    pub product_id: u16,
    /// Optional stable device serial.
    pub serial: Option<String>,
    /// Optional host USB topology path.
    pub usb_path: Option<String>,
}

/// Inventory classification for a driver-owned USB transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsbTransportKind {
    /// Generic USB transport.
    Usb,
    /// MIDI transport attached to a USB-discovered device.
    Midi,
}

/// How core should execute a driver-owned transport open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportConnectExecution {
    /// Run on the async executor.
    Async,
    /// Run through the lifecycle background-connect lane.
    Background,
}

/// Neutral lifecycle requirements declared by a transport binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransportLifecycleHints {
    /// Optional connection timeout.
    pub connect_timeout: Option<Duration>,
    /// Executor lane required for connection.
    pub connect_execution: TransportConnectExecution,
    /// Whether a lifecycle timeout should be retried.
    pub retry_on_connect_timeout: bool,
}

impl Default for TransportLifecycleHints {
    fn default() -> Self {
        Self {
            connect_timeout: None,
            connect_execution: TransportConnectExecution::Async,
            retry_on_connect_timeout: true,
        }
    }
}

/// Driver-owned USB transport opener and lifecycle metadata.
#[derive(Clone, Copy)]
pub struct UsbTransportBinding {
    /// Stable binding identifier.
    pub id: &'static str,
    /// Inventory classification exposed by the driver database.
    pub kind: UsbTransportKind,
    /// Connection lifecycle requirements.
    pub lifecycle: TransportLifecycleHints,
    /// Driver-owned transport factory.
    pub open: UsbTransportFactory,
}

impl fmt::Debug for UsbTransportBinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UsbTransportBinding")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .field("lifecycle", &self.lifecycle)
            .finish_non_exhaustive()
    }
}

impl PartialEq for UsbTransportBinding {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.kind == other.kind && self.lifecycle == other.lifecycle
    }
}

impl Eq for UsbTransportBinding {}

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

    /// How to treat this device's reported USB serial, when it lies.
    pub serial_quirk: Option<SerialQuirk>,
}

/// Known defects in a device family's reported USB serial number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SerialQuirk {
    /// The firmware reports one of these fixed strings for every unit, so the
    /// value identifies the model rather than the device. Treating it as an
    /// identity collapses a chain of identical panels into one device.
    PlaceholderValues(&'static [&'static str]),
}

impl DeviceDescriptor {
    /// Whether `serial` is a factory placeholder rather than a real identity.
    ///
    /// Compared case-insensitively against the trimmed value, since these
    /// strings come back from firmware with incidental padding.
    #[must_use]
    pub fn is_placeholder_serial(&self, serial: &str) -> bool {
        let serial = serial.trim();
        match self.serial_quirk {
            Some(SerialQuirk::PlaceholderValues(values)) => values
                .iter()
                .any(|placeholder| placeholder.eq_ignore_ascii_case(serial)),
            None => false,
        }
    }

    /// Driver module that owns this protocol descriptor.
    #[must_use]
    pub fn driver_id(&self) -> Cow<'_, str> {
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
        /// Full HID report buffer length used for reads and feature-report
        /// requests, including the report ID byte when the OS API expects it.
        max_report_len: usize,
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

    /// Driver-owned transport for a USB-discovered device.
    DriverUsb {
        /// Driver factory and neutral lifecycle metadata.
        binding: UsbTransportBinding,
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
    /// Send and receive HID feature reports where protocol packets already
    /// include the HID report ID byte.
    FeatureReportWithReportId,
    /// Send output reports and receive input reports where protocol packets
    /// already include the HID report ID byte.
    OutputReportWithReportId,
}
