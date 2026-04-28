//! Ableton Push 2 descriptor registration.

use std::sync::LazyLock;

use hypercolor_types::device::DeviceFamily;

use crate::protocol::Protocol;
use crate::registry::{DeviceDescriptor, ProtocolBinding, TransportType};

use super::protocol::Push2Protocol;

/// Ableton AG USB vendor ID.
pub const ABLETON_VENDOR_ID: u16 = 0x2982;
/// Ableton Push 2 USB product ID.
pub const PID_PUSH_2: u16 = 0x1967;
/// Push 2 MIDI user-port interface number.
pub const PUSH2_MIDI_INTERFACE: u8 = 2;
/// Push 2 display bulk interface number.
pub const PUSH2_DISPLAY_INTERFACE: u8 = 0;
/// Push 2 display bulk OUT endpoint.
pub const PUSH2_DISPLAY_ENDPOINT: u8 = 0x01;

/// Build a Push 2 protocol instance.
pub fn build_push2_protocol() -> Box<dyn Protocol> {
    Box::new(Push2Protocol::new())
}

static PUSH2_DESCRIPTORS: LazyLock<Vec<DeviceDescriptor>> = LazyLock::new(|| {
    vec![DeviceDescriptor {
        vendor_id: ABLETON_VENDOR_ID,
        product_id: PID_PUSH_2,
        name: "Ableton Push 2",
        family: DeviceFamily::named("Ableton"),
        transport: TransportType::UsbMidi {
            midi_interface: PUSH2_MIDI_INTERFACE,
            display_interface: PUSH2_DISPLAY_INTERFACE,
            display_endpoint: PUSH2_DISPLAY_ENDPOINT,
        },
        protocol: ProtocolBinding {
            id: "push2/push-2",
            build: build_push2_protocol,
        },
        firmware_predicate: None,
    }]
});

/// Static Push 2 descriptors for HAL registration.
#[must_use]
pub fn descriptors() -> &'static [DeviceDescriptor] {
    PUSH2_DESCRIPTORS.as_slice()
}
