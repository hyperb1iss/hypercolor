//! DMX512 universe math shared by E1.31 and Art-Net.

use hypercolor_pixel_protocols::dmx::{
    DMX_UNIVERSE_CHANNELS, UniverseRun, pixel_aligned_universe_channels, split_into_universes,
    universes_spanned,
};

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
    let runs = split_into_universes(0, 900, 510);
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
    let runs = split_into_universes(500, 30, 512);
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
    let runs = split_into_universes(1020 + 3, 6, 510);
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
    assert!(split_into_universes(0, 0, 510).is_empty());
}
