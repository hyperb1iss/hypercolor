//! Shared display encoding for protocols that drive pixel panels.
//!
//! Display drivers differ in wire format but repeat the same two stages:
//! turning pixels into device bytes (see [`repack`]) and getting those bytes
//! onto the wire in fixed-size packets. This module owns the second stage:
//! chunk arithmetic, sequence counters, final flags, zero padding, and the
//! per-chunk command policy.
//!
//! The engines are helpers, not a framework. Each protocol keeps its own
//! state (decode caches, frame preambles, wire keepalives) and calls these
//! functions for the parts that would otherwise be copied per driver. The
//! `_into` variants take a [`CommandBuffer`] so a protocol can emit its own
//! commands before or after the chunk stream while keeping one reusable
//! allocation per frame.

pub mod keepalive;
pub mod repack;

use std::time::Duration;

use hypercolor_types::device::DisplayFrameFormat;

use crate::protocol::{CommandBuffer, ProtocolCommand, ResponsePlan, TransferType};

pub use keepalive::WireKeepalive;
pub use repack::{LineRepack, Packed16Format, RepackError};

/// Per-chunk context handed to a [`DisplayChunkLayout`] while framing.
#[derive(Debug, Clone, Copy)]
pub struct ChunkContext<'a> {
    /// Total encoded frame length across all chunks.
    pub total_len: usize,

    /// Zero-based chunk index within this frame.
    pub packet_index: u32,

    /// This chunk's payload bytes, before zero padding.
    pub payload: &'a [u8],

    /// Whether this is the last chunk of the frame.
    pub is_final: bool,
}

/// Context handed to the header writer of a prefixed single-buffer frame.
///
/// The spec sketch left these fields open; they are the three facts a header
/// writer cannot recover from the buffer it is handed, since the payload is
/// already copied into the frame and its own extent is no longer visible.
#[derive(Debug, Clone, Copy)]
pub struct PrefixContext<'a> {
    /// Payload bytes placed after the header.
    pub payload: &'a [u8],

    /// Length of the header block at the start of the frame.
    pub header_len: usize,

    /// Total on-wire frame length, including the header and any zero padding.
    pub frame_len: usize,
}

/// Everything a [`ProtocolCommand`] needs beyond its bytes.
#[derive(Debug, Clone, Copy)]
pub struct ChunkCommandPolicy {
    /// Transport path for the chunk.
    pub transfer_type: TransferType,

    /// Whether the caller reads a response after sending the chunk.
    pub expects_response: bool,

    /// Minimum delay between sending the chunk and reading its response.
    pub response_delay: Duration,

    /// Minimum delay after sending the chunk. `None` means no pacing.
    pub post_delay: Option<Duration>,

    /// How the chunk's reply is read when `expects_response` is set.
    pub response: ResponsePlan,
}

impl ChunkCommandPolicy {
    /// Policy for unacknowledged, unpaced chunks on `transfer_type`.
    #[must_use]
    pub const fn fire_and_forget(transfer_type: TransferType) -> Self {
        Self {
            transfer_type,
            expects_response: false,
            response_delay: Duration::ZERO,
            post_delay: None,
            response: ResponsePlan {
                count: 1,
                timeout: None,
                capacity: None,
                tolerance: crate::protocol::ResponseTolerance::Required,
            },
        }
    }
}

/// Fixed-size packet geometry and per-chunk policy for one display protocol.
pub trait DisplayChunkLayout: Send + Sync {
    /// Fixed on-wire packet length (header + payload + padding).
    fn packet_len(&self) -> usize;

    /// Maximum payload bytes carried per packet.
    fn max_payload(&self) -> usize;

    /// Where payload bytes start inside the packet.
    fn payload_offset(&self) -> usize;

    /// Write the header (and any trailer) into the zeroed packet buffer.
    ///
    /// The engine has already copied the payload to [`Self::payload_offset`],
    /// so a layout that must transform the whole packet (encrypting a header,
    /// appending a checksum) can do so here.
    fn write_header(&self, packet: &mut [u8], ctx: &ChunkContext<'_>);

    /// Command policy for this chunk: ack-per-chunk vs fire-and-forget,
    /// pacing, transfer path.
    fn command_policy(&self, ctx: &ChunkContext<'_>) -> ChunkCommandPolicy;

    /// Maximum chunk count this layout's sequence counter can express.
    ///
    /// The bound exists so counter overflow surfaces as an error instead of a
    /// wrapped or saturated sequence number on the wire.
    fn max_chunks(&self) -> u32;
}

/// Why a display payload produced no wire commands.
///
/// The actor fails the delivery on any of these; nothing here is ever
/// silently truncated onto the wire.
#[derive(Debug, thiserror::Error)]
pub enum DisplayEncodeError {
    /// The protocol drives no display, or none that takes this payload format.
    #[error("protocol cannot take a {format} display payload")]
    Unsupported {
        /// Format the caller offered.
        format: DisplayFrameFormat,
    },

    /// The payload's pixel geometry is not the panel's.
    #[error("display payload is {width}x{height}, panel is {expected_width}x{expected_height}")]
    WrongGeometry {
        /// Width the panel needs.
        expected_width: u32,

        /// Height the panel needs.
        expected_height: u32,

        /// Width the caller supplied.
        width: u32,

        /// Height the caller supplied.
        height: u32,
    },

    /// Compressed payload bytes that no decoder would accept.
    #[error("display payload could not be decoded: {detail}")]
    Undecodable {
        /// What the decoder objected to.
        detail: String,
    },

    /// Pixel repacking failed.
    #[error(transparent)]
    Repack(#[from] RepackError),

    /// Payload does not fit the frame or packet capacity it was given.
    #[error("display payload of {actual} bytes exceeds the {capacity}-byte capacity")]
    PayloadTooLarge {
        /// Bytes the caller asked to encode.
        actual: usize,

        /// Bytes the frame or packet can carry.
        capacity: usize,
    },

    /// Frame needs more chunks than the layout's sequence counter can express.
    #[error("display frame needs {needed} chunks, exceeding the layout maximum of {max}")]
    TooManyChunks {
        /// Chunks the frame would require.
        needed: usize,

        /// Chunks the layout can address.
        max: u32,
    },
}

/// Chunk `data` across fixed-size packets, one [`ProtocolCommand`] per chunk.
///
/// The buffer is rewritten from the start and truncated to the commands this
/// frame needs. Zero-length `data` emits nothing: the daemon never delivers
/// empty frames, and the Corsair LCD suite pins zero packets for an empty
/// JPEG.
///
/// # Errors
///
/// Returns [`DisplayEncodeError`] when the chunk count exceeds the layout's
/// [`DisplayChunkLayout::max_chunks`] or the layout's payload window does not
/// fit its packet. No commands are emitted in either case.
pub fn encode_chunked_display_frame(
    layout: &dyn DisplayChunkLayout,
    data: &[u8],
    commands: &mut Vec<ProtocolCommand>,
) -> Result<(), DisplayEncodeError> {
    let mut buffer = CommandBuffer::new(commands);
    let result = encode_chunked_display_frame_into(layout, data, &mut buffer);
    buffer.finish();
    result
}

/// Chunk `data` into an existing [`CommandBuffer`] without finishing it.
///
/// Protocols that wrap the chunk stream in their own commands (a frame
/// preamble, a trailing wire keepalive) use this form and call
/// [`CommandBuffer::finish`] themselves.
///
/// # Errors
///
/// Same conditions as [`encode_chunked_display_frame`]. Validation runs
/// before the first chunk is written, so an error leaves the buffer exactly
/// as the caller left it.
pub fn encode_chunked_display_frame_into(
    layout: &dyn DisplayChunkLayout,
    data: &[u8],
    buffer: &mut CommandBuffer<'_>,
) -> Result<(), DisplayEncodeError> {
    let packet_len = layout.packet_len();
    let max_payload = layout.max_payload();
    let payload_offset = layout.payload_offset();

    let window_end = payload_offset.saturating_add(max_payload);
    if window_end > packet_len {
        return Err(DisplayEncodeError::PayloadTooLarge {
            actual: window_end,
            capacity: packet_len,
        });
    }

    if data.is_empty() {
        return Ok(());
    }

    if max_payload == 0 {
        return Err(DisplayEncodeError::PayloadTooLarge {
            actual: data.len(),
            capacity: 0,
        });
    }

    let chunk_count = data.len().div_ceil(max_payload);
    let max_chunks = layout.max_chunks();
    if chunk_count > usize::try_from(max_chunks).unwrap_or(usize::MAX) {
        return Err(DisplayEncodeError::TooManyChunks {
            needed: chunk_count,
            max: max_chunks,
        });
    }

    for (index, payload) in data.chunks(max_payload).enumerate() {
        // `chunk_count <= max_chunks` bounds the index inside u32.
        let ctx = ChunkContext {
            total_len: data.len(),
            packet_index: u32::try_from(index).unwrap_or(u32::MAX),
            payload,
            is_final: index + 1 == chunk_count,
        };
        let policy = layout.command_policy(&ctx);

        buffer
            .push_fill(
                policy.expects_response,
                policy.response_delay,
                policy.post_delay.unwrap_or(Duration::ZERO),
                policy.transfer_type,
                |packet| {
                    packet.resize(packet_len, 0);
                    packet[payload_offset..payload_offset + payload.len()].copy_from_slice(payload);
                    layout.write_header(packet, &ctx);
                },
            )
            .response = policy.response;
    }

    Ok(())
}

/// Emit one header-prefixed frame as a single wire write.
///
/// `fixed_frame_len: Some(n)` zero-pads the buffer to exactly `n` bytes for
/// devices that demand a constant-size write; `None` sends
/// `header_len + data.len()`. The header block occupies `[0..header_len]` and
/// the payload follows it.
///
/// # Errors
///
/// Returns [`DisplayEncodeError::PayloadTooLarge`] when the header plus
/// payload do not fit `fixed_frame_len`.
pub fn encode_prefixed_display_frame(
    header_len: usize,
    write_header: impl FnOnce(&mut [u8], &PrefixContext<'_>),
    data: &[u8],
    fixed_frame_len: Option<usize>,
    policy: ChunkCommandPolicy,
    commands: &mut Vec<ProtocolCommand>,
) -> Result<(), DisplayEncodeError> {
    let mut buffer = CommandBuffer::new(commands);
    let result = encode_prefixed_display_frame_into(
        header_len,
        write_header,
        data,
        fixed_frame_len,
        policy,
        &mut buffer,
    );
    buffer.finish();
    result
}

/// Emit one header-prefixed frame into an existing [`CommandBuffer`].
///
/// # Errors
///
/// Same conditions as [`encode_prefixed_display_frame`]. Validation runs
/// before the command is written.
pub fn encode_prefixed_display_frame_into(
    header_len: usize,
    write_header: impl FnOnce(&mut [u8], &PrefixContext<'_>),
    data: &[u8],
    fixed_frame_len: Option<usize>,
    policy: ChunkCommandPolicy,
    buffer: &mut CommandBuffer<'_>,
) -> Result<(), DisplayEncodeError> {
    let natural_len =
        header_len
            .checked_add(data.len())
            .ok_or(DisplayEncodeError::PayloadTooLarge {
                actual: data.len(),
                capacity: usize::MAX.saturating_sub(header_len),
            })?;
    let frame_len = fixed_frame_len.unwrap_or(natural_len);
    if natural_len > frame_len {
        return Err(DisplayEncodeError::PayloadTooLarge {
            actual: data.len(),
            capacity: frame_len.saturating_sub(header_len),
        });
    }

    buffer
        .push_fill(
            policy.expects_response,
            policy.response_delay,
            policy.post_delay.unwrap_or(Duration::ZERO),
            policy.transfer_type,
            |frame| {
                frame.resize(frame_len, 0);
                frame[header_len..natural_len].copy_from_slice(data);
                write_header(
                    frame,
                    &PrefixContext {
                        payload: data,
                        header_len,
                        frame_len,
                    },
                );
            },
        )
        .response = policy.response;

    Ok(())
}
