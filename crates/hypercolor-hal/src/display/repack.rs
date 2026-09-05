//! Pixel repacking for raw-framebuffer display panels.
//!
//! Panels that take a raw framebuffer want packed 16-bit pixels laid out in
//! fixed-stride lines, often with filler bytes after the pixel region and a
//! repeating XOR mask over the whole line (a signal-integrity shroud on the
//! USB wire, not a pixel transform). This module owns that conversion so the
//! per-driver code stays wire framing.

/// Packed 16-bit pixel layouts, written little-endian.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Packed16Format {
    /// `rrrrrggg gggbbbbb`: red occupies the high five bits.
    Rgb565,

    /// `bbbbbggg gggrrrrr`: blue occupies the high five bits.
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
            Self::Rgb565 => (red5 << 11) | (green6 << 5) | blue5,
            Self::Bgr565 => (blue5 << 11) | (green6 << 5) | red5,
        }
    }
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
}

impl LineRepack<'_> {
    /// Bytes the packed pixels of one line occupy.
    #[must_use]
    pub const fn packed_line_bytes(&self) -> usize {
        self.width.saturating_mul(2)
    }

    /// Total output length for this geometry.
    ///
    /// # Errors
    ///
    /// Returns [`RepackError::Geometry`] when the geometry overflows `usize`.
    pub fn output_len(&self) -> Result<usize, RepackError> {
        self.line_len
            .checked_mul(self.height)
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
            out.clear();
            out.resize(total, 0);
        }

        for row in 0..self.height {
            let source_row = &source[row * source_stride..(row + 1) * source_stride];
            let line = &mut out[row * self.line_len..(row + 1) * self.line_len];

            for (pixel, packed) in source_row
                .chunks_exact(3)
                .zip(line[..packed_line_bytes].chunks_exact_mut(2))
            {
                packed
                    .copy_from_slice(&self.format.pack(pixel[0], pixel[1], pixel[2]).to_le_bytes());
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
}
