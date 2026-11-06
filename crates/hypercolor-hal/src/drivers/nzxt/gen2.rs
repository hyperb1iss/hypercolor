//! Gen2 NZXT HID lighting core (Spec 39 §4.3).
//!
//! One protocol type drives every device on the `0x22` stream and apply
//! packets: Gen2 controllers, NZXT motherboards, and the Kraken X3 and Z3. A
//! [`Gen2Profile`] carries what differs between them. Profiles that discover
//! their topology rebuild the LED layout from the `0x21 0x03` reply.

use std::borrow::Cow;
use std::sync::RwLock;
use std::time::Duration;

use hypercolor_types::device::{
    DeviceCapabilities, DeviceColorFormat, DeviceColorSpace, DeviceFeatures, DeviceTopologyHint,
    SegmentInfo,
};
use tracing::debug;
use zerocopy::{FromZeros, IntoBytes};

use crate::protocol::{
    CommandBuffer, Protocol, ProtocolCommand, ProtocolError, ProtocolResponse, ResponseStatus,
    TransferType,
};

use super::kraken_modern::{KRAKEN_EXTERNAL_LED_CAP, KrakenX3Logo, kraken_x3_logo_packet};
use super::motherboard::{AnalogColorOrder, analog_header_packet};
use super::packet::{
    GEN2_LEDS_PER_PACKET, GEN2_MAX_PACKETS_PER_CHANNEL, GEN2_STREAM_COMMAND,
    GEN2_STREAM_SUBCOMMAND, GEN2_TOPOLOGY_QUERY, Gen2ApplyPacket, Gen2QueryPacket,
    Gen2StreamPacket, fill_grb,
};
use super::topology::{
    ACCESSORY_KRAKEN_X3_LOGO, ACCESSORY_KRAKEN_X3_RING, GEN2_MAX_CHANNELS, Gen2Channel,
    Gen2Topology, MAX_ACCESSORY_LEDS, TOPOLOGY_SLOTS_PER_CHANNEL, is_topology_response,
    parse_topology_response,
};

/// Target frame interval. No NZXT family documents its pacing (Spec 39 §9.4),
/// so this is a 30 FPS default pending hardware.
pub const GEN2_FRAME_INTERVAL: Duration = Duration::from_millis(33);

const MAX_FPS: u32 = 30;

/// LEDs one channel can carry before the 4-bit packet number overflows.
const MAX_STREAM_LEDS: usize = GEN2_LEDS_PER_PACKET * GEN2_MAX_PACKETS_PER_CHANNEL;

const _: () = assert!(
    TOPOLOGY_SLOTS_PER_CHANNEL * MAX_ACCESSORY_LEDS <= MAX_STREAM_LEDS,
    "a fully populated topology channel fits the 4-bit packet number"
);
const _: () = assert!(
    KRAKEN_EXTERNAL_LED_CAP as usize <= MAX_STREAM_LEDS,
    "the Kraken external cap fits the 4-bit packet number"
);

/// How a profile learns which channels carry LEDs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelDiscovery {
    /// Send `0x20 0x03` at connect and build the layout from the reply.
    TopologyQuery,
    /// One channel with a fixed LED count; nothing is queried.
    Fixed {
        /// Channel index (the stream mask is `1 << channel`).
        channel: u8,
        /// LEDs streamed on that channel.
        leds: u32,
    },
}

/// Per-family policy for [`NzxtGen2Protocol`].
///
/// The controller profile lives here; `motherboard.rs` and
/// `kraken_modern.rs` add the motherboard and Kraken profiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gen2Profile {
    pub(super) discovery: ChannelDiscovery,
    pub(super) max_channels: u8,
    pub(super) channel_led_cap: Option<u32>,
    pub(super) kraken_x3_logo: Option<KrakenX3Logo>,
    pub(super) analog_headers: u8,
    pub(super) analog_color_order: AnalogColorOrder,
}

impl Gen2Profile {
    /// Gen2 lighting controller: Hue 2, Smart Device V2, RGB and Fan
    /// Controller, Control Hub.
    #[must_use]
    pub const fn controller() -> Self {
        Self {
            discovery: ChannelDiscovery::TopologyQuery,
            max_channels: GEN2_MAX_CHANNELS,
            channel_led_cap: None,
            kraken_x3_logo: None,
            analog_headers: 0,
            analog_color_order: AnalogColorOrder::Rgb,
        }
    }

    /// Override the 12V header byte order.
    #[must_use]
    pub const fn with_analog_color_order(mut self, order: AnalogColorOrder) -> Self {
        self.analog_color_order = order;
        self
    }

    /// How this profile finds its channels.
    #[must_use]
    pub const fn discovery(&self) -> ChannelDiscovery {
        self.discovery
    }

    /// Highest channel count this profile streams to.
    #[must_use]
    pub const fn max_channels(&self) -> u8 {
        self.max_channels
    }

    /// Per-channel LED cap, if the family documents one.
    #[must_use]
    pub const fn channel_led_cap(&self) -> Option<u32> {
        self.channel_led_cap
    }

    /// Kraken X3 logo handling, for Kraken X3 profiles.
    #[must_use]
    pub const fn kraken_x3_logo(&self) -> Option<KrakenX3Logo> {
        self.kraken_x3_logo
    }

    /// 12V analog RGB headers driven as fixed-color zones.
    #[must_use]
    pub const fn analog_headers(&self) -> u8 {
        self.analog_headers
    }

    /// Byte order used for the 12V header color.
    #[must_use]
    pub const fn analog_color_order(&self) -> AnalogColorOrder {
        self.analog_color_order
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FixedZone {
    AnalogHeader,
    KrakenX3Logo,
}

#[derive(Debug, Clone, Copy)]
struct StreamChannel {
    mask: u8,
    leds: usize,
}

#[derive(Debug, Clone, Default)]
struct Gen2Layout {
    channels: Vec<StreamChannel>,
    fixed: Vec<FixedZone>,
    segments: Vec<SegmentInfo>,
    total_leds: usize,
}

impl Gen2Layout {
    fn build(profile: &Gen2Profile, topology: Option<&Gen2Topology>) -> Self {
        let mut layout = Self::default();

        match profile.discovery {
            ChannelDiscovery::Fixed { channel, leds } => {
                let leds = profile.channel_led_cap.map_or(leds, |cap| leds.min(cap));
                layout.push_channel(channel, usize::try_from(leds).unwrap_or_default());
                layout.segments.push(SegmentInfo {
                    name: "External Channel".to_owned(),
                    led_count: leds,
                    topology: DeviceTopologyHint::Strip,
                    color_format: DeviceColorFormat::Grb,
                    layout_hint: None,
                });
            }
            ChannelDiscovery::TopologyQuery => {
                for channel in topology.iter().flat_map(|topology| &topology.channels) {
                    layout.push_topology_channel(profile, channel);
                }
            }
        }

        for _ in 0..profile.analog_headers {
            layout.push_fixed(
                FixedZone::AnalogHeader,
                "12V RGB Header",
                profile.analog_color_order.segment_format(),
            );
        }
        if profile.kraken_x3_logo == Some(KrakenX3Logo::FixedColorWrite) {
            layout.push_fixed(FixedZone::KrakenX3Logo, "Logo", DeviceColorFormat::Grb);
        }

        layout
    }

    fn push_channel(&mut self, index: u8, leds: usize) {
        self.channels.push(StreamChannel {
            mask: 1_u8 << index,
            leds,
        });
        self.total_leds += leds;
    }

    fn push_topology_channel(&mut self, profile: &Gen2Profile, channel: &Gen2Channel) {
        if channel.index >= profile.max_channels {
            debug!(
                channel = channel.index,
                max_channels = profile.max_channels,
                "NZXT channel is beyond this profile's digital channel count; not streaming it"
            );
            return;
        }
        if profile.kraken_x3_logo.is_some() && channel.carries(ACCESSORY_KRAKEN_X3_LOGO) {
            // The X3 logo goes through the separate fixed-color write.
            return;
        }

        let cap = profile
            .channel_led_cap
            .map_or(usize::MAX, |cap| usize::try_from(cap).unwrap_or(usize::MAX));
        let mut kept = 0_usize;
        let mut leds = 0_usize;
        for accessory in &channel.accessories {
            let next = leds + usize::from(accessory.leds);
            if next > cap {
                debug!(
                    channel = channel.index,
                    cap,
                    dropped = channel.accessories.len() - kept,
                    "NZXT channel exceeds its LED cap; dropping the accessories past it"
                );
                break;
            }
            leds = next;
            kept += 1;
        }
        if leds == 0 {
            return;
        }

        let accessories = &channel.accessories[..kept];
        for (slot, accessory) in accessories.iter().enumerate() {
            let name = if accessory.id == ACCESSORY_KRAKEN_X3_RING {
                "Pump Ring".to_owned()
            } else if accessories.len() == 1 {
                format!("Channel {} {}", channel.index + 1, accessory.name)
            } else {
                format!(
                    "Channel {} {} {}",
                    channel.index + 1,
                    accessory.name,
                    slot + 1
                )
            };
            self.segments.push(SegmentInfo {
                name,
                led_count: u32::from(accessory.leds),
                topology: accessory.topology_hint(),
                color_format: DeviceColorFormat::Grb,
                layout_hint: None,
            });
        }
        self.push_channel(channel.index, leds);
    }

    fn push_fixed(&mut self, zone: FixedZone, name: &str, color_format: DeviceColorFormat) {
        self.fixed.push(zone);
        self.segments.push(SegmentInfo {
            name: name.to_owned(),
            led_count: 1,
            topology: DeviceTopologyHint::Point,
            color_format,
            layout_hint: None,
        });
        self.total_leds += 1;
    }
}

/// Gen2 NZXT HID lighting protocol.
pub struct NzxtGen2Protocol {
    name: &'static str,
    profile: Gen2Profile,
    layout: RwLock<Gen2Layout>,
}

impl NzxtGen2Protocol {
    /// Build a protocol for one device with its family profile.
    #[must_use]
    pub fn new(name: &'static str, profile: Gen2Profile) -> Self {
        Self {
            name,
            profile,
            layout: RwLock::new(Gen2Layout::build(&profile, None)),
        }
    }

    /// The profile this protocol was built with.
    #[must_use]
    pub const fn profile(&self) -> &Gen2Profile {
        &self.profile
    }

    fn read_layout(&self) -> std::sync::RwLockReadGuard<'_, Gen2Layout> {
        self.layout
            .read()
            .expect("NZXT Gen2 layout lock should not be poisoned")
    }

    fn normalize_colors<'a>(&self, colors: &'a [[u8; 3]], expected: usize) -> Cow<'a, [[u8; 3]]> {
        if colors.len() == expected {
            return Cow::Borrowed(colors);
        }

        debug!(
            device = self.name,
            expected,
            actual = colors.len(),
            "NZXT Gen2 frame length mismatch; padding or truncating"
        );
        let mut normalized = vec![[0_u8; 3]; expected];
        let copy_len = colors.len().min(expected);
        normalized[..copy_len].copy_from_slice(&colors[..copy_len]);
        Cow::Owned(normalized)
    }
}

fn encode_channel_into(buffer: &mut CommandBuffer<'_>, mask: u8, colors: &[[u8; 3]]) {
    for (packet_index, chunk) in colors.chunks(GEN2_LEDS_PER_PACKET).enumerate() {
        let mut packet = Gen2StreamPacket::new_zeroed();
        packet.command = GEN2_STREAM_COMMAND;
        packet.packet = GEN2_STREAM_SUBCOMMAND
            | u8::try_from(packet_index)
                .ok()
                .filter(|index| usize::from(*index) < GEN2_MAX_PACKETS_PER_CHANNEL)
                .expect("NZXT layouts keep every channel within 16 stream packets");
        packet.channel_mask = mask;
        fill_grb(&mut packet.colors, chunk);
        buffer.push_struct(
            &packet,
            false,
            Duration::ZERO,
            Duration::ZERO,
            TransferType::Primary,
        );
    }

    buffer.push_struct(
        &Gen2ApplyPacket::new(mask),
        false,
        Duration::ZERO,
        Duration::ZERO,
        TransferType::Primary,
    );
}

impl Protocol for NzxtGen2Protocol {
    fn name(&self) -> &'static str {
        self.name
    }

    fn init_sequence(&self) -> Vec<ProtocolCommand> {
        match self.profile.discovery {
            ChannelDiscovery::TopologyQuery => vec![ProtocolCommand {
                data: Gen2QueryPacket::new(GEN2_TOPOLOGY_QUERY)
                    .as_bytes()
                    .to_vec(),
                expects_response: true,
                transfer_type: TransferType::Primary,
                ..Default::default()
            }],
            ChannelDiscovery::Fixed { .. } => Vec::new(),
        }
    }

    fn shutdown_sequence(&self) -> Vec<ProtocolCommand> {
        Vec::new()
    }

    fn encode_frame(&self, colors: &[[u8; 3]]) -> Vec<ProtocolCommand> {
        let mut commands = Vec::new();
        self.encode_frame_into(colors, &mut commands);
        commands
    }

    fn encode_frame_into(&self, colors: &[[u8; 3]], commands: &mut Vec<ProtocolCommand>) {
        let layout = self.read_layout();
        let normalized = self.normalize_colors(colors, layout.total_leds);
        let mut buffer = CommandBuffer::new(commands);
        let mut cursor = 0_usize;

        for channel in &layout.channels {
            let end = cursor + channel.leds;
            encode_channel_into(&mut buffer, channel.mask, &normalized[cursor..end]);
            cursor = end;
        }

        for zone in &layout.fixed {
            let color = normalized[cursor];
            cursor += 1;
            let packet = match zone {
                FixedZone::AnalogHeader => {
                    analog_header_packet(color, self.profile.analog_color_order)
                }
                FixedZone::KrakenX3Logo => kraken_x3_logo_packet(color),
            };
            buffer.push_struct(
                &packet,
                false,
                Duration::ZERO,
                Duration::ZERO,
                TransferType::Primary,
            );
        }

        buffer.finish();
    }

    fn parse_response(&self, data: &[u8]) -> Result<ProtocolResponse, ProtocolError> {
        if data.is_empty() {
            // hidapi reports a read timeout as an empty buffer. The topology
            // reply is required, so this fails init the way a hidraw timeout
            // does instead of leaving the device with no channels.
            return Err(ProtocolError::MalformedResponse {
                detail: format!(
                    "{} sent no topology reply (0x21 0x03) within the response timeout",
                    self.name
                ),
            });
        }

        if !is_topology_response(data) {
            // Fan status (0x67 0x02) and Kraken status (0x75 0x02) arrive on
            // the device's own schedule. Unsolicited makes the backend read
            // the next report without resending, so the reply queued behind
            // them is the one that gets read.
            debug!(
                device = self.name,
                report = format_args!("{:02X?}", &data[..data.len().min(2)]),
                "NZXT Gen2 report is not the topology reply; reading past it"
            );
            return Ok(ProtocolResponse {
                status: ResponseStatus::Unsolicited,
                data: Vec::new(),
            });
        }

        if self.profile.discovery != ChannelDiscovery::TopologyQuery {
            return Ok(ProtocolResponse {
                status: ResponseStatus::Ok,
                data: Vec::new(),
            });
        }

        let topology = parse_topology_response(data)?;
        let layout = Gen2Layout::build(&self.profile, Some(&topology));
        debug!(
            device = self.name,
            reported_channels = topology.channels.len(),
            streamed_channels = layout.channels.len(),
            total_leds = layout.total_leds,
            "NZXT Gen2 topology resolved"
        );
        *self
            .layout
            .write()
            .expect("NZXT Gen2 layout lock should not be poisoned") = layout;

        Ok(ProtocolResponse {
            status: ResponseStatus::Ok,
            data: data.to_vec(),
        })
    }

    fn zones(&self) -> Vec<SegmentInfo> {
        self.read_layout().segments.clone()
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
        u32::try_from(self.read_layout().total_leds).unwrap_or(u32::MAX)
    }

    fn frame_interval(&self) -> Duration {
        GEN2_FRAME_INTERVAL
    }
}
