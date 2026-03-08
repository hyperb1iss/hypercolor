//! DDP (Distributed Display Protocol) packet builder.
//!
//! DDP is the preferred protocol for streaming pixel data to WLED devices.
//! A 10-byte header followed by raw pixel data — no universe management,
//! no 170-pixel limits, no ACN boilerplate.
//!
//! Protocol reference: <http://www.3waylabs.com/ddp/>

/// Maximum pixel data bytes per DDP packet.
/// 480 RGB pixels * 3 = 1440, or 360 RGBW pixels * 4 = 1440.
/// Stays under 1472-byte UDP payload (1500 MTU - 20 IP - 8 UDP).
pub const DDP_MAX_PAYLOAD: usize = 1440;

/// DDP header is always 10 bytes (we never use timecodes).
pub const DDP_HEADER_SIZE: usize = 10;

/// DDP default port.
pub const DDP_PORT: u16 = 4048;

/// DDP protocol version 1 (bits 7:6 = 01).
const DDP_VERSION: u8 = 0x40;

/// DDP flag: push (latch frame on final packet).
const DDP_FLAG_PUSH: u8 = 0x01;

/// DDP data type: RGB, 8-bit per channel (TTT=001, BBB=011).
///
/// WLED's DDP receiver expects `0x0B` (`DDP_TYPE_RGB24`).
pub const DDP_DTYPE_RGB8: u8 = 0x0B;

/// DDP data type: RGBW, 8-bit per channel (TTT=011, BBB=011).
///
/// WLED's DDP receiver expects `0x1B` (`DDP_TYPE_RGBW32`).
pub const DDP_DTYPE_RGBW8: u8 = 0x1B;

/// DDP destination: default output device.
const DDP_ID_DEFAULT: u8 = 0x01;

// ── DDP Packet ──────────────────────────────────────────────────────────

/// A single DDP packet ready for UDP transmission.
///
/// Contains a pre-built 10-byte header concatenated with raw pixel data.
/// Maximum total size: 10 + 1440 = 1450 bytes.
#[derive(Debug, Clone)]
pub struct DdpPacket {
    /// Pre-built header + data buffer.
    buf: Vec<u8>,
}

impl DdpPacket {
    /// Build a DDP data packet.
    ///
    /// # Arguments
    ///
    /// * `pixel_data` — slice of RGB or RGBW bytes for this fragment (max 1440)
    /// * `offset` — byte offset into the device's pixel buffer
    /// * `push` — `true` if this is the final packet of the frame
    /// * `sequence` — 1-15 wrapping sequence number
    /// * `data_type` — `DDP_DTYPE_RGB8` or `DDP_DTYPE_RGBW8`
    #[must_use]
    pub fn new(pixel_data: &[u8], offset: u32, push: bool, sequence: u8, data_type: u8) -> Self {
        debug_assert!(pixel_data.len() <= DDP_MAX_PAYLOAD);

        let mut buf = Vec::with_capacity(DDP_HEADER_SIZE + pixel_data.len());

        // Byte 0: flags — version 1, optional push
        let flags = DDP_VERSION | if push { DDP_FLAG_PUSH } else { 0 };
        buf.push(flags);

        // Byte 1: sequence (low nibble only)
        buf.push(sequence & 0x0F);

        // Byte 2: data type
        buf.push(data_type);

        // Byte 3: destination ID
        buf.push(DDP_ID_DEFAULT);

        // Bytes 4-7: data offset (big-endian u32)
        buf.extend_from_slice(&offset.to_be_bytes());

        // Bytes 8-9: data length (big-endian u16)
        #[allow(clippy::cast_possible_truncation, clippy::as_conversions)]
        let len = pixel_data.len() as u16;
        buf.extend_from_slice(&len.to_be_bytes());

        // Payload
        buf.extend_from_slice(pixel_data);

        Self { buf }
    }

    /// The raw bytes to send over UDP.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.buf
    }

    /// Total packet size (header + payload).
    #[must_use]
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    /// Whether the packet is empty (should never be true for a valid packet).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }
}

// ── DDP Sequence ────────────────────────────────────────────────────────

/// Wrapping 1-15 sequence counter for DDP packets.
///
/// Sequence `0` means "not used" in the DDP spec, so we skip it.
/// WLED uses sequence numbers for packet ordering.
#[derive(Debug, Default)]
pub struct DdpSequence(u8);

impl DdpSequence {
    /// Advance to the next sequence number, wrapping from 15 back to 1.
    pub fn advance(&mut self) -> u8 {
        self.0 = if self.0 >= 15 { 1 } else { self.0 + 1 };
        self.0
    }

    /// Current sequence number (0 if `advance()` has never been called).
    #[must_use]
    pub fn current(&self) -> u8 {
        self.0
    }
}

// ── Frame Builder ───────────────────────────────────────────────────────

/// Build a sequence of DDP packets for a complete frame.
///
/// Fragments the pixel data into `DDP_MAX_PAYLOAD`-sized chunks,
/// setting the push flag only on the final packet to trigger WLED's
/// frame latch.
#[must_use]
pub fn build_ddp_frame(
    pixel_data: &[u8],
    data_type: u8,
    sequence: &mut DdpSequence,
) -> Vec<DdpPacket> {
    let seq = sequence.advance();
    let chunks: Vec<&[u8]> = pixel_data.chunks(DDP_MAX_PAYLOAD).collect();
    let last_idx = chunks.len().saturating_sub(1);

    chunks
        .into_iter()
        .enumerate()
        .map(|(i, chunk)| {
            #[allow(clippy::cast_possible_truncation, clippy::as_conversions)]
            let offset = (i * DDP_MAX_PAYLOAD) as u32;
            let push = i == last_idx;
            DdpPacket::new(chunk, offset, push, seq, data_type)
        })
        .collect()
}
