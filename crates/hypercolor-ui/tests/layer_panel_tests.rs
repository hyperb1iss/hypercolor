//! Contract tests for the extracted layer panel (Spec 65 §10).
//!
//! Exercises the leptos-free source vocabulary — the blend/fit options,
//! source labels, and the five-source builders — so the prop/event
//! surface Studio mounts cannot drift unnoticed.

#![allow(dead_code, unused_imports)]

#[path = "../src/components/layer_panel/source.rs"]
mod source;

use std::collections::HashMap;

use hypercolor_types::layer::{LayerBlendMode, LayerSource};
use hypercolor_types::scene::{Zone, ZoneId, ZoneRole};
use hypercolor_types::spatial::{EdgeBehavior, SamplingMode, SpatialLayout};
use hypercolor_types::viewport::FitMode;

use source::{
    AddLayerScope, LayerSourceKind, available_add_layer_scopes, blend_options, blend_value,
    color_layer_source, effect_layer_source, fit_options, fit_value, hex_to_layer_rgba,
    layer_source_label, media_layer_source, parse_blend, parse_fit, resolve_add_layer_targets,
    screen_layer_source, web_layer_source,
};

/// A valid UUID string for effect/media id parsing.
const SAMPLE_ID: &str = "0192f5a0-1234-7890-abcd-ef0123456789";

#[test]
fn picker_exposes_exactly_the_five_layer_sources() {
    assert_eq!(LayerSourceKind::ALL.len(), 5);
    let labels: Vec<&str> = LayerSourceKind::ALL
        .iter()
        .map(|kind| kind.label())
        .collect();
    assert_eq!(
        labels,
        ["Effect", "Media", "Screen Capture", "Web Page", "Color"]
    );
}

#[test]
fn blend_modes_round_trip_through_their_wire_tokens() {
    let modes = [
        LayerBlendMode::Replace,
        LayerBlendMode::Alpha,
        LayerBlendMode::Add,
        LayerBlendMode::Screen,
        LayerBlendMode::Multiply,
        LayerBlendMode::Overlay,
        LayerBlendMode::SoftLight,
        LayerBlendMode::ColorDodge,
        LayerBlendMode::Difference,
        LayerBlendMode::Tint,
        LayerBlendMode::LumaReveal,
    ];
    for mode in modes {
        assert_eq!(parse_blend(blend_value(mode)), mode);
    }

    let options = blend_options();
    assert_eq!(options.len(), modes.len());
    for (value, _label) in &options {
        assert_eq!(blend_value(parse_blend(value)), value.as_str());
    }
}

#[test]
fn unknown_blend_token_falls_back_to_alpha() {
    assert_eq!(parse_blend("not-a-blend"), LayerBlendMode::Alpha);
}

#[test]
fn fit_modes_round_trip_through_their_wire_tokens() {
    let modes = [
        FitMode::Contain,
        FitMode::Cover,
        FitMode::Stretch,
        FitMode::Tile,
        FitMode::Mirror,
    ];
    for mode in modes {
        assert_eq!(parse_fit(fit_value(mode)), mode);
    }

    let options = fit_options();
    assert_eq!(options.len(), modes.len());
    for (value, _label) in &options {
        assert_eq!(fit_value(parse_fit(value)), value.as_str());
    }
}

#[test]
fn effect_source_requires_a_uuid_and_starts_with_clean_state() {
    let source = effect_layer_source(SAMPLE_ID).expect("valid uuid is accepted");
    match source {
        LayerSource::Effect {
            effect_id,
            controls,
            control_bindings,
            preset_id,
        } => {
            assert_eq!(effect_id.to_string(), SAMPLE_ID);
            assert!(controls.is_empty());
            assert!(control_bindings.is_empty());
            assert!(preset_id.is_none());
        }
        other => panic!("expected an Effect source, got {other:?}"),
    }

    assert!(effect_layer_source("pulse-temp").is_err());
    assert!(effect_layer_source("").is_err());
}

#[test]
fn media_source_requires_a_uuid() {
    match media_layer_source(SAMPLE_ID).expect("valid uuid is accepted") {
        LayerSource::Media { asset_id, .. } => {
            assert_eq!(asset_id.to_string(), SAMPLE_ID);
        }
        other => panic!("expected a Media source, got {other:?}"),
    }

    assert!(media_layer_source("paimon.gif").is_err());
    assert!(media_layer_source("").is_err());
}

#[test]
fn screen_source_is_a_screen_region() {
    assert!(matches!(
        screen_layer_source(),
        LayerSource::ScreenRegion { .. }
    ));
}

#[test]
fn web_source_trims_the_url() {
    match web_layer_source("  https://example.com  ") {
        LayerSource::WebViewport { url, .. } => assert_eq!(url, "https://example.com"),
        other => panic!("expected a WebViewport source, got {other:?}"),
    }
}

#[test]
fn color_source_carries_its_rgba() {
    let rgba = [0.12, 0.34, 0.56, 1.0];
    match color_layer_source(rgba) {
        LayerSource::ColorFill { rgba: out } => assert_eq!(out, rgba),
        other => panic!("expected a ColorFill source, got {other:?}"),
    }
}

#[test]
fn hex_parses_to_linear_rgba() {
    let white = hex_to_layer_rgba("#ffffff").expect("white is valid");
    assert!((white[0] - 1.0).abs() < 1e-3);
    assert_eq!(white[3], 1.0);

    assert_eq!(
        hex_to_layer_rgba("#000000").expect("black is valid"),
        [0.0, 0.0, 0.0, 1.0]
    );

    // Three-digit shorthand expands, and a leading `#` is optional.
    let short = hex_to_layer_rgba("#fff").expect("shorthand is valid");
    assert!((short[0] - 1.0).abs() < 1e-3);
    assert!(hex_to_layer_rgba("ffffff").is_some());

    assert!(hex_to_layer_rgba("#xyz123").is_none());
    assert!(hex_to_layer_rgba("#12345").is_none());
    assert!(hex_to_layer_rgba("").is_none());
}

#[test]
fn layer_source_label_resolves_names_and_never_leaks_raw_types() {
    let mut media_names = HashMap::new();
    media_names.insert(SAMPLE_ID.to_owned(), "paimon.gif".to_owned());
    let mut effect_names = HashMap::new();
    effect_names.insert(SAMPLE_ID.to_owned(), "Aurora".to_owned());

    let known_media = media_layer_source(SAMPLE_ID).expect("valid uuid");
    assert_eq!(
        layer_source_label(&known_media, &media_names, &effect_names),
        "Media paimon.gif"
    );

    // An unresolved id reads as the bare kind — never the raw UUID (§15.2).
    let unknown_media =
        media_layer_source("0192f5a0-aaaa-7890-abcd-ef0123456789").expect("valid uuid");
    assert_eq!(
        layer_source_label(&unknown_media, &media_names, &effect_names),
        "Media"
    );

    // An effect id resolves to its registry name, never the raw UUID.
    let known_effect = effect_layer_source(SAMPLE_ID).expect("valid uuid");
    assert_eq!(
        layer_source_label(&known_effect, &media_names, &effect_names),
        "Effect Aurora"
    );

    // An unmatched effect id falls back to the bare kind, never the UUID —
    // the case a native display face outside the HTML catalog hits.
    let unknown_effect =
        effect_layer_source("0192f5a0-bbbb-7890-abcd-ef0123456789").expect("valid uuid");
    assert_eq!(
        layer_source_label(&unknown_effect, &media_names, &effect_names),
        "Effect"
    );

    assert_eq!(
        layer_source_label(&screen_layer_source(), &media_names, &effect_names),
        "Screen region"
    );
    assert_eq!(
        layer_source_label(
            &web_layer_source("https://hyperb1iss.dev"),
            &media_names,
            &effect_names,
        ),
        "Web https://hyperb1iss.dev"
    );
    assert_eq!(
        layer_source_label(&color_layer_source([0.0; 4]), &media_names, &effect_names),
        "Color fill"
    );
}

// ── Add-layer target scope (§6.6) ───────────────────────────────────────

fn sample_layout() -> SpatialLayout {
    SpatialLayout {
        id: "layout".to_owned(),
        name: "Layout".to_owned(),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones: Vec::new(),
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    }
}

fn group(name: &str, role: ZoneRole) -> Zone {
    Zone {
        id: ZoneId::new(),
        name: name.to_owned(),
        description: None,
        effect_id: None,
        controls: HashMap::new(),
        control_bindings: HashMap::new(),
        preset_id: None,
        layers: Vec::new(),
        layout: sample_layout(),
        brightness: 1.0,
        enabled: true,
        color: None,
        display_target: None,
        role,
        controls_version: 0,
        layers_version: 0,
    }
}

#[test]
fn a_single_surface_offers_no_target_scope() {
    let scopes = available_add_layer_scopes(&[group("Zone A", ZoneRole::Primary)]);
    assert!(scopes.is_empty());
}

#[test]
fn a_light_and_screen_scene_offers_every_relevant_scope() {
    let groups = [
        group("Zone A", ZoneRole::Primary),
        group("AIO Screen", ZoneRole::Display),
    ];
    assert_eq!(
        available_add_layer_scopes(&groups),
        [
            AddLayerScope::ThisSurface,
            AddLayerScope::AllLightZones,
            AddLayerScope::AllScreens,
            AddLayerScope::WholeScene,
        ]
    );
}

#[test]
fn all_screens_scope_is_dropped_when_no_screens_exist() {
    let groups = [
        group("Zone A", ZoneRole::Primary),
        group("Zone B", ZoneRole::Custom),
    ];
    let scopes = available_add_layer_scopes(&groups);
    assert!(!scopes.contains(&AddLayerScope::AllScreens));
    assert!(scopes.contains(&AddLayerScope::AllLightZones));
}

#[test]
fn scope_resolution_picks_the_right_surfaces() {
    let groups = [
        group("Zone A", ZoneRole::Primary),
        group("Zone B", ZoneRole::Custom),
        group("AIO Screen", ZoneRole::Display),
    ];
    let selected = groups[0].id.to_string();

    assert_eq!(
        resolve_add_layer_targets(AddLayerScope::ThisSurface, &groups, &selected),
        vec![selected.clone()]
    );
    assert_eq!(
        resolve_add_layer_targets(AddLayerScope::AllLightZones, &groups, &selected).len(),
        2
    );
    assert_eq!(
        resolve_add_layer_targets(AddLayerScope::AllScreens, &groups, &selected),
        [groups[2].id.to_string()]
    );
    assert_eq!(
        resolve_add_layer_targets(AddLayerScope::WholeScene, &groups, &selected).len(),
        3
    );
}
