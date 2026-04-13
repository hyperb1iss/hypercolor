//! Integration tests for persisted named-scene storage.

use hypercolor_core::scene::{SceneManager, make_scene};
use hypercolor_daemon::scene_store::SceneStore;
use hypercolor_types::device::DeviceId;
use hypercolor_types::effect::EffectId;
use hypercolor_types::scene::{
    DisplayFaceTarget, RenderGroup, RenderGroupId, RenderGroupRole, SceneId, SceneScope,
};
use hypercolor_types::spatial::{
    DeviceZone, EdgeBehavior, LedTopology, NormalizedPosition, SamplingMode, SpatialLayout,
    StripDirection,
};
use tempfile::TempDir;
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

fn sample_group(name: &str, zone_id: &str) -> RenderGroup {
    RenderGroup {
        id: RenderGroupId::new(),
        name: name.to_owned(),
        description: None,
        effect_id: Some(EffectId::from(Uuid::now_v7())),
        controls: Default::default(),
        control_bindings: Default::default(),
        preset_id: None,
        layout: sample_layout(zone_id),
        brightness: 1.0,
        enabled: true,
        color: None,
        display_target: None,
        role: RenderGroupRole::Custom,
    }
}

#[test]
fn scene_store_round_trips_named_scenes() {
    let tempdir = TempDir::new().expect("tempdir");
    let path = tempdir.path().join("scenes.json");

    let mut store = SceneStore::new(path.clone());
    store.replace_named_scenes([make_scene("Movie Night"), make_scene("Focus")]);
    store.save().expect("scene store should save");

    let loaded = SceneStore::load(&path).expect("scene store should load");
    let names = loaded
        .list()
        .map(|scene| scene.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(loaded.len(), 2);
    assert!(names.contains(&"Movie Night"));
    assert!(names.contains(&"Focus"));
}

#[test]
fn scene_store_sync_from_manager_filters_default_scene() {
    let tempdir = TempDir::new().expect("tempdir");
    let path = tempdir.path().join("scenes.json");

    let mut manager = SceneManager::with_default();
    let named_scene = make_scene("Relax");
    let named_scene_id = named_scene.id;
    manager.create(named_scene).expect("scene should create");

    let mut store = SceneStore::new(path);
    store.sync_from_manager(&manager);

    assert_eq!(store.len(), 1);
    assert_eq!(
        store.list().next().map(|scene| scene.id),
        Some(named_scene_id)
    );
    assert!(
        store.list().all(|scene| scene.id != SceneId::DEFAULT),
        "the synthesized default scene should never be persisted"
    );
}

#[test]
fn scene_store_load_promotes_legacy_display_groups() {
    let tempdir = TempDir::new().expect("tempdir");
    let path = tempdir.path().join("scenes.json");
    let device_id = DeviceId::new();

    let mut scene = make_scene("Display Scene");
    scene.groups = vec![RenderGroup {
        display_target: Some(DisplayFaceTarget { device_id }),
        ..sample_group("Face", "desk:display")
    }];

    let payload =
        serde_json::to_string_pretty(&std::collections::HashMap::from([(scene.id, scene)]))
            .expect("scene payload should serialize");
    std::fs::write(&path, payload).expect("scene store payload should write");

    let loaded = SceneStore::load(&path).expect("scene store should load");
    let restored = loaded.list().next().expect("scene should load");
    assert_eq!(restored.groups.len(), 1);
    assert_eq!(restored.groups[0].role, RenderGroupRole::Display);
}

#[test]
fn scene_store_load_promotes_unambiguous_full_scope_primary_group() {
    let tempdir = TempDir::new().expect("tempdir");
    let path = tempdir.path().join("scenes.json");

    let mut scene = make_scene("Legacy Primary");
    scene.groups = vec![sample_group("Legacy Group", "desk:main")];

    let payload =
        serde_json::to_string_pretty(&std::collections::HashMap::from([(scene.id, scene)]))
            .expect("scene payload should serialize");
    std::fs::write(&path, payload).expect("scene store payload should write");

    let loaded = SceneStore::load(&path).expect("scene store should load");
    let restored = loaded.list().next().expect("scene should load");
    assert_eq!(restored.groups.len(), 1);
    assert_eq!(restored.groups[0].role, RenderGroupRole::Primary);
}

#[test]
fn scene_store_load_keeps_multiple_legacy_non_display_groups_custom() {
    let tempdir = TempDir::new().expect("tempdir");
    let path = tempdir.path().join("scenes.json");

    let mut scene = make_scene("Custom Split");
    scene.scope = SceneScope::Full;
    scene.groups = vec![
        sample_group("Left", "desk:left"),
        sample_group("Right", "desk:right"),
    ];

    let payload =
        serde_json::to_string_pretty(&std::collections::HashMap::from([(scene.id, scene)]))
            .expect("scene payload should serialize");
    std::fs::write(&path, payload).expect("scene store payload should write");

    let loaded = SceneStore::load(&path).expect("scene store should load");
    let restored = loaded.list().next().expect("scene should load");
    assert!(
        restored
            .groups
            .iter()
            .all(|group| group.role == RenderGroupRole::Custom),
        "ambiguous legacy groups should remain custom"
    );
}
