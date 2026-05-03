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
pub(super) const LEGACY_LEDS_PER_PACKET: usize = 20;
pub(super) const STREAM65_LEDS_PER_PACKET: usize = 20;
pub const GEN1_HID_REPORT_SIZE: usize = 65;
pub const CDC_SERIAL_REPORT_SIZE: usize = 64;
pub const GEN2_COLOR_REPORT_SIZE: usize = 1024;
pub const GEN2_SETTINGS_REPORT_SIZE: usize = 513;

pub(super) const CHANNELS_NOLLIE_1: usize = 1;
pub(super) const CHANNELS_NOLLIE_8: usize = 8;
pub(super) const CHANNELS_NOLLIE_28_12: usize = 12;
pub(super) const CHANNELS_NOLLIE_16_V3: usize = 16;
pub(super) const CHANNELS_NOLLIE_32_MAIN: usize = 20;
pub(super) const CHANNELS_NOLLIE_2: usize = 2;
pub(super) const CHANNELS_NOLLIE_4: usize = 4;
pub(super) const GEN2_PHYSICAL_CHANNELS: usize = 32;

pub(super) const LEDS_NOLLIE_1: usize = 630;
pub(super) const LEDS_NOLLIE_8: usize = 126;
pub(super) const LEDS_NOLLIE_28_12: usize = 42;
pub(super) const LEDS_NOLLIE_MATRIX: usize = 256;
pub(super) const LEDS_NOLLIE_LEGACY_8: usize = 100;
pub(super) const LEDS_NOLLIE_LEGACY_2: usize = 512;
pub(super) const LEDS_NOLLIE_LEGACY_28_12: usize = 30;
pub(super) const LEDS_NOLLIE_V12_HIGH: usize = 525;
pub(super) const LEDS_NOLLIE_4: usize = 636;
pub(super) const LEDS_NOLLIE_8_YOUTH: usize = 300;
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
    Nollie1Cdc,
    Nollie8Cdc,
    Nollie16v3Nos2,
    Nollie32Nos2,
    NollieMatrix,
    NollieLegacy8,
    NollieLegacy2,
    NollieLegacyTt,
    NollieLegacy16_1,
    NollieLegacy16_2,
    NollieLegacy28_12,
    NollieLegacy28L1,
    NollieLegacy28L2,
    Nollie8V12,
    Nollie16_1V12,
    Nollie16_2V12,
    NollieL1V12,
    NollieL2V12,
    Nollie4,
    Nollie8Youth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NollieProtocolKind {
    ModernGen1,
    DenseGen1,
    LegacyHeader,
    SerialCdc,
    Gen2Grouped,
    Nos2Hid,
    Stream65,
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
        self.model.expected_leds(self.nollie32_config)
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
            Self::Nollie1Cdc => "Nollie 1 CDC",
            Self::Nollie8Cdc => "Nollie 8 CDC",
            Self::Nollie16v3Nos2 => "Nollie 16 v3 NOS2",
            Self::Nollie32Nos2 => "Nollie 32 NOS2",
            Self::NollieMatrix => "Nollie Matrix",
            Self::NollieLegacy8 => "Nollie 8",
            Self::NollieLegacy2 => "Nollie 2",
            Self::NollieLegacyTt => "Nollie TT",
            Self::NollieLegacy16_1 => "Nollie 16 #1",
            Self::NollieLegacy16_2 => "Nollie 16 #2",
            Self::NollieLegacy28_12 => "Nollie 28/12 legacy",
            Self::NollieLegacy28L1 => "Nollie 28 L1",
            Self::NollieLegacy28L2 => "Nollie 28 L2",
            Self::Nollie8V12 => "Nollie 8 v1.2",
            Self::Nollie16_1V12 => "Nollie 16 #1 v1.2",
            Self::Nollie16_2V12 => "Nollie 16 #2 v1.2",
            Self::NollieL1V12 => "Nollie L1 v1.2",
            Self::NollieL2V12 => "Nollie L2 v1.2",
            Self::Nollie4 => "Nollie 4",
            Self::Nollie8Youth => "Nollie 8 Youth",
        }
    }

    #[must_use]
    pub(super) const fn protocol_kind(self) -> NollieProtocolKind {
        match self {
            Self::Nollie1 | Self::Nollie8 | Self::Prism8 => NollieProtocolKind::ModernGen1,
            Self::Nollie28_12
            | Self::NollieMatrix
            | Self::Nollie8V12
            | Self::Nollie16_1V12
            | Self::Nollie16_2V12
            | Self::NollieL1V12
            | Self::NollieL2V12 => NollieProtocolKind::DenseGen1,
            Self::NollieLegacy8
            | Self::NollieLegacy2
            | Self::NollieLegacyTt
            | Self::NollieLegacy16_1
            | Self::NollieLegacy16_2
            | Self::NollieLegacy28_12
            | Self::NollieLegacy28L1
            | Self::NollieLegacy28L2 => NollieProtocolKind::LegacyHeader,
            Self::Nollie1Cdc | Self::Nollie8Cdc => NollieProtocolKind::SerialCdc,
            Self::Nollie16v3 | Self::Nollie32 { .. } => NollieProtocolKind::Gen2Grouped,
            Self::Nollie16v3Nos2 | Self::Nollie32Nos2 => NollieProtocolKind::Nos2Hid,
            Self::Nollie4 | Self::Nollie8Youth => NollieProtocolKind::Stream65,
        }
    }

    #[must_use]
    pub(super) const fn color_format(self) -> DeviceColorFormat {
        match self {
            Self::Nollie28_12
            | Self::NollieMatrix
            | Self::NollieLegacy8
            | Self::NollieLegacy2
            | Self::NollieLegacyTt
            | Self::NollieLegacy16_1
            | Self::NollieLegacy16_2
            | Self::NollieLegacy28_12
            | Self::NollieLegacy28L1
            | Self::NollieLegacy28L2
            | Self::Nollie8V12
            | Self::Nollie16_1V12
            | Self::Nollie16_2V12
            | Self::NollieL1V12
            | Self::NollieL2V12 => DeviceColorFormat::Rgb,
            Self::Nollie1
            | Self::Nollie8
            | Self::Prism8
            | Self::Nollie16v3
            | Self::Nollie32 { .. }
            | Self::Nollie1Cdc
            | Self::Nollie8Cdc
            | Self::Nollie16v3Nos2
            | Self::Nollie32Nos2
            | Self::Nollie4
            | Self::Nollie8Youth => DeviceColorFormat::Grb,
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
            | Self::Nollie32 { .. }
            | Self::Nollie1Cdc
            | Self::Nollie8Cdc
            | Self::Nollie16v3Nos2
            | Self::Nollie32Nos2
            | Self::NollieMatrix
            | Self::NollieLegacy8
            | Self::NollieLegacy2
            | Self::NollieLegacyTt
            | Self::NollieLegacy16_1
            | Self::NollieLegacy16_2
            | Self::NollieLegacy28_12
            | Self::NollieLegacy28L1
            | Self::NollieLegacy28L2
            | Self::Nollie8V12
            | Self::Nollie16_1V12
            | Self::Nollie16_2V12
            | Self::NollieL1V12
            | Self::NollieL2V12
            | Self::Nollie4
            | Self::Nollie8Youth => 1.0,
        }
    }

    #[must_use]
    pub(super) const fn max_fps(self) -> u32 {
        match self {
            Self::Nollie32 { .. } | Self::Nollie16v3 => 30,
            Self::Nollie1
            | Self::Nollie8
            | Self::Nollie28_12
            | Self::Prism8
            | Self::Nollie1Cdc
            | Self::Nollie8Cdc
            | Self::Nollie16v3Nos2
            | Self::Nollie32Nos2
            | Self::NollieMatrix
            | Self::NollieLegacy8
            | Self::NollieLegacy2
            | Self::NollieLegacyTt
            | Self::NollieLegacy16_1
            | Self::NollieLegacy16_2
            | Self::NollieLegacy28_12
            | Self::NollieLegacy28L1
            | Self::NollieLegacy28L2
            | Self::Nollie8V12
            | Self::Nollie16_1V12
            | Self::Nollie16_2V12
            | Self::NollieL1V12
            | Self::NollieL2V12
            | Self::Nollie4
            | Self::Nollie8Youth => 60,
        }
    }

    #[must_use]
    pub(super) const fn expected_leds(self, config: Nollie32Config) -> usize {
        match self {
            Self::Nollie1 | Self::Nollie1Cdc => CHANNELS_NOLLIE_1 * LEDS_NOLLIE_1,
            Self::Nollie8 | Self::Prism8 | Self::Nollie8Cdc => CHANNELS_NOLLIE_8 * LEDS_NOLLIE_8,
            Self::Nollie28_12 => CHANNELS_NOLLIE_28_12 * LEDS_NOLLIE_28_12,
            Self::Nollie16v3 | Self::Nollie16v3Nos2 => CHANNELS_NOLLIE_16_V3 * LEDS_GEN2_CHANNEL,
            Self::Nollie32 { .. } | Self::Nollie32Nos2 => {
                CHANNELS_NOLLIE_32_MAIN * LEDS_GEN2_CHANNEL + config.cable_leds()
            }
            Self::NollieMatrix => CHANNELS_NOLLIE_1 * LEDS_NOLLIE_MATRIX,
            Self::NollieLegacy8
            | Self::NollieLegacy16_1
            | Self::NollieLegacy16_2
            | Self::NollieLegacy28L1
            | Self::NollieLegacy28L2 => CHANNELS_NOLLIE_8 * LEDS_NOLLIE_LEGACY_8,
            Self::NollieLegacy2 | Self::NollieLegacyTt => CHANNELS_NOLLIE_2 * LEDS_NOLLIE_LEGACY_2,
            Self::NollieLegacy28_12 => CHANNELS_NOLLIE_28_12 * LEDS_NOLLIE_LEGACY_28_12,
            Self::Nollie8V12
            | Self::Nollie16_1V12
            | Self::Nollie16_2V12
            | Self::NollieL1V12
            | Self::NollieL2V12 => CHANNELS_NOLLIE_8 * LEDS_NOLLIE_V12_HIGH,
            Self::Nollie4 => CHANNELS_NOLLIE_4 * LEDS_NOLLIE_4,
            Self::Nollie8Youth => CHANNELS_NOLLIE_8 * LEDS_NOLLIE_8_YOUTH,
        }
    }
}

impl Protocol for NollieProtocol {
    fn name(&self) -> &'static str {
        self.model.name()
    }

    fn init_sequence(&self) -> Vec<ProtocolCommand> {
        match self.model.protocol_kind() {
            NollieProtocolKind::ModernGen1 => super::gen1::init_sequence(),
            NollieProtocolKind::DenseGen1 | NollieProtocolKind::LegacyHeader => {
                super::legacy::init_sequence(self.model)
            }
            NollieProtocolKind::SerialCdc | NollieProtocolKind::Nos2Hid => Vec::new(),
            NollieProtocolKind::Gen2Grouped => {
                super::gen2::init_sequence(self.model, self.nollie32_config)
            }
            NollieProtocolKind::Stream65 => Vec::new(),
        }
    }

    fn shutdown_sequence(&self) -> Vec<ProtocolCommand> {
        match self.model.protocol_kind() {
            NollieProtocolKind::ModernGen1 => super::gen1::shutdown_sequence(self.model),
            NollieProtocolKind::DenseGen1 | NollieProtocolKind::LegacyHeader => {
                super::legacy::shutdown_sequence(self.model)
            }
            NollieProtocolKind::SerialCdc => super::serial::shutdown_sequence(self.model),
            NollieProtocolKind::Gen2Grouped => super::gen2::shutdown_sequence(self.nollie32_config),
            NollieProtocolKind::Nos2Hid | NollieProtocolKind::Stream65 => Vec::new(),
        }
    }

    fn encode_frame(&self, colors: &[[u8; 3]]) -> Vec<ProtocolCommand> {
        let mut commands = Vec::new();
        self.encode_frame_into(colors, &mut commands);
        commands
    }

    fn encode_frame_into(&self, colors: &[[u8; 3]], commands: &mut Vec<ProtocolCommand>) {
        match self.model.protocol_kind() {
            NollieProtocolKind::ModernGen1 => {
                super::gen1::encode_frame_into(self, colors, commands);
            }
            NollieProtocolKind::DenseGen1 | NollieProtocolKind::LegacyHeader => {
                super::legacy::encode_frame_into(self, colors, commands);
            }
            NollieProtocolKind::SerialCdc => {
                super::serial::encode_frame_into(self, colors, commands);
            }
            NollieProtocolKind::Gen2Grouped => {
                super::gen2::encode_frame_into(self, colors, commands);
            }
            NollieProtocolKind::Nos2Hid => {
                super::nos2::encode_frame_into(self, colors, commands);
            }
            NollieProtocolKind::Stream65 => {
                super::stream65::encode_frame_into(self, colors, commands);
            }
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
            NollieModel::Nollie16v3 | NollieModel::Nollie16v3Nos2 => gen1_zones(
                CHANNELS_NOLLIE_16_V3,
                LEDS_GEN2_CHANNEL,
                NollieModel::Nollie16v3,
            ),
            NollieModel::Nollie32 { .. } | NollieModel::Nollie32Nos2 => {
                nollie32_zones(self.nollie32_config)
            }
            NollieModel::Nollie1Cdc => gen1_zones(CHANNELS_NOLLIE_1, LEDS_NOLLIE_1, self.model),
            NollieModel::Nollie8Cdc => gen1_zones(CHANNELS_NOLLIE_8, LEDS_NOLLIE_8, self.model),
            NollieModel::NollieMatrix => {
                gen1_zones(CHANNELS_NOLLIE_1, LEDS_NOLLIE_MATRIX, self.model)
            }
            NollieModel::NollieLegacy8
            | NollieModel::NollieLegacy16_1
            | NollieModel::NollieLegacy16_2 => {
                gen1_zones(CHANNELS_NOLLIE_8, LEDS_NOLLIE_LEGACY_8, self.model)
            }
            NollieModel::NollieLegacy28L1 | NollieModel::NollieLegacy28L2 => {
                legacy_l_channels(self.model)
            }
            NollieModel::NollieLegacy2 | NollieModel::NollieLegacyTt => {
                gen1_zones(CHANNELS_NOLLIE_2, LEDS_NOLLIE_LEGACY_2, self.model)
            }
            NollieModel::NollieLegacy28_12 => {
                gen1_zones(CHANNELS_NOLLIE_28_12, LEDS_NOLLIE_LEGACY_28_12, self.model)
            }
            NollieModel::Nollie8V12 | NollieModel::NollieL1V12 | NollieModel::NollieL2V12 => {
                gen1_zones(CHANNELS_NOLLIE_8, LEDS_NOLLIE_V12_HIGH, self.model)
            }
            NollieModel::Nollie16_1V12 => {
                numbered_zones(1, CHANNELS_NOLLIE_8, LEDS_NOLLIE_V12_HIGH, self.model)
            }
            NollieModel::Nollie16_2V12 => {
                numbered_zones(9, CHANNELS_NOLLIE_8, LEDS_NOLLIE_V12_HIGH, self.model)
            }
            NollieModel::Nollie4 => gen1_zones(CHANNELS_NOLLIE_4, LEDS_NOLLIE_4, self.model),
            NollieModel::Nollie8Youth => {
                gen1_zones(CHANNELS_NOLLIE_8, LEDS_NOLLIE_8_YOUTH, self.model)
            }
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
    numbered_zones(1, channels, leds_per_channel, model)
}

fn numbered_zones(
    first_channel: usize,
    channels: usize,
    leds_per_channel: usize,
    model: NollieModel,
) -> Vec<ProtocolZone> {
    (0..channels)
        .map(|index| ProtocolZone {
            name: format!("Channel {}", index + first_channel),
            led_count: u32::try_from(leds_per_channel).unwrap_or(u32::MAX),
            topology: DeviceTopologyHint::Strip,
            color_format: model.color_format(),
            layout_hint: None,
        })
        .collect()
}

fn legacy_l_channels(model: NollieModel) -> Vec<ProtocolZone> {
    const LABELS: [&str; 8] = [
        "Channel 6",
        "Channel 5",
        "Channel 4",
        "Channel 3",
        "Channel 2",
        "Channel 1",
        "Channel 7",
        "Channel 8",
    ];

    LABELS
        .iter()
        .map(|label| ProtocolZone {
            name: (*label).to_owned(),
            led_count: u32::try_from(LEDS_NOLLIE_LEGACY_8).unwrap_or(u32::MAX),
            topology: DeviceTopologyHint::Strip,
            color_format: model.color_format(),
            layout_hint: None,
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
            layout_hint: None,
        });
    }

    if let Some(topology) = config.gpu_cable_type.topology() {
        zones.push(ProtocolZone {
            name: "GPU Strimer".to_owned(),
            led_count: u32::try_from(config.gpu_cable_type.led_count()).unwrap_or(u32::MAX),
            topology,
            color_format: DeviceColorFormat::Grb,
            layout_hint: None,
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
