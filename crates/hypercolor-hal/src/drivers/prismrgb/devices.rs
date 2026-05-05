//! Self-contained `PrismRGB` device registry entries.

use hypercolor_types::device::DeviceFamily;

use crate::protocol::Protocol;
#[cfg(windows)]
use crate::registry::HidRawReportMode;
use crate::registry::{DeviceDescriptor, ProtocolBinding, TransportType};

#[cfg(windows)]
use super::protocol::HID_REPORT_SIZE;
use super::protocol::{PrismRgbModel, PrismRgbProtocol};

/// Shared vendor ID used by Prism S and Prism Mini.
pub const PRISM_GCS_VENDOR_ID: u16 = 0x16D0;

/// `PrismRGB` Prism S PID.
pub const PID_PRISM_S: u16 = 0x1294;

/// `PrismRGB` Prism Mini PID.
pub const PID_PRISM_MINI: u16 = 0x1407;

/// Build a Prism S protocol instance.
pub fn build_prism_s_protocol() -> Box<dyn Protocol> {
    Box::new(PrismRgbProtocol::new(PrismRgbModel::PrismS))
}

/// Build a Prism Mini protocol instance.
pub fn build_prism_mini_protocol() -> Box<dyn Protocol> {
    Box::new(PrismRgbProtocol::new(PrismRgbModel::PrismMini))
}

#[cfg(windows)]
const fn prismrgb_hid_transport(interface: u8) -> TransportType {
    TransportType::UsbHidApi {
        interface: Some(interface),
        report_id: 0x00,
        report_mode: HidRawReportMode::OutputReportWithReportId,
        max_report_len: HID_REPORT_SIZE,
        usage_page: None,
        usage: None,
    }
}

#[cfg(not(windows))]
const fn prismrgb_hid_transport(interface: u8) -> TransportType {
    TransportType::UsbHid { interface }
}

macro_rules! prismrgb_descriptor {
    (
        vid: $vid:expr,
        pid: $pid:expr,
        name: $name:expr,
        protocol_id: $protocol_id:expr,
        interface: $interface:expr,
        builder: $builder:path
    ) => {
        DeviceDescriptor {
            vendor_id: $vid,
            product_id: $pid,
            name: $name,
            family: DeviceFamily::new_static("prismrgb", "PrismRGB"),
            transport: prismrgb_hid_transport($interface),
            protocol: ProtocolBinding {
                id: $protocol_id,
                build: $builder,
            },
            firmware_predicate: None,
        }
    };
}

static PRISMRGB_DESCRIPTORS: &[DeviceDescriptor] = &[
    prismrgb_descriptor!(
        vid: PRISM_GCS_VENDOR_ID,
        pid: PID_PRISM_S,
        name: "PrismRGB Prism S",
        protocol_id: "prismrgb/prism-s",
        interface: 2,
        builder: build_prism_s_protocol
    ),
    prismrgb_descriptor!(
        vid: PRISM_GCS_VENDOR_ID,
        pid: PID_PRISM_MINI,
        name: "PrismRGB Prism Mini",
        protocol_id: "prismrgb/prism-mini",
        interface: 2,
        builder: build_prism_mini_protocol
    ),
];

/// Static `PrismRGB` descriptors for HAL registration.
#[must_use]
pub fn descriptors() -> &'static [DeviceDescriptor] {
    PRISMRGB_DESCRIPTORS
}
