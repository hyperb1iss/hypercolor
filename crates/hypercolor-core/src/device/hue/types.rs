//! Hue bridge and entertainment data types.

use std::collections::HashMap;
use std::net::IpAddr;

use serde::{Deserialize, Serialize};

use crate::device::discovery::{DiscoveredDevice, DiscoveryConnectBehavior};
use crate::types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceColorSpace, DeviceFamily,
    DeviceFeatures, DeviceFingerprint, DeviceInfo, DeviceOrigin, DeviceTopologyHint, ZoneInfo,
};

use super::color::{ColorGamut, GAMUT_A, GAMUT_B, GAMUT_C};

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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HueLight {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub model_id: Option<String>,
    #[serde(default)]
    pub gamut_type: Option<String>,
    #[serde(default)]
    pub gamut: Option<ColorGamut>,
}

impl HueLight {
    /// Resolve the best-known gamut for this light.
    #[must_use]
    pub fn resolved_gamut(&self) -> ColorGamut {
        self.gamut.unwrap_or_else(|| {
            self.gamut_type
                .as_deref()
                .and_then(gamut_from_type)
                .unwrap_or(GAMUT_C)
        })
    }
}

/// Minimal bridge identity returned by the Hue bridge config endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HueBridgeIdentity {
    pub bridge_id: String,
    pub name: String,
    pub model_id: String,
    pub sw_version: String,
}

/// Rich Hue discovery result shared between the scanner and backend.
#[derive(Debug, Clone)]
pub struct HueDiscoveredBridge {
    pub bridge_id: String,
    pub ip: IpAddr,
    pub api_port: u16,
    pub info: DeviceInfo,
    pub entertainment_config: Option<HueEntertainmentConfig>,
    pub lights: Vec<HueLight>,
    pub connect_behavior: DiscoveryConnectBehavior,
    pub metadata: HashMap<String, String>,
}

impl HueDiscoveredBridge {
    /// Convert into the generic discovery representation.
    #[must_use]
    pub fn into_discovered(self) -> DiscoveredDevice {
        let fingerprint = DeviceFingerprint(format!("hue:{}", self.bridge_id));

        DiscoveredDevice {
            connection_type: ConnectionType::Network,
            origin: self.info.origin.clone(),
            name: self.info.name.clone(),
            family: DeviceFamily::Hue,
            fingerprint,
            connect_behavior: self.connect_behavior,
            info: self.info,
            metadata: self.metadata,
        }
    }
}

/// Choose the preferred entertainment configuration by ID or name.
#[must_use]
pub fn choose_entertainment_config(
    preferred: Option<&str>,
    configs: &[HueEntertainmentConfig],
) -> Option<HueEntertainmentConfig> {
    let preferred = preferred.map(str::trim).filter(|value| !value.is_empty());
    if let Some(preferred) = preferred {
        let preferred_lower = preferred.to_ascii_lowercase();
        if let Some(config) = configs.iter().find(|config| {
            config.id.eq_ignore_ascii_case(preferred)
                || config.name.to_ascii_lowercase() == preferred_lower
        }) {
            return Some(config.clone());
        }
    }

    let mut sorted = configs.to_vec();
    sorted.sort_by(|left, right| left.name.cmp(&right.name).then(left.id.cmp(&right.id)));
    sorted.into_iter().next()
}

/// Build `DeviceInfo` from bridge metadata and an optional entertainment config.
#[must_use]
pub fn build_device_info(
    bridge_id: &str,
    bridge_name: &str,
    model_id: Option<&str>,
    sw_version: Option<&str>,
    entertainment_config: Option<&HueEntertainmentConfig>,
    lights: &[HueLight],
) -> DeviceInfo {
    let fingerprint = DeviceFingerprint(format!("hue:{bridge_id}"));
    let device_id = fingerprint.stable_device_id();
    let lights_by_id: HashMap<&str, &HueLight> = lights
        .iter()
        .map(|light| (light.id.as_str(), light))
        .collect();
    let zones: Vec<ZoneInfo> = entertainment_config
        .map(|config| {
            config
                .channels
                .iter()
                .map(|channel| {
                    let led_count = channel.segment_count.max(1);
                    ZoneInfo {
                        name: resolved_channel_name(channel, &lights_by_id),
                        led_count,
                        topology: if led_count == 1 {
                            DeviceTopologyHint::Point
                        } else {
                            DeviceTopologyHint::Strip
                        },
                        color_format: DeviceColorFormat::Rgb,
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    let total_led_count = zones.iter().map(|zone| zone.led_count).sum();

    DeviceInfo {
        id: device_id,
        name: resolved_device_name(bridge_id, bridge_name, entertainment_config),
        vendor: "Philips Hue".to_owned(),
        family: DeviceFamily::Hue,
        model: model_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
        connection_type: ConnectionType::Network,
        origin: DeviceOrigin::native("hue", "hue", ConnectionType::Network),
        zones,
        firmware_version: sw_version
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
        capabilities: DeviceCapabilities {
            led_count: total_led_count,
            supports_direct: true,
            supports_brightness: true,
            has_display: false,
            display_resolution: None,
            max_fps: 25,
            color_space: DeviceColorSpace::CieXy,
            features: DeviceFeatures::default(),
        },
    }
}

fn resolved_device_name(
    bridge_id: &str,
    bridge_name: &str,
    entertainment_config: Option<&HueEntertainmentConfig>,
) -> String {
    let config_name = entertainment_config
        .map(|config| config.name.trim())
        .filter(|value| !value.is_empty());
    if let Some(config_name) = config_name {
        return config_name.to_owned();
    }

    let bridge_name = bridge_name.trim();
    if bridge_name.is_empty() {
        format!("Hue Bridge {bridge_id}")
    } else {
        bridge_name.to_owned()
    }
}

fn resolved_channel_name(channel: &HueChannel, lights_by_id: &HashMap<&str, &HueLight>) -> String {
    let channel_name = channel.name.trim();
    if !channel_name.is_empty() && !is_generic_channel_name(channel, channel_name) {
        return channel_name.to_owned();
    }

    let mut member_names = Vec::new();
    for member in &channel.members {
        let Some(light_id) = member.light_id.as_deref() else {
            continue;
        };
        let Some(name) = lights_by_id
            .get(light_id)
            .map(|light| light.name.trim())
            .filter(|name| !name.is_empty())
        else {
            continue;
        };
        if member_names.iter().any(|existing| existing == name) {
            continue;
        }
        member_names.push(name.to_owned());
    }

    match member_names.len() {
        0 => channel_name.to_owned(),
        1 => member_names[0].clone(),
        2 => format!("{} + {}", member_names[0], member_names[1]),
        _ => format!("{} +{}", member_names[0], member_names.len() - 1),
    }
}

fn is_generic_channel_name(channel: &HueChannel, channel_name: &str) -> bool {
    channel_name.eq_ignore_ascii_case(&format!("Channel {}", channel.id))
}

fn gamut_from_type(raw: &str) -> Option<ColorGamut> {
    match raw.trim().to_ascii_uppercase().as_str() {
        "A" => Some(GAMUT_A),
        "B" => Some(GAMUT_B),
        "C" => Some(GAMUT_C),
        _ => None,
    }
}
