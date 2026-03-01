//! E1.31/sACN (Streaming ACN) packet builder.
//!
//! E1.31 is the fallback protocol for WLED devices that don't support DDP
//! or for interop with DMX ecosystems (xLights, Vixen, etc.). Each universe
//! carries a maximum of 512 DMX channels.
//!
//! Protocol reference: ANSI E1.31-2018

use std::collections::HashMap;

use uuid::Uuid;

/// Pixels per E1.31 universe for RGB data (170 * 3 = 510 channels).
pub const E131_PIXELS_PER_UNIVERSE_RGB: usize = 170;

/// Pixels per E1.31 universe for RGBW data (127 * 4 = 508 channels).
pub const E131_PIXELS_PER_UNIVERSE_RGBW: usize = 127;

/// Max DMX channels per universe (not counting start code).
pub const E131_CHANNELS_PER_UNIVERSE: usize = 512;

/// E1.31 default port.
pub const E131_PORT: u16 = 5568;

/// Hypercolor's E1.31 priority. Higher than default (100) to take precedence.
pub const E131_PRIORITY: u8 = 150;

/// E1.31 packet header size (fixed, before DMX data).
const E131_HEADER_SIZE: usize = 126;

/// Total max E1.31 packet size (header + start code + 512 DMX channels).
const E131_MAX_PACKET_SIZE: usize = 638;

/// ACN Packet Identifier: "ASC-E1.17\0\0\0"
const ACN_PACKET_ID: [u8; 12] = *b"ASC-E1.17\x00\x00\x00";

// ── E1.31 Packet ────────────────────────────────────────────────────────

/// Complete E1.31 packet with all fixed fields pre-populated.
///
/// The header is mostly static — only sequence, universe, data length,
/// and flags/length fields change per packet.
#[derive(Debug, Clone)]
pub struct E131Packet {
    buf: [u8; E131_MAX_PACKET_SIZE],
    /// Number of DMX channels actually used in this packet (1-512).
    channel_count: u16,
}

impl E131Packet {
    /// Create a new E1.31 packet with all fixed fields pre-populated.
    ///
    /// # Arguments
    ///
    /// * `source_name` — Human-readable source identifier (max 63 bytes, null-padded)
    /// * `cid` — Sender CID (stable UUID per Hypercolor instance)
    /// * `universe` — E1.31 universe number (1-63999)
    /// * `priority` — E1.31 priority (0-200, typically 150)
    #[must_use]
    pub fn new(source_name: &str, cid: Uuid, universe: u16, priority: u8) -> Self {
        let mut buf = [0u8; E131_MAX_PACKET_SIZE];

        // ---- Root Layer ----
        // Preamble size: 0x0010
        buf[0..2].copy_from_slice(&0x0010_u16.to_be_bytes());
        // Postamble size: 0x0000
        buf[2..4].copy_from_slice(&0x0000_u16.to_be_bytes());
        // ACN Packet Identifier
        buf[4..16].copy_from_slice(&ACN_PACKET_ID);
        // Root vector: 0x00000004 (`VECTOR_ROOT_E131_DATA`)
        buf[18..22].copy_from_slice(&0x0000_0004_u32.to_be_bytes());
        // CID (sender UUID)
        buf[22..38].copy_from_slice(cid.as_bytes());

        // ---- Framing Layer ----
        // Framing vector: 0x00000002 (`VECTOR_E131_DATA_PACKET`)
        buf[40..44].copy_from_slice(&0x0000_0002_u32.to_be_bytes());
        // Source name (64 bytes, null-padded)
        let name_bytes = source_name.as_bytes();
        let copy_len = name_bytes.len().min(63);
        buf[44..44 + copy_len].copy_from_slice(&name_bytes[..copy_len]);
        // Priority
        buf[108] = priority;
        // Sync address: 0x0000 (no synchronization)
        buf[109..111].copy_from_slice(&0x0000_u16.to_be_bytes());
        // Options: 0x00
        buf[112] = 0x00;
        // Universe
        buf[113..115].copy_from_slice(&universe.to_be_bytes());

        // ---- DMP Layer ----
        // DMP vector: 0x02 (`VECTOR_DMP_SET_PROPERTY`)
        buf[117] = 0x02;
        // Address & Data Type: 0xA1
        buf[118] = 0xA1;
        // First Property Address: 0x0000
        buf[119..121].copy_from_slice(&0x0000_u16.to_be_bytes());
        // Address Increment: 0x0001
        buf[121..123].copy_from_slice(&0x0001_u16.to_be_bytes());
        // DMX start code at byte 125
        buf[125] = 0x00;

        Self {
            buf,
            channel_count: 0,
        }
    }

    /// Write pixel data into the DMX channel slots and update length fields.
    ///
    /// `channels` is raw DMX data (RGB triplets or RGBW quads), max 512 bytes.
    #[allow(clippy::cast_possible_truncation, clippy::as_conversions)]
    pub fn set_channels(&mut self, channels: &[u8], sequence: u8) {
        let count = channels.len().min(E131_CHANNELS_PER_UNIVERSE);
        self.channel_count = count as u16;

        // DMX channel data starts at byte 126 (after start code at 125)
        self.buf[126..126 + count].copy_from_slice(&channels[..count]);

        // Sequence number
        self.buf[111] = sequence;

        // Property value count = channels + 1 (for start code)
        let prop_count = (count + 1) as u16;
        self.buf[123..125].copy_from_slice(&prop_count.to_be_bytes());

        // Update Flags & Length fields (high nibble 0x7 for flags)
        // DMP layer length: from byte 115 to end of data
        let dmp_len = (count + 11) as u16; // 10 DMP header bytes + start code + data
        self.buf[115..117].copy_from_slice(&(0x7000 | dmp_len).to_be_bytes());

        // Framing layer length: from byte 38 to end of data
        let frame_len = (count + 88) as u16;
        self.buf[38..40].copy_from_slice(&(0x7000 | frame_len).to_be_bytes());

        // Root layer length: from byte 16 to end of data
        let root_len = (count + 110) as u16;
        self.buf[16..18].copy_from_slice(&(0x7000 | root_len).to_be_bytes());
    }

    /// The raw bytes to send over UDP.
    ///
    /// Only returns the portion of the buffer that is actually populated,
    /// not the full 638-byte buffer.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        let total = E131_HEADER_SIZE + 1 + usize::from(self.channel_count);
        &self.buf[..total]
    }

    /// The universe number this packet is addressed to.
    #[must_use]
    pub fn universe(&self) -> u16 {
        u16::from_be_bytes([self.buf[113], self.buf[114]])
    }
}

// ── Sequence Tracker ────────────────────────────────────────────────────

/// Per-universe sequence number tracker for E1.31.
///
/// E1.31 uses a `u8` sequence number per universe that wraps from 255
/// back to 0. Receivers use this to detect out-of-order and dropped packets.
#[derive(Debug, Default, Clone)]
pub struct E131SequenceTracker {
    sequences: HashMap<u16, u8>,
}

impl E131SequenceTracker {
    /// Advance and return the next sequence number for the given universe.
    pub fn advance(&mut self, universe: u16) -> u8 {
        let seq = self.sequences.entry(universe).or_insert(0);
        *seq = seq.wrapping_add(1);
        *seq
    }

    /// Current sequence for a universe (0 if never advanced).
    #[must_use]
    pub fn current(&self, universe: u16) -> u8 {
        self.sequences.get(&universe).copied().unwrap_or(0)
    }
}

// ── Universe Math ───────────────────────────────────────────────────────

/// Compute how many E1.31 universes are needed for a given pixel count.
///
/// # Arguments
///
/// * `pixel_count` — total number of pixels
/// * `bytes_per_pixel` — 3 for RGB, 4 for RGBW
#[must_use]
pub fn universes_needed(pixel_count: usize, bytes_per_pixel: usize) -> usize {
    let pixels_per_universe = E131_CHANNELS_PER_UNIVERSE / bytes_per_pixel;
    pixel_count.div_ceil(pixels_per_universe)
}
