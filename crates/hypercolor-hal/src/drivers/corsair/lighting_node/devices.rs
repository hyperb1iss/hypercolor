//! Descriptor registration for Corsair Lighting Node devices.

use hypercolor_types::device::DeviceFamily;

use crate::drivers::corsair::CORSAIR_VID;
use crate::protocol::Protocol;
use crate::registry::{DeviceDescriptor, ProtocolBinding, TransportType};

use super::protocol::CorsairLightingNodeProtocol;

/// Lighting Node Core PID.
pub const PID_LIGHTING_NODE_CORE: u16 = 0x0C1A;
/// Lighting Node Pro PID.
pub const PID_LIGHTING_NODE_PRO: u16 = 0x0C0B;
/// Commander Pro PID.
pub const PID_COMMANDER_PRO: u16 = 0x0C10;
/// LS100 Starter Kit PID.
pub const PID_LS100_STARTER_KIT: u16 = 0x0C1E;
/// LT100 Tower PID.
pub const PID_LT100_TOWER: u16 = 0x0C23;
/// 1000D Obsidian PID.
pub const PID_1000D_OBSIDIAN: u16 = 0x1D00;
/// SPEC OMEGA RGB PID.
pub const PID_SPEC_OMEGA_RGB: u16 = 0x1D04;

fn build_protocol(name: &'static str, channel_count: u8) -> Box<dyn Protocol> {
    Box::new(CorsairLightingNodeProtocol::new(name, channel_count))
}

/// Build a Lighting Node Core protocol instance.
pub fn build_lighting_node_core_protocol() -> Box<dyn Protocol> {
    build_protocol("Corsair Lighting Node Core", 1)
}

/// Build a Lighting Node Pro protocol instance.
pub fn build_lighting_node_pro_protocol() -> Box<dyn Protocol> {
    build_protocol("Corsair Lighting Node Pro", 2)
}

/// Build a Commander Pro protocol instance.
pub fn build_commander_pro_protocol() -> Box<dyn Protocol> {
    build_protocol("Corsair Commander Pro", 2)
}

/// Build an LS100 Starter Kit protocol instance.
pub fn build_ls100_starter_kit_protocol() -> Box<dyn Protocol> {
    build_protocol("Corsair LS100 Starter Kit", 1)
}

/// Build an LT100 Tower protocol instance.
pub fn build_lt100_tower_protocol() -> Box<dyn Protocol> {
    build_protocol("Corsair LT100 Tower", 2)
}

/// Build a 1000D Obsidian protocol instance.
pub fn build_1000d_obsidian_protocol() -> Box<dyn Protocol> {
    build_protocol("Corsair 1000D Obsidian", 2)
}

/// Build a SPEC OMEGA RGB protocol instance.
pub fn build_spec_omega_rgb_protocol() -> Box<dyn Protocol> {
    build_protocol("Corsair SPEC OMEGA RGB", 2)
}

macro_rules! lighting_node_descriptor {
    (
        pid: $pid:expr,
        name: $name:expr,
        protocol_id: $protocol_id:expr,
        channels: $channels:expr,
        builder: $builder:path
    ) => {
        DeviceDescriptor {
            vendor_id: CORSAIR_VID,
            product_id: $pid,
            name: $name,
            family: DeviceFamily::Corsair,
            transport: TransportType::UsbHid { interface: 0 },
            protocol: ProtocolBinding {
                id: $protocol_id,
                build: $builder,
            },
            firmware_predicate: None,
        }
    };
}

static LIGHTING_NODE_DESCRIPTORS: &[DeviceDescriptor] = &[
    lighting_node_descriptor!(
        pid: PID_LIGHTING_NODE_CORE,
        name: "Corsair Lighting Node Core",
        protocol_id: "corsair/lighting-node-core",
        channels: 1,
        builder: build_lighting_node_core_protocol
    ),
    lighting_node_descriptor!(
        pid: PID_LIGHTING_NODE_PRO,
        name: "Corsair Lighting Node Pro",
        protocol_id: "corsair/lighting-node-pro",
        channels: 2,
        builder: build_lighting_node_pro_protocol
    ),
    lighting_node_descriptor!(
        pid: PID_COMMANDER_PRO,
        name: "Corsair Commander Pro",
        protocol_id: "corsair/commander-pro",
        channels: 2,
        builder: build_commander_pro_protocol
    ),
    lighting_node_descriptor!(
        pid: PID_LS100_STARTER_KIT,
        name: "Corsair LS100 Starter Kit",
        protocol_id: "corsair/ls100-starter-kit",
        channels: 1,
        builder: build_ls100_starter_kit_protocol
    ),
    lighting_node_descriptor!(
        pid: PID_LT100_TOWER,
        name: "Corsair LT100 Tower",
        protocol_id: "corsair/lt100-tower",
        channels: 2,
        builder: build_lt100_tower_protocol
    ),
    lighting_node_descriptor!(
        pid: PID_1000D_OBSIDIAN,
        name: "Corsair 1000D Obsidian",
        protocol_id: "corsair/1000d-obsidian",
        channels: 2,
        builder: build_1000d_obsidian_protocol
    ),
    lighting_node_descriptor!(
        pid: PID_SPEC_OMEGA_RGB,
        name: "Corsair SPEC OMEGA RGB",
        protocol_id: "corsair/spec-omega-rgb",
        channels: 2,
        builder: build_spec_omega_rgb_protocol
    ),
];

/// Static Lighting Node descriptors for HAL registration.
#[must_use]
pub fn descriptors() -> &'static [DeviceDescriptor] {
    LIGHTING_NODE_DESCRIPTORS
}
