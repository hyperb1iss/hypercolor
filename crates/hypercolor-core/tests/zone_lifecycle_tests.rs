use std::collections::HashMap;

use hypercolor_core::scene::{RenderGroupMetaPatch, SceneManager, ZoneMutationError, make_scene};
use hypercolor_types::device::DeviceId;
use hypercolor_types::effect::{EffectCategory, EffectId, EffectMetadata, EffectSource};
use hypercolor_types::scene::{
    RenderGroupId, RenderGroupRole, SceneId, SceneMutationMode, UnassignedBehavior,
};
use hypercolor_types::spatial::{
    DeviceZone, EdgeBehavior, LedTopology, NormalizedPosition, SamplingMode, SpatialLayout,
    StripDirection,
};
use uuid::Uuid;

fn sample_effect(name: &str) -> EffectMetadata {
    EffectMetadata {
        id: EffectId::new(Uuid::now_v7()),
        name: name.to_owned(),
        author: "test".to_owned(),
        version: "0.1.0".to_owned(),
        description: format!("{name} description"),
        category: EffectCategory::Ambient,
        tags: Vec::new(),
        controls: Vec::new(),
        presets: Vec::new(),
        audio_reactive: false,
        screen_reactive: false,
        source: EffectSource::Native {
            path: format!("builtin/{name}").into(),
        },
        license: None,
    }
}

fn sample_layout(zone_id: &str) -> SpatialLayout {
    SpatialLayout {
        id: format!("layout-{zone_id}"),
        name: format!("Layout {zone_id}"),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones: vec![sample_zone(zone_id)],
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    }
}

fn sample_zone(id: &str) -> DeviceZone {
    DeviceZone {
        id: id.to_owned(),
        name: id.to_owned(),
        device_id: format!("mock:{id}"),
        zone_name: Some("main".to_owned()),
        position: NormalizedPosition::new(0.1, 0.2),
        size: NormalizedPosition::new(0.3, 0.4),
        rotation: 0.75,
        scale: 1.5,
        display_order: 7,
        orientation: None,
        topology: LedTopology::Strip {
            count: 3,
            direction: StripDirection::LeftToRight,
        },
        led_positions: Vec::new(),
        led_mapping: Some(vec![2, 1, 0]),
        sampling_mode: Some(SamplingMode::Bilinear),
        edge_behavior: Some(EdgeBehavior::Clamp),
        shape: None,
        shape_preset: None,
        attachment: None,
        brightness: None,
    }
}

#[test]
fn create_and_delete_custom_zone_refreshes_active_group_cache() {
    let mut manager = SceneManager::with_default();
    let scene_id = SceneId::DEFAULT;
    let groups_revision = manager
        .active_scene()
        .expect("default scene should be active")
        .groups_revision;
    let cache_revision = manager.active_render_groups_revision();

    let group_id = manager
        .create_render_group(
            &scene_id,
            "Desk".to_owned(),
            Some("#80ffea".to_owned()),
            (320, 200),
        )
        .expect("custom zone should be created");

    let scene = manager
        .active_scene()
        .expect("default scene should stay active");
    let group = scene
        .groups
        .iter()
        .find(|group| group.id == group_id)
        .expect("new custom zone should be in the active scene");
    assert_eq!(group.role, RenderGroupRole::Custom);
    assert!(group.layout.zones.is_empty());
    assert!(scene.groups_revision > groups_revision);
    assert!(manager.active_render_groups_revision() > cache_revision);
    assert!(
        manager
            .active_render_groups()
            .iter()
            .any(|group| group.id == group_id)
    );

    manager
        .delete_render_group(&scene_id, group_id)
        .expect("custom zone should be deleted");

    assert!(
        !manager
            .active_scene()
            .expect("default scene should stay active")
            .groups
            .iter()
            .any(|group| group.id == group_id)
    );
}

#[test]
fn structural_zone_mutations_reject_snapshot_scenes() {
    let mut manager = SceneManager::with_default();
    let mut scene = make_scene("Locked");
    scene.mutation_mode = SceneMutationMode::Snapshot;
    let scene_id = scene.id;
    manager
        .create(scene)
        .expect("snapshot scene should register");

    let error = manager
        .create_render_group(&scene_id, "Nope".to_owned(), None, (320, 200))
        .expect_err("snapshot scene should reject structural zone creation");

    assert_eq!(error, ZoneMutationError::SnapshotLocked);
}

#[test]
fn metadata_patch_can_promote_primary_atomically() {
    let mut manager = SceneManager::with_default();
    let scene_id = SceneId::DEFAULT;
    manager
        .upsert_primary_group(
            &sample_effect("Primary"),
            HashMap::new(),
            None,
            sample_layout("primary-zone"),
        )
        .expect("primary should be created");
    let custom_id = manager
        .create_render_group(&scene_id, "Ambient".to_owned(), None, (320, 200))
        .expect("custom zone should be created");
    let before = manager
        .active_scene()
        .expect("default scene should be active")
        .groups_revision;

    let updated = manager
        .update_render_group_meta(
            &scene_id,
            custom_id,
            RenderGroupMetaPatch {
                name: Some("Room".to_owned()),
                brightness: Some(1.7),
                make_primary: Some(true),
                ..RenderGroupMetaPatch::default()
            },
        )
        .expect("custom zone should promote to primary");

    let scene = manager
        .active_scene()
        .expect("default scene should stay active");
    assert_eq!(updated.role, RenderGroupRole::Primary);
    assert_eq!(updated.name, "Room");
    assert_eq!(updated.brightness, 1.0);
    assert_eq!(
        scene
            .groups
            .iter()
            .filter(|group| group.role == RenderGroupRole::Primary)
            .count(),
        1
    );
    assert!(scene.groups_revision > before);
}

#[test]
fn assignment_moves_zones_and_resets_cross_zone_placement() {
    let mut manager = SceneManager::with_default();
    let scene_id = SceneId::DEFAULT;
    manager
        .upsert_primary_group(
            &sample_effect("Primary"),
            HashMap::new(),
            None,
            sample_layout("device-zone"),
        )
        .expect("primary should be created");
    let custom_id = manager
        .create_render_group(&scene_id, "Custom".to_owned(), None, (320, 200))
        .expect("custom zone should be created");
    let mut zone = sample_zone("device-zone");
    zone.position = NormalizedPosition::new(0.9, 0.8);
    zone.rotation = 1.2;
    zone.scale = 2.0;

    manager
        .assign_device_zone(&scene_id, custom_id, zone.clone())
        .expect("zone should move into custom group");

    let scene = manager
        .active_scene()
        .expect("default scene should stay active");
    let custom_zone = scene
        .groups
        .iter()
        .find(|group| group.id == custom_id)
        .and_then(|group| group.layout.zones.first())
        .expect("custom group should own moved zone");
    assert_eq!(custom_zone.id, "device-zone");
    assert_eq!(custom_zone.position, NormalizedPosition::new(0.5, 0.5));
    assert_eq!(custom_zone.size, NormalizedPosition::new(1.0, 1.0));
    assert_eq!(custom_zone.rotation, 0.0);
    assert_eq!(custom_zone.scale, 1.0);
    assert_eq!(custom_zone.led_mapping, Some(vec![2, 1, 0]));
    assert!(
        scene
            .primary_group()
            .expect("primary should still exist")
            .layout
            .zones
            .is_empty()
    );

    manager
        .unassign_device_zone(&scene_id, "device-zone")
        .expect("zone should unassign");

    assert!(
        manager
            .active_scene()
            .expect("default scene should stay active")
            .groups
            .iter()
            .all(|group| group.layout.zones.is_empty())
    );
}

#[test]
fn primary_and_display_zones_cannot_be_deleted_as_custom_zones() {
    let mut manager = SceneManager::with_default();
    let scene_id = SceneId::DEFAULT;
    manager
        .upsert_primary_group(
            &sample_effect("Primary"),
            HashMap::new(),
            None,
            sample_layout("primary-zone"),
        )
        .expect("primary should be created");
    let primary_id = manager
        .active_scene()
        .and_then(|scene| scene.primary_group())
        .map(|group| group.id)
        .expect("primary group should exist");
    let device_id = DeviceId::new();
    manager
        .upsert_display_group(
            device_id,
            "Face",
            &sample_effect("Display"),
            HashMap::new(),
            sample_layout("display-zone"),
        )
        .expect("display group should be created");
    let display_id = manager
        .active_scene()
        .and_then(|scene| scene.display_group_for(device_id))
        .map(|group| group.id)
        .expect("display group should exist");

    assert_eq!(
        manager.delete_render_group(&scene_id, primary_id),
        Err(ZoneMutationError::InvalidRole {
            role: RenderGroupRole::Primary
        })
    );
    assert_eq!(
        manager.delete_render_group(&scene_id, display_id),
        Err(ZoneMutationError::InvalidRole {
            role: RenderGroupRole::Display
        })
    );
}

#[test]
fn unassigned_behavior_validates_fallback_group() {
    let mut manager = SceneManager::with_default();
    let scene_id = SceneId::DEFAULT;
    let custom_id = manager
        .create_render_group(&scene_id, "Fallback".to_owned(), None, (320, 200))
        .expect("custom fallback zone should be created");
    let revision = manager
        .active_scene()
        .expect("default scene should be active")
        .groups_revision;

    let behavior = manager
        .set_unassigned_behavior(&scene_id, UnassignedBehavior::Fallback(custom_id))
        .expect("fallback should accept existing LED group");

    assert_eq!(behavior, UnassignedBehavior::Fallback(custom_id));
    assert!(
        manager
            .active_scene()
            .expect("default scene should stay active")
            .groups_revision
            > revision
    );
    assert_eq!(
        manager.set_unassigned_behavior(
            &scene_id,
            UnassignedBehavior::Fallback(RenderGroupId::new())
        ),
        Err(ZoneMutationError::GroupMissing)
    );
}

#[test]
fn fallback_unassigned_behavior_rejects_display_group() {
    let mut manager = SceneManager::with_default();
    let scene_id = SceneId::DEFAULT;
    let device_id = DeviceId::new();
    manager
        .upsert_display_group(
            device_id,
            "Face",
            &sample_effect("Display"),
            HashMap::new(),
            sample_layout("display-zone"),
        )
        .expect("display group should be created");
    let display_id = manager
        .active_scene()
        .and_then(|scene| scene.display_group_for(device_id))
        .map(|group| group.id)
        .expect("display group should exist");

    assert_eq!(
        manager.set_unassigned_behavior(&scene_id, UnassignedBehavior::Fallback(display_id)),
        Err(ZoneMutationError::GroupMissing)
    );
}

#[test]
fn effect_apply_preserves_primary_assignment_when_custom_zones_exist() {
    let mut manager = SceneManager::with_default();
    let scene_id = SceneId::DEFAULT;
    manager
        .upsert_primary_group(
            &sample_effect("Primary"),
            HashMap::new(),
            None,
            sample_layout("primary-zone"),
        )
        .expect("primary should be created");
    let custom_id = manager
        .create_render_group(&scene_id, "Custom".to_owned(), None, (320, 200))
        .expect("custom zone should be created");
    manager
        .assign_device_zone(&scene_id, custom_id, sample_zone("custom-zone"))
        .expect("custom zone should claim a device zone");
    let before = manager
        .active_scene()
        .and_then(|scene| scene.primary_group())
        .map(|group| group.layout.clone())
        .expect("primary group should exist");
    let full_layout = SpatialLayout {
        zones: vec![sample_zone("primary-zone"), sample_zone("custom-zone")],
        ..sample_layout("primary-zone")
    };

    manager
        .upsert_primary_group(
            &sample_effect("Next"),
            HashMap::new(),
            None,
            full_layout.clone(),
        )
        .expect("primary effect update should succeed");

    let after = manager
        .active_scene()
        .and_then(|scene| scene.primary_group())
        .map(|group| group.layout.clone())
        .expect("primary group should exist");
    assert_eq!(after, before);
}

#[test]
fn effect_apply_seeds_new_primary_with_unclaimed_zones_only() {
    let mut manager = SceneManager::with_default();
    let scene_id = SceneId::DEFAULT;
    let custom_id = manager
        .create_render_group(&scene_id, "Custom".to_owned(), None, (320, 200))
        .expect("custom zone should be created");
    manager
        .assign_device_zone(&scene_id, custom_id, sample_zone("custom-zone"))
        .expect("custom zone should claim a device zone");
    let full_layout = SpatialLayout {
        zones: vec![sample_zone("primary-zone"), sample_zone("custom-zone")],
        ..sample_layout("primary-zone")
    };

    manager
        .upsert_primary_group(&sample_effect("Primary"), HashMap::new(), None, full_layout)
        .expect("primary should be created");

    let primary = manager
        .active_scene()
        .and_then(|scene| scene.primary_group())
        .expect("primary group should exist");
    assert_eq!(primary.layout.zones.len(), 1);
    assert_eq!(primary.layout.zones[0].id, "primary-zone");
}

#[test]
fn layout_sync_preserves_primary_assignment_when_custom_zones_exist() {
    let mut manager = SceneManager::with_default();
    let scene_id = SceneId::DEFAULT;
    manager
        .upsert_primary_group(
            &sample_effect("Primary"),
            HashMap::new(),
            None,
            sample_layout("primary-zone"),
        )
        .expect("primary should be created");
    manager
        .create_render_group(&scene_id, "Custom".to_owned(), None, (320, 200))
        .expect("custom zone should be created");
    let before = manager
        .active_scene()
        .and_then(|scene| scene.primary_group())
        .map(|group| group.layout.clone())
        .expect("primary group should exist");

    let changed = manager.sync_primary_group_layout(&sample_layout("new-full-layout"));

    let after = manager
        .active_scene()
        .and_then(|scene| scene.primary_group())
        .map(|group| group.layout.clone())
        .expect("primary group should exist");
    assert!(!changed);
    assert_eq!(after, before);
}
