//! Integration tests for persisted named-scene storage.

use hypercolor_core::scene::{SceneManager, make_scene};
use hypercolor_daemon::scene_store::SceneStore;
use hypercolor_types::device::DeviceId;
use hypercolor_types::effect::EffectId;
use hypercolor_types::scene::{RenderGroupId, SceneId};
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
fn scene_store_load_rejects_groups_missing_role() {
    let tempdir = TempDir::new().expect("tempdir");
    let path = tempdir.path().join("scenes.json");
    let mut scene = make_scene("Strict Display");
    scene.groups = vec![serde_json::from_value(serde_json::json!({
        "id": RenderGroupId::new(),
        "name": "Face",
        "description": null,
        "effect_id": EffectId::from(Uuid::now_v7()),
        "controls": {},
        "control_bindings": {},
        "preset_id": null,
        "layout": sample_layout("desk:display"),
        "brightness": 1.0,
        "enabled": true,
        "color": null,
        "display_target": {
            "device_id": DeviceId::new()
        },
        "role": "display"
    }))
    .expect("group should deserialize")];
    let mut payload =
        serde_json::to_value(std::collections::HashMap::from([(scene.id, scene)]))
            .expect("scene payload should serialize");
    payload
        .as_object_mut()
        .and_then(|scenes| scenes.values_mut().next())
        .and_then(|scene| scene.get_mut("groups"))
        .and_then(serde_json::Value::as_array_mut)
        .and_then(|groups| groups.first_mut())
        .and_then(serde_json::Value::as_object_mut)
        .expect("group should serialize as an object")
        .remove("role");
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&payload).expect("scene payload should serialize"),
    )
    .expect("scene store payload should write");

    let error = SceneStore::load(&path).expect_err("missing role should fail");
    assert!(
        error.to_string().contains("failed to parse scenes"),
        "expected parse failure, got {error}"
    );
}

#[test]
fn scene_store_load_rejects_scenes_missing_kind() {
    let tempdir = TempDir::new().expect("tempdir");
    let path = tempdir.path().join("scenes.json");
    let mut scene = make_scene("Strict Primary");
    scene.groups = vec![serde_json::from_value(serde_json::json!({
        "id": RenderGroupId::new(),
        "name": "Primary",
        "description": null,
        "effect_id": EffectId::from(Uuid::now_v7()),
        "controls": {},
        "control_bindings": {},
        "preset_id": null,
        "layout": sample_layout("desk:main"),
        "brightness": 1.0,
        "enabled": true,
        "color": null,
        "display_target": null,
        "role": "primary"
    }))
    .expect("group should deserialize")];
    let mut payload =
        serde_json::to_value(std::collections::HashMap::from([(scene.id, scene)]))
            .expect("scene payload should serialize");
    payload
        .as_object_mut()
        .and_then(|scenes| scenes.values_mut().next())
        .and_then(serde_json::Value::as_object_mut)
        .expect("scene should serialize as an object")
        .remove("kind");
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&payload).expect("scene payload should serialize"),
    )
    .expect("scene store payload should write");

    let error = SceneStore::load(&path).expect_err("missing kind should fail");
    assert!(
        error.to_string().contains("failed to parse scenes"),
        "expected parse failure, got {error}"
    );
}
