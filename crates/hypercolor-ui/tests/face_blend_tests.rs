//! Contract tests for the shared face-composition vocabulary.

use hypercolor_types::scene::DisplayFaceBlendMode;
use hypercolor_ui::face_blend::{
    FACE_BLEND_OPTIONS, FACE_BLEND_PRESETS, face_blend_option, face_blend_select_options,
    face_blend_value, parse_face_blend,
};

#[test]
fn every_blend_mode_round_trips_through_wire_tokens() {
    for option in FACE_BLEND_OPTIONS {
        assert_eq!(parse_face_blend(face_blend_value(option.mode)), option.mode);
    }
}

#[test]
fn alpha_presents_as_cutout() {
    assert_eq!(
        face_blend_option(DisplayFaceBlendMode::Alpha).label,
        "Cutout"
    );
}

#[test]
fn unknown_wire_token_falls_back_to_blended() {
    assert_eq!(parse_face_blend("nonsense"), DisplayFaceBlendMode::Alpha);
}

#[test]
fn select_options_cover_the_full_table_in_order() {
    let options = face_blend_select_options();
    assert_eq!(options.len(), FACE_BLEND_OPTIONS.len());
    for (option, (value, label)) in FACE_BLEND_OPTIONS.iter().zip(&options) {
        assert_eq!(value, face_blend_value(option.mode));
        assert_eq!(label, option.label);
    }
}

#[test]
fn presets_reference_presentable_modes_with_valid_opacity() {
    for preset in FACE_BLEND_PRESETS {
        assert_eq!(face_blend_option(preset.mode).mode, preset.mode);
        assert!((0.0..=1.0).contains(&preset.opacity));
        // A preset that names a non-blending mode would render a dead
        // Blend Amount slider; every quick look must blend.
        assert!(preset.mode.blends_with_effect());
    }
}
