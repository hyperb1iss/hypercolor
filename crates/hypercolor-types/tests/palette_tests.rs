//! Tests for the palette registry.

use hypercolor_types::palette::{PALETTE_COUNT, Palette};

#[test]
fn all_palettes_have_correct_count() {
    assert_eq!(Palette::ALL.len(), PALETTE_COUNT);
    assert_eq!(Palette::names().len(), PALETTE_COUNT);
}

#[test]
fn palette_index_is_sequential() {
    for (i, palette) in Palette::ALL.iter().enumerate() {
        assert_eq!(palette.index(), i);
    }
}

#[test]
fn palette_display_name_matches_registry() {
    assert_eq!(Palette::SilkCircuit.display_name(), "SilkCircuit");
    assert_eq!(Palette::CherryBlossom.display_name(), "Cherry Blossom");
    assert_eq!(Palette::CottonCandy.display_name(), "Cotton Candy");
    assert_eq!(Palette::Phosphor.display_name(), "Phosphor");
}

#[test]
fn palette_from_id_roundtrip() {
    for palette in Palette::ALL {
        let id = palette.id();
        let resolved = Palette::from_id(id);
        assert_eq!(resolved, Some(palette), "failed for id: {id}");
    }
}

#[test]
fn palette_from_id_unknown_returns_none() {
    assert_eq!(Palette::from_id("nonexistent"), None);
}

#[test]
fn palette_sample_endpoints() {
    for palette in Palette::ALL {
        let start = palette.sample(0.0);
        let end = palette.sample(1.0);
        // Colors should be valid (0.0-1.0 range)
        for channel in start.into_iter().chain(end) {
            assert!((0.0..=1.0).contains(&channel), "out of range for {palette}");
        }
    }
}

#[test]
fn palette_sample_endpoints_match_stops() {
    // Oklab interpolation must round-trip the exact stop colors at t=0 and t=1.
    for palette in Palette::ALL {
        let stops = palette.stops();
        let first = stops.first().expect("palette has stops");
        let last = stops.last().expect("palette has stops");
        let start = palette.sample(0.0);
        let end = palette.sample(1.0);
        for channel in 0..3 {
            assert!(
                (start[channel] - first[channel]).abs() < 0.01,
                "{palette} start channel {channel}: {} vs stop {}",
                start[channel],
                first[channel]
            );
            assert!(
                (end[channel] - last[channel]).abs() < 0.01,
                "{palette} end channel {channel}: {} vs stop {}",
                end[channel],
                last[channel]
            );
        }
    }
}

#[test]
fn palette_sample_interpolates_in_oklab_like_canvas_runtime() {
    // Reference values computed with the canvas SDK runtime math
    // (sdk/packages/core/src/palette/runtime.ts): srgb -> linear -> Oklab,
    // lerp, back to srgb. Raw-sRGB lerp would give very different values
    // (e.g. Cyberpunk t=0.5 would be (0.5, 0.5, 0.698)).
    let cases: [(Palette, f32, [f32; 3]); 3] = [
        (Palette::Cyberpunk, 0.5, [0.8244, 0.6585, 0.6939]),
        (Palette::Ocean, 0.5, [0.0837, 0.6332, 0.8340]),
        // Midpoint of Cyberpunk's #ff0066 -> #6600ff (red <-> blue) segment.
        (Palette::Cyberpunk, 5.0 / 6.0, [0.6848, 0.2500, 0.7247]),
    ];
    for (palette, t, expected) in cases {
        let got = palette.sample(t);
        for channel in 0..3 {
            assert!(
                (got[channel] - expected[channel]).abs() < 0.01,
                "{palette} t={t} channel {channel}: got {} expected {}",
                got[channel],
                expected[channel]
            );
        }
    }
}

#[test]
fn palette_red_blue_midpoint_is_not_gray_mud() {
    // The midpoint of a red <-> blue span must stay saturated and bright, not
    // collapse into gray or murky half-brightness purple.
    let mid = Palette::Cyberpunk.sample(5.0 / 6.0);
    let max = mid[0].max(mid[1]).max(mid[2]);
    let min = mid[0].min(mid[1]).min(mid[2]);
    assert!(max > 0.5, "midpoint too dark: {mid:?}");
    assert!(
        (max - min) / max > 0.5,
        "midpoint desaturated to mud: {mid:?}"
    );

    let ocean_mid = Palette::Ocean.sample(0.5);
    let max = ocean_mid[0].max(ocean_mid[1]).max(ocean_mid[2]);
    let min = ocean_mid[0].min(ocean_mid[1]).min(ocean_mid[2]);
    assert!(
        (max - min) / max > 0.6,
        "ocean midpoint desaturated: {ocean_mid:?}"
    );
}

#[test]
fn palette_sample_iq_is_clamped() {
    for palette in Palette::ALL {
        for t in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let color = palette.sample_iq(t);
            for channel in color {
                assert!(
                    (0.0..=1.0).contains(&channel),
                    "IQ out of range for {palette} at t={t}"
                );
            }
        }
    }
}

#[test]
fn palette_by_mood_returns_results() {
    let warm = Palette::by_mood("warm");
    assert!(!warm.is_empty(), "expected palettes with 'warm' mood");
    assert!(warm.contains(&Palette::Ember));
    assert!(warm.contains(&Palette::Sunset));

    let dark = Palette::by_mood("dark");
    assert!(!dark.is_empty(), "expected palettes with 'dark' mood");
    assert!(dark.contains(&Palette::Cyberpunk));
    assert!(dark.contains(&Palette::Matrix));
}

#[test]
fn palette_by_mood_case_insensitive() {
    let upper = Palette::by_mood("WARM");
    let lower = Palette::by_mood("warm");
    assert_eq!(upper, lower);
}

#[test]
fn palette_stops_not_empty() {
    for palette in Palette::ALL {
        assert!(
            !palette.stops().is_empty(),
            "palette {palette} has no stops"
        );
    }
}

#[test]
fn palette_accent_and_background_valid() {
    for palette in Palette::ALL {
        for channel in palette.accent() {
            assert!((0.0..=1.0).contains(&channel));
        }
        for channel in palette.background() {
            assert!((0.0..=1.0).contains(&channel));
        }
    }
}

#[test]
fn palette_serde_round_trip() {
    for palette in Palette::ALL {
        let json = serde_json::to_string(&palette).expect("serialize");
        let back: Palette = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, palette);
    }
}

#[test]
fn silkcircuit_is_our_signature_palette() {
    let sc = Palette::SilkCircuit;
    assert_eq!(sc.id(), "silkcircuit");
    assert!(sc.mood().iter().any(|m| m == "electric"));
    // Accent should be Electric Purple (#e135ff)
    let accent = sc.accent();
    assert!(accent[0] > 0.8, "accent red channel should be bright");
    assert!(accent[2] > 0.9, "accent blue channel should be bright");
}
