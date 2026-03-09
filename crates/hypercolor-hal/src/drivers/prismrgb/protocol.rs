//! Pure `PrismRGB` protocol encoder/decoder.

use std::borrow::Cow;
use std::cmp::min;
use std::time::Duration;

use hypercolor_types::device::{DeviceCapabilities, DeviceColorFormat, DeviceTopologyHint};
use tracing::warn;

use crate::protocol::{
    CommandBuffer, Protocol, ProtocolCommand, ProtocolError, ProtocolResponse, ProtocolZone,
    ResponseStatus, TransferType,
};

const CHANNELS_PRISM_8: usize = 8;
const LEDS_PER_PRISM_8_CHANNEL: usize = 126;
const PRISM_8_LEDS_PER_PACKET: usize = 21;

const PRISM_S_ATX_LEDS: usize = 120;
const PRISM_S_GPU_DUAL_LEDS: usize = 108;
const PRISM_S_GPU_TRIPLE_LEDS: usize = 162;
const PRISM_S_ATX_PACKET_DATA_LEN: usize = 63;
const PRISM_S_LAST_ATX_PACKET_OFFSET: usize = 320;
const PRISM_S_GPU_MARKER_PACKET_LEN: usize = 46;
const PRISM_S_GPU_INLINE_BYTES: usize = 18;
const PRISM_S_LAST_ATX_PACKET_ID: u8 = 0x0F;
const PRISM_S_GPU_MARKER_PACKET_ID: u8 = 0x05;
const PRISM_S_GPU_DUAL_CHUNK_IDS: [u8; 5] = [6, 7, 8, 9, 20];
const PRISM_S_GPU_CHUNK_IDS: [u8; 8] = [6, 7, 8, 9, 10, 11, 12, 13];

const PRISM_MINI_MAX_LEDS: usize = 128;
const PRISM_MINI_LEDS_PER_PACKET: usize = 20;
const PRISM_MINI_DATA_MARKER_OFFSET: usize = 4;
const PRISM_MINI_DATA_OFFSET: usize = 5;

/// Fixed-size `PrismRGB` HID report size.
pub const HID_REPORT_SIZE: usize = 65;

/// Maximum sum of `R + G + B` in Prism Mini low-power mode.
pub const LOW_POWER_THRESHOLD: u16 = 175;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrismRgbModel {
    Prism8,
    Nollie8,
    PrismS,
    PrismMini,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrismSGpuCable {
    Dual8Pin,
    Triple8Pin,
}

impl PrismSGpuCable {
    #[must_use]
    const fn led_count(self) -> usize {
        match self {
            Self::Dual8Pin => PRISM_S_GPU_DUAL_LEDS,
            Self::Triple8Pin => PRISM_S_GPU_TRIPLE_LEDS,
        }
    }

    #[must_use]
    const fn topology(self) -> DeviceTopologyHint {
        match self {
            Self::Dual8Pin => DeviceTopologyHint::Matrix { rows: 4, cols: 27 },
            Self::Triple8Pin => DeviceTopologyHint::Matrix { rows: 6, cols: 27 },
        }
    }

    #[must_use]
    const fn settings_mode(self) -> u8 {
        match self {
            Self::Dual8Pin => 0x01,
            Self::Triple8Pin => 0x00,
        }
    }

    #[must_use]
    const fn packet_ids(self) -> &'static [u8] {
        match self {
            Self::Dual8Pin => &PRISM_S_GPU_DUAL_CHUNK_IDS,
            Self::Triple8Pin => &PRISM_S_GPU_CHUNK_IDS,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrismSConfig {
    pub atx_present: bool,
    pub gpu_cable: Option<PrismSGpuCable>,
}

impl PrismSConfig {
    #[must_use]
    pub const fn total_leds(self) -> usize {
        let gpu_leds = match self.gpu_cable {
            Some(cable) => cable.led_count(),
            None => 0,
        };

        (if self.atx_present {
            PRISM_S_ATX_LEDS
        } else {
            0
        }) + gpu_leds
    }

    #[must_use]
    const fn settings_mode(self) -> u8 {
        match self.gpu_cable {
            Some(cable) => cable.settings_mode(),
            None => 0x00,
        }
    }
}

impl Default for PrismSConfig {
    fn default() -> Self {
        Self {
            atx_present: true,
            gpu_cable: Some(PrismSGpuCable::Triple8Pin),
        }
    }
}

impl PrismRgbModel {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Prism8 => "PrismRGB Prism 8",
            Self::Nollie8 => "Nollie 8 v2",
            Self::PrismS => "PrismRGB Prism S",
            Self::PrismMini => "PrismRGB Prism Mini",
        }
    }

    #[must_use]
    pub const fn color_format(self) -> DeviceColorFormat {
        match self {
            Self::Prism8 | Self::Nollie8 => DeviceColorFormat::Grb,
            Self::PrismS | Self::PrismMini => DeviceColorFormat::Rgb,
        }
    }

    #[must_use]
    pub const fn brightness_scale(self) -> f32 {
        match self {
            Self::Prism8 => 0.75,
            Self::Nollie8 | Self::PrismMini => 1.0,
            Self::PrismS => 0.50,
        }
    }

    #[must_use]
    pub const fn total_leds(self) -> usize {
        match self {
            Self::Prism8 | Self::Nollie8 => CHANNELS_PRISM_8 * LEDS_PER_PRISM_8_CHANNEL,
            Self::PrismS => PRISM_S_ATX_LEDS + PRISM_S_GPU_TRIPLE_LEDS,
            Self::PrismMini => PRISM_MINI_MAX_LEDS,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PrismRgbProtocol {
    model: PrismRgbModel,
    low_power_mode: bool,
    compression_enabled: bool,
    prism_s_config: PrismSConfig,
}

impl PrismRgbProtocol {
    #[must_use]
    pub const fn new(model: PrismRgbModel) -> Self {
        Self {
            model,
            low_power_mode: matches!(model, PrismRgbModel::PrismMini),
            compression_enabled: false,
            prism_s_config: PrismSConfig {
                atx_present: true,
                gpu_cable: Some(PrismSGpuCable::Triple8Pin),
            },
        }
    }

    #[must_use]
    pub const fn with_low_power_mode(mut self, low_power_mode: bool) -> Self {
        self.low_power_mode = low_power_mode;
        self
    }

    #[must_use]
    pub const fn with_compression_enabled(mut self, compression_enabled: bool) -> Self {
        self.compression_enabled = compression_enabled;
        self
    }

    #[must_use]
    pub const fn with_prism_s_config(mut self, prism_s_config: PrismSConfig) -> Self {
        self.prism_s_config = prism_s_config;
        self
    }

    #[must_use]
    const fn expected_leds(&self) -> usize {
        match self.model {
            PrismRgbModel::PrismS => self.prism_s_config.total_leds(),
            _ => self.model.total_leds(),
        }
    }

    fn normalize_colors<'a>(&self, colors: &'a [[u8; 3]]) -> Cow<'a, [[u8; 3]]> {
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

        if colors.len() != expected {
            warn!(
                expected,
                actual = colors.len(),
                model = self.model.name(),
                "prismrgb frame length mismatch; applying truncate/pad"
            );
        }

        Cow::Owned(normalized)
    }

    fn encode_prism8_frame_into(&self, colors: &[[u8; 3]], commands: &mut Vec<ProtocolCommand>) {
        let normalized = self.normalize_colors(colors);
        let normalized = normalized.as_ref();
        let mut command_buffer = CommandBuffer::new(commands);

        for channel in 0..CHANNELS_PRISM_8 {
            let start = channel * LEDS_PER_PRISM_8_CHANNEL;
            let end = start + LEDS_PER_PRISM_8_CHANNEL;
            for (packet_index, chunk) in normalized[start..end]
                .chunks(PRISM_8_LEDS_PER_PACKET)
                .enumerate()
            {
                let mut packet = [0_u8; HID_REPORT_SIZE];
                packet[1] = u8::try_from(packet_index + channel * 6).unwrap_or(u8::MAX);

                let mut cursor = 2;
                for color in chunk {
                    let encoded_color = encode_color(
                        *color,
                        self.model.brightness_scale(),
                        self.model.color_format(),
                    );
                    packet[cursor..cursor + 3].copy_from_slice(&encoded_color);
                    cursor += 3;
                }

                command_buffer.push_slice(
                    &packet,
                    false,
                    Duration::ZERO,
                    Duration::ZERO,
                    TransferType::Primary,
                );
            }
        }

        let mut commit = [0_u8; HID_REPORT_SIZE];
        commit[1] = 0xFF;
        command_buffer.push_slice(
            &commit,
            false,
            Duration::ZERO,
            Duration::ZERO,
            TransferType::Primary,
        );
        command_buffer.finish();
    }

    fn prism_s_settings_command(&self) -> ProtocolCommand {
        let mut packet = [0_u8; HID_REPORT_SIZE];
        packet[1] = 0xFE;
        packet[2] = 0x01;
        packet[6] = self.prism_s_config.settings_mode();
        command_from_packet(packet, false, Duration::from_millis(50))
    }

    fn encode_prism_s_frame_into(&self, colors: &[[u8; 3]], commands: &mut Vec<ProtocolCommand>) {
        let normalized = self.normalize_colors(colors);
        let normalized = normalized.as_ref();
        let atx_leds = if self.prism_s_config.atx_present {
            PRISM_S_ATX_LEDS
        } else {
            0
        };
        let gpu_leds = self
            .prism_s_config
            .gpu_cable
            .map_or(0, PrismSGpuCable::led_count);
        let atx = &normalized[..atx_leds];
        let gpu = &normalized[atx_leds..atx_leds + gpu_leds];

        let atx_bytes = flatten_colors(atx, self.model.brightness_scale(), DeviceColorFormat::Rgb);
        let gpu_bytes = flatten_colors(gpu, self.model.brightness_scale(), DeviceColorFormat::Rgb);

        let mut send_data = Vec::new();
        if self.prism_s_config.atx_present {
            send_data.reserve(6 * 64);
            let mut atx_remaining = atx_bytes.as_slice();

            for packet_id in 0_u8..5 {
                let take = min(PRISM_S_ATX_PACKET_DATA_LEN, atx_remaining.len());
                send_data.push(packet_id);
                send_data.extend_from_slice(&atx_remaining[..take]);
                atx_remaining = &atx_remaining[take..];
            }

            send_data.push(PRISM_S_LAST_ATX_PACKET_ID);
            send_data.extend_from_slice(atx_remaining);
            while send_data.len() < PRISM_S_LAST_ATX_PACKET_OFFSET + PRISM_S_GPU_MARKER_PACKET_LEN {
                send_data.push(0x00);
            }
        }

        if let Some(gpu_cable) = self.prism_s_config.gpu_cable {
            if send_data.is_empty() {
                send_data.resize(PRISM_S_GPU_MARKER_PACKET_LEN, 0x00);
                send_data[0] = PRISM_S_GPU_MARKER_PACKET_ID;
            } else {
                send_data[PRISM_S_LAST_ATX_PACKET_OFFSET] = PRISM_S_GPU_MARKER_PACKET_ID;
            }

            let inline_gpu = min(PRISM_S_GPU_INLINE_BYTES, gpu_bytes.len());
            send_data.extend_from_slice(&gpu_bytes[..inline_gpu]);

            let mut gpu_remaining = &gpu_bytes[inline_gpu..];
            for packet_id in gpu_cable.packet_ids() {
                if gpu_remaining.is_empty() {
                    break;
                }

                let take = min(PRISM_S_ATX_PACKET_DATA_LEN, gpu_remaining.len());
                send_data.push(*packet_id);
                send_data.extend_from_slice(&gpu_remaining[..take]);
                gpu_remaining = &gpu_remaining[take..];
            }
        }

        let mut encoder = CommandBuffer::new(commands);
        for chunk in send_data.chunks(64) {
            let mut packet = [0_u8; HID_REPORT_SIZE];
            packet[1..=chunk.len()].copy_from_slice(chunk);
            encoder.push_slice(
                &packet,
                false,
                Duration::ZERO,
                Duration::ZERO,
                TransferType::Primary,
            );
        }
        encoder.finish();
    }

    fn prism_mini_firmware_query() -> ProtocolCommand {
        let mut packet = [0_u8; HID_REPORT_SIZE];
        packet[4] = 0xCC;
        command_from_packet(packet, true, Duration::ZERO)
    }

    fn encode_prism_mini_frame_into(
        &self,
        colors: &[[u8; 3]],
        commands: &mut Vec<ProtocolCommand>,
    ) {
        let normalized = self.normalize_colors(colors);
        let normalized = normalized.as_ref();
        let encoded_bytes = if self.compression_enabled {
            encode_prism_mini_compressed(normalized)
        } else {
            normalized
                .iter()
                .flat_map(|color| {
                    let [r, g, b] = if self.low_power_mode {
                        let (r, g, b) = apply_low_power_saver(color[0], color[1], color[2]);
                        [r, g, b]
                    } else {
                        *color
                    };
                    [r, g, b]
                })
                .collect::<Vec<_>>()
        };

        let total_packets = if self.compression_enabled {
            encoded_bytes.len().div_ceil(60)
        } else {
            normalized.len().div_ceil(PRISM_MINI_LEDS_PER_PACKET)
        };

        let mut command_buffer = CommandBuffer::new(commands);
        for (index, chunk) in encoded_bytes.chunks(60).enumerate() {
            let mut packet = [0_u8; HID_REPORT_SIZE];
            packet[1] = u8::try_from(index + 1).unwrap_or(u8::MAX);
            packet[2] = u8::try_from(total_packets).unwrap_or(u8::MAX);
            packet[PRISM_MINI_DATA_MARKER_OFFSET] = 0xAA;
            packet[PRISM_MINI_DATA_OFFSET..PRISM_MINI_DATA_OFFSET + chunk.len()]
                .copy_from_slice(chunk);
            command_buffer.push_slice(
                &packet,
                false,
                Duration::ZERO,
                Duration::ZERO,
                TransferType::Primary,
            );
        }
        command_buffer.finish();
    }
}

impl Protocol for PrismRgbProtocol {
    fn name(&self) -> &str {
        self.model.name()
    }

    fn init_sequence(&self) -> Vec<ProtocolCommand> {
        match self.model {
            PrismRgbModel::Prism8 | PrismRgbModel::Nollie8 => {
                let mut firmware = [0_u8; HID_REPORT_SIZE];
                firmware[1] = 0xFC;
                firmware[2] = 0x01;

                let mut channels = [0_u8; HID_REPORT_SIZE];
                channels[1] = 0xFC;
                channels[2] = 0x03;

                let mut hardware_effect = [0_u8; HID_REPORT_SIZE];
                hardware_effect[1] = 0xFE;
                hardware_effect[2] = 0x02;
                hardware_effect[7] = 0x64;
                hardware_effect[8] = 0x0A;
                hardware_effect[10] = 0x01;

                vec![
                    command_from_packet(firmware, true, Duration::ZERO),
                    command_from_packet(channels, true, Duration::ZERO),
                    command_from_packet(hardware_effect, false, Duration::ZERO),
                ]
            }
            PrismRgbModel::PrismS => vec![self.prism_s_settings_command()],
            PrismRgbModel::PrismMini => vec![Self::prism_mini_firmware_query()],
        }
    }

    fn shutdown_sequence(&self) -> Vec<ProtocolCommand> {
        match self.model {
            PrismRgbModel::Prism8 | PrismRgbModel::Nollie8 => {
                let mut hardware_effect = [0_u8; HID_REPORT_SIZE];
                hardware_effect[1] = 0xFE;
                hardware_effect[2] = 0x02;
                hardware_effect[7] = 0x64;
                hardware_effect[8] = 0x0A;
                hardware_effect[10] = 0x01;

                let mut hardware_mode = [0_u8; HID_REPORT_SIZE];
                hardware_mode[1] = 0xFE;
                hardware_mode[2] = 0x01;

                vec![
                    command_from_packet(hardware_effect, false, Duration::ZERO),
                    command_from_packet(hardware_mode, false, Duration::ZERO),
                ]
            }
            PrismRgbModel::PrismS => vec![self.prism_s_settings_command()],
            PrismRgbModel::PrismMini => Vec::new(),
        }
    }

    fn encode_frame(&self, colors: &[[u8; 3]]) -> Vec<ProtocolCommand> {
        let mut commands = Vec::new();
        self.encode_frame_into(colors, &mut commands);
        commands
    }

    fn encode_frame_into(&self, colors: &[[u8; 3]], commands: &mut Vec<ProtocolCommand>) {
        match self.model {
            PrismRgbModel::Prism8 | PrismRgbModel::Nollie8 => {
                self.encode_prism8_frame_into(colors, commands);
            }
            PrismRgbModel::PrismS => self.encode_prism_s_frame_into(colors, commands),
            PrismRgbModel::PrismMini => self.encode_prism_mini_frame_into(colors, commands),
        }
    }

    fn parse_response(&self, data: &[u8]) -> Result<ProtocolResponse, ProtocolError> {
        let min_len = match self.model {
            PrismRgbModel::PrismMini => 4,
            PrismRgbModel::Prism8 | PrismRgbModel::Nollie8 | PrismRgbModel::PrismS => 1,
        };

        if data.len() < min_len {
            return Err(ProtocolError::MalformedResponse {
                detail: format!(
                    "{} response too short: expected at least {min_len} byte(s), got {}",
                    self.model.name(),
                    data.len()
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
            PrismRgbModel::Prism8 | PrismRgbModel::Nollie8 => (0..CHANNELS_PRISM_8)
                .map(|index| ProtocolZone {
                    name: format!("Channel {}", index + 1),
                    led_count: u32::try_from(LEDS_PER_PRISM_8_CHANNEL).unwrap_or(u32::MAX),
                    topology: DeviceTopologyHint::Strip,
                    color_format: self.model.color_format(),
                })
                .collect(),
            PrismRgbModel::PrismS => {
                let mut zones = Vec::new();
                if self.prism_s_config.atx_present {
                    zones.push(ProtocolZone {
                        name: "ATX Strimer".to_owned(),
                        led_count: u32::try_from(PRISM_S_ATX_LEDS).unwrap_or(u32::MAX),
                        topology: DeviceTopologyHint::Matrix { rows: 6, cols: 20 },
                        color_format: self.model.color_format(),
                    });
                }
                if let Some(gpu_cable) = self.prism_s_config.gpu_cable {
                    zones.push(ProtocolZone {
                        name: "GPU Strimer".to_owned(),
                        led_count: u32::try_from(gpu_cable.led_count()).unwrap_or(u32::MAX),
                        topology: gpu_cable.topology(),
                        color_format: self.model.color_format(),
                    });
                }
                zones
            }
            PrismRgbModel::PrismMini => vec![ProtocolZone {
                name: "Channel 1".to_owned(),
                led_count: u32::try_from(PRISM_MINI_MAX_LEDS).unwrap_or(u32::MAX),
                topology: DeviceTopologyHint::Strip,
                color_format: self.model.color_format(),
            }],
        }
    }

    fn capabilities(&self) -> DeviceCapabilities {
        DeviceCapabilities {
            led_count: self.total_leds(),
            supports_direct: true,
            supports_brightness: false,
            has_display: false,
            display_resolution: None,
            max_fps: 60,
        }
    }

    fn total_leds(&self) -> u32 {
        u32::try_from(self.expected_leds()).unwrap_or(u32::MAX)
    }

    fn frame_interval(&self) -> Duration {
        Duration::from_millis(16)
    }
}

#[must_use]
pub fn apply_low_power_saver(r: u8, g: u8, b: u8) -> (u8, u8, u8) {
    let total = u16::from(r) + u16::from(g) + u16::from(b);
    if total > LOW_POWER_THRESHOLD {
        let scale = f32::from(LOW_POWER_THRESHOLD) / f32::from(total);
        (
            scale_channel(r, scale),
            scale_channel(g, scale),
            scale_channel(b, scale),
        )
    } else {
        (r, g, b)
    }
}

#[must_use]
pub fn compress_color_pair(led1: (u8, u8, u8), led2: (u8, u8, u8)) -> [u8; 3] {
    let (r1, g1, b1) = led1;
    let (r2, g2, b2) = led2;
    [
        (r1 >> 4) | ((g1 >> 4) << 4),
        (b1 >> 4) | ((r2 >> 4) << 4),
        (g2 >> 4) | ((b2 >> 4) << 4),
    ]
}

fn encode_prism_mini_compressed(colors: &[[u8; 3]]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(colors.len().div_ceil(2) * 3);
    let mut iter = colors.iter().copied();
    while let Some(first) = iter.next() {
        let second = iter.next().unwrap_or([0, 0, 0]);
        let (r1, g1, b1) = apply_low_power_saver(first[0], first[1], first[2]);
        let (r2, g2, b2) = apply_low_power_saver(second[0], second[1], second[2]);
        bytes.extend_from_slice(&compress_color_pair((r1, g1, b1), (r2, g2, b2)));
    }
    bytes
}

fn flatten_colors(colors: &[[u8; 3]], scale: f32, format: DeviceColorFormat) -> Vec<u8> {
    colors
        .iter()
        .flat_map(|color| encode_color(*color, scale, format))
        .collect()
}

fn encode_color(color: [u8; 3], scale: f32, format: DeviceColorFormat) -> [u8; 3] {
    let rs = scale_channel(color[0], scale);
    let gs = scale_channel(color[1], scale);
    let bs = scale_channel(color[2], scale);

    match format {
        DeviceColorFormat::Grb => [gs, rs, bs],
        DeviceColorFormat::Rbg => [rs, bs, gs],
        DeviceColorFormat::Rgb | DeviceColorFormat::Rgbw | DeviceColorFormat::Jpeg => [rs, gs, bs],
    }
}

fn command_from_packet(
    packet: [u8; HID_REPORT_SIZE],
    expects_response: bool,
    post_delay: Duration,
) -> ProtocolCommand {
    ProtocolCommand {
        data: packet.to_vec(),
        expects_response,
        response_delay: Duration::ZERO,
        post_delay,
        transfer_type: TransferType::Primary,
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
