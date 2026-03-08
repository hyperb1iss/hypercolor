//! Native Corsair Lighting Node direct RGB protocol.

use std::time::Duration;

use hypercolor_types::device::{DeviceCapabilities, DeviceColorFormat, DeviceTopologyHint};
use tracing::warn;

use crate::drivers::corsair::CORSAIR_KEEPALIVE_INTERVAL;
use crate::drivers::corsair::framing::LN_WRITE_BUF_SIZE;
use crate::drivers::corsair::types::{
    LightingNodeColorChannel, LightingNodePacketId, LightingNodePortState,
};
use crate::protocol::{
    Protocol, ProtocolCommand, ProtocolError, ProtocolKeepalive, ProtocolResponse, ProtocolZone,
    ResponseStatus, TransferType,
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

    fn normalize_colors(&self, colors: &[[u8; 3]]) -> Vec<[u8; 3]> {
        let expected = self.total_leds_usize();
        if colors.len() == expected {
            return colors.to_vec();
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

        normalized
    }

    fn packet(
        packet_id: LightingNodePacketId,
        payload: &[u8],
        expects_response: bool,
    ) -> ProtocolCommand {
        let mut packet = vec![0_u8; LN_WRITE_BUF_SIZE];
        packet[1] = packet_id.byte();
        let payload_len = payload.len().min(LN_WRITE_BUF_SIZE.saturating_sub(2));
        packet[2..2 + payload_len].copy_from_slice(&payload[..payload_len]);

        ProtocolCommand {
            data: packet,
            expects_response,
            response_delay: Duration::ZERO,
            post_delay: Duration::ZERO,
            transfer_type: TransferType::Primary,
        }
    }

    fn firmware_query() -> ProtocolCommand {
        Self::packet(LightingNodePacketId::Firmware, &[], true)
    }

    fn direct_packet(
        channel: u8,
        start: u8,
        count: u8,
        color_channel: LightingNodeColorChannel,
        colors: &[u8],
    ) -> ProtocolCommand {
        let mut payload = Vec::with_capacity(colors.len().saturating_add(4));
        payload.extend_from_slice(&[channel, start, count, color_channel.byte()]);
        payload.extend_from_slice(colors);
        Self::packet(LightingNodePacketId::Direct, &payload, true)
    }

    fn commit_packet() -> ProtocolCommand {
        Self::packet(LightingNodePacketId::Commit, &[0xFF], true)
    }

    fn port_state_packet(channel: u8, state: LightingNodePortState) -> ProtocolCommand {
        Self::packet(
            LightingNodePacketId::PortState,
            &[channel, state.byte()],
            true,
        )
    }

    fn brightness_packet(channel: u8, brightness: u8) -> ProtocolCommand {
        Self::packet(
            LightingNodePacketId::Brightness,
            &[channel, brightness],
            true,
        )
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
        let normalized = self.normalize_colors(colors);
        let mut commands = Vec::new();
        let mut offset = 0_usize;

        for (channel_index, &channel_led_count) in self.channel_leds.iter().enumerate() {
            let channel = u8::try_from(channel_index).unwrap_or(u8::MAX);
            let count = usize::try_from(channel_led_count).unwrap_or_default();
            let channel_colors = &normalized[offset..offset + count];
            offset += count;

            commands.push(Self::port_state_packet(
                channel,
                LightingNodePortState::Software,
            ));

            for (chunk_index, chunk) in channel_colors.chunks(DIRECT_CHUNK_SIZE).enumerate() {
                let reds = chunk.iter().map(|color| color[0]).collect::<Vec<_>>();
                let greens = chunk.iter().map(|color| color[1]).collect::<Vec<_>>();
                let blues = chunk.iter().map(|color| color[2]).collect::<Vec<_>>();
                let start = u8::try_from(chunk_index * DIRECT_CHUNK_SIZE).unwrap_or(u8::MAX);
                let count = u8::try_from(chunk.len()).unwrap_or(u8::MAX);

                commands.push(Self::direct_packet(
                    channel,
                    start,
                    count,
                    LightingNodeColorChannel::Red,
                    &reds,
                ));
                commands.push(Self::direct_packet(
                    channel,
                    start,
                    count,
                    LightingNodeColorChannel::Green,
                    &greens,
                ));
                commands.push(Self::direct_packet(
                    channel,
                    start,
                    count,
                    LightingNodeColorChannel::Blue,
                    &blues,
                ));
            }

            commands.push(Self::commit_packet());
        }

        commands
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
