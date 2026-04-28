//! Runtime protocol configuration derived from device attachment profiles.

use hypercolor_types::attachment::{AttachmentBinding, DeviceAttachmentProfile};
use hypercolor_types::device::DeviceInfo;

use crate::drivers::nollie::{
    GpuCableType, Nollie32Config, NollieModel, NollieProtocol, ProtocolVersion,
};
use crate::drivers::prismrgb::{PrismRgbModel, PrismRgbProtocol, PrismSConfig, PrismSGpuCable};
use crate::protocol::Protocol;

const PRISM_S_PROTOCOL_ID: &str = "prismrgb/prism-s";
const NOLLIE32_PROTOCOL_ID: &str = "nollie/nollie-32";
const ATX_STRIMER_LEDS: usize = 120;
const GPU_DUAL_STRIMER_LEDS: usize = 108;
const GPU_TRIPLE_STRIMER_LEDS: usize = 162;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolRuntimeConfig {
    PrismS(PrismSConfig),
    Nollie32(Nollie32Config),
}

impl ProtocolRuntimeConfig {
    #[must_use]
    pub const fn protocol_id(self) -> &'static str {
        match self {
            Self::PrismS(_) => PRISM_S_PROTOCOL_ID,
            Self::Nollie32(_) => NOLLIE32_PROTOCOL_ID,
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
                    protocol_version: ProtocolVersion::V2,
                })
                .with_nollie32_config(config),
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
        }
    }
}

pub fn runtime_config_for_attachment_profile(
    device: &DeviceInfo,
    profile: &DeviceAttachmentProfile,
    binding_led_count: impl FnMut(&AttachmentBinding) -> Option<u32>,
) -> Option<ProtocolRuntimeConfig> {
    if has_protocol(device, PRISM_S_PROTOCOL_ID) {
        return Some(ProtocolRuntimeConfig::PrismS(
            prism_s_config_for_attachment_profile(profile, binding_led_count),
        ));
    }

    if has_protocol(device, NOLLIE32_PROTOCOL_ID) {
        return Some(ProtocolRuntimeConfig::Nollie32(
            nollie32_config_for_attachment_profile(profile, binding_led_count),
        ));
    }

    None
}

fn prism_s_config_for_attachment_profile(
    profile: &DeviceAttachmentProfile,
    mut binding_led_count: impl FnMut(&AttachmentBinding) -> Option<u32>,
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
    profile: &DeviceAttachmentProfile,
    mut binding_led_count: impl FnMut(&AttachmentBinding) -> Option<u32>,
) -> Nollie32Config {
    let mut config = Nollie32Config::default();

    for binding in profile.bindings.iter().filter(|binding| binding.enabled) {
        match binding.slot_id.as_str() {
            "atx-strimer" => config.atx_cable_present = true,
            "gpu-strimer" => {
                config.gpu_cable_type = match binding_led_count(binding) {
                    Some(108) => GpuCableType::Dual8Pin,
                    Some(162) => GpuCableType::Triple8Pin,
                    _ => config.gpu_cable_type,
                };
            }
            _ => {}
        }
    }

    config
}

fn has_protocol(device: &DeviceInfo, protocol_id: &str) -> bool {
    device.origin.protocol_id.as_deref() == Some(protocol_id)
}
