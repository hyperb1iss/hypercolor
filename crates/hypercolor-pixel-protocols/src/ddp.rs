//! DDP (Distributed Display Protocol) packet builder.
//!
//! DDP streams pixel data as an offset-addressed byte stream: a 10-byte
//! header followed by raw channel bytes. There is no universe management,
//! no 170-pixel ceiling, and no ACN boilerplate. Receivers latch the frame
//! when a packet arrives with the push flag set.
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

/// DDP data type: RGB, 8-bit per channel (TTT=001, SSS=011).
///
/// WLED's DDP receiver expects `0x0B` (`DDP_TYPE_RGB24`).
pub const DDP_DTYPE_RGB8: u8 = 0x0B;

/// DDP data type: RGBW, 8-bit per channel (TTT=011, SSS=011).
///
/// WLED's DDP receiver expects `0x1B` (`DDP_TYPE_RGBW32`).
pub const DDP_DTYPE_RGBW8: u8 = 0x1B;

/// DDP data type: not used or undefined.
///
/// The DDP spec sets the whole type byte to zero when the pixel type is
/// undefined, so the receiver applies its own configured layout. Used
/// when the channel stride is neither plain RGB nor RGBW.
pub const DDP_DTYPE_UNDEFINED: u8 = 0x00;

/// DDP destination: default output device.
pub const DDP_ID_DEFAULT: u8 = 0x01;

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
    /// Build a DDP data packet addressed to the default output device.
    ///
    /// # Arguments
    ///
    /// * `pixel_data`: slice of channel bytes for this fragment (max 1440)
    /// * `offset`: byte offset into the device's pixel buffer
    /// * `push`: `true` if this is the final packet of the frame
    /// * `sequence`: 1-15 wrapping sequence number
    /// * `data_type`: `DDP_DTYPE_RGB8`, `DDP_DTYPE_RGBW8`, or another data type byte
    #[must_use]
    pub fn new(pixel_data: &[u8], offset: u32, push: bool, sequence: u8, data_type: u8) -> Self {
        Self::with_destination(
            pixel_data,
            offset,
            push,
            sequence,
            data_type,
            DDP_ID_DEFAULT,
        )
    }

    /// Build a DDP data packet addressed to an explicit destination ID.
    #[must_use]
    pub fn with_destination(
        pixel_data: &[u8],
        offset: u32,
        push: bool,
        sequence: u8,
        data_type: u8,
        destination: u8,
    ) -> Self {
        let mut buf = Vec::with_capacity(DDP_HEADER_SIZE + pixel_data.len());
        encode_ddp_packet(
            &mut buf,
            pixel_data,
            offset,
            push,
            sequence,
            data_type,
            destination,
        );
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

/// Encode one DDP data packet into `buf`, replacing its previous contents.
///
/// `pixel_data` must not exceed [`DDP_MAX_PAYLOAD`] bytes.
pub fn encode_ddp_packet(
    buf: &mut Vec<u8>,
    pixel_data: &[u8],
    offset: u32,
    push: bool,
    sequence: u8,
    data_type: u8,
    destination: u8,
) {
    debug_assert!(pixel_data.len() <= DDP_MAX_PAYLOAD);

    buf.clear();
    buf.reserve(DDP_HEADER_SIZE + pixel_data.len());

    // Byte 0: flags, version 1 with optional push
    let flags = DDP_VERSION | if push { DDP_FLAG_PUSH } else { 0 };
    buf.push(flags);

    // Byte 1: sequence (low nibble only)
    buf.push(sequence & 0x0F);

    // Byte 2: data type
    buf.push(data_type);

    // Byte 3: destination ID
    buf.push(destination);

    // Bytes 4-7: data offset (big-endian u32)
    buf.extend_from_slice(&offset.to_be_bytes());

    // Bytes 8-9: data length (big-endian u16)
    #[allow(clippy::cast_possible_truncation, clippy::as_conversions)]
    let len = pixel_data.len() as u16;
    buf.extend_from_slice(&len.to_be_bytes());

    // Payload
    buf.extend_from_slice(pixel_data);
}

// ── DDP Sequence ────────────────────────────────────────────────────────

/// Wrapping 1-15 sequence counter for DDP packets.
///
/// Sequence `0` means "not used" in the DDP spec, so we skip it.
/// Receivers use sequence numbers to discard stale or reordered packets.
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

/// One contiguous run of channel bytes placed at a DDP data offset.
#[derive(Debug, Clone, Copy)]
pub struct DdpSpan<'a> {
    /// Byte offset of `data[0]` in the receiver's channel buffer.
    pub offset: u32,
    /// Channel bytes for this run.
    pub data: &'a [u8],
}

/// Build a sequence of DDP packets for a complete frame.
///
/// Fragments the pixel data into `DDP_MAX_PAYLOAD`-sized chunks,
/// setting the push flag only on the final packet to trigger the
/// receiver's frame latch. Every packet of the frame carries the same
/// sequence number, which is the framing the WLED driver has always sent;
/// WLED's receiver only rejects sequence numbers that fall just behind the
/// last one it recorded, so this framing holds for any frame size there.
/// New senders should use [`build_ddp_spans`], which numbers each packet.
#[must_use]
pub fn build_ddp_frame(
    pixel_data: &[u8],
    data_type: u8,
    sequence: &mut DdpSequence,
) -> Vec<DdpPacket> {
    build_packets(
        &[DdpSpan {
            offset: 0,
            data: pixel_data,
        }],
        data_type,
        DDP_ID_DEFAULT,
        DDP_MAX_PAYLOAD,
        sequence,
        Sequencing::PerFrame,
    )
}

/// Largest packet payload that holds only whole pixels.
///
/// Receivers such as WLED convert the byte offset to a pixel index, so a
/// packet boundary inside a pixel would misalign the rest of the frame.
/// Returns [`DDP_MAX_PAYLOAD`] for a zero stride or one wider than a packet.
#[must_use]
pub const fn pixel_aligned_chunk_len(channels_per_pixel: usize) -> usize {
    if channels_per_pixel == 0 || channels_per_pixel > DDP_MAX_PAYLOAD {
        return DDP_MAX_PAYLOAD;
    }
    (DDP_MAX_PAYLOAD / channels_per_pixel) * channels_per_pixel
}

/// Build the DDP packets for one frame assembled from several spans.
///
/// The DDP spec says the sequence number "should be incremented with each
/// new packet sent" and lets receivers ignore back-to-back duplicates, so
/// every packet takes the next number. Each span is fragmented into chunks
/// of at most `chunk_len` bytes (clamped to 1..=`DDP_MAX_PAYLOAD`) at
/// increasing offsets, and only the final packet of the whole frame carries
/// the push flag.
#[must_use]
pub fn build_ddp_spans(
    spans: &[DdpSpan<'_>],
    data_type: u8,
    destination: u8,
    chunk_len: usize,
    sequence: &mut DdpSequence,
) -> Vec<DdpPacket> {
    build_packets(
        spans,
        data_type,
        destination,
        chunk_len,
        sequence,
        Sequencing::PerPacket,
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Sequencing {
    PerFrame,
    PerPacket,
}

fn build_packets(
    spans: &[DdpSpan<'_>],
    data_type: u8,
    destination: u8,
    chunk_len: usize,
    sequence: &mut DdpSequence,
    sequencing: Sequencing,
) -> Vec<DdpPacket> {
    let chunk_len = chunk_len.clamp(1, DDP_MAX_PAYLOAD);
    let frame_sequence = sequence.advance();
    let total = spans
        .iter()
        .map(|span| span.data.len().div_ceil(chunk_len))
        .sum::<usize>();
    let mut packets = Vec::with_capacity(total);

    for span in spans {
        for (i, chunk) in span.data.chunks(chunk_len).enumerate() {
            #[allow(clippy::cast_possible_truncation, clippy::as_conversions)]
            let offset = span.offset.wrapping_add((i * chunk_len) as u32);
            let push = packets.len() + 1 == total;
            let seq = if sequencing == Sequencing::PerPacket && !packets.is_empty() {
                sequence.advance()
            } else {
                frame_sequence
            };
            packets.push(DdpPacket::with_destination(
                chunk,
                offset,
                push,
                seq,
                data_type,
                destination,
            ));
        }
    }

    packets
}
