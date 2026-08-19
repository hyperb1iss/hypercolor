//! Integration tests for persisted named-scene storage.

use hypercolor_core::scene::{SceneManager, default_primary_group, make_scene};
use hypercolor_daemon::persistence::AtomicWriteOutcome;
use hypercolor_daemon::scene_store::SceneStore;
use hypercolor_types::device::DeviceId;
use hypercolor_types::effect::EffectId;
use hypercolor_types::layer::{SceneLayer, SceneLayerId};
use hypercolor_types::scene::{SceneId, ZoneId};
use hypercolor_types::spatial::{
    EdgeBehavior, LedTopology, NormalizedPosition, Output, SamplingMode, SpatialLayout,
    StripDirection,
};
use tempfile::TempDir;
use uuid::Uuid;

#[cfg(feature = "persistence-test-hooks")]
use hypercolor_daemon::persistence::AtomicFileWriter;
#[cfg(feature = "persistence-test-hooks")]
use std::time::Duration;

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
fn scene_store_materializes_and_persists_fresh_legacy_layer_ids() {
    let tempdir = TempDir::new().expect("tempdir");
    let path = tempdir.path().join("scenes.json");
    let mut scene = make_scene("Legacy");
    scene.groups = vec![default_primary_group(sample_layout("desk:main"))];
    let zone_id = scene.groups[0].id;
    let effect_id = EffectId::from(Uuid::now_v7());
    scene.groups[0].layers = vec![SceneLayer::from_effect(
        SceneLayerId::from_uuid(zone_id.0),
        effect_id,
        std::collections::HashMap::new(),
        std::collections::HashMap::new(),
        None,
    )];
    let payload = serde_json::to_value(std::collections::HashMap::from([(scene.id, scene)]))
        .expect("scene payload should serialize");
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&payload).expect("scene payload should serialize"),
    )
    .expect("legacy scene store should write");

    let loaded = SceneStore::load(&path).expect("legacy scene store should migrate");
    let migrated_id = loaded
        .list()
        .next()
        .and_then(|scene| scene.groups.first())
        .and_then(|zone| zone.layers.first())
        .map(|layer| layer.id)
        .expect("legacy effect should become a layer");
    assert_ne!(migrated_id.as_uuid(), zone_id.0);

    let reloaded = SceneStore::load(&path).expect("migrated scene store should reload");
    assert_eq!(
        reloaded
            .list()
            .next()
            .and_then(|scene| scene.groups.first())
            .and_then(|zone| zone.layers.first())
            .map(|layer| layer.id),
        Some(migrated_id),
        "the minted layer id must persist across daemon restarts"
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
fn scene_store_rejects_an_overtaken_snapshot() {
    let tempdir = TempDir::new().expect("tempdir");
    let path = tempdir.path().join("scenes.json");
    let mut store = SceneStore::new(path.clone()).expect("scene store");
    let older = make_scene("Older");
    let newer = make_scene("Newer");

    let older_save = store
        .reserve_save([older])
        .expect("reserve older scene snapshot");
    let newer_save = store
        .reserve_save([newer])
        .expect("reserve newer scene snapshot");

    assert_eq!(
        store
            .save_reserved(newer_save)
            .expect("save newer scene snapshot"),
        AtomicWriteOutcome::Written
    );
    assert_eq!(
        store
            .save_reserved(older_save)
            .expect("reject older scene snapshot"),
        AtomicWriteOutcome::Superseded
    );

    let loaded = SceneStore::load(&path).expect("scene store should reload");
    let names = loaded
        .list()
        .map(|scene| scene.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["Newer"]);
}

#[cfg(feature = "persistence-test-hooks")]
#[test]
fn failed_scene_delete_keeps_live_state_and_does_not_resurrect() {
    let tempdir = TempDir::new().expect("tempdir");
    let path = tempdir.path().join("scenes.json");
    let mut store = SceneStore::new(path.clone()).expect("scene store");
    store.replace_named_scenes([make_scene("Ephemeral")]);
    store.save().expect("seed scene store");
    let writer = AtomicFileWriter::new(&path).expect("atomic writer");
    writer.set_injected_replace_failures(usize::MAX);
    let pending = store
        .reserve_save(std::iter::empty())
        .expect("reserve scene deletion");

    assert!(store.save_reserved(pending).is_err());
    assert!(store.is_empty(), "live scene state remains authoritative");

    writer.set_injected_replace_failures(0);
    store
        .kick_persistence()
        .expect("kick scene persistence retry");
    writer
        .flush(Duration::from_secs(5))
        .expect("scene deletion should converge");
    assert!(
        SceneStore::load(&path)
            .expect("reload scene store")
            .is_empty()
    );
}

#[test]
fn scene_store_load_rejects_groups_missing_role() {
    let tempdir = TempDir::new().expect("tempdir");
    let path = tempdir.path().join("scenes.json");
    let mut scene = make_scene("Strict Display");
    scene.groups = vec![
        serde_json::from_value(serde_json::json!({
            "id": ZoneId::new(),
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
        .expect("group should deserialize"),
    ];
    let mut payload = serde_json::to_value(std::collections::HashMap::from([(scene.id, scene)]))
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
    scene.groups = vec![
        serde_json::from_value(serde_json::json!({
            "id": ZoneId::new(),
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
        .expect("group should deserialize"),
    ];
    let mut payload = serde_json::to_value(std::collections::HashMap::from([(scene.id, scene)]))
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
