//! Runtime protocol configuration derived from device attachment profiles.

use hypercolor_types::attachment::{ComponentBinding, DeviceComponentProfile};
use hypercolor_types::device::DeviceInfo;

use crate::drivers::nollie::{
    GpuCableType, Nollie32Config, NollieModel, NollieProtocol, ProtocolVersion,
};
use crate::drivers::prismrgb::{PrismRgbModel, PrismRgbProtocol, PrismSConfig, PrismSGpuCable};
use crate::protocol::Protocol;

const PRISM_S_PROTOCOL_ID: &str = "prismrgb/prism-s";
const NOLLIE32_PROTOCOL_ID: &str = "nollie/nollie-32";
const NOLLIE32_NOS2_PROTOCOL_IDS: &[&str] = &["nollie/nollie-32-nos2", "nollie/nollie-32-nos2-alt"];
const ATX_STRIMER_LEDS: usize = 120;
const GPU_DUAL_STRIMER_LEDS: usize = 108;
const GPU_TRIPLE_STRIMER_LEDS: usize = 162;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolRuntimeConfig {
    PrismS(PrismSConfig),
    Nollie32(Nollie32Config),
    Nollie32Nos2(Nollie32Config),
}

impl ProtocolRuntimeConfig {
    #[must_use]
    pub const fn protocol_id(self) -> &'static str {
        match self {
            Self::PrismS(_) => PRISM_S_PROTOCOL_ID,
            Self::Nollie32(_) => NOLLIE32_PROTOCOL_ID,
            Self::Nollie32Nos2(_) => "nollie/nollie-32-nos2",
        }
    }

    #[must_use]
    pub fn build_protocol(self) -> Box<dyn Protocol> {
        match self {
            Self::PrismS(config) => {
                Box::new(PrismRgbProtocol::new(PrismRgbModel::PrismS).with_prism_s_config(config))
            }
            Self::Nollie32(config) => Box::new(
                NollieProtocol::new(NollieModel::Nollie32 {
                    protocol_version: ProtocolVersion::V1,
                })
                .with_nollie32_config(config),
            ),
            Self::Nollie32Nos2(config) => Box::new(
                NollieProtocol::new(NollieModel::Nollie32Nos2).with_nollie32_config(config),
            ),
        }
    }

    #[must_use]
    pub const fn atx_attachment_leds(self) -> usize {
        match self {
            Self::PrismS(config) => {
                if config.atx_present {
                    ATX_STRIMER_LEDS
                } else {
                    0
                }
            }
            Self::Nollie32(config) => {
                if config.atx_cable_present {
                    ATX_STRIMER_LEDS
                } else {
                    0
                }
            }
            Self::Nollie32Nos2(config) => {
                if config.atx_cable_present {
                    ATX_STRIMER_LEDS
                } else {
                    0
                }
            }
        }
    }

    #[must_use]
    pub const fn gpu_attachment_leds(self) -> usize {
        match self {
            Self::PrismS(config) => match config.gpu_cable {
                Some(PrismSGpuCable::Dual8Pin) => GPU_DUAL_STRIMER_LEDS,
                Some(PrismSGpuCable::Triple8Pin) => GPU_TRIPLE_STRIMER_LEDS,
                None => 0,
            },
            Self::Nollie32(config) => config.gpu_cable_type.led_count(),
            Self::Nollie32Nos2(config) => config.gpu_cable_type.led_count(),
        }
    }
}

pub fn runtime_config_for_attachment_profile(
    device: &DeviceInfo,
    profile: &DeviceComponentProfile,
    binding_led_count: impl FnMut(&ComponentBinding) -> Option<u32>,
) -> Option<ProtocolRuntimeConfig> {
    if has_protocol(device, PRISM_S_PROTOCOL_ID) {
        return Some(ProtocolRuntimeConfig::PrismS(
            prism_s_config_for_attachment_profile(profile, binding_led_count),
        ));
    }

    if has_protocol(device, NOLLIE32_PROTOCOL_ID) {
        return Some(ProtocolRuntimeConfig::Nollie32(
            nollie32_config_for_attachment_profile(device, profile, binding_led_count),
        ));
    }

    if has_any_protocol(device, NOLLIE32_NOS2_PROTOCOL_IDS) {
        return Some(ProtocolRuntimeConfig::Nollie32Nos2(
            nollie32_config_for_attachment_profile(device, profile, binding_led_count),
        ));
    }

    None
}

fn prism_s_config_for_attachment_profile(
    profile: &DeviceComponentProfile,
    mut binding_led_count: impl FnMut(&ComponentBinding) -> Option<u32>,
) -> PrismSConfig {
    let has_enabled_bindings = profile.bindings.iter().any(|binding| binding.enabled);
    if !has_enabled_bindings {
        return PrismSConfig::default();
    }

    let mut config = PrismSConfig {
        atx_present: false,
        gpu_cable: None,
    };

    for binding in profile.bindings.iter().filter(|binding| binding.enabled) {
        match binding.slot_id.as_str() {
            "atx-strimer" => config.atx_present = true,
            "gpu-strimer" => {
                config.gpu_cable = match binding_led_count(binding) {
                    Some(108) => Some(PrismSGpuCable::Dual8Pin),
                    Some(162) => Some(PrismSGpuCable::Triple8Pin),
                    _ => config.gpu_cable,
                };
            }
            _ => {}
        }
    }

    config
}

fn nollie32_config_for_attachment_profile(
    device: &DeviceInfo,
    profile: &DeviceComponentProfile,
    mut binding_led_count: impl FnMut(&ComponentBinding) -> Option<u32>,
) -> Nollie32Config {
    let mut config =
        nollie32_config_from_device_zones(device).unwrap_or(Nollie32Config::OFFICIAL_DEFAULT);

    for binding in &profile.bindings {
        match binding.slot_id.as_str() {
            "atx-strimer" => config.atx_cable_present = binding.enabled,
            "gpu-strimer" => {
                config.gpu_cable_type = if binding.enabled {
                    gpu_cable_type_for_led_count(binding_led_count(binding))
                        .unwrap_or(config.gpu_cable_type)
                } else {
                    GpuCableType::None
                }
            }
            _ => {}
        }
    }

    config
}

fn nollie32_config_from_device_zones(device: &DeviceInfo) -> Option<Nollie32Config> {
    let mut config = Nollie32Config::default();
    let mut has_cable_zone = false;

    for zone in &device.zones {
        match zone.name.as_str() {
            "ATX Strimer" => {
                config.atx_cable_present = true;
                has_cable_zone = true;
            }
            "GPU Strimer" => {
                if let Some(cable_type) = gpu_cable_type_for_led_count(Some(zone.led_count)) {
                    config.gpu_cable_type = cable_type;
                    has_cable_zone = true;
                }
            }
            _ => {}
        }
    }

    has_cable_zone.then_some(config)
}

fn gpu_cable_type_for_led_count(led_count: Option<u32>) -> Option<GpuCableType> {
    match led_count {
        Some(108) => Some(GpuCableType::Dual8Pin),
        Some(162) => Some(GpuCableType::Triple8Pin),
        _ => None,
    }
}

fn has_protocol(device: &DeviceInfo, protocol_id: &str) -> bool {
    device.origin.protocol_id.as_deref() == Some(protocol_id)
}

fn has_any_protocol(device: &DeviceInfo, protocol_ids: &[&str]) -> bool {
    device
        .origin
        .protocol_id
        .as_deref()
        .is_some_and(|value| protocol_ids.contains(&value))
}
