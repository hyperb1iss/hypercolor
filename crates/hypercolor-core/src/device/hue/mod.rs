//! Philips Hue backend primitives.

pub mod backend;
mod bridge;
mod color;
mod scanner;
mod streaming;
mod types;

pub use backend::HueBackend;
pub use bridge::{DEFAULT_HUE_API_PORT, DEFAULT_HUE_STREAM_PORT, HueBridgeClient, HueNupnpBridge};
pub use color::{CieXyb, ColorGamut, GAMUT_A, GAMUT_B, GAMUT_C, rgb_to_cie_xyb};
pub use scanner::{HueKnownBridge, HueScanner};
pub use streaming::{HueStreamSession, encode_packet_into};
pub use types::{
    HueBridgeIdentity, HueChannel, HueChannelMember, HueDiscoveredBridge, HueEntertainmentConfig,
    HueEntertainmentType, HueLight, HuePairResult, HuePosition, build_device_info,
    choose_entertainment_config,
};
