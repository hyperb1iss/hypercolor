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

fn zone(id: u8, role: &str) -> hypercolor_types::api::scene::ZoneResource {
    serde_json::from_value(serde_json::json!({
        "id": format!("019a0000-0000-7000-8000-0000000000a{id}"),
        "name": role,
        "role": role,
        "enabled": true,
        "brightness": 1.0,
        "members": [],
        "layers": []
    }))
    .expect("zone fixture should decode")
}

#[test]
fn a_scene_with_no_primary_zone_falls_back_to_a_lighting_zone_not_a_screen() {
    use hypercolor_cli::commands::effects::effect_zone_of;

    let zones = [zone(1, "display"), zone(2, "custom"), zone(3, "primary")];
    assert_eq!(
        effect_zone_of(&zones).map(|zone| zone.id),
        Some(zones[2].id)
    );
    assert_eq!(
        effect_zone_of(&zones[..2]).map(|zone| zone.id),
        Some(zones[1].id),
        "a screen's zone listed first is not the zone an effect command edits"
    );
    assert!(effect_zone_of(&zones[..1]).is_none());
}
