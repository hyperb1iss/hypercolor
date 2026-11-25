//! Pixel repacking for raw-framebuffer display panels.
//!
//! Panels that take a raw framebuffer want packed 16-bit pixels laid out in
//! fixed-stride lines, often with filler bytes after the pixel region and a
//! repeating XOR mask over the whole line (a signal-integrity shroud on the
//! USB wire, not a pixel transform). Some take the frame turned a quarter,
//! because the glass is mounted across its own scan direction. This module
//! owns that conversion so the per-driver code stays wire framing.

/// Packed 16-bit pixel layouts as they go on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Packed16Format {
    /// `rrrrrggg gggbbbbb`, low byte first: red occupies the high five bits.
    Rgb565,

    /// The [`Self::Rgb565`] word written high byte first.
    Rgb565BigEndian,

    /// `bbbbbggg gggrrrrr`, low byte first: blue occupies the high five bits.
    Bgr565,
}

impl Packed16Format {
    /// Pack one RGB888 pixel into its 16-bit representation.
    #[must_use]
    pub const fn pack(self, red: u8, green: u8, blue: u8) -> u16 {
        let red5 = (red >> 3) as u16;
        let green6 = (green >> 2) as u16;
        let blue5 = (blue >> 3) as u16;

        match self {
            Self::Rgb565 | Self::Rgb565BigEndian => (red5 << 11) | (green6 << 5) | blue5,
            Self::Bgr565 => (blue5 << 11) | (green6 << 5) | red5,
        }
    }

    /// Pack one RGB888 pixel into the two bytes the wire carries.
    #[must_use]
    pub const fn wire_bytes(self, red: u8, green: u8, blue: u8) -> [u8; 2] {
        let packed = self.pack(red, green, blue);
        match self {
            Self::Rgb565BigEndian => packed.to_be_bytes(),
            Self::Rgb565 | Self::Bgr565 => packed.to_le_bytes(),
        }
    }
}

/// How a frame is turned while it is repacked.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum QuarterTurn {
    /// Lines are the source rows, top to bottom.
    #[default]
    None,

    /// The frame is turned 90 degrees clockwise: output line `n` is source
    /// column `n` read from the bottom row up, so a `width`x`height` source
    /// becomes `width` lines of `height` pixels.
    Clockwise,
}

/// Repack geometry: one source frame in, one line-strided framebuffer out.
#[derive(Debug, Clone, Copy)]
pub struct LineRepack<'a> {
    /// Pixels per source row.
    pub width: usize,

    /// Rows in the source frame.
    pub height: usize,

    /// Packed output pixel format.
    pub format: Packed16Format,

    /// Total output bytes per line, including filler after the pixels.
    pub line_len: usize,

    /// Byte written into every filler slot, before masking.
    pub filler: u8,

    /// Repeating XOR mask applied across every output byte of a line, phase
    /// aligned to the start of that line. Empty means no masking.
    pub xor_mask: &'a [u8],

    /// Turn applied to the source frame. `width` and `height` always
    /// describe the source; the turn decides what a line holds.
    pub turn: QuarterTurn,
}

impl LineRepack<'_> {
    /// Pixels one output line holds.
    #[must_use]
    pub const fn line_pixels(&self) -> usize {
        match self.turn {
            QuarterTurn::None => self.width,
            QuarterTurn::Clockwise => self.height,
        }
    }

    /// Output lines one frame produces.
    #[must_use]
    pub const fn line_count(&self) -> usize {
        match self.turn {
            QuarterTurn::None => self.height,
            QuarterTurn::Clockwise => self.width,
        }
    }

    /// Bytes the packed pixels of one line occupy.
    #[must_use]
    pub const fn packed_line_bytes(&self) -> usize {
        self.line_pixels().saturating_mul(2)
    }

    /// Total output length for this geometry.
    ///
    /// # Errors
    ///
    /// Returns [`RepackError::Geometry`] when the geometry overflows `usize`.
    pub fn output_len(&self) -> Result<usize, RepackError> {
        self.line_len
            .checked_mul(self.line_count())
            .ok_or(RepackError::Geometry {
                width: self.width,
                height: self.height,
                line_len: self.line_len,
            })
    }

    /// Repack an RGB888 frame into `out`, sized to exactly [`Self::output_len`].
    ///
    /// Every output byte is written on every call, so a buffer reused across
    /// frames can never carry stale pixels.
    ///
    /// # Errors
    ///
    /// Returns [`RepackError`] when `source` is shorter than the geometry
    /// needs, when a line cannot hold its packed pixels, or when the geometry
    /// overflows `usize`.
    pub fn repack_rgb888(&self, source: &[u8], out: &mut Vec<u8>) -> Result<(), RepackError> {
        let total = self.output_len()?;
        if out.len() != total {
            out.clear();
            out.resize(total, 0);
        }
        self.repack_rgb888_into(source, out)
    }

    /// Repack an RGB888 frame into a slice of exactly [`Self::output_len`]
    /// bytes, for a framebuffer that sits inside a larger wire buffer.
    ///
    /// # Errors
    ///
    /// Returns [`RepackError`] for the conditions [`Self::repack_rgb888`]
    /// names, and [`RepackError::OutputLength`] when `out` is not exactly
    /// the output length.
    pub fn repack_rgb888_into(&self, source: &[u8], out: &mut [u8]) -> Result<(), RepackError> {
        let packed_line_bytes = self.packed_line_bytes();
        if self.line_len < packed_line_bytes {
            return Err(RepackError::LineTooShort {
                line_len: self.line_len,
                required: packed_line_bytes,
            });
        }

        let source_stride = self
            .width
            .checked_mul(3)
            .ok_or_else(|| self.geometry_error())?;
        let required = source_stride
            .checked_mul(self.height)
            .ok_or_else(|| self.geometry_error())?;
        if source.len() < required {
            return Err(RepackError::SourceTooSmall {
                expected: required,
                actual: source.len(),
                width: self.width,
                height: self.height,
            });
        }

        let total = self.output_len()?;
        if out.len() != total {
            return Err(RepackError::OutputLength {
                expected: total,
                actual: out.len(),
            });
        }

        for line_index in 0..self.line_count() {
            let line = &mut out[line_index * self.line_len..(line_index + 1) * self.line_len];
            let packed_line = line[..packed_line_bytes].as_chunks_mut::<2>().0;

            match self.turn {
                QuarterTurn::None => {
                    let source_row =
                        &source[line_index * source_stride..(line_index + 1) * source_stride];
                    for (pixel, packed) in source_row.as_chunks::<3>().0.iter().zip(packed_line) {
                        *packed = self.format.wire_bytes(pixel[0], pixel[1], pixel[2]);
                    }
                }
                QuarterTurn::Clockwise => {
                    let column = line_index * 3;
                    for (offset, packed) in packed_line.iter_mut().enumerate() {
                        let start = (self.height - 1 - offset) * source_stride + column;
                        let pixel = &source[start..start + 3];
                        *packed = self.format.wire_bytes(pixel[0], pixel[1], pixel[2]);
                    }
                }
            }

            line[packed_line_bytes..].fill(self.filler);

            if !self.xor_mask.is_empty() {
                for (byte, mask) in line.iter_mut().zip(self.xor_mask.iter().cycle()) {
                    *byte ^= *mask;
                }
            }
        }

        Ok(())
    }

    fn geometry_error(&self) -> RepackError {
        RepackError::Geometry {
            width: self.width,
            height: self.height,
            line_len: self.line_len,
        }
    }
}

/// Pixel repack failures.
#[derive(Debug, thiserror::Error)]
pub enum RepackError {
    /// Source frame is shorter than the declared geometry needs.
    #[error(
        "RGB888 source of {actual} bytes is short of the {expected} bytes {width}x{height} needs"
    )]
    SourceTooSmall {
        /// Bytes the geometry requires.
        expected: usize,

        /// Bytes the caller supplied.
        actual: usize,

        /// Declared source width in pixels.
        width: usize,

        /// Declared source height in rows.
        height: usize,
    },

    /// Output line cannot hold the packed pixels of a row.
    #[error("line length {line_len} cannot hold {required} packed pixel bytes")]
    LineTooShort {
        /// Declared output line length.
        line_len: usize,

        /// Bytes the packed pixels need.
        required: usize,
    },

    /// Declared geometry overflows the address space.
    #[error("geometry {width}x{height} with {line_len}-byte lines overflows usize")]
    Geometry {
        /// Declared source width in pixels.
        width: usize,

        /// Declared source height in rows.
        height: usize,

        /// Declared output line length.
        line_len: usize,
    },

    /// Output slice is not the length the geometry produces.
    #[error("output of {actual} bytes does not match the {expected} bytes the geometry produces")]
    OutputLength {
        /// Bytes the geometry produces.
        expected: usize,

        /// Bytes the caller supplied.
        actual: usize,
    },
}
