//! Razer HID report CRC helpers.

use zerocopy::{FromBytes, IntoBytes, KnownLayout, Immutable};
use zerocopy::byteorder::{U16, LittleEndian};

/// Razer HID packet size in bytes.
pub const RAZER_REPORT_LEN: usize = 90;

/// Wire-format Razer HID report (90 bytes, fixed layout).
///
/// Derives `FromBytes`/`IntoBytes` from the `zerocopy` crate for safe,
/// zero-cost reinterpretation between typed fields and raw `&[u8]`.
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct RazerReport {
    /// Response status (0x00 for outgoing requests).
    pub status: u8,
    /// Transaction ID — encodes protocol version (0xFF, 0x3F, 0x1F, etc.).
    pub transaction_id: u8,
    /// Remaining packets in multi-packet transfers (little-endian).
    pub remaining_packets: U16<LittleEndian>,
    /// Protocol type marker (always 0x00).
    pub protocol_type: u8,
    /// Declared argument payload size.
    pub data_size: u8,
    /// Command class (0x00 = device, 0x03 = standard, 0x0F = extended).
    pub command_class: u8,
    /// Command ID within the class.
    pub command_id: u8,
    /// Variable-length argument field (up to 80 bytes).
    pub args: [u8; 80],
    /// XOR checksum of bytes `[2..88]`.
    pub crc: u8,
    /// Reserved trailing byte (always 0x00).
    pub reserved: u8,
}

/// Compute the Razer XOR checksum over a typed [`RazerReport`].
///
/// The checksum is XOR of bytes `2..=87` and is stored at offset `88`.
#[must_use]
pub fn razer_crc(report: &RazerReport) -> u8 {
    let bytes = report.as_bytes();
    bytes[2..88].iter().fold(0_u8, |acc, byte| acc ^ byte)
}
