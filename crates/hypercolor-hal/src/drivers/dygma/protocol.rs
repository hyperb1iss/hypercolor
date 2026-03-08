//! Pure Dygma Focus protocol encoder/decoder.

use std::cmp::min;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::time::Duration;

use hypercolor_types::device::{DeviceCapabilities, DeviceColorFormat, DeviceTopologyHint};
use tracing::warn;

use crate::protocol::{
    Protocol, ProtocolCommand, ProtocolError, ProtocolResponse, ProtocolZone, ResponseStatus,
    TransferType,
};

const TOTAL_LEDS: usize = 176;
const LEFT_KEYS: u32 = 35;
const RIGHT_KEYS: u32 = 35;
const LEFT_UNDERGLOW: u32 = 53;
const RIGHT_UNDERGLOW: u32 = 53;
const COLOR_MODE_RGB: u8 = 3;
const COLOR_MODE_RGBW: u8 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DygmaVariant {
    DefyWired,
    DefyWireless,
}

impl DygmaVariant {
    #[must_use]
    pub const fn device_name(self) -> &'static str {
        match self {
            Self::DefyWired => "Dygma Defy",
            Self::DefyWireless => "Dygma Defy Wireless",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusColorMode {
    Rgb,
    Rgbw,
}

impl FocusColorMode {
    #[must_use]
    pub const fn channel_count(self) -> usize {
        match self {
            Self::Rgb => 3,
            Self::Rgbw => 4,
        }
    }

    #[must_use]
    pub const fn device_color_format(self) -> DeviceColorFormat {
        match self {
            Self::Rgb => DeviceColorFormat::Rgb,
            Self::Rgbw => DeviceColorFormat::Rgbw,
        }
    }

    #[must_use]
    pub const fn from_probe_channels(channels: usize) -> Option<Self> {
        match channels {
            3 => Some(Self::Rgb),
            4 => Some(Self::Rgbw),
            _ => None,
        }
    }
}

/// Pure text protocol encoder/decoder for Dygma Focus devices.
pub struct DygmaProtocol {
    variant: DygmaVariant,
    color_mode: AtomicU8,
    direct_stream_warned: AtomicBool,
}

impl DygmaProtocol {
    #[must_use]
    pub const fn new(variant: DygmaVariant) -> Self {
        Self {
            variant,
            // Default to the documented Focus RGB format until the `led.at 0`
            // probe proves that the device expects RGBW values on the wire.
            color_mode: AtomicU8::new(COLOR_MODE_RGB),
            direct_stream_warned: AtomicBool::new(false),
        }
    }

    #[must_use]
    pub fn color_mode(&self) -> FocusColorMode {
        match self.color_mode.load(Ordering::Acquire) {
            COLOR_MODE_RGB => FocusColorMode::Rgb,
            _ => FocusColorMode::Rgbw,
        }
    }

    fn set_color_mode(&self, mode: FocusColorMode) {
        let stored = match mode {
            FocusColorMode::Rgb => COLOR_MODE_RGB,
            FocusColorMode::Rgbw => COLOR_MODE_RGBW,
        };
        self.color_mode.store(stored, Ordering::Release);
    }

    fn normalize_colors(&self, colors: &[[u8; 3]]) -> Vec<[u8; 3]> {
        if colors.len() == TOTAL_LEDS {
            return colors.to_vec();
        }

        let mut normalized = vec![[0_u8; 3]; TOTAL_LEDS];
        let copy_len = min(colors.len(), TOTAL_LEDS);
        normalized[..copy_len].copy_from_slice(&colors[..copy_len]);

        warn!(
            expected = TOTAL_LEDS,
            actual = colors.len(),
            device = self.variant.device_name(),
            "dygma frame length mismatch; applying truncate/pad"
        );

        normalized
    }

    fn text_command(
        command: impl Into<String>,
        expects_response: bool,
        post_delay: Duration,
    ) -> ProtocolCommand {
        let mut data = command.into().into_bytes();
        data.push(b'\n');
        ProtocolCommand {
            data,
            expects_response,
            response_delay: Duration::ZERO,
            post_delay,
            transfer_type: TransferType::Primary,
        }
    }

    fn black_frame_command(post_delay: Duration) -> ProtocolCommand {
        Self::text_command("led.setAll 0 0 0", true, post_delay)
    }

    fn update_color_mode_from_response(&self, response: &str) {
        let values = response
            .split_whitespace()
            .map(str::parse::<u8>)
            .collect::<Result<Vec<_>, _>>();

        let Ok(values) = values else {
            return;
        };

        if let Some(mode) = FocusColorMode::from_probe_channels(values.len()) {
            self.set_color_mode(mode);
        }
    }
}

impl Protocol for DygmaProtocol {
    fn name(&self) -> &str {
        self.variant.device_name()
    }

    fn init_sequence(&self) -> Vec<ProtocolCommand> {
        vec![
            Self::text_command("hardware.chip_id", true, Duration::ZERO),
            Self::text_command("hardware.firmware", true, Duration::ZERO),
            Self::text_command("led.at 0", true, Duration::ZERO),
            Self::text_command("led.fade 0", true, Duration::ZERO),
            Self::text_command("led.mode 0", true, Duration::ZERO),
            Self::black_frame_command(Duration::from_millis(50)),
        ]
    }

    fn shutdown_sequence(&self) -> Vec<ProtocolCommand> {
        vec![
            Self::black_frame_command(Duration::ZERO),
            Self::text_command("led.fade 1", true, Duration::ZERO),
            Self::text_command("led.mode 0", true, Duration::ZERO),
        ]
    }

    fn encode_frame(&self, colors: &[[u8; 3]]) -> Vec<ProtocolCommand> {
        let _ = self.normalize_colors(colors);

        if !self.direct_stream_warned.swap(true, Ordering::AcqRel) {
            warn!(
                device = self.variant.device_name(),
                "stock Defy firmware does not expose a non-persistent direct LED streaming path; ignoring live frame writes"
            );
        }

        Vec::new()
    }

    fn encode_brightness(&self, brightness: u8) -> Option<Vec<ProtocolCommand>> {
        let (keys, underglow) = match self.variant {
            DygmaVariant::DefyWired => ("led.brightness", "led.brightnessUG"),
            DygmaVariant::DefyWireless => ("led.brightness.wireless", "led.brightnessUG.wireless"),
        };

        Some(vec![
            Self::text_command(format!("{keys} {brightness}"), true, Duration::ZERO),
            Self::text_command(format!("{underglow} {brightness}"), true, Duration::ZERO),
        ])
    }

    fn parse_response(&self, data: &[u8]) -> Result<ProtocolResponse, ProtocolError> {
        let text = std::str::from_utf8(data).map_err(|error| ProtocolError::MalformedResponse {
            detail: format!("invalid UTF-8: {error}"),
        })?;
        let trimmed = text.trim();
        self.update_color_mode_from_response(trimmed);

        Ok(ProtocolResponse {
            status: ResponseStatus::Ok,
            data: trimmed.as_bytes().to_vec(),
        })
    }

    fn response_timeout(&self) -> Duration {
        Duration::from_millis(2_000)
    }

    fn zones(&self) -> Vec<ProtocolZone> {
        let color_format = self.color_mode().device_color_format();
        vec![
            ProtocolZone {
                name: "Left Keys".to_owned(),
                led_count: LEFT_KEYS,
                topology: DeviceTopologyHint::Custom,
                color_format,
            },
            ProtocolZone {
                name: "Right Keys".to_owned(),
                led_count: RIGHT_KEYS,
                topology: DeviceTopologyHint::Custom,
                color_format,
            },
            ProtocolZone {
                name: "Left Underglow".to_owned(),
                led_count: LEFT_UNDERGLOW,
                topology: DeviceTopologyHint::Strip,
                color_format,
            },
            ProtocolZone {
                name: "Right Underglow".to_owned(),
                led_count: RIGHT_UNDERGLOW,
                topology: DeviceTopologyHint::Strip,
                color_format,
            },
        ]
    }

    fn capabilities(&self) -> DeviceCapabilities {
        DeviceCapabilities {
            led_count: self.total_leds(),
            supports_direct: false,
            supports_brightness: true,
            has_display: false,
            display_resolution: None,
            max_fps: 10,
        }
    }

    fn total_leds(&self) -> u32 {
        u32::try_from(TOTAL_LEDS).unwrap_or(u32::MAX)
    }

    fn frame_interval(&self) -> Duration {
        Duration::from_millis(100)
    }
}

#[must_use]
pub fn rgb_to_rgbw(r: u8, g: u8, b: u8) -> (u8, u8, u8, u8) {
    let w = r.min(g).min(b);
    (r - w, g - w, b - w, w)
}
