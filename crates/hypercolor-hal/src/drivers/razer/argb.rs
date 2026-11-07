//! Razer Chroma Addressable RGB Controller frame protocol.
//!
//! The controller's six ARGB headers take color frames as a separate
//! 320-byte HID feature report on interface 1, one report per header. The
//! 90-byte Razer report on interface 0 carries hardware effects and
//! brightness, neither of which direct color needs, so this protocol speaks
//! only the frame report. See Spec 17 section 6.3.
//!
//! The controller cannot see what is plugged into a header, so each header
//! is exposed at its full 80-LED capacity and the attachment profile says
//! which LEDs exist.

use std::borrow::Cow;
use std::cmp::min;
use std::time::Duration;

use hypercolor_types::device::{
    DeviceCapabilities, DeviceColorFormat, DeviceColorSpace, DeviceFeatures, DeviceTopologyHint,
    SegmentInfo,
};
use tracing::warn;
use zerocopy::{FromZeros, Immutable, IntoBytes, KnownLayout};

use crate::protocol::{
    CommandBuffer, FrameBatch, Protocol, ProtocolCommand, ProtocolError, ProtocolResponse,
    ResponseStatus, TransferType,
};

/// Frame report length on the wire, excluding the HIDAPI report ID prefix.
pub const ARGB_REPORT_LEN: usize = 320;

/// ARGB headers on the controller.
pub const ARGB_CHANNELS: usize = 6;

/// LEDs each header addresses.
pub const ARGB_LEDS_PER_CHANNEL: usize = 80;

/// Color bytes the report carries (105 LEDs), of which a header uses 240.
const ARGB_COLOR_CAPACITY: usize = 315;

/// Report selector for headers 0 through 4.
const SELECTOR_LOW_CHANNELS: u8 = 0x04;

/// Report selector for header 5.
const SELECTOR_HIGH_CHANNEL: u8 = 0x84;

/// First header that takes the high selector.
const FIRST_HIGH_CHANNEL: usize = 5;

/// Pause after each header's report, matching OpenRGB's per-header pacing.
const CHANNEL_POST_DELAY: Duration = Duration::from_millis(1);

const TOTAL_LEDS: usize = ARGB_CHANNELS * ARGB_LEDS_PER_CHANNEL;

const _: () = assert!(
    std::mem::size_of::<RazerArgbReport>() == ARGB_REPORT_LEN,
    "RazerArgbReport must match the 320-byte frame report"
);

const _: () = assert!(
    ARGB_LEDS_PER_CHANNEL * 3 <= ARGB_COLOR_CAPACITY,
    "a full header must fit in one frame report"
);

/// Wire-format frame report for one ARGB header (320 bytes).
#[derive(FromZeros, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
struct RazerArgbReport {
    /// `0x04` for headers 0 through 4, `0x84` for header 5.
    selector: u8,
    /// Header index, 0 through 5.
    channel: u8,
    /// Header index, repeated.
    channel_repeat: u8,
    /// Always `0x00`.
    pad: u8,
    /// Index of the last LED carried, which is the LED count minus one.
    last_index: u8,
    /// Red, green, blue per LED, zero-padded past the last LED.
    colors: [u8; ARGB_COLOR_CAPACITY],
}

impl RazerArgbReport {
    fn for_channel(channel: usize, colors: &[[u8; 3]; ARGB_LEDS_PER_CHANNEL]) -> Self {
        let mut report = Self::new_zeroed();
        let channel_byte = u8::try_from(channel).expect("ARGB channel index fits in u8");
        report.selector = if channel >= FIRST_HIGH_CHANNEL {
            SELECTOR_HIGH_CHANNEL
        } else {
            SELECTOR_LOW_CHANNELS
        };
        report.channel = channel_byte;
        report.channel_repeat = channel_byte;
        report.last_index =
            u8::try_from(ARGB_LEDS_PER_CHANNEL - 1).expect("ARGB channel LED count fits in u8");
        let (slots, _) = report.colors.as_chunks_mut::<3>();
        for (slot, color) in slots.iter_mut().zip(colors) {
            *slot = *color;
        }
        report
    }
}

/// Write-only frame encoder for the Chroma Addressable RGB Controller.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RazerArgbProtocol;

impl RazerArgbProtocol {
    /// Create the encoder.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    fn normalize_colors(colors: &[[u8; 3]]) -> Cow<'_, [[u8; 3]]> {
        if colors.len() == TOTAL_LEDS {
            return Cow::Borrowed(colors);
        }

        warn!(
            expected = TOTAL_LEDS,
            actual = colors.len(),
            "razer argb frame length mismatch; applying truncate/pad"
        );

        let mut normalized = vec![[0_u8; 3]; TOTAL_LEDS];
        let copy_len = min(colors.len(), TOTAL_LEDS);
        normalized[..copy_len].copy_from_slice(&colors[..copy_len]);
        Cow::Owned(normalized)
    }
}

impl Protocol for RazerArgbProtocol {
    fn name(&self) -> &'static str {
        "Razer Chroma ARGB"
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
        let normalized = Self::normalize_colors(colors);
        let mut buffer = CommandBuffer::new(commands);
        let (channels, _) = normalized.as_chunks::<ARGB_LEDS_PER_CHANNEL>();
        for (channel, channel_colors) in channels.iter().enumerate() {
            buffer.push_struct(
                &RazerArgbReport::for_channel(channel, channel_colors),
                false,
                Duration::ZERO,
                CHANNEL_POST_DELAY,
                TransferType::Primary,
            );
        }
        buffer.finish();
        FrameBatch::from_commands(commands)
    }

    fn encode_brightness(&self, _brightness: u8) -> Option<Vec<ProtocolCommand>> {
        None
    }

    fn parse_response(&self, _data: &[u8]) -> Result<ProtocolResponse, ProtocolError> {
        Ok(ProtocolResponse {
            status: ResponseStatus::Ok,
            data: Vec::new(),
        })
    }

    fn zones(&self) -> Vec<SegmentInfo> {
        (1..=ARGB_CHANNELS)
            .map(|channel| SegmentInfo {
                name: format!("Channel {channel}"),
                led_count: u32::try_from(ARGB_LEDS_PER_CHANNEL).unwrap_or(u32::MAX),
                topology: DeviceTopologyHint::Strip,
                color_format: DeviceColorFormat::Rgb,
                layout_hint: None,
            })
            .collect()
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let interval_ms = self.frame_interval().as_millis().max(1);
        let max_fps = u32::try_from(1_000 / interval_ms).unwrap_or(u32::MAX);

        DeviceCapabilities {
            led_count: self.total_leds(),
            supports_direct: true,
            supports_brightness: false,
            has_display: false,
            display_resolution: None,
            max_fps,
            color_space: DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        }
    }

    fn total_leds(&self) -> u32 {
        u32::try_from(TOTAL_LEDS).unwrap_or(u32::MAX)
    }

    fn frame_interval(&self) -> Duration {
        // One paced report per header, plus a millisecond of slack.
        let channels = u32::try_from(ARGB_CHANNELS).unwrap_or(u32::MAX);
        CHANNEL_POST_DELAY
            .saturating_mul(channels)
            .saturating_add(Duration::from_millis(1))
    }
}
