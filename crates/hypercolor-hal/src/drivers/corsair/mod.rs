//! Corsair protocol driver family.

use std::time::Duration;

pub mod devices;
pub mod framing;
pub mod lighting_node;
pub mod link;
pub mod types;

pub use devices::descriptors;
pub use lighting_node::devices::{
    PID_1000D_OBSIDIAN, PID_COMMANDER_PRO, PID_LIGHTING_NODE_CORE, PID_LIGHTING_NODE_PRO,
    PID_LS100_STARTER_KIT, PID_LT100_TOWER, PID_SPEC_OMEGA_RGB, build_1000d_obsidian_protocol,
    build_commander_pro_protocol, build_lighting_node_core_protocol,
    build_lighting_node_pro_protocol, build_ls100_starter_kit_protocol, build_lt100_tower_protocol,
    build_spec_omega_rgb_protocol,
};
pub use lighting_node::protocol::CorsairLightingNodeProtocol;
pub use link::devices::{PID_ICUE_LINK_SYSTEM_HUB, build_icue_link_system_hub_protocol};
pub use link::protocol::{CorsairLinkProtocol, LinkChild};
pub use types::{
    EP_GET_DEVICES, EP_SET_COLOR, EndpointConfig, LightingNodeColorChannel, LightingNodePacketId,
    LightingNodePortState, LinkCommand, LinkDeviceType,
};

/// Corsair USB vendor ID.
pub const CORSAIR_VID: u16 = 0x1B1C;

/// Corsair vendor-specific HID usage page.
pub const CORSAIR_USAGE_PAGE: u16 = 0xFF42;

/// Shared keepalive interval used by native Corsair RGB protocols.
pub const CORSAIR_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(5);
