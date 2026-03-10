//! Razer HID report CRC helpers.

use zerocopy::byteorder::{LittleEndian, U16};
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

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
/// The checksum is XOR of bytes `2..=87` (86 bytes) and is stored at
/// offset `88`. This runs on every USB report during animation — hot path.
///
/// Uses u64-wide XOR accumulation (ported from uchroma) to process 8 bytes
/// at a time, then folds the accumulator down to a single byte.
#[must_use]
pub fn razer_crc(report: &RazerReport) -> u8 {
    let slice = &report.as_bytes()[2..88]; // 86 bytes

    let chunks = slice.chunks_exact(8);
    let remainder = chunks.remainder();

    let mut acc: u64 = 0;
    for chunk in chunks {
        // chunks_exact guarantees 8 bytes — infallible conversion
        let val = u64::from_ne_bytes(
            chunk
                .try_into()
                .expect("chunks_exact(8) guarantees 8-byte slices"),
        );
        acc ^= val;
    }

    // Horizontal XOR: fold all 8 bytes of the accumulator into one
    let bytes = acc.to_ne_bytes();
    let mut result = bytes[0] ^ bytes[1] ^ bytes[2] ^ bytes[3]
        ^ bytes[4] ^ bytes[5] ^ bytes[6] ^ bytes[7];

    for &byte in remainder {
        result ^= byte;
    }

    result
}
