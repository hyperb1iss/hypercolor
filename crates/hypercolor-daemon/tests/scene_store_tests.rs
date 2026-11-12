//! Integration tests for persisted named-scene storage.

use hypercolor_core::scene::{default_primary_zone, make_scene};
use hypercolor_daemon::scene_store;
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
        version: 1,
    }
}

fn scene_store_payload(scene: hypercolor_types::scene::Scene) -> serde_json::Value {
    serde_json::json!({
        "schema_version": 2,
        "scenes": { scene.id.to_string(): scene },
    })
}

fn write_scene_store(
    path: &std::path::Path,
    scenes: impl IntoIterator<Item = hypercolor_types::scene::Scene>,
) {
    let scenes = scenes
        .into_iter()
        .map(|scene| (scene.id.to_string(), scene))
        .collect::<std::collections::HashMap<_, _>>();
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": 2,
            "scenes": scenes,
        }))
        .expect("scene store should serialize"),
    )
    .expect("scene store should write");
}

#[test]
fn scene_store_round_trips_named_scenes() {
    let tempdir = TempDir::new().expect("tempdir");
    let path = tempdir.path().join("scenes.json");

    write_scene_store(&path, [make_scene("Movie Night"), make_scene("Focus")]);

    let loaded = scene_store::load(&path).expect("scene store should load");
    let names = loaded
        .list()
        .map(|scene| scene.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(loaded.len(), 2);
    assert!(names.contains(&"Movie Night"));
    assert!(names.contains(&"Focus"));
}

#[test]
fn scene_store_loads_empty_unversioned_store_without_rewriting_the_file() {
    let tempdir = TempDir::new().expect("tempdir");
    let path = tempdir.path().join("scenes.json");
    let payload = "{}";
    std::fs::write(&path, payload).expect("legacy scene store should write");

    let loaded = scene_store::load(&path).expect("empty legacy scene store should load");

    assert!(loaded.is_empty());
    assert_eq!(
        std::fs::read_to_string(&path).expect("legacy payload should remain readable"),
        payload,
        "read admission must never acquire write authority"
    );
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

    let error = scene_store::load(&path).expect_err("invalid scenes must fail closed");
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

    let error = scene_store::load(&path).expect_err("legacy scene store must fail closed");
    let message = format!("{error:#}");
    assert!(message.contains(r#"{"schema_version":3,"scenes":{...}}"#));
    assert!(message.contains("pre-v2 Hypercolor release"));
    assert_eq!(
        std::fs::read_to_string(&path).expect("legacy payload should remain readable"),
        payload,
        "a rejected legacy store must remain byte-for-byte untouched"
    );
}

#[test]
fn scene_store_rejects_a_newer_schema_without_rewriting_the_file() {
    let tempdir = TempDir::new().expect("tempdir");
    let path = tempdir.path().join("scenes.json");
    let payload = r#"{"schema_version":4,"scenes":{},"written_by_a_newer_release":true}"#;
    std::fs::write(&path, payload).expect("future scene store should write");

    let error = scene_store::load(&path).expect_err("future schema must fail closed");
    let message = format!("{error:#}");
    assert!(message.contains("schema version 4"), "{message}");
    assert!(message.contains("newer Hypercolor"), "{message}");
    assert!(message.contains("schemas 2 through 3"), "{message}");
    assert_eq!(
        std::fs::read_to_string(&path).expect("future payload should remain readable"),
        payload
    );
}

#[test]
fn scene_store_rejects_a_schema_below_the_floor_without_rewriting_the_file() {
    let tempdir = TempDir::new().expect("tempdir");
    let path = tempdir.path().join("scenes.json");
    let payload = r#"{"schema_version":1,"scenes":{}}"#;
    std::fs::write(&path, payload).expect("old scene store should write");

    let error = scene_store::load(&path).expect_err("a retired schema must fail closed");
    assert!(format!("{error:#}").contains("zones/layers schema"));
    assert_eq!(
        std::fs::read_to_string(&path).expect("old payload should remain readable"),
        payload
    );
}

/// `scenes.json` as the real v0.9.0 scene store wrote it: schema 2, one
/// named scene with a primary zone that places three screens, a custom
/// zone with a two-layer stack, and three display zones (a single face, a
/// three-layer stack under a `screen` target at 0.8 opacity, and a surface
/// with no layers whose face is the kept one in the sibling
/// `display-preferences.json`). Captured by running the v0.9.0 scene
/// manager and `SceneStore`. Frozen: never edit it to make a test pass.
const V0_9_0_SCENES: &str = include_str!("fixtures/storage_v0_9_0/scenes.json");

fn v0_9_0_scenes() -> serde_json::Value {
    serde_json::from_str(V0_9_0_SCENES).expect("the captured store is valid JSON")
}

#[test]
fn the_scene_store_captured_from_v0_9_0_loads_with_its_scenes_unchanged() {
    use hypercolor_types::layer::BlendMode;
    use hypercolor_types::scene::{Scene, ZoneRole};

    let tempdir = TempDir::new().expect("tempdir");
    let path = tempdir.path().join("scenes.json");
    std::fs::write(&path, V0_9_0_SCENES).expect("captured store should write");
    assert_eq!(v0_9_0_scenes()["schema_version"], 2);

    let loaded = scene_store::load(&path).expect("a schema 2 store loads");

    assert_eq!(
        std::fs::read_to_string(&path).expect("captured store should remain readable"),
        V0_9_0_SCENES,
        "read admission must never acquire write authority"
    );
    let scene = loaded.list().next().expect("the captured scene");
    assert_eq!(loaded.len(), 1);
    assert_eq!(scene.name, "Studio Session");
    let expected: Scene =
        serde_json::from_value(v0_9_0_scenes()["scenes"][scene.id.to_string()].clone())
            .expect("the captured scene decodes on its own");
    assert_eq!(
        scene, &expected,
        "a schema 2 store upgrades with no change to its scenes"
    );

    let roles = scene.zones.iter().map(|zone| zone.role).collect::<Vec<_>>();
    assert_eq!(
        roles,
        [
            ZoneRole::Primary,
            ZoneRole::Custom,
            ZoneRole::Display,
            ZoneRole::Display,
            ZoneRole::Display,
        ]
    );
    assert!(
        scene
            .zones
            .iter()
            .flat_map(|zone| &zone.layers)
            .all(|layer| layer.scope.is_none()),
        "no 0.9.0 layer has a scope"
    );
    let screens = scene.zones[0]
        .layout
        .zones
        .iter()
        .filter(|output| output.zone_name.as_deref() == Some("Display"))
        .count();
    assert_eq!(screens, 3, "the primary zone places every screen");
    let stacked = &scene.zones[3];
    assert_eq!(stacked.layers.len(), 3);
    let target = stacked.display_target.as_ref().expect("display target");
    assert_eq!(target.blend_mode, BlendMode::Screen);
    assert!((target.opacity - 0.8).abs() < f32::EPSILON);
    assert!(
        scene.zones[4].layers.is_empty(),
        "the kept face's screen has a surface and no layers"
    );
}

#[test]
fn the_kept_face_captured_beside_the_v0_9_0_scenes_names_its_layerless_display_zone() {
    let preferences: serde_json::Value = serde_json::from_str(include_str!(
        "fixtures/storage_v0_9_0/display-preferences.json"
    ))
    .expect("the captured preferences are valid JSON");
    let scenes = v0_9_0_scenes();
    let layerless = scenes["scenes"]
        .as_object()
        .expect("scenes")
        .values()
        .flat_map(|scene| scene["zones"].as_array().expect("zones"))
        .find(|zone| {
            zone["role"] == "display" && zone["layers"].as_array().is_some_and(Vec::is_empty)
        })
        .expect("the layerless display zone");
    let device = layerless["display_target"]["device_id"]
        .as_str()
        .expect("device id");

    assert!(
        preferences[device]["effect_id"].is_string(),
        "the kept face belongs to the screen the scene leaves empty"
    );
}

#[test]
fn scene_store_load_rejects_zones_missing_role() {
    let tempdir = TempDir::new().expect("tempdir");
    let path = tempdir.path().join("scenes.json");
    let mut scene = make_scene("Strict Display");
    scene.zones = vec![default_primary_zone(sample_layout("desk:display"))];
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

    let error = scene_store::load(&path).expect_err("missing role should fail");
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
    scene.zones = vec![default_primary_zone(sample_layout("desk:main"))];
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

    let error = scene_store::load(&path).expect_err("missing kind should fail");
    assert!(
        error.to_string().contains("failed to parse scenes"),
        "expected parse failure, got {error}"
    );
}
