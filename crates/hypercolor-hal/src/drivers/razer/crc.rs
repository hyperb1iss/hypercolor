//! Razer HID report CRC helpers.

/// Razer HID packet size in bytes.
pub const RAZER_REPORT_LEN: usize = 90;

/// Compute the Razer XOR checksum for a 90-byte report.
///
/// The checksum is XOR of bytes `2..=87` and is stored at offset `88`.
#[must_use]
pub fn razer_crc(buf: &[u8; RAZER_REPORT_LEN]) -> u8 {
    buf[2..88].iter().fold(0_u8, |acc, byte| acc ^ byte)
}
