use std::collections::HashMap;
use std::path::PathBuf;

use hypercolor_core::scene::SceneManager;
use hypercolor_types::device::DeviceId;
use hypercolor_types::effect::{EffectCategory, EffectId, EffectMetadata, EffectSource};
use hypercolor_types::scene::DisplayFaceTarget;
use hypercolor_types::spatial::{EdgeBehavior, SamplingMode, SpatialLayout};
use uuid::Uuid;

fn sample_effect(name: &str) -> EffectMetadata {
    EffectMetadata {
        id: EffectId::new(Uuid::now_v7()),
        name: name.to_owned(),
        author: "test".into(),
        version: "0.1.0".into(),
        description: format!("{name} effect"),
        category: EffectCategory::Ambient,
        tags: Vec::new(),
        controls: Vec::new(),
        presets: Vec::new(),
        audio_reactive: false,
        screen_reactive: false,
        input_reactive: false,
        source: EffectSource::Native {
            path: PathBuf::from(format!("native/{name}.wgsl")),
        },
        license: None,
    }
}

fn sample_layout() -> SpatialLayout {
    SpatialLayout {
        id: "scene-invalidation".into(),
        name: "Scene Invalidation".into(),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones: Vec::new(),
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        version: 1,
    }
}

#[test]
fn invalidate_resolved_zones_bumps_revision_without_mutating_zones() {
    let mut manager = SceneManager::with_default();
    manager
        .upsert_primary_zone(
            &sample_effect("aurora"),
            HashMap::new(),
            None,
            sample_layout(),
        )
        .expect("primary zone should be created");

    let zones_before = manager.resolved_zones();
    let revision_before = manager.resolved_zones_revision();

    manager.invalidate_resolved_zones();

    assert!(
        manager.resolved_zones_revision() > revision_before,
        "invalidating active zones should advance the revision"
    );
    assert_eq!(
        manager.resolved_zones().as_ref(),
        zones_before.as_ref(),
        "invalidating external dependencies should not rewrite the active zones"
    );
}

#[test]
fn effect_id_migration_rewrites_scene_and_overlay_and_fences_stale_layouts() {
    let legacy_id = EffectId::new(Uuid::now_v7());
    let canonical_id = EffectId::new(Uuid::now_v7());
    let mut effect = sample_effect("legacy");
    effect.id = legacy_id;
    let mut manager = SceneManager::with_default();
    manager
        .upsert_primary_zone(&effect, HashMap::new(), None, sample_layout())
        .expect("primary zone should be created");
    let mut overlay = manager.resolved_zones()[0].clone();
    overlay.display_target = Some(DisplayFaceTarget::new(DeviceId::new()));
    manager.set_default_display_zone(overlay);
    let revision_before = manager.resolved_zones_revision();

    let migrated = manager.remap_effect_ids(&HashMap::from([(legacy_id, canonical_id)]));

    assert_eq!(migrated, 2);
    assert!(manager.resolved_zones_revision() > revision_before);
    assert!(
        manager
            .resolved_zones()
            .iter()
            .flat_map(hypercolor_types::scene::Zone::effect_ids)
            .all(|effect_id| effect_id == canonical_id)
    );
    assert!(
        manager
            .default_display_zones()
            .iter()
            .flat_map(hypercolor_types::scene::Zone::effect_ids)
            .all(|effect_id| effect_id == canonical_id)
    );
}
