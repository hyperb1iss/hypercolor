//! Which layer `hypercolor effects patch` and `effects reset` act on.

use std::collections::HashMap;

use hypercolor_cli::commands::effects::effect_layer_of;
use hypercolor_types::effect::EffectId;
use hypercolor_types::layer::{LayerScope, SceneLayer, SceneLayerId};

fn effect_layer(scope: Option<&str>) -> SceneLayer {
    let layer_id = SceneLayerId::new();
    let mut layer = SceneLayer::from_effect(
        layer_id,
        // Any distinct id will do: the layer's own serves.
        EffectId(layer_id.0),
        HashMap::new(),
        HashMap::new(),
        None,
    );
    layer.scope = scope.map(|output| LayerScope {
        outputs: vec![output.to_owned()],
    });
    layer
}

#[test]
fn a_screens_face_above_the_lighting_is_not_the_effect_a_patch_edits() {
    let lighting = effect_layer(None);
    let face = effect_layer(Some("pump-display"));

    let (layer, effect_id, _) =
        effect_layer_of(&[lighting.clone(), face]).expect("the zone has an effect");

    assert_eq!(layer.id, lighting.id);
    assert_eq!(
        Some(effect_id),
        lighting.effect_id().map(|id| id.to_string())
    );
}

#[test]
fn a_layer_scoped_to_a_keyboard_is_not_the_effect_a_patch_edits() {
    let lighting = effect_layer(None);
    let reactive = effect_layer(Some("keyboard"));

    let (layer, _, _) =
        effect_layer_of(&[lighting.clone(), reactive]).expect("the zone has an effect");

    assert_eq!(layer.id, lighting.id);
}

#[test]
fn a_zone_with_only_scoped_layers_has_no_effect_to_patch() {
    assert!(
        effect_layer_of(&[
            effect_layer(Some("keyboard")),
            effect_layer(Some("pump-display"))
        ])
        .is_none()
    );
    assert!(effect_layer_of(&[]).is_none());
}
