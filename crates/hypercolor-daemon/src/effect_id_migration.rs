use std::collections::HashMap;

use hypercolor_types::effect::EffectId;
use hypercolor_types::layer::LayerSource;
use hypercolor_types::scene::Zone;

pub(crate) type EffectIdMigrations = HashMap<EffectId, EffectId>;

pub(crate) fn remap_effect_id(effect_id: &mut EffectId, migrations: &EffectIdMigrations) -> bool {
    let Some(canonical_id) = migrations.get(effect_id).copied() else {
        return false;
    };
    *effect_id = canonical_id;
    true
}

pub(crate) fn remap_zones(zones: &mut [Zone], migrations: &EffectIdMigrations) -> usize {
    zones
        .iter_mut()
        .flat_map(|zone| &mut zone.layers)
        .map(|layer| match &mut layer.source {
            LayerSource::Effect { effect_id, .. } => {
                usize::from(remap_effect_id(effect_id, migrations))
            }
            LayerSource::Media { .. }
            | LayerSource::ScreenRegion { .. }
            | LayerSource::WebViewport { .. }
            | LayerSource::ColorFill { .. } => 0,
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use hypercolor_core::scene::SceneManager;
    use hypercolor_types::effect::EffectId;
    use hypercolor_types::layer::{SceneLayer, SceneLayerId};
    use hypercolor_types::scene::SceneId;

    use super::remap_zones;

    #[test]
    fn remaps_effect_layers_without_touching_other_layer_state() {
        let legacy_id = EffectId::new(uuid::Uuid::now_v7());
        let canonical_id = EffectId::new(uuid::Uuid::now_v7());
        let mut zone = SceneManager::with_default()
            .get(&SceneId::DEFAULT)
            .and_then(|scene| scene.zones.first())
            .cloned()
            .expect("default scene should expose a primary zone");
        let layer_id = SceneLayerId::new();
        zone.layers = vec![SceneLayer::from_effect(
            layer_id,
            legacy_id,
            HashMap::new(),
            HashMap::new(),
            None,
        )];

        let migrated = remap_zones(
            std::slice::from_mut(&mut zone),
            &HashMap::from([(legacy_id, canonical_id)]),
        );

        assert_eq!(migrated, 1);
        assert_eq!(zone.layers[0].id, layer_id);
        assert_eq!(zone.effect_ids().collect::<Vec<_>>(), vec![canonical_id]);
    }
}
