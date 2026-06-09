//! Contract tests for the extracted layer panel (Spec 65 §10).
//!
//! Exercises the leptos-free source vocabulary — the blend/fit options,
//! source labels, and picker targeting — so the prop/event surface Studio
//! mounts cannot drift unnoticed.

use std::collections::HashMap;

use hypercolor_types::layer::WebViewportRender;
use hypercolor_types::layer::{LayerBlendMode, LayerSource};
use hypercolor_types::scene::{Zone, ZoneId, ZoneRole};
use hypercolor_types::spatial::{EdgeBehavior, SamplingMode, SpatialLayout};
use hypercolor_types::viewport::{FitMode, ViewportRect};

use hypercolor_ui::components::layer_panel::source::{
    AddLayerScope, EffectPickerMode, LayerSourceKind, available_add_layer_scopes, blend_options,
    blend_value, default_blend_for_added_layer, effect_category_label, effect_layer_source,
    effect_picker_matches_query, effect_picker_mode, fit_options, fit_value, layer_source_label,
    media_layer_source, parse_blend, parse_fit, resolve_add_layer_targets,
};

/// A valid UUID string for effect/media id parsing.
const SAMPLE_ID: &str = "0192f5a0-1234-7890-abcd-ef0123456789";

#[test]
fn picker_exposes_only_effect_and_media_sources() {
    assert_eq!(LayerSourceKind::ALL.len(), 2);
    let labels: Vec<&str> = LayerSourceKind::ALL
        .iter()
        .map(|kind| kind.label())
        .collect();
    assert_eq!(labels, ["Effect", "Media"]);
}

#[test]
fn effect_picker_mode_tracks_surface_and_scope() {
    assert_eq!(
        effect_picker_mode(AddLayerScope::ThisSurface, Some(ZoneRole::Display)),
        EffectPickerMode::Faces
    );
    assert_eq!(
        effect_picker_mode(AddLayerScope::ThisSurface, Some(ZoneRole::Primary)),
        EffectPickerMode::Effects
    );
    assert_eq!(
        effect_picker_mode(AddLayerScope::AllScreens, Some(ZoneRole::Primary)),
        EffectPickerMode::Faces
    );
    assert_eq!(
        effect_picker_mode(AddLayerScope::AllZones, Some(ZoneRole::Display)),
        EffectPickerMode::Effects
    );
    assert_eq!(
        effect_picker_mode(AddLayerScope::WholeScene, Some(ZoneRole::Display)),
        EffectPickerMode::Mixed
    );
    assert_eq!(
        effect_picker_mode(AddLayerScope::ThisSurface, None),
        EffectPickerMode::Effects
    );

    assert_eq!(EffectPickerMode::Faces.tab_label(), "Face");
    assert_eq!(EffectPickerMode::Effects.tab_label(), "Effect");
    assert_eq!(EffectPickerMode::Mixed.tab_label(), "Effect");
    assert_eq!(
        EffectPickerMode::Faces.search_placeholder(),
        "Search faces and effects..."
    );
    assert_eq!(
        EffectPickerMode::Effects.search_placeholder(),
        "Search effects..."
    );
    assert_eq!(
        EffectPickerMode::Mixed.empty_detail(),
        "No matching effects"
    );
    assert_eq!(
        EffectPickerMode::Faces.empty_detail(),
        "No matching faces or effects"
    );
    assert!(EffectPickerMode::Faces.includes_category("display"));
    assert!(EffectPickerMode::Faces.includes_category("source"));
    assert!(EffectPickerMode::Faces.includes_category("utility"));
    assert!(EffectPickerMode::Faces.includes_category("ambient"));
    assert!(EffectPickerMode::Effects.includes_category("source"));
    assert!(!EffectPickerMode::Effects.includes_category("display"));
    assert!(EffectPickerMode::Mixed.includes_category("display"));
    assert!(EffectPickerMode::Mixed.includes_category("ambient"));
    assert_eq!(EffectPickerMode::Faces.sort_bucket("display"), 0);
    assert_eq!(EffectPickerMode::Faces.sort_bucket("source"), 1);
    assert_eq!(EffectPickerMode::Effects.sort_bucket("display"), 1);
}

#[test]
fn effect_category_label_renames_display_to_face() {
    assert_eq!(effect_category_label("display"), "face");
    assert_eq!(effect_category_label("DiSpLaY"), "face");
    assert_eq!(effect_category_label("source"), "source");
}

#[test]
fn effect_picker_query_matches_display_by_face_label() {
    assert!(effect_picker_matches_query("LCD Gauge", "display", "face"));
    assert!(effect_picker_matches_query(
        "Screen Cast",
        "utility",
        "cast"
    ));
    assert!(effect_picker_matches_query(
        "Screen Cast",
        "utility",
        "utility"
    ));
    assert!(!effect_picker_matches_query(
        "Screen Cast",
        "utility",
        "face"
    ));
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
fn added_effect_layers_screen_over_existing_content_by_default() {
    let effect = effect_layer_source(SAMPLE_ID).expect("valid uuid is accepted");
    let media = media_layer_source(SAMPLE_ID).expect("valid uuid is accepted");

    assert_eq!(
        default_blend_for_added_layer(&effect, 0),
        LayerBlendMode::Alpha
    );
    assert_eq!(
        default_blend_for_added_layer(&effect, 1),
        LayerBlendMode::Screen
    );
    assert_eq!(
        default_blend_for_added_layer(&media, 1),
        LayerBlendMode::Alpha
    );
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
        layer_source_label(
            &LayerSource::ScreenRegion {
                viewport: ViewportRect::default()
            },
            &media_names,
            &effect_names
        ),
        "Screen region"
    );
    assert_eq!(
        layer_source_label(
            &LayerSource::WebViewport {
                url: "https://hyperb1iss.dev".to_owned(),
                viewport: ViewportRect::default(),
                render: WebViewportRender::default(),
            },
            &media_names,
            &effect_names,
        ),
        "Web https://hyperb1iss.dev"
    );
    assert_eq!(
        layer_source_label(
            &LayerSource::ColorFill { rgba: [0.0; 4] },
            &media_names,
            &effect_names
        ),
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
            AddLayerScope::AllZones,
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
    assert!(scopes.contains(&AddLayerScope::AllZones));
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
        resolve_add_layer_targets(AddLayerScope::AllZones, &groups, &selected).len(),
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
