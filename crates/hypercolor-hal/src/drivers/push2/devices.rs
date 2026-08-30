//! Ableton Push 2 descriptor registration.

use std::sync::LazyLock;
use std::time::Duration;

use hypercolor_types::device::DeviceFamily;

use crate::protocol::Protocol;
use crate::registry::{
    DeviceDescriptor, ProtocolBinding, TransportConnectExecution, TransportLifecycleHints,
    TransportType, UsbTransportBinding, UsbTransportKind,
};

use super::protocol::Push2Protocol;
use super::transport::open_push2_transport;

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
        transport: TransportType::DriverUsb {
            binding: UsbTransportBinding {
                id: "push2/native-midi-display",
                kind: UsbTransportKind::Midi,
                lifecycle: TransportLifecycleHints {
                    connect_timeout: Some(Duration::from_secs(30)),
                    connect_execution: TransportConnectExecution::Background,
                    retry_on_connect_timeout: false,
                },
                open: open_push2_transport,
            },
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
