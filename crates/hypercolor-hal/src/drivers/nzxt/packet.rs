//! Wire-format packet structs for NZXT Phase 1 lighting (Spec 39 §9.5).
//!
//! Byte 0 of every struct is the first byte on the wire. The transport runs in
//! a report-ID-in-payload mode, so nothing is prepended or stripped.

use zerocopy::{FromZeros, Immutable, IntoBytes, KnownLayout};

/// Report length of every Gen2-family packet (controllers, motherboards,
/// Kraken X3 and Z3).
pub const GEN2_REPORT_LEN: usize = 64;

/// Report length of every Kraken Elite v2 packet.
pub const ELITE_V2_REPORT_LEN: usize = 512;

/// Bytes ahead of the color payload in a Gen2 stream packet.
pub const GEN2_STREAM_HEADER_LEN: usize = 4;

/// LEDs carried by one Gen2 stream packet (60 payload bytes, 3 per LED).
pub const GEN2_LEDS_PER_PACKET: usize = 20;

/// Stream packets one channel can address: the packet number lives in the
/// low nibble of byte 1.
pub const GEN2_MAX_PACKETS_PER_CHANNEL: usize = 16;

/// Gen2 command byte for the topology query.
pub const GEN2_TOPOLOGY_QUERY: [u8; 2] = [0x20, 0x03];

/// First two bytes of the Gen2 topology response.
pub const GEN2_TOPOLOGY_RESPONSE: [u8; 2] = [0x21, 0x03];

/// Gen2 direct stream command byte.
pub const GEN2_STREAM_COMMAND: u8 = 0x22;

/// High nibble of byte 1 of a Gen2 stream packet.
pub const GEN2_STREAM_SUBCOMMAND: u8 = 0x10;

/// Gen2 apply subcommand (byte 1 of the apply packet).
pub const GEN2_APPLY_SUBCOMMAND: u8 = 0xA0;

/// Apply packet bytes 3 through 15, verbatim from Spec 39 §4.3. Byte 7 of the
/// packet (index 4 here) is the count-like field, `0x28`.
pub const GEN2_APPLY_TAIL: [u8; 13] = [
    0x00, 0x01, 0x00, 0x00, 0x28, 0x00, 0x00, 0x80, 0x00, 0x32, 0x00, 0x00, 0x01,
];

/// Fixed-color write command byte (motherboard 12V header, Kraken X3 logo).
pub const FIXED_COLOR_COMMAND: u8 = 0x2A;

/// Fixed-color write bytes 4 through 6, shared by both targets.
pub const FIXED_COLOR_MIDDLE: [u8; 3] = [0x00, 0x32, 0x00];

/// Fixed-color write bytes 56 through 59.
pub const FIXED_COLOR_TRAILER: [u8; 4] = [0x01, 0x00, 0x01, 0x03];

/// Kraken Elite v2 ring write header.
pub const ELITE_V2_RING_HEADER: [u8; 4] = [0x26, 0x14, 0x01, 0x01];

/// LEDs on the Kraken Elite v2 pump ring. The apply hard-codes this count.
pub const ELITE_V2_RING_LEDS: usize = 24;

/// Kraken Elite v2 apply packet bytes 0 through 15, verbatim from Spec 39
/// §4.6.4.
pub const ELITE_V2_APPLY_HEADER: [u8; 16] = [
    0x26, 0x06, 0x01, 0x00, 0x01, 0x00, 0x00, 0x18, 0x00, 0x00, 0x80, 0x00, 0x32, 0x00, 0x00, 0x01,
];

const _: () = assert!(
    std::mem::size_of::<Gen2QueryPacket>() == GEN2_REPORT_LEN,
    "Gen2QueryPacket must match the 64-byte Gen2 report"
);
const _: () = assert!(
    std::mem::size_of::<Gen2StreamPacket>() == GEN2_REPORT_LEN,
    "Gen2StreamPacket must match the 64-byte Gen2 report"
);
const _: () = assert!(
    std::mem::size_of::<Gen2ApplyPacket>() == GEN2_REPORT_LEN,
    "Gen2ApplyPacket must match the 64-byte Gen2 report"
);
const _: () = assert!(
    std::mem::size_of::<FixedColorPacket>() == GEN2_REPORT_LEN,
    "FixedColorPacket must match the 64-byte Gen2 report"
);
const _: () = assert!(
    std::mem::size_of::<EliteV2RingPacket>() == ELITE_V2_REPORT_LEN,
    "EliteV2RingPacket must match the 512-byte Elite v2 report"
);
const _: () = assert!(
    std::mem::size_of::<EliteV2ApplyPacket>() == ELITE_V2_REPORT_LEN,
    "EliteV2ApplyPacket must match the 512-byte Elite v2 report"
);
const _: () = assert!(
    GEN2_STREAM_HEADER_LEN + GEN2_LEDS_PER_PACKET * 3 == GEN2_REPORT_LEN,
    "a Gen2 stream packet is a 4-byte header plus 20 LEDs"
);
const _: () = assert!(
    std::mem::offset_of!(FixedColorPacket, trailer) == 56,
    "the fixed-color trailer starts at byte 56"
);

/// Gen2 two-byte query, zero padded to the report length.
#[derive(FromZeros, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct Gen2QueryPacket {
    /// Command and subcommand bytes.
    pub command: [u8; 2],
    /// Zero padding.
    pub padding: [u8; 62],
}

impl Gen2QueryPacket {
    /// Build a zero-padded query carrying `command`.
    #[must_use]
    pub fn new(command: [u8; 2]) -> Self {
        let mut packet = Self::new_zeroed();
        packet.command = command;
        packet
    }
}

/// Gen2 direct stream packet: up to 20 GRB LEDs for one channel.
#[derive(FromZeros, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct Gen2StreamPacket {
    /// Always [`GEN2_STREAM_COMMAND`].
    pub command: u8,
    /// [`GEN2_STREAM_SUBCOMMAND`] OR'd with the packet number.
    pub packet: u8,
    /// `1 << channel`.
    pub channel_mask: u8,
    /// Always zero.
    pub reserved: u8,
    /// Color bytes, three per LED, zero padded.
    pub colors: [u8; GEN2_LEDS_PER_PACKET * 3],
}

/// Gen2 apply packet: the commit barrier after one channel's stream packets.
#[derive(FromZeros, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct Gen2ApplyPacket {
    /// Always [`GEN2_STREAM_COMMAND`].
    pub command: u8,
    /// Always [`GEN2_APPLY_SUBCOMMAND`].
    pub subcommand: u8,
    /// `1 << channel`, matching the stream packets it commits.
    pub channel_mask: u8,
    /// [`GEN2_APPLY_TAIL`].
    pub tail: [u8; 13],
    /// Zero padding.
    pub padding: [u8; 48],
}

impl Gen2ApplyPacket {
    /// Build the apply packet for one channel mask.
    #[must_use]
    pub fn new(channel_mask: u8) -> Self {
        let mut packet = Self::new_zeroed();
        packet.command = GEN2_STREAM_COMMAND;
        packet.subcommand = GEN2_APPLY_SUBCOMMAND;
        packet.channel_mask = channel_mask;
        packet.tail = GEN2_APPLY_TAIL;
        packet
    }
}

/// Fixed-color write used by the motherboard 12V header and the Kraken X3
/// logo. Only the target bytes and the color byte order differ.
#[derive(FromZeros, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct FixedColorPacket {
    /// Always [`FIXED_COLOR_COMMAND`].
    pub command: u8,
    /// Bytes 1 through 3: `04 08 08` for the 12V header, `04 04 04` for the
    /// Kraken X3 logo.
    pub target: [u8; 3],
    /// [`FIXED_COLOR_MIDDLE`].
    pub middle: [u8; 3],
    /// The color, already in wire byte order.
    pub color: [u8; 3],
    /// Zero bytes 10 through 55.
    pub gap: [u8; 46],
    /// [`FIXED_COLOR_TRAILER`].
    pub trailer: [u8; 4],
    /// Zero padding.
    pub padding: [u8; 4],
}

impl FixedColorPacket {
    /// Build a fixed-color write for `target` carrying a wire-ordered color.
    #[must_use]
    pub fn new(target: [u8; 3], wire_color: [u8; 3]) -> Self {
        let mut packet = Self::new_zeroed();
        packet.command = FIXED_COLOR_COMMAND;
        packet.target = target;
        packet.middle = FIXED_COLOR_MIDDLE;
        packet.color = wire_color;
        packet.trailer = FIXED_COLOR_TRAILER;
        packet
    }
}

/// Kraken Elite v2 pump ring write.
#[derive(FromZeros, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct EliteV2RingPacket {
    /// [`ELITE_V2_RING_HEADER`].
    pub header: [u8; 4],
    /// 24 LEDs as G, R, B.
    pub colors: [u8; ELITE_V2_RING_LEDS * 3],
    /// Zero padding.
    pub padding: [u8; 436],
}

/// Kraken Elite v2 apply packet.
#[derive(FromZeros, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct EliteV2ApplyPacket {
    /// [`ELITE_V2_APPLY_HEADER`].
    pub header: [u8; 16],
    /// Zero padding.
    pub padding: [u8; 496],
}

impl EliteV2ApplyPacket {
    /// Build the apply packet.
    #[must_use]
    pub fn new() -> Self {
        let mut packet = Self::new_zeroed();
        packet.header = ELITE_V2_APPLY_HEADER;
        packet
    }
}

impl Default for EliteV2ApplyPacket {
    fn default() -> Self {
        Self::new()
    }
}

/// Reorder an RGB triple into NZXT's default G, R, B wire order.
#[must_use]
pub const fn grb([r, g, b]: [u8; 3]) -> [u8; 3] {
    [g, r, b]
}

/// Fill a color payload with `colors` in G, R, B order. Writes
/// `min(out.len() / 3, colors.len())` LEDs and leaves the rest untouched.
pub fn fill_grb(out: &mut [u8], colors: &[[u8; 3]]) {
    let (slots, _) = out.as_chunks_mut::<3>();
    for (slot, color) in slots.iter_mut().zip(colors) {
        *slot = grb(*color);
    }
}
