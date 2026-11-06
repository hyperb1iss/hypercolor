//! DMX512 universe addressing shared by E1.31 and Art-Net.
//!
//! Both protocols carry one DMX512 universe per packet. A pixel strip
//! longer than one universe is laid out as a continuous channel stream
//! that wraps into consecutive universes. This module does that math.

use std::ops::Range;

/// DMX512 data slots per universe, not counting the START code.
pub const DMX_UNIVERSE_CHANNELS: usize = 512;

/// Largest per-universe channel count that holds only whole pixels.
///
/// 510 for 3-channel pixels (170 pixels), 512 for 4-channel pixels (128).
/// Returns 512 for a zero stride so callers never divide by zero.
#[must_use]
pub const fn pixel_aligned_universe_channels(channels_per_pixel: usize) -> usize {
    if channels_per_pixel == 0 || channels_per_pixel > DMX_UNIVERSE_CHANNELS {
        return DMX_UNIVERSE_CHANNELS;
    }
    (DMX_UNIVERSE_CHANNELS / channels_per_pixel) * channels_per_pixel
}

/// Number of universes a run of `channel_count` channels touches when it
/// starts at zero-based `start_slot` of the first universe.
#[must_use]
pub const fn universes_spanned(
    start_slot: usize,
    channel_count: usize,
    channels_per_universe: usize,
) -> usize {
    if channel_count == 0 || channels_per_universe == 0 {
        return 0;
    }
    let first = start_slot / channels_per_universe;
    let last = (start_slot + channel_count - 1) / channels_per_universe;
    last - first + 1
}

/// One slice of a channel stream that lands inside a single universe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UniverseRun {
    /// Universe offset from the stream's first universe (0 = first).
    pub universe_offset: usize,
    /// Zero-based DMX slot where this run starts inside its universe.
    pub first_slot: usize,
    /// Range of the source channel stream carried by this run.
    pub source: Range<usize>,
}

/// Split a channel stream into per-universe runs.
///
/// `start_slot` is the zero-based position of the stream's first channel,
/// counted from slot 0 of the first universe; it may exceed one universe.
/// `channels_per_universe` is clamped to 1-512.
#[must_use]
pub fn split_into_universes(
    start_slot: usize,
    channel_count: usize,
    channels_per_universe: usize,
) -> Vec<UniverseRun> {
    let per_universe = channels_per_universe.clamp(1, DMX_UNIVERSE_CHANNELS);
    let mut runs = Vec::with_capacity(universes_spanned(start_slot, channel_count, per_universe));
    let mut consumed = 0;
    while consumed < channel_count {
        let absolute = start_slot + consumed;
        let universe_offset = absolute / per_universe;
        let first_slot = absolute % per_universe;
        let take = (per_universe - first_slot).min(channel_count - consumed);
        runs.push(UniverseRun {
            universe_offset,
            first_slot,
            source: consumed..consumed + take,
        });
        consumed += take;
    }
    runs
}
