//! Integration tests for persisted named-scene storage.

use hypercolor_core::scene::{SceneManager, default_primary_group, make_scene};
use hypercolor_daemon::scene_store::SceneStore;
use hypercolor_types::scene::SceneId;
use hypercolor_types::spatial::{
    EdgeBehavior, LedTopology, NormalizedPosition, Output, SamplingMode, SpatialLayout,
    StripDirection,
};
use tempfile::TempDir;

fn sample_layout(zone_id: &str) -> SpatialLayout {
    SpatialLayout {
        id: format!("layout-{zone_id}"),
        name: format!("Layout {zone_id}"),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones: vec![Output {
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
            brightness: None,
        }],
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    }
}

fn scene_store_payload(scene: hypercolor_types::scene::Scene) -> serde_json::Value {
    serde_json::json!({
        "schema_version": 2,
        "scenes": { scene.id.to_string(): scene },
    })
}

#[test]
fn scene_store_round_trips_named_scenes() {
    let tempdir = TempDir::new().expect("tempdir");
    let path = tempdir.path().join("scenes.json");

    let mut store = SceneStore::new(path.clone()).expect("scene store");
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
fn scene_store_rejects_invalid_scenes_without_rewriting_the_file() {
    let tempdir = TempDir::new().expect("tempdir");
    let path = tempdir.path().join("scenes.json");
    let mut scene = make_scene("Invalid");
    scene.name = "   ".to_owned();
    let payload = serde_json::to_string_pretty(&scene_store_payload(scene))
        .expect("scene payload should serialize");
    std::fs::write(&path, &payload).expect("scene payload should write");

    let error = SceneStore::load(&path).expect_err("invalid scenes must fail closed");
    assert!(
        format!("{error:#}").contains("scene name must not be empty"),
        "the validation cause is preserved: {error:#}"
    );
    assert_eq!(
        std::fs::read_to_string(&path).expect("scene payload should remain readable"),
        payload,
        "a rejected store must remain byte-for-byte untouched"
    );
}

#[test]
fn scene_store_rejects_unversioned_legacy_data_without_rewriting_the_file() {
    let tempdir = TempDir::new().expect("tempdir");
    let path = tempdir.path().join("scenes.json");
    let scene = make_scene("Legacy");
    let payload =
        serde_json::to_string_pretty(&std::collections::HashMap::from([(scene.id, scene)]))
            .expect("legacy scene payload should serialize");
    std::fs::write(&path, &payload).expect("legacy scene store should write");

    let error = SceneStore::load(&path).expect_err("legacy scene store must fail closed");
    let message = format!("{error:#}");
    assert!(message.contains(r#"{"schema_version":2,"scenes":{...}}"#));
    assert!(message.contains("pre-v2 Hypercolor release"));
    assert_eq!(
        std::fs::read_to_string(&path).expect("legacy payload should remain readable"),
        payload,
        "a rejected legacy store must remain byte-for-byte untouched"
    );
}

#[test]
fn scene_store_rejects_unknown_versions_without_rewriting_the_file() {
    let tempdir = TempDir::new().expect("tempdir");
    let path = tempdir.path().join("scenes.json");
    let payload = r#"{"schema_version":3,"scenes":{}}"#;
    std::fs::write(&path, payload).expect("future scene store should write");

    let error = SceneStore::load(&path).expect_err("future schema must fail closed");
    assert!(format!("{error:#}").contains("zones/layers schema"));
    assert_eq!(
        std::fs::read_to_string(&path).expect("future payload should remain readable"),
        payload
    );
}

#[test]
fn scene_store_sync_from_manager_filters_default_scene() {
    let tempdir = TempDir::new().expect("tempdir");
    let path = tempdir.path().join("scenes.json");

    let mut manager = SceneManager::with_default();
    let named_scene = make_scene("Relax");
    let named_scene_id = named_scene.id;
    manager.create(named_scene).expect("scene should create");

    let mut store = SceneStore::new(path).expect("scene store");
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
fn scene_store_load_rejects_zones_missing_role() {
    let tempdir = TempDir::new().expect("tempdir");
    let path = tempdir.path().join("scenes.json");
    let mut scene = make_scene("Strict Display");
    scene.zones = vec![default_primary_group(sample_layout("desk:display"))];
    let mut payload = scene_store_payload(scene);
    payload
        .get_mut("scenes")
        .and_then(serde_json::Value::as_object_mut)
        .and_then(|scenes| scenes.values_mut().next())
        .and_then(|scene| scene.get_mut("zones"))
        .and_then(serde_json::Value::as_array_mut)
        .and_then(|zones| zones.first_mut())
        .and_then(serde_json::Value::as_object_mut)
        .expect("zone should serialize as an object")
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
    scene.zones = vec![default_primary_group(sample_layout("desk:main"))];
    let mut payload = scene_store_payload(scene);
    payload
        .get_mut("scenes")
        .and_then(serde_json::Value::as_object_mut)
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
