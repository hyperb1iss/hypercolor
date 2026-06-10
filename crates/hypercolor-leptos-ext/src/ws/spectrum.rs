//! Binary spectrum frame codec — tag `0x02` on the daemon WebSocket.
//!
//! Layout (all integers little-endian):
//!
//! ```text
//! 0       tag (0x02)
//! 1..5    timestamp_ms: u32
//! 5       bin_count: u8
//! 6..10   level: f32
//! 10..14  bass: f32
//! 14..18  mid: f32
//! 18..22  treble: f32
//! 22      beat: u8 (0|1)
//! 23..27  beat_confidence: f32
//! 27..    bins: bin_count × f32
//! ```

use bytes::{BufMut, Bytes, BytesMut};
use thiserror::Error;

pub const SPECTRUM_FRAME_TAG: u8 = 0x02;
pub const SPECTRUM_FRAME_HEADER_LEN: usize = 27;

/// A decoded audio spectrum snapshot.
///
/// BPM is not part of the binary format — clients that need it read the
/// metrics channel instead.
#[derive(Debug, Clone, PartialEq)]
pub struct SpectrumFrame {
    pub timestamp_ms: u32,
    pub level: f32,
    pub bass: f32,
    pub mid: f32,
    pub treble: f32,
    pub beat: bool,
    pub beat_confidence: f32,
    pub bins: Vec<f32>,
}

impl SpectrumFrame {
    /// Number of bins that fit the wire format (`bin_count` is a `u8`).
    #[must_use]
    pub fn encoded_bin_count(&self) -> usize {
        self.bins.len().min(usize::from(u8::MAX))
    }

    #[must_use]
    pub fn encoded_len(&self) -> usize {
        SPECTRUM_FRAME_HEADER_LEN.saturating_add(self.encoded_bin_count().saturating_mul(4))
    }

    #[must_use]
    pub fn encode(&self) -> Bytes {
        let bin_count = self.encoded_bin_count();
        let mut out = BytesMut::with_capacity(self.encoded_len());
        out.put_u8(SPECTRUM_FRAME_TAG);
        out.put_u32_le(self.timestamp_ms);
        out.put_u8(u8::try_from(bin_count).unwrap_or(u8::MAX));
        out.put_f32_le(self.level);
        out.put_f32_le(self.bass);
        out.put_f32_le(self.mid);
        out.put_f32_le(self.treble);
        out.put_u8(u8::from(self.beat));
        out.put_f32_le(self.beat_confidence);
        for bin in self.bins.iter().take(bin_count) {
            out.put_f32_le(*bin);
        }
        out.freeze()
    }

    pub fn decode(input: &[u8]) -> Result<Self, SpectrumFrameDecodeError> {
        if input.len() < SPECTRUM_FRAME_HEADER_LEN {
            return Err(SpectrumFrameDecodeError::TooShort {
                expected: SPECTRUM_FRAME_HEADER_LEN,
                actual: input.len(),
            });
        }
        if input[0] != SPECTRUM_FRAME_TAG {
            return Err(SpectrumFrameDecodeError::UnknownTag { actual: input[0] });
        }

        let bin_count = usize::from(input[5]);
        let bins_end = SPECTRUM_FRAME_HEADER_LEN.saturating_add(bin_count.saturating_mul(4));
        if input.len() < bins_end {
            return Err(SpectrumFrameDecodeError::BinsTooShort {
                expected: bin_count,
                actual: input.len().saturating_sub(SPECTRUM_FRAME_HEADER_LEN) / 4,
            });
        }

        let bins = input[SPECTRUM_FRAME_HEADER_LEN..bins_end]
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes(chunk.try_into().expect("chunk has 4 bytes")))
            .collect();

        Ok(Self {
            timestamp_ms: u32::from_le_bytes(input[1..5].try_into().expect("slice has 4 bytes")),
            level: read_f32(input, 6),
            bass: read_f32(input, 10),
            mid: read_f32(input, 14),
            treble: read_f32(input, 18),
            beat: input[22] != 0,
            beat_confidence: read_f32(input, 23),
            bins,
        })
    }
}

fn read_f32(input: &[u8], offset: usize) -> f32 {
    f32::from_le_bytes(
        input[offset..offset + 4]
            .try_into()
            .expect("slice has 4 bytes"),
    )
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SpectrumFrameDecodeError {
    #[error("spectrum frame is too short: expected at least {expected} bytes, got {actual}")]
    TooShort { expected: usize, actual: usize },
    #[error("unknown spectrum frame tag {actual:#04x}")]
    UnknownTag { actual: u8 },
    #[error("spectrum frame bins are truncated: expected {expected} bins, got {actual}")]
    BinsTooShort { expected: usize, actual: usize },
}
