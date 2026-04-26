//! Pure Nollie protocol encoder/decoder.

use std::borrow::Cow;
use std::cmp::min;
use std::sync::Mutex;
use std::time::Duration;

use hypercolor_types::device::{
    DeviceCapabilities, DeviceColorFormat, DeviceFeatures, DeviceTopologyHint,
};
use tracing::warn;

use crate::protocol::{
    Protocol, ProtocolCommand, ProtocolError, ProtocolResponse, ProtocolZone, ResponseStatus,
};

pub(super) const GEN1_LEDS_PER_PACKET: usize = 21;
pub const GEN1_HID_REPORT_SIZE: usize = 65;
pub const GEN2_COLOR_REPORT_SIZE: usize = 1024;
pub const GEN2_SETTINGS_REPORT_SIZE: usize = 513;

pub(super) const CHANNELS_NOLLIE_1: usize = 1;
pub(super) const CHANNELS_NOLLIE_8: usize = 8;
pub(super) const CHANNELS_NOLLIE_28_12: usize = 12;
pub(super) const CHANNELS_NOLLIE_16_V3: usize = 16;
pub(super) const CHANNELS_NOLLIE_32_MAIN: usize = 20;
pub(super) const GEN2_PHYSICAL_CHANNELS: usize = 32;

pub(super) const LEDS_NOLLIE_1: usize = 630;
pub(super) const LEDS_NOLLIE_8: usize = 126;
pub(super) const LEDS_NOLLIE_28_12: usize = 42;
pub(super) const LEDS_GEN2_CHANNEL: usize = 256;
pub(super) const LEDS_ATX_STRIMER: usize = 120;
pub(super) const LEDS_GPU_DUAL_STRIMER: usize = 108;
pub(super) const LEDS_GPU_TRIPLE_STRIMER: usize = 162;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolVersion {
    V1,
    V2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NollieModel {
    Nollie1,
    Nollie8,
    Nollie28_12,
    Prism8,
    Nollie16v3,
    Nollie32 { protocol_version: ProtocolVersion },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuCableType {
    None,
    Dual8Pin,
    Triple8Pin,
}

impl GpuCableType {
    #[must_use]
    pub const fn led_count(self) -> usize {
        match self {
            Self::None => 0,
            Self::Dual8Pin => LEDS_GPU_DUAL_STRIMER,
            Self::Triple8Pin => LEDS_GPU_TRIPLE_STRIMER,
        }
    }

    #[must_use]
    pub const fn rows(self) -> usize {
        match self {
            Self::None => 0,
            Self::Dual8Pin => 4,
            Self::Triple8Pin => 6,
        }
    }

    #[must_use]
    pub const fn mos_byte(self) -> u8 {
        match self {
            Self::Dual8Pin => 0x01,
            Self::None | Self::Triple8Pin => 0x00,
        }
    }

    #[must_use]
    const fn topology(self) -> Option<DeviceTopologyHint> {
        match self {
            Self::None => None,
            Self::Dual8Pin => Some(DeviceTopologyHint::Matrix { rows: 4, cols: 27 }),
            Self::Triple8Pin => Some(DeviceTopologyHint::Matrix { rows: 6, cols: 27 }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Nollie32Config {
    pub atx_cable_present: bool,
    pub gpu_cable_type: GpuCableType,
}

impl Nollie32Config {
    #[must_use]
    pub const fn cable_leds(self) -> usize {
        (if self.atx_cable_present {
            LEDS_ATX_STRIMER
        } else {
            0
        }) + self.gpu_cable_type.led_count()
    }
}

impl Default for Nollie32Config {
    fn default() -> Self {
        Self {
            atx_cable_present: false,
            gpu_cable_type: GpuCableType::None,
        }
    }
}

#[derive(Debug)]
pub struct NollieProtocol {
    model: NollieModel,
    nollie32_config: Nollie32Config,
    last_gen2_counts: Mutex<[u16; GEN2_PHYSICAL_CHANNELS]>,
}

impl NollieProtocol {
    #[must_use]
    pub const fn new(model: NollieModel) -> Self {
        Self {
            model,
            nollie32_config: Nollie32Config {
                atx_cable_present: false,
                gpu_cable_type: GpuCableType::None,
            },
            last_gen2_counts: Mutex::new([0; GEN2_PHYSICAL_CHANNELS]),
        }
    }

    #[must_use]
    pub const fn with_nollie32_config(mut self, config: Nollie32Config) -> Self {
        self.nollie32_config = config;
        self
    }

    #[must_use]
    pub const fn model(&self) -> NollieModel {
        self.model
    }

    #[must_use]
    pub const fn nollie32_config(&self) -> Nollie32Config {
        self.nollie32_config
    }

    #[must_use]
    pub(super) const fn is_gen1(&self) -> bool {
        self.model.is_gen1()
    }

    pub(super) fn normalize_colors<'a>(&self, colors: &'a [[u8; 3]]) -> Cow<'a, [[u8; 3]]> {
        let expected = self.expected_leds();
        if expected == 0 {
            return Cow::Borrowed(&[]);
        }

        if colors.len() == expected {
            return Cow::Borrowed(colors);
        }

        let mut normalized = vec![[0_u8; 3]; expected];
        let copy_len = min(colors.len(), expected);
        normalized[..copy_len].copy_from_slice(&colors[..copy_len]);

        warn!(
            expected,
            actual = colors.len(),
            model = self.model.name(),
            "nollie frame length mismatch; applying truncate/pad"
        );

        Cow::Owned(normalized)
    }

    pub(super) fn encode_gen2_counts_if_changed(
        &self,
        counts: [u16; GEN2_PHYSICAL_CHANNELS],
        commands: &mut Vec<ProtocolCommand>,
    ) {
        let mut last_counts = self
            .last_gen2_counts
            .lock()
            .expect("gen2 count cache lock should not be poisoned");
        if *last_counts != counts {
            super::gen2::push_count_config(counts, commands);
            *last_counts = counts;
        }
    }

    #[must_use]
    fn expected_leds(&self) -> usize {
        match self.model {
            NollieModel::Nollie1 => CHANNELS_NOLLIE_1 * LEDS_NOLLIE_1,
            NollieModel::Nollie8 | NollieModel::Prism8 => CHANNELS_NOLLIE_8 * LEDS_NOLLIE_8,
            NollieModel::Nollie28_12 => CHANNELS_NOLLIE_28_12 * LEDS_NOLLIE_28_12,
            NollieModel::Nollie16v3 => CHANNELS_NOLLIE_16_V3 * LEDS_GEN2_CHANNEL,
            NollieModel::Nollie32 { .. } => {
                CHANNELS_NOLLIE_32_MAIN * LEDS_GEN2_CHANNEL + self.nollie32_config.cable_leds()
            }
        }
    }
}

impl NollieModel {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Nollie1 => "Nollie 1",
            Self::Nollie8 => "Nollie 8 v2",
            Self::Nollie28_12 => "Nollie 28/12",
            Self::Prism8 => "PrismRGB Prism 8",
            Self::Nollie16v3 => "Nollie 16 v3",
            Self::Nollie32 { .. } => "Nollie 32",
        }
    }

    #[must_use]
    pub(super) const fn is_gen1(self) -> bool {
        matches!(
            self,
            Self::Nollie1 | Self::Nollie8 | Self::Nollie28_12 | Self::Prism8
        )
    }

    #[must_use]
    pub(super) const fn color_format(self) -> DeviceColorFormat {
        match self {
            Self::Nollie28_12 => DeviceColorFormat::Rgb,
            Self::Nollie1
            | Self::Nollie8
            | Self::Prism8
            | Self::Nollie16v3
            | Self::Nollie32 { .. } => DeviceColorFormat::Grb,
        }
    }

    #[must_use]
    pub(super) const fn brightness_scale(self) -> f32 {
        match self {
            Self::Prism8 => 0.75,
            Self::Nollie1
            | Self::Nollie8
            | Self::Nollie28_12
            | Self::Nollie16v3
            | Self::Nollie32 { .. } => 1.0,
        }
    }

    #[must_use]
    pub(super) const fn max_fps(self) -> u32 {
        match self {
            Self::Nollie32 { .. } | Self::Nollie16v3 => 30,
            Self::Nollie1 | Self::Nollie8 | Self::Nollie28_12 | Self::Prism8 => 60,
        }
    }
}

impl Protocol for NollieProtocol {
    fn name(&self) -> &'static str {
        self.model.name()
    }

    fn init_sequence(&self) -> Vec<ProtocolCommand> {
        if self.is_gen1() {
            super::gen1::init_sequence()
        } else {
            super::gen2::init_sequence(self.model, self.nollie32_config)
        }
    }

    fn shutdown_sequence(&self) -> Vec<ProtocolCommand> {
        if self.is_gen1() {
            super::gen1::shutdown_sequence(self.model)
        } else {
            super::gen2::shutdown_sequence(self.nollie32_config)
        }
    }

    fn encode_frame(&self, colors: &[[u8; 3]]) -> Vec<ProtocolCommand> {
        let mut commands = Vec::new();
        self.encode_frame_into(colors, &mut commands);
        commands
    }

    fn encode_frame_into(&self, colors: &[[u8; 3]], commands: &mut Vec<ProtocolCommand>) {
        if self.is_gen1() {
            super::gen1::encode_frame_into(self, colors, commands);
        } else {
            super::gen2::encode_frame_into(self, colors, commands);
        }
    }

    fn parse_response(&self, data: &[u8]) -> Result<ProtocolResponse, ProtocolError> {
        if data.is_empty() {
            return Err(ProtocolError::MalformedResponse {
                detail: format!(
                    "{} response too short: expected at least 1 byte",
                    self.name()
                ),
            });
        }

        Ok(ProtocolResponse {
            status: ResponseStatus::Ok,
            data: data.to_vec(),
        })
    }

    fn zones(&self) -> Vec<ProtocolZone> {
        match self.model {
            NollieModel::Nollie1 => gen1_zones(CHANNELS_NOLLIE_1, LEDS_NOLLIE_1, self.model),
            NollieModel::Nollie8 | NollieModel::Prism8 => {
                gen1_zones(CHANNELS_NOLLIE_8, LEDS_NOLLIE_8, self.model)
            }
            NollieModel::Nollie28_12 => {
                gen1_zones(CHANNELS_NOLLIE_28_12, LEDS_NOLLIE_28_12, self.model)
            }
            NollieModel::Nollie16v3 => gen1_zones(
                CHANNELS_NOLLIE_16_V3,
                LEDS_GEN2_CHANNEL,
                NollieModel::Nollie16v3,
            ),
            NollieModel::Nollie32 { .. } => nollie32_zones(self.nollie32_config),
        }
    }

    fn capabilities(&self) -> DeviceCapabilities {
        DeviceCapabilities {
            led_count: self.total_leds(),
            supports_direct: true,
            supports_brightness: false,
            has_display: false,
            display_resolution: None,
            max_fps: self.model.max_fps(),
            color_space: hypercolor_types::device::DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        }
    }

    fn total_leds(&self) -> u32 {
        u32::try_from(self.expected_leds()).unwrap_or(u32::MAX)
    }

    fn frame_interval(&self) -> Duration {
        if self.model.max_fps() <= 30 {
            Duration::from_millis(33)
        } else {
            Duration::from_millis(16)
        }
    }
}

fn gen1_zones(channels: usize, leds_per_channel: usize, model: NollieModel) -> Vec<ProtocolZone> {
    (0..channels)
        .map(|index| ProtocolZone {
            name: format!("Channel {}", index + 1),
            led_count: u32::try_from(leds_per_channel).unwrap_or(u32::MAX),
            topology: DeviceTopologyHint::Strip,
            color_format: model.color_format(),
        })
        .collect()
}

fn nollie32_zones(config: Nollie32Config) -> Vec<ProtocolZone> {
    let mut zones = gen1_zones(
        CHANNELS_NOLLIE_32_MAIN,
        LEDS_GEN2_CHANNEL,
        NollieModel::Nollie32 {
            protocol_version: ProtocolVersion::V2,
        },
    );

    if config.atx_cable_present {
        zones.push(ProtocolZone {
            name: "ATX Strimer".to_owned(),
            led_count: u32::try_from(LEDS_ATX_STRIMER).unwrap_or(u32::MAX),
            topology: DeviceTopologyHint::Matrix { rows: 6, cols: 20 },
            color_format: DeviceColorFormat::Grb,
        });
    }

    if let Some(topology) = config.gpu_cable_type.topology() {
        zones.push(ProtocolZone {
            name: "GPU Strimer".to_owned(),
            led_count: u32::try_from(config.gpu_cable_type.led_count()).unwrap_or(u32::MAX),
            topology,
            color_format: DeviceColorFormat::Grb,
        });
    }

    zones
}

#[must_use]
pub(super) fn encode_color(color: [u8; 3], scale: f32, format: DeviceColorFormat) -> [u8; 3] {
    let rs = scale_channel(color[0], scale);
    let gs = scale_channel(color[1], scale);
    let bs = scale_channel(color[2], scale);

    match format {
        DeviceColorFormat::Grb => [gs, rs, bs],
        DeviceColorFormat::Rbg => [rs, bs, gs],
        DeviceColorFormat::Rgb | DeviceColorFormat::Rgbw | DeviceColorFormat::Jpeg => [rs, gs, bs],
    }
}

#[must_use]
pub(super) fn command_from_packet(
    data: Vec<u8>,
    expects_response: bool,
    response_delay: Duration,
    post_delay: Duration,
) -> ProtocolCommand {
    ProtocolCommand {
        data,
        expects_response,
        response_delay,
        post_delay,
        transfer_type: crate::protocol::TransferType::Primary,
    }
}

#[allow(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn scale_channel(value: u8, scale: f32) -> u8 {
    (f32::from(value) * scale).round().clamp(0.0, 255.0) as u8
}
