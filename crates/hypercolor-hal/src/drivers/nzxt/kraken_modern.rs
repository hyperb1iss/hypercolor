//! Kraken X3 and Z3 lighting (Spec 39 §4.6.1, §4.6.2).
//!
//! Both run the Gen2 `0x22` stream and apply packets, so they are
//! [`NzxtGen2Protocol`](super::gen2::NzxtGen2Protocol) profiles rather than a
//! separate protocol type. This module owns what differs: the external
//! channel cap, the Z3's fixed channel, and the X3 logo write.

use super::gen2::{ChannelDiscovery, Gen2Profile};
use super::motherboard::AnalogColorOrder;
use super::packet::{FixedColorPacket, grb};
use super::topology::GEN2_MAX_CHANNELS;

/// LED cap on a Kraken external channel.
pub const KRAKEN_EXTERNAL_LED_CAP: u32 = 40;

/// The Kraken Z3's single external channel.
pub const KRAKEN_Z3_CHANNEL: u8 = 0;

/// Bytes 1 through 3 of the Kraken X3 logo write.
pub const KRAKEN_X3_LOGO_TARGET: [u8; 3] = [0x04, 0x04, 0x04];

/// How a Kraken X3 PID drives its logo LED.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KrakenX3Logo {
    /// The `0x2A 04 04 04` fixed-color write (`0x2014`).
    FixedColorWrite,
    /// The logo packet for this PID is unresolved (Spec 39 §9.4), so the
    /// logo is not driven. Its channel is still kept out of the stream.
    Unresolved,
}

/// Build the Kraken X3 logo write for one color. The color goes out as G, R, B.
#[must_use]
pub fn kraken_x3_logo_packet(color: [u8; 3]) -> FixedColorPacket {
    FixedColorPacket::new(KRAKEN_X3_LOGO_TARGET, grb(color))
}

impl Gen2Profile {
    /// Kraken X3 (`0x2007`, `0x2014`). The pump ring and external channels
    /// come from the topology query; every streamed channel is capped at
    /// [`KRAKEN_EXTERNAL_LED_CAP`]. The channel carrying the logo accessory is
    /// never streamed.
    #[must_use]
    pub const fn kraken_x3(logo: KrakenX3Logo) -> Self {
        Self {
            discovery: ChannelDiscovery::TopologyQuery,
            max_channels: GEN2_MAX_CHANNELS,
            channel_led_cap: Some(KRAKEN_EXTERNAL_LED_CAP),
            kraken_x3_logo: Some(logo),
            analog_headers: 0,
            analog_color_order: AnalogColorOrder::Rgb,
        }
    }

    /// Kraken Z3 (`0x3008`): one 40-LED external channel on channel 0, no
    /// topology query.
    #[must_use]
    pub const fn kraken_z3() -> Self {
        Self {
            discovery: ChannelDiscovery::Fixed {
                channel: KRAKEN_Z3_CHANNEL,
                leds: KRAKEN_EXTERNAL_LED_CAP,
            },
            max_channels: 1,
            channel_led_cap: Some(KRAKEN_EXTERNAL_LED_CAP),
            kraken_x3_logo: None,
            analog_headers: 0,
            analog_color_order: AnalogColorOrder::Rgb,
        }
    }
}
