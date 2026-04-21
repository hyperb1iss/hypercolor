use std::collections::HashMap;
use std::path::PathBuf;

use hypercolor_core::scene::SceneManager;
use hypercolor_types::effect::{EffectCategory, EffectId, EffectMetadata, EffectSource};
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
        spaces: None,
        version: 1,
    }
}

#[test]
fn invalidate_active_render_groups_bumps_revision_without_mutating_groups() {
    let mut manager = SceneManager::with_default();
    manager
        .upsert_primary_group(
            &sample_effect("aurora"),
            HashMap::new(),
            None,
            sample_layout(),
        )
        .expect("primary group should be created");

    let groups_before = manager.active_render_groups();
    let revision_before = manager.active_render_groups_revision();

    manager.invalidate_active_render_groups();

    assert!(
        manager.active_render_groups_revision() > revision_before,
        "invalidating active render groups should advance the revision"
    );
    assert_eq!(
        manager.active_render_groups().as_ref(),
        groups_before.as_ref(),
        "invalidating external dependencies should not rewrite the active groups"
    );
}
