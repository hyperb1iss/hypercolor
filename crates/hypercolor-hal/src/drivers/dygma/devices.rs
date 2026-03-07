//! Self-contained Dygma device registry entries.

use hypercolor_types::device::DeviceFamily;

use crate::protocol::Protocol;
use crate::registry::{DeviceDescriptor, ProtocolBinding, TransportType};

use super::protocol::{DygmaProtocol, DygmaVariant};

/// Dygma USB vendor ID.
pub const DYGMA_VENDOR_ID: u16 = 0x35EF;

/// Dygma Defy wired product ID.
pub const PID_DEFY_WIRED: u16 = 0x0010;

/// Dygma Defy wireless product ID.
pub const PID_DEFY_WIRELESS: u16 = 0x0012;

/// Build a Dygma Defy wired protocol instance.
pub fn build_defy_wired_protocol() -> Box<dyn Protocol> {
    Box::new(DygmaProtocol::new(DygmaVariant::DefyWired))
}

/// Build a Dygma Defy wireless protocol instance.
pub fn build_defy_wireless_protocol() -> Box<dyn Protocol> {
    Box::new(DygmaProtocol::new(DygmaVariant::DefyWireless))
}

macro_rules! dygma_descriptor {
    (
        pid: $pid:expr,
        name: $name:expr,
        protocol_id: $protocol_id:expr,
        builder: $builder:path
    ) => {
        DeviceDescriptor {
            vendor_id: DYGMA_VENDOR_ID,
            product_id: $pid,
            name: $name,
            family: DeviceFamily::Dygma,
            transport: TransportType::UsbSerial { baud_rate: 115_200 },
            protocol: ProtocolBinding {
                id: $protocol_id,
                build: $builder,
            },
            firmware_predicate: None,
        }
    };
}

static DYGMA_DESCRIPTORS: &[DeviceDescriptor] = &[
    dygma_descriptor!(
        pid: PID_DEFY_WIRED,
        name: "Dygma Defy",
        protocol_id: "dygma/defy-wired",
        builder: build_defy_wired_protocol
    ),
    dygma_descriptor!(
        pid: PID_DEFY_WIRELESS,
        name: "Dygma Defy Wireless",
        protocol_id: "dygma/defy-wireless",
        builder: build_defy_wireless_protocol
    ),
];

/// Static Dygma descriptors for HAL registration.
#[must_use]
pub fn descriptors() -> &'static [DeviceDescriptor] {
    DYGMA_DESCRIPTORS
}
