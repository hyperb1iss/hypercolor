//! Descriptor registration for the Corsair iCUE LINK System Hub.

use std::sync::LazyLock;

use hypercolor_types::device::DeviceFamily;

use crate::drivers::corsair::CORSAIR_VID;
use crate::drivers::corsair::framing::LINK_WRITE_BUF_SIZE;
use crate::protocol::Protocol;
use crate::registry::HidRawReportMode;
use crate::registry::{DeviceDescriptor, ProtocolBinding};
use crate::transport::{
    HidAccessMode, HidTransportIntent, TransportIntent, resolve_current_transport,
};

use super::protocol::CorsairLinkProtocol;

/// Corsair iCUE LINK System Hub PID.
pub const PID_ICUE_LINK_SYSTEM_HUB: u16 = 0x0C3F;

/// Build a LINK hub protocol instance.
pub fn build_icue_link_system_hub_protocol() -> Box<dyn Protocol> {
    Box::new(CorsairLinkProtocol::new())
}

const CORSAIR_LINK_TRANSPORT_INTENT: TransportIntent = TransportIntent::Hid(HidTransportIntent {
    access: HidAccessMode::Direct,
    interface: 0,
    report_id: 0x00,
    report_mode: HidRawReportMode::OutputReportWithReportId,
    max_report_len: LINK_WRITE_BUF_SIZE,
    usage_page: None,
    usage: None,
});

static LINK_DESCRIPTORS: LazyLock<Vec<DeviceDescriptor>> = LazyLock::new(|| {
    vec![DeviceDescriptor {
        vendor_id: CORSAIR_VID,
        product_id: PID_ICUE_LINK_SYSTEM_HUB,
        name: "Corsair iCUE LINK System Hub",
        family: DeviceFamily::new_static("corsair", "Corsair"),
        transport: resolve_current_transport(CORSAIR_LINK_TRANSPORT_INTENT)
            .expect("Corsair LINK HID transport should support the current platform"),
        protocol: ProtocolBinding {
            id: "corsair/icue-link-system-hub",
            build: build_icue_link_system_hub_protocol,
        },
        firmware_predicate: None,
    }]
});

/// Static LINK descriptors for HAL registration.
#[must_use]
pub fn descriptors() -> &'static [DeviceDescriptor] {
    LINK_DESCRIPTORS.as_slice()
}
