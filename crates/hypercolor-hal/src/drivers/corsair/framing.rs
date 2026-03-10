//! Shared framing helpers for Corsair packet formats.

use zerocopy::byteorder::{LittleEndian, U16};
use zerocopy::{FromZeros, Immutable, IntoBytes, KnownLayout};

/// LINK Hub write buffer geometry.
pub const LINK_WRITE_BUF_SIZE: usize = 513;

/// LINK Hub read buffer geometry.
pub const LINK_READ_BUF_SIZE: usize = 512;

/// LINK Hub maximum per-command payload size.
pub const LINK_MAX_PAYLOAD: usize = 508;

/// Lighting Node write buffer size.
pub const LN_WRITE_BUF_SIZE: usize = 65;

/// Lighting Node read buffer size.
pub const LN_READ_BUF_SIZE: usize = 17;

/// Corsair LCD bulk packet size.
pub const LCD_PACKET_SIZE: usize = 1_024;

/// JPEG payload capacity per LCD bulk packet.
pub const LCD_DATA_PER_PACKET: usize = 1_016;

/// Corsair LCD HID report size.
pub const LCD_REPORT_SIZE: usize = 32;

const _: () = assert!(
    std::mem::size_of::<LcdDisplayPacket>() == LCD_PACKET_SIZE,
    "LcdDisplayPacket must match LCD_PACKET_SIZE (1024 bytes)"
);

/// Wire-format Corsair LCD display bulk packet (1024 bytes).
///
/// Each packet carries up to 1016 bytes of JPEG payload for one display
/// zone. The final packet in a sequence sets `is_final` to `0x01`.
#[derive(FromZeros, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
struct LcdDisplayPacket {
    /// Command marker (always `0x02`).
    command: u8,
    /// Sub-command marker (always `0x05`).
    sub_command: u8,
    /// Target display zone.
    zone: u8,
    /// `0x01` for the final packet in the sequence, `0x00` otherwise.
    is_final: u8,
    /// Packet sequence number.
    packet_number: u8,
    /// Reserved (always `0x00`).
    reserved: u8,
    /// Declared payload length (always `LCD_DATA_PER_PACKET`, little-endian).
    data_length: U16<LittleEndian>,
    /// JPEG payload (up to 1016 bytes, zero-padded).
    data: [u8; LCD_DATA_PER_PACKET],
}

/// Pad a byte slice to a fixed length with zeros.
#[must_use]
pub fn pad_to(data: &[u8], len: usize) -> Vec<u8> {
    let mut padded = vec![0_u8; len];
    let copy_len = data.len().min(len);
    padded[..copy_len].copy_from_slice(&data[..copy_len]);
    padded
}

/// Build a 513-byte LINK packet from command bytes and payload bytes.
#[must_use]
pub fn build_link_packet(command: &[u8], data: &[u8]) -> Vec<u8> {
    let mut buf = vec![0_u8; LINK_WRITE_BUF_SIZE];
    buf[2] = 0x01;

    let command_len = command.len().min(LINK_WRITE_BUF_SIZE.saturating_sub(3));
    buf[3..3 + command_len].copy_from_slice(&command[..command_len]);

    let data_offset = 3 + command_len;
    let data_len = data
        .len()
        .min(LINK_WRITE_BUF_SIZE.saturating_sub(data_offset));
    buf[data_offset..data_offset + data_len].copy_from_slice(&data[..data_len]);
    buf
}

/// Build the framed LINK payload for a typed endpoint write.
///
/// Layout:
/// `len_le16 | 00 00 | data_type[2] | payload...`
#[must_use]
pub fn build_link_write_buffer(data_type: [u8; 2], payload: &[u8]) -> Vec<u8> {
    let data_len = u16::try_from(payload.len().saturating_add(2)).unwrap_or(u16::MAX);
    let mut buf = Vec::with_capacity(payload.len().saturating_add(6));
    buf.extend_from_slice(&data_len.to_le_bytes());
    buf.extend_from_slice(&[0x00, 0x00]);
    buf.extend_from_slice(&data_type);
    buf.extend_from_slice(payload);
    buf
}

/// Split bytes into owned chunks of at most `chunk_size`.
#[must_use]
pub fn chunk_bytes(data: &[u8], chunk_size: usize) -> Vec<Vec<u8>> {
    data.chunks(chunk_size).map(<[u8]>::to_vec).collect()
}

/// Build a fixed-size Corsair LCD display packet.
#[must_use]
pub fn build_lcd_display_packet(
    zone_byte: u8,
    final_packet: bool,
    packet_number: u8,
    payload: &[u8],
) -> Vec<u8> {
    let mut buffer = Vec::with_capacity(LCD_PACKET_SIZE);
    append_lcd_display_packet(&mut buffer, zone_byte, final_packet, packet_number, payload);
    buffer
}

/// Append one fixed-size Corsair LCD display packet to an existing buffer.
pub fn append_lcd_display_packet(
    buffer: &mut Vec<u8>,
    zone_byte: u8,
    final_packet: bool,
    packet_number: u8,
    payload: &[u8],
) {
    let mut packet = LcdDisplayPacket::new_zeroed();
    packet.command = 0x02;
    packet.sub_command = 0x05;
    packet.zone = zone_byte;
    packet.is_final = u8::from(final_packet);
    packet.packet_number = packet_number;
    packet.data_length = U16::new(u16::try_from(LCD_DATA_PER_PACKET).unwrap_or(u16::MAX));

    let copy_len = payload.len().min(LCD_DATA_PER_PACKET);
    packet.data[..copy_len].copy_from_slice(&payload[..copy_len]);
    buffer.extend_from_slice(packet.as_bytes());
}

/// Build a fixed-size Corsair LCD HID feature report.
#[must_use]
pub fn build_lcd_report(payload: &[u8]) -> Vec<u8> {
    pad_to(payload, LCD_REPORT_SIZE)
}
