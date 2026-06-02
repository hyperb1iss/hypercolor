use thiserror::Error;

use crate::packet::PacketId;

/// Result type used by the OpenRGB SDK crate.
pub type Result<T> = std::result::Result<T, OpenRgbError>;

/// Errors produced while encoding, decoding, or parsing OpenRGB SDK data.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum OpenRgbError {
    /// Packet magic was not `ORGB`.
    #[error("invalid OpenRGB packet magic: {0:?}")]
    InvalidMagic([u8; 4]),

    /// Packet body exceeds the configured safety limit.
    #[error("OpenRGB packet payload size {size} exceeds limit {max}")]
    PacketTooLarge { size: usize, max: usize },

    /// A packet or structure ended before the requested field was available.
    #[error("truncated OpenRGB data: needed {needed} bytes, remaining {remaining}")]
    Truncated { needed: usize, remaining: usize },

    /// A count multiplied by element size overflowed or exceeded allowed bounds.
    #[error("OpenRGB count overflow: count {count}, element size {element_size}")]
    CountOverflow { count: usize, element_size: usize },

    /// A string field did not include the documented trailing NUL byte.
    #[error("OpenRGB string field is missing its trailing NUL byte")]
    StringMissingNul,

    /// A documented UTF-8 string contained invalid bytes.
    #[error("OpenRGB string field contains invalid UTF-8")]
    InvalidUtf8,

    /// A data block advertised a size that did not match the received payload.
    #[error("OpenRGB data block size mismatch: advertised {advertised}, actual {actual}")]
    DataSizeMismatch { advertised: usize, actual: usize },

    /// Matrix byte length was malformed.
    #[error("OpenRGB zone matrix byte length {0} is invalid")]
    InvalidMatrixLength(usize),

    /// The requested protocol version is outside this crate's supported range.
    #[error("OpenRGB protocol version {version} is outside supported range {min}..={max}")]
    UnsupportedProtocolVersion { version: u32, min: u32, max: u32 },

    /// The caller tried to encode a packet this crate intentionally forbids.
    #[error("OpenRGB packet {0:?} is forbidden for Hypercolor clients")]
    ForbiddenPacket(PacketId),

    /// A request received a different packet ID than expected.
    #[error("unexpected OpenRGB packet: expected {expected:?}, got {actual:?}")]
    UnexpectedPacket {
        expected: PacketId,
        actual: PacketId,
    },

    /// A timed operation exceeded its configured deadline.
    #[error("OpenRGB {operation} timed out")]
    Timeout { operation: &'static str },

    /// The TCP peer closed the stream while a packet was expected.
    #[error("OpenRGB connection closed")]
    ConnectionClosed,

    /// Socket I/O failed.
    #[error("OpenRGB I/O error: {0}")]
    Io(String),
}

impl From<std::io::Error> for OpenRgbError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value.to_string())
    }
}
