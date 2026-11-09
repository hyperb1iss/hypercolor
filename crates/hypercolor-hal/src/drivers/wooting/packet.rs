//! Wooting report layouts and colour encoders (Spec 90 §4 to §7).

use zerocopy::byteorder::{LittleEndian, U16};
use zerocopy::{FromZeros, Immutable, IntoBytes, KnownLayout};

use super::types::{
    COMMAND_RAW_COLORS, FEATURE_COMMAND_LEN, MAGIC_TAIL, MATRIX_CELLS, MATRIX_COLS,
    V1_PART_PAYLOAD_LEN, V1_REPORT_LEN, V2_REPORT_LEN, V2_SMALL_CHUNK_LEN, V2_SMALL_CHUNK_PAYLOAD,
    V3_REPORT_LEN,
};

/// Wire colour matrix: 6 rows by 21 columns of little-endian RGB565.
pub type Rgb565Matrix = [U16<LittleEndian>; MATRIX_CELLS];

/// Eight-byte feature report that carries every command.
///
/// The four parameter bytes go out in reverse: the vendor SDK names them
/// `parameter0..3` and writes `parameter3` first.
#[derive(Debug, Clone, Copy, FromZeros, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct FeatureCommand {
    /// HID report ID (`0x00` on v1/v2, `0x01` on the multi-report collection).
    pub report_id: u8,
    /// First magic byte (`0xD0`, or `0xD1` on the multi-report collection).
    pub magic_head: u8,
    /// Second magic byte (`0xDA`).
    pub magic_tail: u8,
    /// Command identifier.
    pub command: u8,
    /// Parameters in wire order: `parameter3, parameter2, parameter1, parameter0`.
    pub params: [u8; 4],
}

const _: () = assert!(
    size_of::<FeatureCommand>() == FEATURE_COMMAND_LEN,
    "FeatureCommand must match the 8-byte feature report"
);

impl FeatureCommand {
    /// A parameterless command.
    #[must_use]
    pub const fn new(report_id: u8, magic_head: u8, command: u8) -> Self {
        Self {
            report_id,
            magic_head,
            magic_tail: MAGIC_TAIL,
            command,
            params: [0; 4],
        }
    }
}

/// v2 colour report: the whole matrix in one 256-byte output report.
#[derive(Debug, Clone, FromZeros, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct V2ColorReport {
    /// HID report ID (`0x00`, unnumbered).
    pub report_id: u8,
    /// First magic byte (`0xD0`).
    pub magic_head: u8,
    /// Second magic byte (`0xDA`).
    pub magic_tail: u8,
    /// Raw colours command (`0x0B`).
    pub command: u8,
    /// Row-major RGB565 matrix, 21-column stride.
    pub matrix: Rgb565Matrix,
    /// Zero byte that pads the report to 256 data bytes.
    pub padding: [u8; 1],
}

const _: () = assert!(
    size_of::<V2ColorReport>() == V2_REPORT_LEN,
    "V2ColorReport must match the 257-byte write"
);

/// One quarter of a v2 colour report for collections with 64-byte reports.
#[derive(Debug, Clone, Copy, FromZeros, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct V2SmallChunk {
    /// HID report ID (`0x00`, unnumbered).
    pub report_id: u8,
    /// Sixty-four bytes of the 256-byte v2 report body.
    pub payload: [u8; V2_SMALL_CHUNK_PAYLOAD],
}

const _: () = assert!(
    size_of::<V2SmallChunk>() == V2_SMALL_CHUNK_LEN,
    "V2SmallChunk must match the 65-byte write"
);

/// Multi-report colour write: report `0x05` padded to the collection's
/// longest output report.
#[derive(Debug, Clone, FromZeros, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct V3ColorReport {
    /// HID report ID (`0x05`).
    pub report_id: u8,
    /// First magic byte (`0xD1`).
    pub magic_head: u8,
    /// Second magic byte (`0xDA`).
    pub magic_tail: u8,
    /// Raw colours command (`0x0B`).
    pub command: u8,
    /// Row-major RGB565 matrix, 21-column stride.
    pub matrix: Rgb565Matrix,
    /// Zero padding out to 2047 bytes.
    pub padding: [u8; V3_REPORT_LEN - 4 - 2 * MATRIX_CELLS],
}

const _: () = assert!(
    size_of::<V3ColorReport>() == V3_REPORT_LEN,
    "V3ColorReport must match the 2047-byte write"
);

/// v1 colour report: one 96-byte PWM half-bank of one LED driver chip.
#[derive(Debug, Clone, FromZeros, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct V1ColorReport {
    /// HID report ID (`0x00`, unnumbered).
    pub report_id: u8,
    /// First magic byte (`0xD0`).
    pub magic_head: u8,
    /// Second magic byte (`0xDA`).
    pub magic_tail: u8,
    /// Raw colours command (`0x0B`).
    pub command: u8,
    /// LED driver chip the part addresses.
    pub chip: u8,
    /// Register the 96 bytes start at inside that chip (`0` or `96`).
    pub register: u8,
    /// PWM values for 24 LEDs in the chip's register layout.
    pub pwm: [u8; V1_PART_PAYLOAD_LEN],
    /// Zero padding before the checksum.
    pub padding: [u8; 25],
    /// CRC-16/XMODEM over bytes `0..127`, little-endian.
    pub crc: U16<LittleEndian>,
}

const _: () = assert!(
    size_of::<V1ColorReport>() == V1_REPORT_LEN,
    "V1ColorReport must match the 129-byte write"
);

/// Bytes the v1 checksum covers: everything before the checksum itself.
pub const V1_CRC_COVERED_LEN: usize = V1_REPORT_LEN - 2;

impl V1ColorReport {
    /// Stamp the checksum over the report's first 127 bytes.
    pub fn seal(&mut self) {
        self.crc = U16::new(0);
        let crc = crc16_xmodem(&self.as_bytes()[..V1_CRC_COVERED_LEN]);
        self.crc = U16::new(crc);
    }
}

/// Header shared by v2 and v3 colour reports.
#[must_use]
pub const fn color_header(magic_head: u8) -> [u8; 3] {
    [magic_head, MAGIC_TAIL, COMMAND_RAW_COLORS]
}

/// Pack 8-bit RGB into RGB565: five bits red, six green, five blue.
#[must_use]
pub const fn rgb565(red: u8, green: u8, blue: u8) -> u16 {
    // Widening casts only; `u16::from` is not const.
    ((red as u16 & 0xF8) << 8) | ((green as u16 & 0xFC) << 3) | (blue as u16 >> 3)
}

/// Write `rows` by `cols` colours, row-major, into a zeroed wire matrix,
/// starting at wire row `first_row`.
///
/// Cells outside the model's rows and columns stay black, and so does
/// anything `colors` is too short to cover.
pub fn fill_matrix(
    matrix: &mut Rgb565Matrix,
    colors: &[[u8; 3]],
    first_row: usize,
    rows: usize,
    cols: usize,
) {
    let cols = cols.min(MATRIX_COLS);
    for (row, row_colors) in colors.chunks(cols).take(rows).enumerate() {
        let start = (first_row + row) * MATRIX_COLS;
        let Some(cells) = matrix.get_mut(start..start + row_colors.len()) else {
            return;
        };
        for (cell, &[red, green, blue]) in cells.iter_mut().zip(row_colors) {
            *cell = U16::new(rgb565(red, green, blue));
        }
    }
}

/// CRC-16/XMODEM: polynomial `0x1021`, initial value `0`, no reflection, no
/// final XOR. The v1 colour reports carry it.
#[must_use]
pub fn crc16_xmodem(bytes: &[u8]) -> u16 {
    bytes.iter().fold(0_u16, |crc, &byte| {
        let mut crc = crc ^ (u16::from(byte) << 8);
        for _ in 0..8 {
            crc = if crc & 0x8000 == 0 {
                crc << 1
            } else {
                (crc << 1) ^ 0x1021
            };
        }
        crc
    })
}
