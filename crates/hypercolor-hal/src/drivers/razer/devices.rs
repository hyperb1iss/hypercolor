//! Self-contained Razer device registry entries.

use hypercolor_types::device::DeviceFamily;

use crate::protocol::Protocol;
use crate::registry::{DeviceDescriptor, ProtocolBinding, TransportType};

use super::protocol::RazerProtocol;
use super::types::{LED_ID_BACKLIGHT, LED_ID_ZERO, RazerMatrixType, RazerProtocolVersion};

/// Razer vendor ID.
pub const RAZER_VENDOR_ID: u16 = 0x1532;

/// Razer Huntsman V2 (full-size).
pub const PID_HUNTSMAN_V2: u16 = 0x026C;

/// Razer Basilisk V3.
pub const PID_BASILISK_V3: u16 = 0x0099;

/// Razer Seiren Emote.
pub const PID_SEIREN_EMOTE: u16 = 0x0F1B;

/// Build a Huntsman V2 protocol instance.
pub fn build_huntsman_v2_protocol() -> Box<dyn Protocol> {
    Box::new(RazerProtocol::new(
        RazerProtocolVersion::Extended,
        RazerMatrixType::Extended,
        (6, 22),
        LED_ID_BACKLIGHT,
    ))
}

/// Build a Basilisk V3 protocol instance.
pub fn build_basilisk_v3_protocol() -> Box<dyn Protocol> {
    Box::new(RazerProtocol::new(
        RazerProtocolVersion::Modern,
        RazerMatrixType::Extended,
        (1, 11),
        LED_ID_ZERO,
    ))
}

/// Build a Seiren Emote protocol instance.
pub fn build_seiren_emote_protocol() -> Box<dyn Protocol> {
    Box::new(RazerProtocol::new(
        RazerProtocolVersion::Extended,
        RazerMatrixType::Extended,
        (8, 8),
        LED_ID_ZERO,
    ))
}

macro_rules! razer_descriptor {
    (
        pid: $pid:expr,
        name: $name:expr,
        protocol_id: $protocol_id:expr,
        builder: $builder:path
    ) => {
        DeviceDescriptor {
            vendor_id: RAZER_VENDOR_ID,
            product_id: $pid,
            name: $name,
            family: DeviceFamily::Razer,
            transport: TransportType::UsbControl {
                interface: 3,
                report_id: 0x00,
            },
            protocol: ProtocolBinding {
                id: $protocol_id,
                build: $builder,
            },
            firmware_predicate: None,
        }
    };
}

static RAZER_DESCRIPTORS: &[DeviceDescriptor] = &[
    razer_descriptor!(
        pid: PID_HUNTSMAN_V2,
        name: "Razer Huntsman V2",
        protocol_id: "razer/huntsman-v2",
        builder: build_huntsman_v2_protocol
    ),
    razer_descriptor!(
        pid: PID_BASILISK_V3,
        name: "Razer Basilisk V3",
        protocol_id: "razer/basilisk-v3",
        builder: build_basilisk_v3_protocol
    ),
    razer_descriptor!(
        pid: PID_SEIREN_EMOTE,
        name: "Razer Seiren Emote",
        protocol_id: "razer/seiren-emote",
        builder: build_seiren_emote_protocol
    ),
];

/// Static Razer descriptors for HAL registration.
#[must_use]
pub fn descriptors() -> &'static [DeviceDescriptor] {
    RAZER_DESCRIPTORS
}
