//! Philips Hue backend primitives.

mod color;
mod streaming;
mod types;

pub use color::{CieXyb, ColorGamut, GAMUT_A, GAMUT_B, GAMUT_C, rgb_to_cie_xyb};
pub use streaming::encode_packet_into;
pub use types::{
    HueChannel, HueChannelMember, HueEntertainmentConfig, HueEntertainmentType, HueLight,
    HuePairResult, HuePosition,
};
