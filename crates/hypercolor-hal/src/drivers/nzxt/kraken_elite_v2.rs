//! Kraken Elite v2 lighting (Spec 39 §4.6.4).
//!
//! Same commit-barrier shape as Gen2, but 512-byte reports and the `0x26`
//! command family. Phase 1 drives the 24-LED pump ring only: the external
//! channel write has no documented commit packet (Spec 39 §9.4).

use std::borrow::Cow;
use std::time::Duration;

use hypercolor_types::device::{
    DeviceCapabilities, DeviceColorFormat, DeviceColorSpace, DeviceFeatures, DeviceTopologyHint,
    SegmentInfo,
};
use tracing::debug;
use zerocopy::FromZeros;

use crate::protocol::{
    CommandBuffer, FrameBatch, Protocol, ProtocolCommand, ProtocolError, ProtocolResponse,
    ResponseStatus, TransferType,
};

use super::gen2::GEN2_FRAME_INTERVAL;
use super::packet::{
    ELITE_V2_RING_HEADER, ELITE_V2_RING_LEDS, EliteV2ApplyPacket, EliteV2RingPacket, fill_grb,
};

const MAX_FPS: u32 = 30;

/// Kraken Elite v2 pump ring protocol.
pub struct NzxtKrakenEliteV2Protocol {
    name: &'static str,
}

impl NzxtKrakenEliteV2Protocol {
    /// Build a protocol for one Elite v2 device.
    #[must_use]
    pub const fn new(name: &'static str) -> Self {
        Self { name }
    }

    fn normalize_colors<'a>(&self, colors: &'a [[u8; 3]]) -> Cow<'a, [[u8; 3]]> {
        if colors.len() == ELITE_V2_RING_LEDS {
            return Cow::Borrowed(colors);
        }

        debug!(
            device = self.name,
            expected = ELITE_V2_RING_LEDS,
            actual = colors.len(),
            "NZXT Kraken Elite v2 frame length mismatch; padding or truncating"
        );
        let mut normalized = vec![[0_u8; 3]; ELITE_V2_RING_LEDS];
        let copy_len = colors.len().min(ELITE_V2_RING_LEDS);
        normalized[..copy_len].copy_from_slice(&colors[..copy_len]);
        Cow::Owned(normalized)
    }
}

impl Protocol for NzxtKrakenEliteV2Protocol {
    fn name(&self) -> &'static str {
        self.name
    }

    fn init_sequence(&self) -> Vec<ProtocolCommand> {
        Vec::new()
    }

    fn shutdown_sequence(&self) -> Vec<ProtocolCommand> {
        Vec::new()
    }

    fn encode_frame(&self, colors: &[[u8; 3]]) -> Vec<ProtocolCommand> {
        let mut commands = Vec::new();
        self.encode_frame_into(colors, &mut commands);
        commands
    }

    fn encode_frame_into(
        &self,
        colors: &[[u8; 3]],
        commands: &mut Vec<ProtocolCommand>,
    ) -> FrameBatch {
        let normalized = self.normalize_colors(colors);
        let mut buffer = CommandBuffer::new(commands);

        let mut ring = EliteV2RingPacket::new_zeroed();
        ring.header = ELITE_V2_RING_HEADER;
        fill_grb(&mut ring.colors, &normalized);
        buffer.push_struct(
            &ring,
            false,
            Duration::ZERO,
            Duration::ZERO,
            TransferType::Primary,
        );
        buffer.push_struct(
            &EliteV2ApplyPacket::new(),
            false,
            Duration::ZERO,
            Duration::ZERO,
            TransferType::Primary,
        );

        buffer.finish();
        FrameBatch::from_commands(commands)
    }

    fn parse_response(&self, data: &[u8]) -> Result<ProtocolResponse, ProtocolError> {
        // No Phase 1 command expects a reply; status reports are not ours.
        Ok(ProtocolResponse {
            status: ResponseStatus::Unsupported,
            data: data.to_vec(),
        })
    }

    fn zones(&self) -> Vec<SegmentInfo> {
        vec![SegmentInfo {
            name: "Pump Ring".to_owned(),
            led_count: self.total_leds(),
            topology: DeviceTopologyHint::Ring {
                count: self.total_leds(),
            },
            color_format: DeviceColorFormat::Grb,
            layout_hint: None,
        }]
    }

    fn capabilities(&self) -> DeviceCapabilities {
        DeviceCapabilities {
            led_count: self.total_leds(),
            supports_direct: true,
            supports_brightness: false,
            has_display: false,
            display_resolution: None,
            max_fps: MAX_FPS,
            color_space: DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        }
    }

    fn total_leds(&self) -> u32 {
        u32::try_from(ELITE_V2_RING_LEDS).unwrap_or(u32::MAX)
    }

    fn frame_interval(&self) -> Duration {
        GEN2_FRAME_INTERVAL
    }
}
