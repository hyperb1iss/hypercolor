//! Art-Net 4 ArtDmx packet builder.
//!
//! ArtDmx carries one DMX512 universe, addressed by a 15-bit Port-Address,
//! over UDP port 6454. Multi-byte header fields are little-endian except
//! the protocol version and data length, which the standard sends high
//! byte first. This module only encodes; it never touches a socket.
//!
//! Protocol reference: Art-Net 4 specification (Artistic Licence).

use std::collections::HashMap;

/// Art-Net UDP port (0x1936).
pub const ARTNET_PORT: u16 = 6454;

/// Packet identifier that opens every Art-Net packet.
pub const ARTNET_ID: [u8; 8] = *b"Art-Net\x00";

/// Art-Net protocol revision sent in `ProtVerHi`/`ProtVerLo`.
pub const ARTNET_PROTOCOL_VERSION: u16 = 14;

/// `OpDmx` (also called `OpOutput`): one universe of DMX512 data.
pub const ARTNET_OP_DMX: u16 = 0x5000;

/// Size of the ArtDmx header before the channel data.
pub const ARTDMX_HEADER_SIZE: usize = 18;

/// Maximum DMX channels carried by one ArtDmx packet.
pub const ARTDMX_MAX_CHANNELS: usize = 512;

/// Highest valid 15-bit Port-Address.
pub const ARTNET_MAX_PORT_ADDRESS: u16 = 0x7FFF;

const ARTDMX_MAX_PACKET_SIZE: usize = ARTDMX_HEADER_SIZE + ARTDMX_MAX_CHANNELS;

// ── Port-Address ────────────────────────────────────────────────────────

/// 15-bit Art-Net Port-Address: `Net` (7 bits), `Sub-Net` (4), `Universe` (4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PortAddress(u16);

impl PortAddress {
    /// Wrap a flat Port-Address, rejecting values above 32767.
    #[must_use]
    pub const fn new(value: u16) -> Option<Self> {
        if value > ARTNET_MAX_PORT_ADDRESS {
            None
        } else {
            Some(Self(value))
        }
    }

    /// Compose a Port-Address from its `Net`, `Sub-Net`, and `Universe` parts.
    #[must_use]
    pub const fn from_parts(net: u8, sub_net: u8, universe: u8) -> Option<Self> {
        if net > 0x7F || sub_net > 0x0F || universe > 0x0F {
            return None;
        }
        Some(Self(
            ((net as u16) << 8) | ((sub_net as u16) << 4) | universe as u16,
        ))
    }

    /// Flat 15-bit value.
    #[must_use]
    pub const fn value(self) -> u16 {
        self.0
    }

    /// `Net` field: Port-Address bits 14-8.
    #[must_use]
    pub const fn net(self) -> u8 {
        ((self.0 >> 8) & 0x7F) as u8
    }

    /// `Sub-Net` field: Port-Address bits 7-4.
    #[must_use]
    pub const fn sub_net(self) -> u8 {
        ((self.0 >> 4) & 0x0F) as u8
    }

    /// `Universe` field: Port-Address bits 3-0.
    #[must_use]
    pub const fn universe(self) -> u8 {
        (self.0 & 0x0F) as u8
    }

    /// `SubUni` header byte: `Sub-Net` in the high nibble, `Universe` low.
    #[must_use]
    pub const fn sub_uni(self) -> u8 {
        (self.0 & 0xFF) as u8
    }
}

// ── ArtDmx Packet ───────────────────────────────────────────────────────

/// One ArtDmx packet with its fixed header fields pre-populated.
#[derive(Debug, Clone)]
pub struct ArtDmxPacket {
    buf: [u8; ARTDMX_MAX_PACKET_SIZE],
    /// Encoded data length (even, 2-512).
    length: u16,
}

impl ArtDmxPacket {
    /// Create an ArtDmx packet for `port_address`.
    ///
    /// `physical` is the informational physical input port byte; senders
    /// that are not DMX input gateways leave it at zero.
    #[must_use]
    pub fn new(port_address: PortAddress, physical: u8) -> Self {
        let mut buf = [0u8; ARTDMX_MAX_PACKET_SIZE];
        buf[0..8].copy_from_slice(&ARTNET_ID);
        // OpCode, little-endian
        buf[8..10].copy_from_slice(&ARTNET_OP_DMX.to_le_bytes());
        // ProtVerHi, ProtVerLo
        buf[10..12].copy_from_slice(&ARTNET_PROTOCOL_VERSION.to_be_bytes());
        // Sequence (12) is written per packet.
        buf[13] = physical;
        buf[14] = port_address.sub_uni();
        buf[15] = port_address.net();
        Self { buf, length: 0 }
    }

    /// Write channel data and the sequence number.
    ///
    /// Data longer than 512 channels is truncated. Odd lengths gain one
    /// zero pad channel and empty data becomes two zero channels, because
    /// ArtDmx requires an even length in the range 2-512.
    #[allow(clippy::cast_possible_truncation, clippy::as_conversions)]
    pub fn set_data(&mut self, channels: &[u8], sequence: u8) {
        let count = channels.len().min(ARTDMX_MAX_CHANNELS);
        let length = count.next_multiple_of(2).max(2);
        let data_start = ARTDMX_HEADER_SIZE;
        self.buf[data_start..data_start + count].copy_from_slice(&channels[..count]);
        self.buf[data_start + count..data_start + length].fill(0);
        self.buf[12] = sequence;
        self.length = length as u16;
        // LengthHi, Length: high byte first.
        self.buf[16..18].copy_from_slice(&self.length.to_be_bytes());
    }

    /// The raw bytes to send over UDP.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.buf[..ARTDMX_HEADER_SIZE + usize::from(self.length)]
    }

    /// The Port-Address this packet is addressed to.
    #[must_use]
    pub fn port_address(&self) -> PortAddress {
        PortAddress((u16::from(self.buf[15]) << 8) | u16::from(self.buf[14]))
    }

    /// Encoded data length in channels.
    #[must_use]
    pub fn data_length(&self) -> u16 {
        self.length
    }
}

// ── Sequence Tracker ────────────────────────────────────────────────────

/// Per-Port-Address ArtDmx sequence counter.
///
/// Sequence `0` disables receiver-side reordering, so the counter runs
/// 1-255 and wraps back to 1.
#[derive(Debug, Default, Clone)]
pub struct ArtDmxSequenceTracker {
    sequences: HashMap<PortAddress, u8>,
}

impl ArtDmxSequenceTracker {
    /// Advance and return the next sequence number for `port_address`.
    pub fn advance(&mut self, port_address: PortAddress) -> u8 {
        let seq = self.sequences.entry(port_address).or_insert(0);
        *seq = if *seq == u8::MAX { 1 } else { *seq + 1 };
        *seq
    }

    /// Current sequence for `port_address` (0 if never advanced).
    #[must_use]
    pub fn current(&self, port_address: PortAddress) -> u8 {
        self.sequences.get(&port_address).copied().unwrap_or(0)
    }
}
