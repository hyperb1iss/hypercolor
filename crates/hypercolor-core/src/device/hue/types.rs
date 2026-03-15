//! Hue bridge and entertainment data types.

use serde::{Deserialize, Serialize};

/// Result of a successful Hue bridge pairing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HuePairResult {
    pub api_key: String,
    pub client_key: String,
}

/// Entertainment configuration from CLIP v2.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HueEntertainmentConfig {
    pub id: String,
    pub name: String,
    pub config_type: HueEntertainmentType,
    #[serde(default)]
    pub channels: Vec<HueChannel>,
}

/// One entertainment channel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HueChannel {
    pub id: u8,
    pub name: String,
    pub position: HuePosition,
    pub segment_count: u32,
    #[serde(default)]
    pub members: Vec<HueChannelMember>,
}

/// Hue entertainment channel member.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HueChannelMember {
    pub id: String,
    #[serde(default)]
    pub light_id: Option<String>,
}

/// Channel spatial position in Hue's normalized coordinate space.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct HuePosition {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

/// Entertainment configuration category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HueEntertainmentType {
    Screen,
    Monitor,
    Music,
    ThreeDSpace,
    Other,
}

/// Minimal Hue light metadata used for gamut lookup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HueLight {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub model_id: Option<String>,
}
