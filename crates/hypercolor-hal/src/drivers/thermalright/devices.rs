//! Descriptor registration for Thermalright LCD panels.

use std::sync::LazyLock;

use hypercolor_types::device::{DeviceFamily, DriverStability};

use crate::protocol::Protocol;
use crate::registry::{DeviceDescriptor, HidRawReportMode, ProtocolBinding};
use crate::transport::{
    HidAccessMode, HidTransportIntent, TransportIntent, resolve_current_transport,
};

use super::hid_lcd::{
    HID_LCD_PACKET_LEN, HID_LCD_REPORT_ID, HidLcdProtocol, THERMALRIGHT_HID_LCD_PID,
    THERMALRIGHT_VENDOR_ID,
};

/// The HID interface; interface 1 is a vendor function nothing drives.
const HID_LCD_INTERFACE: u8 = 0;

/// Interface 0 through the host HID stack, one unnumbered output report per
/// 512 bytes. The report ID byte rides in every packet.
const HID_LCD_TRANSPORT_INTENT: TransportIntent = TransportIntent::Hid(HidTransportIntent {
    access: HidAccessMode::HostManaged,
    interface: HID_LCD_INTERFACE,
    report_id: HID_LCD_REPORT_ID,
    report_mode: HidRawReportMode::OutputReportWithReportId,
    max_report_len: HID_LCD_PACKET_LEN,
    usage_page: None,
    usage: None,
});

/// Build a Thermalright HID LCD protocol instance.
#[must_use]
pub fn build_hid_lcd_protocol() -> Box<dyn Protocol> {
    Box::new(HidLcdProtocol::new())
}

static THERMALRIGHT_DESCRIPTORS: LazyLock<Vec<DeviceDescriptor>> = LazyLock::new(|| {
    vec![DeviceDescriptor {
        vendor_id: THERMALRIGHT_VENDOR_ID,
        product_id: THERMALRIGHT_HID_LCD_PID,
        name: "Thermalright LCD",
        family: DeviceFamily::new_static("thermalright", "Thermalright"),
        transport: resolve_current_transport(HID_LCD_TRANSPORT_INTENT)
            .expect("Thermalright HID transport should support the current platform"),
        protocol: ProtocolBinding {
            id: "thermalright/hid-lcd",
            build: build_hid_lcd_protocol,
        },
        firmware_predicate: None,
        release_predicate: None,
        serial_quirk: None,
        stability: DriverStability::Experimental,
    }]
});

/// Static Thermalright descriptors for HAL registration.
#[must_use]
pub fn descriptors() -> &'static [DeviceDescriptor] {
    THERMALRIGHT_DESCRIPTORS.as_slice()
}
