//! ASUS Aura device registry entries.

use hypercolor_types::device::DeviceFamily;

use crate::protocol::Protocol;
use crate::registry::{DeviceDescriptor, HidRawReportMode, ProtocolBinding, TransportType};

use super::protocol::AuraUsbProtocol;
use super::types::{ASUS_VID, AURA_REPORT_ID, AuraControllerGen};

/// ASUS Aura addressable-only controller, generation 1.
pub const PID_AURA_ADDRESSABLE_GEN1: u16 = 0x1867;
/// ASUS Aura addressable-only controller, generation 2.
pub const PID_AURA_ADDRESSABLE_GEN2: u16 = 0x1872;
/// ASUS Aura addressable-only controller, generation 3.
pub const PID_AURA_ADDRESSABLE_GEN3: u16 = 0x18A3;
/// ASUS Aura addressable-only controller, generation 4.
pub const PID_AURA_ADDRESSABLE_GEN4: u16 = 0x18A5;

/// ASUS Aura motherboard controller, generation 1.
pub const PID_AURA_MOTHERBOARD_GEN1: u16 = 0x18F3;
/// ASUS Aura motherboard controller, generation 2.
pub const PID_AURA_MOTHERBOARD_GEN2: u16 = 0x1939;
/// ASUS Aura motherboard controller, generation 3.
pub const PID_AURA_MOTHERBOARD_GEN3: u16 = 0x19AF;
/// ASUS Aura motherboard controller, generation 4.
pub const PID_AURA_MOTHERBOARD_GEN4: u16 = 0x1AA6;
/// ASUS Aura motherboard controller, generation 5.
pub const PID_AURA_MOTHERBOARD_GEN5: u16 = 0x1BED;

/// ASUS Aura Terminal standalone ARGB controller.
pub const PID_AURA_TERMINAL: u16 = 0x1889;

/// Build an ASUS Aura addressable-only generation 1 protocol instance.
pub fn build_aura_addressable_gen1_protocol() -> Box<dyn Protocol> {
    Box::new(AuraUsbProtocol::new(AuraControllerGen::AddressableOnly).with_gen1_disable(true))
}

/// Build an ASUS Aura addressable-only generation 2 protocol instance.
pub fn build_aura_addressable_gen2_protocol() -> Box<dyn Protocol> {
    Box::new(AuraUsbProtocol::new(AuraControllerGen::AddressableOnly).with_gen1_disable(true))
}

/// Build an ASUS Aura addressable-only generation 3 protocol instance.
pub fn build_aura_addressable_gen3_protocol() -> Box<dyn Protocol> {
    Box::new(AuraUsbProtocol::new(AuraControllerGen::AddressableOnly).with_gen1_disable(false))
}

/// Build an ASUS Aura addressable-only generation 4 protocol instance.
pub fn build_aura_addressable_gen4_protocol() -> Box<dyn Protocol> {
    Box::new(AuraUsbProtocol::new(AuraControllerGen::AddressableOnly).with_gen1_disable(false))
}

/// Build an ASUS Aura motherboard generation 1 protocol instance.
pub fn build_aura_motherboard_gen1_protocol() -> Box<dyn Protocol> {
    Box::new(AuraUsbProtocol::new(AuraControllerGen::Motherboard))
}

/// Build an ASUS Aura motherboard generation 2 protocol instance.
pub fn build_aura_motherboard_gen2_protocol() -> Box<dyn Protocol> {
    Box::new(AuraUsbProtocol::new(AuraControllerGen::Motherboard))
}

/// Build an ASUS Aura motherboard generation 3 protocol instance.
pub fn build_aura_motherboard_gen3_protocol() -> Box<dyn Protocol> {
    Box::new(AuraUsbProtocol::new(AuraControllerGen::Motherboard))
}

/// Build an ASUS Aura motherboard generation 4 protocol instance.
pub fn build_aura_motherboard_gen4_protocol() -> Box<dyn Protocol> {
    Box::new(AuraUsbProtocol::new(AuraControllerGen::Motherboard))
}

/// Build an ASUS Aura motherboard generation 5 protocol instance.
pub fn build_aura_motherboard_gen5_protocol() -> Box<dyn Protocol> {
    Box::new(AuraUsbProtocol::new(AuraControllerGen::Motherboard))
}

/// Build an ASUS Aura Terminal protocol instance.
pub fn build_aura_terminal_protocol() -> Box<dyn Protocol> {
    Box::new(AuraUsbProtocol::new(AuraControllerGen::Terminal))
}

macro_rules! asus_descriptor {
    (
        pid: $pid:expr,
        name: $name:expr,
        protocol_id: $protocol_id:expr,
        builder: $builder:path
    ) => {
        DeviceDescriptor {
            vendor_id: ASUS_VID,
            product_id: $pid,
            name: $name,
            family: DeviceFamily::Asus,
            transport: TransportType::UsbHidRaw {
                interface: 2,
                report_id: AURA_REPORT_ID,
                report_mode: HidRawReportMode::OutputReport,
                usage_page: None,
                usage: None,
            },
            protocol: ProtocolBinding {
                id: $protocol_id,
                build: $builder,
            },
            firmware_predicate: None,
        }
    };
}

static ASUS_DESCRIPTORS: &[DeviceDescriptor] = &[
    asus_descriptor!(
        pid: PID_AURA_MOTHERBOARD_GEN1,
        name: "ASUS Aura Motherboard (Gen 1)",
        protocol_id: "asus/motherboard-gen1",
        builder: build_aura_motherboard_gen1_protocol
    ),
    asus_descriptor!(
        pid: PID_AURA_MOTHERBOARD_GEN2,
        name: "ASUS Aura Motherboard (Gen 2)",
        protocol_id: "asus/motherboard-gen2",
        builder: build_aura_motherboard_gen2_protocol
    ),
    asus_descriptor!(
        pid: PID_AURA_MOTHERBOARD_GEN3,
        name: "ASUS Aura Motherboard (Gen 3)",
        protocol_id: "asus/motherboard-gen3",
        builder: build_aura_motherboard_gen3_protocol
    ),
    asus_descriptor!(
        pid: PID_AURA_MOTHERBOARD_GEN4,
        name: "ASUS Aura Motherboard (Gen 4)",
        protocol_id: "asus/motherboard-gen4",
        builder: build_aura_motherboard_gen4_protocol
    ),
    asus_descriptor!(
        pid: PID_AURA_MOTHERBOARD_GEN5,
        name: "ASUS Aura Motherboard (Gen 5)",
        protocol_id: "asus/motherboard-gen5",
        builder: build_aura_motherboard_gen5_protocol
    ),
    asus_descriptor!(
        pid: PID_AURA_ADDRESSABLE_GEN1,
        name: "ASUS Aura Addressable (Gen 1)",
        protocol_id: "asus/addressable-gen1",
        builder: build_aura_addressable_gen1_protocol
    ),
    asus_descriptor!(
        pid: PID_AURA_ADDRESSABLE_GEN2,
        name: "ASUS Aura Addressable (Gen 2)",
        protocol_id: "asus/addressable-gen2",
        builder: build_aura_addressable_gen2_protocol
    ),
    asus_descriptor!(
        pid: PID_AURA_ADDRESSABLE_GEN3,
        name: "ASUS Aura Addressable (Gen 3)",
        protocol_id: "asus/addressable-gen3",
        builder: build_aura_addressable_gen3_protocol
    ),
    asus_descriptor!(
        pid: PID_AURA_ADDRESSABLE_GEN4,
        name: "ASUS Aura Addressable (Gen 4)",
        protocol_id: "asus/addressable-gen4",
        builder: build_aura_addressable_gen4_protocol
    ),
    asus_descriptor!(
        pid: PID_AURA_TERMINAL,
        name: "ASUS Aura Terminal",
        protocol_id: "asus/terminal",
        builder: build_aura_terminal_protocol
    ),
];

/// Static ASUS Aura descriptors.
#[must_use]
pub fn descriptors() -> &'static [DeviceDescriptor] {
    ASUS_DESCRIPTORS
}
