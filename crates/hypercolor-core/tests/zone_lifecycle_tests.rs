use std::collections::HashMap;

use hypercolor_core::scene::{SceneManager, ZoneMetaPatch, ZoneMutationError, make_scene};
use hypercolor_types::device::DeviceId;
use hypercolor_types::effect::{EffectCategory, EffectId, EffectMetadata, EffectSource};
use hypercolor_types::scene::{SceneId, SceneMutationMode, UnassignedBehavior, ZoneId, ZoneRole};
use hypercolor_types::spatial::{
    EdgeBehavior, LedTopology, NormalizedPosition, Output, SamplingMode, SpatialLayout,
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

fn sample_zone(id: &str) -> Output {
    Output {
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
    assert_eq!(group.role, ZoneRole::Custom);
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
            ZoneMetaPatch {
                name: Some("Room".to_owned()),
                brightness: Some(1.7),
                make_primary: Some(true),
                ..ZoneMetaPatch::default()
            },
        )
        .expect("custom zone should promote to primary");

    let scene = manager
        .active_scene()
        .expect("default scene should stay active");
    assert_eq!(updated.role, ZoneRole::Primary);
    assert_eq!(updated.name, "Room");
    assert_eq!(updated.brightness, 1.0);
    assert_eq!(
        scene
            .groups
            .iter()
            .filter(|group| group.role == ZoneRole::Primary)
            .count(),
        1
    );
    assert!(scene.groups_revision > before);
}

#[test]
fn apply_effect_to_group_targets_a_named_zone_and_keeps_its_layout() {
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
    let custom_layout = manager
        .active_scene()
        .and_then(|scene| scene.groups.iter().find(|group| group.id == custom_id))
        .map(|group| group.layout.clone())
        .expect("custom zone should exist");

    let aurora = sample_effect("Aurora");
    let updated = manager
        .apply_effect_to_group(custom_id, &aurora, HashMap::new(), None)
        .expect("effect should apply to the named zone");
    assert_eq!(updated.id, custom_id);
    assert_eq!(updated.effect_id, Some(aurora.id));
    // A named-zone apply never reshapes the zone — role and layout hold.
    assert_eq!(updated.role, ZoneRole::Custom);
    assert_eq!(updated.layout, custom_layout);

    // The Primary zone keeps whatever effect it had.
    let primary_effect = manager
        .active_scene()
        .and_then(|scene| scene.primary_group())
        .and_then(|group| group.effect_id);
    assert_ne!(primary_effect, Some(aurora.id));
}

#[test]
fn apply_effect_to_group_rejects_an_unknown_zone() {
    let mut manager = SceneManager::with_default();
    manager
        .upsert_primary_group(
            &sample_effect("Primary"),
            HashMap::new(),
            None,
            sample_layout("primary-zone"),
        )
        .expect("primary should be created");

    let result = manager.apply_effect_to_group(
        ZoneId::new(),
        &sample_effect("Aurora"),
        HashMap::new(),
        None,
    );
    assert!(result.is_err(), "an unknown zone id must be rejected");
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
            role: ZoneRole::Primary
        })
    );
    assert_eq!(
        manager.delete_render_group(&scene_id, display_id),
        Err(ZoneMutationError::InvalidRole {
            role: ZoneRole::Display
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
        manager.set_unassigned_behavior(&scene_id, UnassignedBehavior::Fallback(ZoneId::new())),
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

#[test]
fn update_zone_layout_merges_placement_and_preserves_identity() {
    let mut manager = SceneManager::with_default();
    let scene_id = SceneId::DEFAULT;
    manager
        .upsert_primary_group(
            &sample_effect("Glow"),
            HashMap::new(),
            None,
            sample_layout("out-1"),
        )
        .expect("primary should be created");
    let zone_id = manager
        .active_scene()
        .and_then(|scene| scene.primary_group())
        .map(|group| group.id)
        .expect("primary group should exist");
    let revision_before = manager
        .active_scene()
        .expect("default scene should be active")
        .groups_revision;

    // A placement edit that also attempts to rewrite hardware identity.
    let mut moved = sample_zone("out-1");
    moved.position = NormalizedPosition::new(0.9, 0.95);
    moved.display_order = 42;
    moved.device_id = "mock:HIJACKED".to_owned();
    moved.zone_name = Some("rewired".to_owned());
    moved.topology = LedTopology::Strip {
        count: 99,
        direction: StripDirection::RightToLeft,
    };
    moved.led_mapping = Some(vec![9, 9, 9]);
    let mut request = sample_layout("out-1");
    request.id = "attacker-layout".to_owned();
    request.zones = vec![moved];

    let updated = manager
        .update_zone_layout(&scene_id, zone_id, request)
        .expect("placement merge should apply");
    let output = &updated.layout.zones[0];

    // Placement and visual fields are taken from the request.
    assert_eq!(output.position, NormalizedPosition::new(0.9, 0.95));
    assert_eq!(output.display_order, 42);
    // Identity and hardware binding are preserved from the stored output.
    assert_eq!(output.device_id, "mock:out-1");
    assert_eq!(output.zone_name.as_deref(), Some("main"));
    assert!(matches!(
        output.topology,
        LedTopology::Strip { count: 3, .. }
    ));
    assert_eq!(output.led_mapping, Some(vec![2, 1, 0]));
    // The layout's own identity is preserved, not read from the request.
    assert_eq!(updated.layout.id, "layout-out-1");
    assert!(
        manager
            .active_scene()
            .expect("default scene should be active")
            .groups_revision
            > revision_before
    );
}

#[test]
fn update_zone_layout_rejects_output_set_changes() {
    let mut manager = SceneManager::with_default();
    let scene_id = SceneId::DEFAULT;
    manager
        .upsert_primary_group(
            &sample_effect("Glow"),
            HashMap::new(),
            None,
            sample_layout("out-1"),
        )
        .expect("primary should be created");
    let zone_id = manager
        .active_scene()
        .and_then(|scene| scene.primary_group())
        .map(|group| group.id)
        .expect("primary group should exist");

    // Adding a foreign output is rejected.
    let mut extra = sample_layout("out-1");
    extra.zones.push(sample_zone("out-2"));
    assert_eq!(
        manager
            .update_zone_layout(&scene_id, zone_id, extra)
            .expect_err("an added output should be rejected"),
        ZoneMutationError::LayoutOutputMismatch
    );

    // Dropping an owned output is rejected.
    let mut empty = sample_layout("out-1");
    empty.zones.clear();
    assert_eq!(
        manager
            .update_zone_layout(&scene_id, zone_id, empty)
            .expect_err("a dropped output should be rejected"),
        ZoneMutationError::LayoutOutputMismatch
    );
}

#[test]
fn update_zone_layout_retunes_canvas_and_sampling_defaults() {
    let mut manager = SceneManager::with_default();
    let scene_id = SceneId::DEFAULT;
    manager
        .upsert_primary_group(
            &sample_effect("Glow"),
            HashMap::new(),
            None,
            sample_layout("out-1"),
        )
        .expect("primary should be created");
    let zone_id = manager
        .active_scene()
        .and_then(|scene| scene.primary_group())
        .map(|group| group.id)
        .expect("primary group should exist");

    let mut request = sample_layout("out-1");
    request.canvas_width = 800;
    request.canvas_height = 600;
    request.default_sampling_mode = SamplingMode::Nearest;
    request.default_edge_behavior = EdgeBehavior::Wrap;

    let updated = manager
        .update_zone_layout(&scene_id, zone_id, request)
        .expect("canvas retune should apply");
    assert_eq!(updated.layout.canvas_width, 800);
    assert_eq!(updated.layout.canvas_height, 600);
    assert_eq!(updated.layout.default_sampling_mode, SamplingMode::Nearest);
    assert_eq!(updated.layout.default_edge_behavior, EdgeBehavior::Wrap);
}

#[test]
fn update_zone_layout_adopts_request_output_order() {
    let mut manager = SceneManager::with_default();
    let scene_id = SceneId::DEFAULT;
    let mut seed = sample_layout("out-a");
    seed.zones.push(sample_zone("out-b"));
    manager
        .upsert_primary_group(&sample_effect("Glow"), HashMap::new(), None, seed)
        .expect("primary should be created");
    let zone_id = manager
        .active_scene()
        .and_then(|scene| scene.primary_group())
        .map(|group| group.id)
        .expect("primary group should exist");

    // The same outputs in reversed order — the merge adopts request order,
    // since vector order is the canvas tie-breaker and drives routing.
    let mut request = sample_layout("out-a");
    request.zones = vec![sample_zone("out-b"), sample_zone("out-a")];
    let updated = manager
        .update_zone_layout(&scene_id, zone_id, request)
        .expect("reorder should apply");
    let order = updated
        .layout
        .zones
        .iter()
        .map(|zone| zone.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(order, ["out-b", "out-a"]);
}
