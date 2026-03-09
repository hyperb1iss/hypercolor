//! Native Corsair Lighting Node direct RGB protocol.

use std::borrow::Cow;
use std::time::Duration;

use hypercolor_types::device::{DeviceCapabilities, DeviceColorFormat, DeviceTopologyHint};
use tracing::warn;

use crate::drivers::corsair::CORSAIR_KEEPALIVE_INTERVAL;
use crate::drivers::corsair::framing::LN_WRITE_BUF_SIZE;
use crate::drivers::corsair::types::{
    LightingNodeColorChannel, LightingNodePacketId, LightingNodePortState,
};
use crate::protocol::{
    CommandBuffer, Protocol, ProtocolCommand, ProtocolError, ProtocolKeepalive, ProtocolResponse,
    ProtocolZone, ResponseStatus, TransferType,
};

const MAX_LEDS_PER_CHANNEL: u32 = 204;
const DIRECT_CHUNK_SIZE: usize = 50;
const DEFAULT_TARGET_FPS: u32 = 30;

/// Corsair Lighting Node / Commander Pro direct color protocol.
pub struct CorsairLightingNodeProtocol {
    name: &'static str,
    channel_leds: Vec<u32>,
}

impl CorsairLightingNodeProtocol {
    #[must_use]
    pub fn new(name: &'static str, channel_count: u8) -> Self {
        Self {
            name,
            channel_leds: vec![MAX_LEDS_PER_CHANNEL; usize::from(channel_count)],
        }
    }

    fn total_leds_usize(&self) -> usize {
        self.channel_leds
            .iter()
            .copied()
            .map(|count| usize::try_from(count).unwrap_or_default())
            .sum()
    }

    fn normalize_colors<'a>(&self, colors: &'a [[u8; 3]]) -> Cow<'a, [[u8; 3]]> {
        let expected = self.total_leds_usize();
        if colors.len() == expected {
            return Cow::Borrowed(colors);
        }

        let mut normalized = vec![[0_u8; 3]; expected];
        let copy_len = colors.len().min(expected);
        normalized[..copy_len].copy_from_slice(&colors[..copy_len]);

        warn!(
            expected,
            actual = colors.len(),
            device = self.name,
            "corsair lighting node frame length mismatch; applying truncate/pad"
        );

        Cow::Owned(normalized)
    }

    fn write_packet(buffer: &mut Vec<u8>, packet_id: LightingNodePacketId, payload: &[u8]) {
        buffer.resize(LN_WRITE_BUF_SIZE, 0x00);
        buffer[1] = packet_id.byte();
        let payload_len = payload.len().min(LN_WRITE_BUF_SIZE.saturating_sub(2));
        buffer[2..2 + payload_len].copy_from_slice(&payload[..payload_len]);
    }

    fn firmware_query() -> ProtocolCommand {
        let mut data = Vec::with_capacity(LN_WRITE_BUF_SIZE);
        Self::write_packet(&mut data, LightingNodePacketId::Firmware, &[]);
        ProtocolCommand {
            data,
            expects_response: true,
            response_delay: Duration::ZERO,
            post_delay: Duration::ZERO,
            transfer_type: TransferType::Primary,
        }
    }

    fn commit_packet() -> ProtocolCommand {
        let mut data = Vec::with_capacity(LN_WRITE_BUF_SIZE);
        Self::write_packet(&mut data, LightingNodePacketId::Commit, &[0xFF]);
        ProtocolCommand {
            data,
            expects_response: true,
            response_delay: Duration::ZERO,
            post_delay: Duration::ZERO,
            transfer_type: TransferType::Primary,
        }
    }

    fn port_state_packet(channel: u8, state: LightingNodePortState) -> ProtocolCommand {
        let mut data = Vec::with_capacity(LN_WRITE_BUF_SIZE);
        Self::write_packet(
            &mut data,
            LightingNodePacketId::PortState,
            &[channel, state.byte()],
        );
        ProtocolCommand {
            data,
            expects_response: true,
            response_delay: Duration::ZERO,
            post_delay: Duration::ZERO,
            transfer_type: TransferType::Primary,
        }
    }

    fn brightness_packet(channel: u8, brightness: u8) -> ProtocolCommand {
        let mut data = Vec::with_capacity(LN_WRITE_BUF_SIZE);
        Self::write_packet(
            &mut data,
            LightingNodePacketId::Brightness,
            &[channel, brightness],
        );
        ProtocolCommand {
            data,
            expects_response: true,
            response_delay: Duration::ZERO,
            post_delay: Duration::ZERO,
            transfer_type: TransferType::Primary,
        }
    }
}

impl Protocol for CorsairLightingNodeProtocol {
    fn name(&self) -> &str {
        self.name
    }

    fn init_sequence(&self) -> Vec<ProtocolCommand> {
        vec![Self::firmware_query()]
    }

    fn shutdown_sequence(&self) -> Vec<ProtocolCommand> {
        self.channel_leds
            .iter()
            .enumerate()
            .map(|(channel, _)| {
                Self::port_state_packet(
                    u8::try_from(channel).unwrap_or(u8::MAX),
                    LightingNodePortState::Hardware,
                )
            })
            .collect()
    }

    fn encode_frame(&self, colors: &[[u8; 3]]) -> Vec<ProtocolCommand> {
        let mut commands = Vec::new();
        self.encode_frame_into(colors, &mut commands);
        commands
    }

    fn encode_frame_into(&self, colors: &[[u8; 3]], commands: &mut Vec<ProtocolCommand>) {
        let normalized = self.normalize_colors(colors);
        let mut encoder = CommandBuffer::new(commands);
        let mut offset = 0_usize;

        for (channel_index, &channel_led_count) in self.channel_leds.iter().enumerate() {
            let channel = u8::try_from(channel_index).unwrap_or(u8::MAX);
            let count = usize::try_from(channel_led_count).unwrap_or_default();
            let channel_colors = &normalized[offset..offset + count];
            offset += count;

            encoder.push_fill(
                true,
                Duration::ZERO,
                Duration::ZERO,
                TransferType::Primary,
                |buffer| {
                    Self::write_packet(
                        buffer,
                        LightingNodePacketId::PortState,
                        &[channel, LightingNodePortState::Software.byte()],
                    );
                },
            );

            for (chunk_index, chunk) in channel_colors.chunks(DIRECT_CHUNK_SIZE).enumerate() {
                let start = u8::try_from(chunk_index * DIRECT_CHUNK_SIZE).unwrap_or(u8::MAX);
                let count = u8::try_from(chunk.len()).unwrap_or(u8::MAX);

                for (component, color_channel) in [
                    (0_usize, LightingNodeColorChannel::Red),
                    (1_usize, LightingNodeColorChannel::Green),
                    (2_usize, LightingNodeColorChannel::Blue),
                ] {
                    encoder.push_fill(
                        true,
                        Duration::ZERO,
                        Duration::ZERO,
                        TransferType::Primary,
                        |buffer| {
                            buffer.resize(LN_WRITE_BUF_SIZE, 0x00);
                            buffer[1] = LightingNodePacketId::Direct.byte();
                            buffer[2] = channel;
                            buffer[3] = start;
                            buffer[4] = count;
                            buffer[5] = color_channel.byte();
                            for (index, color) in chunk.iter().enumerate() {
                                buffer[6 + index] = color[component];
                            }
                        },
                    );
                }
            }

            encoder.push_fill(
                true,
                Duration::ZERO,
                Duration::ZERO,
                TransferType::Primary,
                |buffer| Self::write_packet(buffer, LightingNodePacketId::Commit, &[0xFF]),
            );
        }
        encoder.finish();
    }

    fn encode_brightness(&self, brightness: u8) -> Option<Vec<ProtocolCommand>> {
        let scaled = u8::try_from((u16::from(brightness) * 100) / 255).unwrap_or(100);
        Some(
            self.channel_leds
                .iter()
                .enumerate()
                .map(|(channel, _)| {
                    Self::brightness_packet(u8::try_from(channel).unwrap_or(u8::MAX), scaled)
                })
                .collect(),
        )
    }

    fn keepalive(&self) -> Option<ProtocolKeepalive> {
        Some(ProtocolKeepalive {
            commands: vec![Self::commit_packet()],
            interval: CORSAIR_KEEPALIVE_INTERVAL,
        })
    }

    fn parse_response(&self, data: &[u8]) -> Result<ProtocolResponse, ProtocolError> {
        Ok(ProtocolResponse {
            status: ResponseStatus::Ok,
            data: data.get(1..).unwrap_or(data).to_vec(),
        })
    }

    fn zones(&self) -> Vec<ProtocolZone> {
        self.channel_leds
            .iter()
            .enumerate()
            .map(|(index, &led_count)| ProtocolZone {
                name: format!("Channel {}", index + 1),
                led_count,
                topology: DeviceTopologyHint::Strip,
                color_format: DeviceColorFormat::Rgb,
            })
            .collect()
    }

    fn capabilities(&self) -> DeviceCapabilities {
        DeviceCapabilities {
            led_count: self.total_leds(),
            supports_direct: true,
            supports_brightness: true,
            has_display: false,
            display_resolution: None,
            max_fps: DEFAULT_TARGET_FPS,
        }
    }

    fn total_leds(&self) -> u32 {
        self.channel_leds.iter().sum()
    }

    fn frame_interval(&self) -> Duration {
        Duration::from_millis(33)
    }
}
