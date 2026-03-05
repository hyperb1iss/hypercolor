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
