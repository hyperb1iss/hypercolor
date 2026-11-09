//! DMX512 universe math shared by E1.31 and Art-Net.

use hypercolor_pixel_protocols::dmx::{
    DMX_UNIVERSE_CHANNELS, UniverseRun, last_universe_offset, pixel_aligned_universe_channels,
    split_into_universes, universes_spanned,
};

fn runs(start_slot: usize, channel_count: usize, per_universe: usize) -> Vec<UniverseRun> {
    split_into_universes(start_slot, channel_count, per_universe).collect()
}

#[test]
fn pixel_aligned_universes_hold_whole_pixels() {
    assert_eq!(DMX_UNIVERSE_CHANNELS, 512);
    assert_eq!(pixel_aligned_universe_channels(3), 510);
    assert_eq!(pixel_aligned_universe_channels(4), 512);
    assert_eq!(pixel_aligned_universe_channels(5), 510);
    assert_eq!(pixel_aligned_universe_channels(0), 512);
}

#[test]
fn rgb_strip_wraps_at_170_pixels() {
    // 300 RGB pixels = 900 channels across 510-channel universes.
    let runs = runs(0, 900, 510);
    assert_eq!(
        runs,
        vec![
            UniverseRun {
                universe_offset: 0,
                first_slot: 0,
                source: 0..510,
            },
            UniverseRun {
                universe_offset: 1,
                first_slot: 0,
                source: 510..900,
            },
        ]
    );
}

#[test]
fn start_slot_offsets_first_universe() {
    // Start at DMX channel 501 (slot 500) of a 512-channel universe.
    let runs = runs(500, 30, 512);
    assert_eq!(
        runs,
        vec![
            UniverseRun {
                universe_offset: 0,
                first_slot: 500,
                source: 0..12,
            },
            UniverseRun {
                universe_offset: 1,
                first_slot: 0,
                source: 12..30,
            },
        ]
    );
}

#[test]
fn start_slot_beyond_one_universe_lands_later() {
    let runs = runs(1020 + 3, 6, 510);
    assert_eq!(
        runs,
        vec![UniverseRun {
            universe_offset: 2,
            first_slot: 3,
            source: 0..6,
        }]
    );
}

#[test]
fn universes_spanned_counts_partial_universes() {
    assert_eq!(universes_spanned(0, 510, 510), 1);
    assert_eq!(universes_spanned(0, 511, 510), 2);
    assert_eq!(universes_spanned(509, 2, 510), 2);
    assert_eq!(universes_spanned(0, 0, 510), 0);
}

#[test]
fn empty_stream_has_no_runs() {
    assert!(runs(0, 0, 510).is_empty());
}

#[test]
fn last_universe_is_computed_without_allocating_impossible_runs() {
    // usize::MAX one-channel universes could never be allocated as runs; the
    // check is pure arithmetic, which is what lets callers reject first.
    assert_eq!(last_universe_offset(0, usize::MAX, 1), Some(usize::MAX - 1));
    assert_eq!(last_universe_offset(0, 510, 510), Some(0));
    assert_eq!(last_universe_offset(1, 510, 510), Some(1));
    assert_eq!(
        last_universe_offset(usize::MAX, 2, 1),
        None,
        "overflowing end"
    );
    assert_eq!(last_universe_offset(0, 0, 510), None, "empty run");
    assert_eq!(last_universe_offset(0, 10, 0), None, "zero stride");
}

#[test]
fn split_is_lazy_for_streams_no_allocator_could_hold() {
    let mut runs = split_into_universes(0, usize::MAX, 1);
    assert_eq!(runs.next().map(|run| run.universe_offset), Some(0));
    assert_eq!(runs.next().map(|run| run.source), Some(1..2));
}

#[test]
fn split_stops_where_the_absolute_slot_would_overflow() {
    assert!(split_into_universes(usize::MAX, 3, 512).nth(1).is_none());
}
