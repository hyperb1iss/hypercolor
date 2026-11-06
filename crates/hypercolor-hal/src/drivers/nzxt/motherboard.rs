//! NZXT motherboard lighting (Spec 39 §4.4).
//!
//! NZXT and ARGB headers ride the Gen2 stream path. The 12V RGB header is a
//! single fixed-color zone written with the `0x2A` packet.

use hypercolor_types::device::DeviceColorFormat;

use super::gen2::{ChannelDiscovery, Gen2Profile};
use super::packet::FixedColorPacket;
use super::topology::GEN2_MAX_CHANNELS;

/// Bytes 1 through 3 of the 12V RGB header write.
pub const ANALOG_HEADER_TARGET: [u8; 3] = [0x04, 0x08, 0x08];

/// Header counts for one motherboard PID (Spec 39 §4.4 channel model).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MotherboardHeaders {
    /// NZXT digital headers.
    pub nzxt: u8,
    /// Generic 5V ARGB headers.
    pub argb: u8,
    /// 12V analog RGB headers.
    pub analog: u8,
}

impl MotherboardHeaders {
    /// Build a header table entry.
    #[must_use]
    pub const fn new(nzxt: u8, argb: u8, analog: u8) -> Self {
        Self { nzxt, argb, analog }
    }

    /// Digital channels the board can expose on the Gen2 stream path.
    #[must_use]
    pub const fn digital_channels(self) -> u8 {
        self.nzxt.saturating_add(self.argb)
    }
}

/// Byte order for the 12V header color, as listed in Spec 39 §4.4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AnalogColorOrder {
    /// Red, green, blue. The order the §4.4 template is written in.
    #[default]
    Rgb,
    /// Red, blue, green.
    Rbg,
    /// Blue, green, red.
    Bgr,
    /// Blue, red, green.
    Brg,
    /// Green, blue, red.
    Gbr,
    /// Green, red, blue.
    Grb,
}

impl AnalogColorOrder {
    /// Reorder an RGB triple into wire order.
    #[must_use]
    pub const fn permute(self, [r, g, b]: [u8; 3]) -> [u8; 3] {
        match self {
            Self::Rgb => [r, g, b],
            Self::Rbg => [r, b, g],
            Self::Bgr => [b, g, r],
            Self::Brg => [b, r, g],
            Self::Gbr => [g, b, r],
            Self::Grb => [g, r, b],
        }
    }

    /// Closest segment color-format label. The protocol performs the
    /// permutation itself; orders the label vocabulary cannot name report RGB.
    #[must_use]
    pub const fn segment_format(self) -> DeviceColorFormat {
        match self {
            Self::Grb => DeviceColorFormat::Grb,
            Self::Rbg => DeviceColorFormat::Rbg,
            Self::Rgb | Self::Bgr | Self::Brg | Self::Gbr => DeviceColorFormat::Rgb,
        }
    }
}

/// Build the 12V RGB header write for one color.
#[must_use]
pub fn analog_header_packet(color: [u8; 3], order: AnalogColorOrder) -> FixedColorPacket {
    FixedColorPacket::new(ANALOG_HEADER_TARGET, order.permute(color))
}

impl Gen2Profile {
    /// NZXT motherboard. Digital channels come from the topology query and
    /// are capped at the board's NZXT plus ARGB header count; each 12V header
    /// becomes a fixed-color zone.
    ///
    /// Only the first 12V header's target bytes are documented, so at most
    /// one analog header is driven. No board in Spec 39 §4.4 has more.
    #[must_use]
    pub const fn motherboard(headers: MotherboardHeaders) -> Self {
        let digital = headers.digital_channels();
        Self {
            discovery: ChannelDiscovery::TopologyQuery,
            max_channels: if digital < GEN2_MAX_CHANNELS {
                digital
            } else {
                GEN2_MAX_CHANNELS
            },
            channel_led_cap: None,
            kraken_x3_logo: None,
            analog_headers: if headers.analog > 1 {
                1
            } else {
                headers.analog
            },
            analog_color_order: AnalogColorOrder::Rgb,
        }
    }
}
