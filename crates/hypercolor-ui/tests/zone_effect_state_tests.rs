//! Per-zone now-playing state: stack labels and frame ownership.

use std::collections::HashMap;

use hypercolor_types::effect::EffectId;
use hypercolor_types::layer::{LayerSource, SceneLayer, SceneLayerId};
use hypercolor_types::scene::ZoneRole;
use uuid::Uuid;

use hypercolor_ui::zones::ZoneEffectState;
use hypercolor_ui::zones::surface::{Surface, layer_label};

fn state(layer_count: usize, effect_id: Option<&str>, labels: &[&str]) -> ZoneEffectState {
    ZoneEffectState {
        zone: Surface {
            id: "zone-a".to_owned(),
            name: "ROLI".to_owned(),
            enabled: true,
            role: ZoneRole::Custom,
            color: None,
            layer_ids: (0..layer_count)
                .map(|index| format!("layer-{index}"))
                .collect(),
            top_layer: labels.last().map(|label| (*label).to_owned()),
        },
        effect_id: effect_id.map(ToOwned::to_owned),
        effect_name: labels.last().map(|label| (*label).to_owned()),
        effect_category: None,
        control_values: HashMap::new(),
        preset_id: None,
        revision: 1,
        layer_labels: labels.iter().map(|label| (*label).to_owned()).collect(),
        featured_layer: None,
    }
}

fn effect_layer(name: Option<&str>) -> SceneLayer {
    let mut layer = SceneLayer::from_effect(
        SceneLayerId::new(),
        EffectId::new(Uuid::nil()),
        HashMap::new(),
        HashMap::new(),
        None,
    );
    layer.name = name.map(ToOwned::to_owned);
    layer
}

#[test]
fn a_one_layer_zone_is_labelled_by_its_effect() {
    let zone = state(1, Some("bubble-garden"), &["Bubble Garden"]);
    assert_eq!(zone.stack_label().as_deref(), Some("Bubble Garden"));
    assert_eq!(zone.stack_description().as_deref(), Some("Bubble Garden"));
}

#[test]
fn a_stacked_zone_names_its_top_layer_and_counts_the_rest() {
    let zone = state(2, Some("touch-ripple"), &["Bubble Garden", "Touch Ripple"]);
    assert_eq!(zone.stack_label().as_deref(), Some("Touch Ripple +1"));
    assert_eq!(
        zone.stack_description().as_deref(),
        Some("Touch Ripple over Bubble Garden")
    );
}

#[test]
fn only_a_lone_effect_layer_owns_the_zone_frame() {
    assert_eq!(
        state(1, Some("bubble-garden"), &["Bubble Garden"]).sole_effect_id(),
        Some("bubble-garden")
    );
    // Two layers compose one picture, so no single effect owns it.
    assert_eq!(
        state(2, Some("touch-ripple"), &["Bubble Garden", "Touch Ripple"]).sole_effect_id(),
        None
    );
    // A lone non-effect layer owns no effect either.
    assert_eq!(state(1, None, &["Color"]).sole_effect_id(), None);
    // A paused zone shows nothing, and a disabled lone layer leaves no
    // enabled label, so neither frame belongs to the effect.
    let mut paused = state(1, Some("bubble-garden"), &["Bubble Garden"]);
    paused.zone.enabled = false;
    assert_eq!(paused.sole_effect_id(), None);
    assert_eq!(state(1, Some("bubble-garden"), &[]).sole_effect_id(), None);
}

#[test]
fn a_layer_label_prefers_its_name_then_its_effect_then_its_kind() {
    assert_eq!(
        layer_label(&effect_layer(Some("Ripples")), Some("Touch Ripple")),
        "Ripples"
    );
    assert_eq!(
        layer_label(&effect_layer(Some("  ")), Some("Touch Ripple")),
        "Touch Ripple"
    );
    assert_eq!(layer_label(&effect_layer(None), None), "Effect");
    let mut fill = effect_layer(None);
    fill.source = LayerSource::ColorFill {
        rgba: [1.0, 0.0, 0.5, 1.0],
    };
    assert_eq!(layer_label(&fill, None), "Color");
}

#[test]
fn a_zone_with_limited_layers_is_captioned_by_its_whole_zone_layer() {
    use hypercolor_types::layer::LayerScope;
    use hypercolor_ui::zones::zone_layer_labels;

    let limited = |name: &str, output: &str| {
        let mut layer = effect_layer(Some(name));
        layer.scope = Some(LayerScope {
            outputs: vec![output.to_owned()],
        });
        layer
    };
    let mut paused = limited("Sparks", "strip");
    paused.enabled = false;
    let zone: hypercolor_ui::api::ZoneResource = serde_json::from_value(serde_json::json!({
        "id": "11111111-1111-4111-8111-111111111111", "name": "Main", "role": "primary",
        "enabled": true, "brightness": 1, "members": [], "layout": { "placements": [] },
        "layers": [],
    }))
    .expect("valid zone fixture");
    let zone = hypercolor_ui::api::ZoneResource {
        layers: vec![
            effect_layer(Some("Rainbow")),
            limited("Gradient", "keys"),
            paused,
            limited("Glow", "mouse"),
            limited("Ripples", "fans"),
        ],
        ..zone
    };

    let (labels, featured) = zone_layer_labels(&zone, |_| None);
    // A paused layer shows nothing, so it is neither named nor counted.
    assert_eq!(labels, ["Rainbow", "Gradient", "Glow", "Ripples"]);
    assert_eq!(featured, Some(0));

    let mut captioned = state(
        5,
        Some("rainbow"),
        &["Rainbow", "Gradient", "Glow", "Ripples"],
    );
    captioned.featured_layer = featured;
    assert_eq!(
        captioned.stack_label().as_deref(),
        Some("Rainbow +3"),
        "the zone's effect, not the layer on one device above it"
    );

    // With nothing covering the whole zone, the top layer stands for it.
    let zone = hypercolor_ui::api::ZoneResource {
        layers: vec![limited("Gradient", "keys"), limited("Glow", "mouse")],
        ..zone
    };
    let (labels, featured) = zone_layer_labels(&zone, |_| None);
    assert_eq!(labels, ["Gradient", "Glow"]);
    assert_eq!(featured, Some(1));
}
