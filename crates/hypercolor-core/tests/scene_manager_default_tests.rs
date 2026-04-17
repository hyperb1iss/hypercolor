use std::collections::HashMap;
use std::path::PathBuf;

use hypercolor_core::scene::{SceneManager, make_scene};
use hypercolor_types::device::DeviceId;
use hypercolor_types::effect::{
    ControlBinding, ControlValue, EffectCategory, EffectId, EffectMetadata, EffectSource,
};
use hypercolor_types::scene::{
    DisplayFaceBlendMode, DisplayFaceTarget, RenderGroup, RenderGroupId, RenderGroupRole, SceneId,
    SceneKind,
};
use hypercolor_types::spatial::{
    DeviceZone, EdgeBehavior, LedTopology, NormalizedPosition, SamplingMode, SpatialLayout,
    StripDirection,
};
use uuid::Uuid;

fn sample_layout(zone_id: &str) -> SpatialLayout {
    SpatialLayout {
        id: format!("layout-{zone_id}"),
        name: format!("Layout {zone_id}"),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones: vec![DeviceZone {
            id: zone_id.into(),
            name: zone_id.into(),
            device_id: "mock:device".into(),
            zone_name: None,
            position: NormalizedPosition::new(0.5, 0.5),
            size: NormalizedPosition::new(1.0, 1.0),
            rotation: 0.0,
            scale: 1.0,
            display_order: 0,
            orientation: None,
            topology: LedTopology::Strip {
                count: 1,
                direction: StripDirection::LeftToRight,
            },
            led_positions: Vec::new(),
            led_mapping: None,
            sampling_mode: Some(SamplingMode::Bilinear),
            edge_behavior: Some(EdgeBehavior::Clamp),
            shape: None,
            shape_preset: None,
            attachment: None,
        }],
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    }
}

fn sample_effect(name: &str) -> EffectMetadata {
    EffectMetadata {
        id: EffectId::from(Uuid::now_v7()),
        name: name.to_owned(),
        author: "test".to_owned(),
        version: "0.1.0".to_owned(),
        description: format!("{name} effect"),
        category: EffectCategory::Ambient,
        tags: Vec::new(),
        controls: Vec::new(),
        presets: Vec::new(),
        audio_reactive: false,
        screen_reactive: false,
        source: EffectSource::Native {
            path: PathBuf::from(format!("{name}.wgsl")),
        },
        license: None,
    }
}

#[test]
fn with_default_installs_default_scene_as_ephemeral() {
    let manager = SceneManager::with_default();

    assert_eq!(manager.scene_count(), 1);
    assert_eq!(manager.active_scene_id(), Some(&SceneId::DEFAULT));
    let scene = manager
        .active_scene()
        .expect("default scene should be active");
    assert_eq!(scene.id, SceneId::DEFAULT);
    assert_eq!(scene.kind, SceneKind::Ephemeral);
    assert_eq!(scene.name, "Default");
}

#[test]
fn default_scene_cannot_be_deleted() {
    let mut manager = SceneManager::with_default();

    let error = manager
        .delete(&SceneId::DEFAULT)
        .expect_err("default scene deletion should fail");
    assert!(error.to_string().contains("cannot delete default scene"));
}

#[test]
fn deactivate_below_default_is_noop() {
    let mut manager = SceneManager::with_default();

    manager.deactivate_current();

    assert_eq!(manager.active_scene_id(), Some(&SceneId::DEFAULT));
    assert_eq!(manager.scene_count(), 1);
}

#[test]
fn upsert_primary_group_creates_when_absent() {
    let mut manager = SceneManager::with_default();
    let effect = sample_effect("Aurora");
    let controls = HashMap::from([("speed".to_owned(), ControlValue::Float(0.5))]);

    let group = manager
        .upsert_primary_group(
            &effect,
            controls.clone(),
            None,
            sample_layout("zone_primary"),
        )
        .expect("primary upsert should succeed")
        .clone();

    assert_eq!(group.role, RenderGroupRole::Primary);
    assert_eq!(group.effect_id, Some(effect.id));
    assert_eq!(group.controls, controls);
    assert_eq!(group.layout.id, "layout-zone_primary");
    assert_eq!(
        manager
            .active_scene()
            .expect("default scene should remain active")
            .groups
            .len(),
        1
    );
}

#[test]
fn upsert_primary_group_updates_effect_id_when_present() {
    let mut manager = SceneManager::with_default();
    let first_effect = sample_effect("Aurora");
    let second_effect = sample_effect("Sunset");
    let first_group_id = manager
        .upsert_primary_group(
            &first_effect,
            HashMap::from([("speed".to_owned(), ControlValue::Float(0.5))]),
            None,
            sample_layout("zone_a"),
        )
        .expect("first primary upsert should succeed")
        .id;
    let binding = ControlBinding {
        sensor: "cpu_temp".to_owned(),
        sensor_min: 30.0,
        sensor_max: 100.0,
        target_min: 0.0,
        target_max: 1.0,
        deadband: 0.0,
        smoothing: 0.5,
    };
    assert!(
        manager
            .set_group_control_binding(first_group_id, "speed".to_owned(), binding)
            .is_some(),
        "binding should attach to the existing primary group"
    );

    let updated_group = manager
        .upsert_primary_group(
            &second_effect,
            HashMap::from([("speed".to_owned(), ControlValue::Float(0.8))]),
            None,
            sample_layout("zone_b"),
        )
        .expect("second primary upsert should succeed")
        .clone();

    assert_eq!(updated_group.id, first_group_id);
    assert_eq!(updated_group.effect_id, Some(second_effect.id));
    assert!(updated_group.control_bindings.is_empty());
    assert_eq!(updated_group.layout.id, "layout-zone_b");
    assert_eq!(
        manager
            .active_scene()
            .expect("default scene should remain active")
            .groups
            .len(),
        1
    );
}

#[test]
fn upsert_display_group_uniqueness_per_device() {
    let mut manager = SceneManager::with_default();
    let device_id = DeviceId::new();
    let first_effect = sample_effect("Monitor");
    let second_effect = sample_effect("Clock");
    let first_group_id = manager
        .upsert_display_group(
            device_id,
            "Pump LCD",
            &first_effect,
            HashMap::from([("label".to_owned(), ControlValue::Text("cpu".to_owned()))]),
            sample_layout("display_a"),
        )
        .expect("first display upsert should succeed")
        .id;

    let updated_group = manager
        .upsert_display_group(
            device_id,
            "Pump LCD",
            &second_effect,
            HashMap::from([("label".to_owned(), ControlValue::Text("clock".to_owned()))]),
            sample_layout("display_b"),
        )
        .expect("second display upsert should succeed")
        .clone();

    assert_eq!(updated_group.id, first_group_id);
    assert_eq!(updated_group.role, RenderGroupRole::Display);
    assert_eq!(updated_group.effect_id, Some(second_effect.id));
    assert_eq!(updated_group.layout.id, "layout-display_b");
    assert_eq!(
        manager
            .active_scene()
            .expect("default scene should remain active")
            .groups
            .iter()
            .filter(|group| group.role == RenderGroupRole::Display)
            .count(),
        1
    );
}

#[test]
fn patch_display_group_target_preserves_opacity_for_effect_blends_and_normalizes_replace() {
    let mut manager = SceneManager::with_default();
    let device_id = DeviceId::new();
    let effect = sample_effect("Monitor");
    let group_id = manager
        .upsert_display_group(
            device_id,
            "Pump LCD",
            &effect,
            HashMap::new(),
            sample_layout("display"),
        )
        .expect("display upsert should succeed")
        .id;

    let screen_group = manager
        .patch_display_group_target(group_id, Some(DisplayFaceBlendMode::Screen), Some(0.42))
        .expect("screen patch should update the display target");
    let screen_target = screen_group
        .display_target
        .clone()
        .expect("display target should remain present");
    assert_eq!(screen_target.device_id, device_id);
    assert_eq!(screen_target.blend_mode, DisplayFaceBlendMode::Screen);
    assert!((screen_target.opacity - 0.42).abs() < f32::EPSILON);

    let replace_group = manager
        .patch_display_group_target(group_id, Some(DisplayFaceBlendMode::Replace), Some(0.08))
        .expect("replace patch should update the display target");
    let replace_target = replace_group
        .display_target
        .clone()
        .expect("display target should remain present");
    assert_eq!(replace_target.device_id, device_id);
    assert_eq!(replace_target.blend_mode, DisplayFaceBlendMode::Replace);
    assert!((replace_target.opacity - 1.0).abs() < f32::EPSILON);
}

#[test]
fn remove_display_group_is_idempotent() {
    let mut manager = SceneManager::with_default();
    let device_id = DeviceId::new();
    let effect = sample_effect("Monitor");
    manager
        .upsert_display_group(
            device_id,
            "Pump LCD",
            &effect,
            HashMap::new(),
            sample_layout("display"),
        )
        .expect("display upsert should succeed");

    assert_eq!(
        manager
            .remove_display_group(device_id)
            .expect("first removal should succeed"),
        true
    );
    assert_eq!(
        manager
            .remove_display_group(device_id)
            .expect("second removal should succeed"),
        false
    );
}

#[test]
fn remove_display_groups_for_device_prunes_named_scenes_too() {
    let mut manager = SceneManager::with_default();
    let device_id = DeviceId::new();
    let other_device_id = DeviceId::new();
    let effect = sample_effect("Monitor");
    manager
        .upsert_display_group(
            device_id,
            "Pump LCD",
            &effect,
            HashMap::new(),
            sample_layout("default-display"),
        )
        .expect("default display group should be created");

    let mut named_scene = make_scene("Desk");
    named_scene.groups = vec![
        RenderGroup {
            id: RenderGroupId::new(),
            name: "Desk Face".to_owned(),
            description: None,
            effect_id: Some(effect.id),
            controls: HashMap::new(),
            control_bindings: HashMap::new(),
            preset_id: None,
            layout: sample_layout("named-display"),
            brightness: 1.0,
            enabled: true,
            color: None,
            display_target: Some(DisplayFaceTarget::new(device_id)),
            role: RenderGroupRole::Display,
            controls_version: 0,
        },
        RenderGroup {
            id: RenderGroupId::new(),
            name: "Other Face".to_owned(),
            description: None,
            effect_id: Some(effect.id),
            controls: HashMap::new(),
            control_bindings: HashMap::new(),
            preset_id: None,
            layout: sample_layout("other-display"),
            brightness: 1.0,
            enabled: true,
            color: None,
            display_target: Some(DisplayFaceTarget::new(other_device_id)),
            role: RenderGroupRole::Display,
            controls_version: 0,
        },
    ];
    let named_scene_id = named_scene.id;
    manager
        .create(named_scene)
        .expect("named scene should be created");

    let removed_groups = manager.remove_display_groups_for_device(device_id);
    assert_eq!(removed_groups.len(), 2);
    assert!(
        removed_groups
            .iter()
            .any(|(scene_id, _)| *scene_id == SceneId::DEFAULT),
        "default scene display group should be removed"
    );
    assert!(
        removed_groups
            .iter()
            .any(|(scene_id, _)| *scene_id == named_scene_id),
        "named scene display group should be removed"
    );

    let default_scene = manager
        .active_scene()
        .expect("default scene should stay active");
    assert!(default_scene.display_group_for(device_id).is_none());

    let named_scene = manager
        .get(&named_scene_id)
        .expect("named scene should still exist");
    assert!(named_scene.display_group_for(device_id).is_none());
    assert!(
        named_scene.display_group_for(other_device_id).is_some(),
        "unrelated display group should be preserved"
    );
}

#[test]
fn patch_group_controls_missing_group_returns_none() {
    let mut manager = SceneManager::with_default();

    assert!(
        manager
            .patch_group_controls(
                RenderGroupId::new(),
                HashMap::from([("speed".to_owned(), ControlValue::Float(0.9))]),
            )
            .is_none()
    );
}

#[test]
fn sync_primary_group_layout_refreshes_primary_but_leaves_display_untouched() {
    let mut manager = SceneManager::with_default();
    let effect = sample_effect("Aurora");
    manager
        .upsert_primary_group(&effect, HashMap::new(), None, sample_layout("zone_stale"))
        .expect("primary upsert should succeed");
    let device_id = DeviceId::new();
    let display_effect = sample_effect("Clock Face");
    manager
        .upsert_display_group(
            device_id,
            "Pump LCD",
            &display_effect,
            HashMap::new(),
            sample_layout("display_stale"),
        )
        .expect("display upsert should succeed");
    let revision_before = manager.active_render_groups_revision();

    let next_layout = sample_layout("zone_fresh");
    let changed = manager.sync_primary_group_layout(&next_layout);

    assert!(changed, "layout swap should be reported as changed");
    let primary_layout_id = manager
        .active_scene()
        .expect("default scene should remain active")
        .primary_group()
        .expect("primary group should exist")
        .layout
        .id
        .clone();
    assert_eq!(primary_layout_id, "layout-zone_fresh");
    let display_layout_id = manager
        .active_scene()
        .expect("default scene should remain active")
        .display_group_for(device_id)
        .expect("display group should exist")
        .layout
        .id
        .clone();
    assert_eq!(
        display_layout_id, "layout-display_stale",
        "display groups own their own layouts and must not be rewritten"
    );
    assert!(
        manager.active_render_groups_revision() > revision_before,
        "render group revision should bump when the primary layout changes"
    );
}

#[test]
fn sync_primary_group_layout_is_noop_when_layout_already_matches() {
    let mut manager = SceneManager::with_default();
    let effect = sample_effect("Aurora");
    let layout = sample_layout("zone_steady");
    manager
        .upsert_primary_group(&effect, HashMap::new(), None, layout.clone())
        .expect("primary upsert should succeed");
    let revision_before = manager.active_render_groups_revision();

    let changed = manager.sync_primary_group_layout(&layout);

    assert!(!changed, "matching layout should not report change");
    assert_eq!(
        manager.active_render_groups_revision(),
        revision_before,
        "render group revision should not move when nothing changed"
    );
}
