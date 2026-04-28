//! Descriptor registration for the Corsair iCUE LINK System Hub.

use hypercolor_types::device::DeviceFamily;

use crate::protocol::Protocol;
use crate::registry::{DeviceDescriptor, ProtocolBinding, TransportType};

use super::protocol::CorsairLinkProtocol;
use crate::drivers::corsair::CORSAIR_VID;

/// Corsair iCUE LINK System Hub PID.
pub const PID_ICUE_LINK_SYSTEM_HUB: u16 = 0x0C3F;

/// Build a LINK hub protocol instance.
pub fn build_icue_link_system_hub_protocol() -> Box<dyn Protocol> {
    Box::new(CorsairLinkProtocol::new())
}

static LINK_DESCRIPTORS: &[DeviceDescriptor] = &[DeviceDescriptor {
    vendor_id: CORSAIR_VID,
    product_id: PID_ICUE_LINK_SYSTEM_HUB,
    name: "Corsair iCUE LINK System Hub",
    family: DeviceFamily::new_static("corsair", "Corsair"),
    transport: TransportType::UsbHid { interface: 0 },
    protocol: ProtocolBinding {
        id: "corsair/icue-link-system-hub",
        build: build_icue_link_system_hub_protocol,
    },
    firmware_predicate: None,
}];

/// Static LINK descriptors for HAL registration.
#[must_use]
pub fn descriptors() -> &'static [DeviceDescriptor] {
    LINK_DESCRIPTORS
}
