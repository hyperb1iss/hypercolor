//! Integration tests for the one-time profile-to-scene import.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use hypercolor_core::scene::make_scene;
use hypercolor_daemon::profile_import::{ProfileImportOutcome, import_profiles};
use hypercolor_daemon::scene_store::SceneStore;
use hypercolor_types::device::DeviceId;
use hypercolor_types::effect::{ControlValue, EffectId};
use hypercolor_types::layer::LayerSource;
use hypercolor_types::library::PresetId;
use hypercolor_types::scene::{SceneMutationMode, ZoneRole};
use hypercolor_types::spatial::{EdgeBehavior, SamplingMode, SpatialLayout};
use serde_json::{Value, json};
use tempfile::TempDir;
use uuid::Uuid;

#[cfg(feature = "persistence-test-hooks")]
use hypercolor_daemon::persistence::AtomicFileWriter;

fn sample_layout(id: &str) -> SpatialLayout {
    SpatialLayout {
        id: id.to_owned(),
        name: format!("Layout {id}"),
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

fn write_profiles(path: &Path, profiles: impl IntoIterator<Item = (&'static str, Value)>) {
    let profiles = profiles
        .into_iter()
        .map(|(key, profile)| (key.to_owned(), profile))
        .collect::<serde_json::Map<_, _>>();
    fs::write(
        path,
        serde_json::to_vec_pretty(&profiles).expect("profiles should serialize"),
    )
    .expect("profiles should write");
}

fn profile(id: &str, name: &str) -> Value {
    let primary_effect = EffectId::new(Uuid::from_u128(10));
    let display_effect = EffectId::new(Uuid::from_u128(11));
    let display_device = DeviceId::from_uuid(Uuid::from_u128(12));
    json!({
        "id": id,
        "name": name,
        "description": null,
        "primary": {
            "effect_id": primary_effect,
            "controls": {},
            "active_preset_id": null
        },
        "displays": [{
            "device_id": display_device,
            "effect_id": display_effect,
            "controls": {}
        }],
        "brightness": null,
        "layout_id": null
    })
}

fn backup_paths(directory: &Path) -> Vec<PathBuf> {
    let mut paths = fs::read_dir(directory)
        .expect("directory should read")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("profiles.json.migrated-"))
        })
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

#[test]
fn import_maps_every_profile_field_and_retires_only_after_durable_save() {
    let tempdir = TempDir::new().expect("tempdir");
    let scenes_path = tempdir.path().join("scenes.json");
    let profiles_path = tempdir.path().join("profiles.json");
    let default_layout = sample_layout("default");
    let named_layout = sample_layout("desk-layout");
    let layouts = HashMap::from([(named_layout.id.clone(), named_layout.clone())]);
    let primary_effect = EffectId::new(Uuid::from_u128(1));
    let display_effect = EffectId::new(Uuid::from_u128(2));
    let display_device = DeviceId::from_uuid(Uuid::from_u128(3));
    let preset = PresetId(Uuid::from_u128(4));
    let primary_controls = HashMap::from([("speed".to_owned(), ControlValue::Float(0.25))]);
    let display_controls = HashMap::from([("invert".to_owned(), ControlValue::Boolean(true))]);
    let source = serde_json::to_vec_pretty(&json!({
        "legacy-key": {
            "id": "profile-a",
            "name": "Aurora",
            "description": "kept verbatim",
            "primary": {
                "effect_id": primary_effect,
                "controls": primary_controls,
                "active_preset_id": preset
            },
            "displays": [{
                "device_id": display_device,
                "effect_id": display_effect,
                "controls": display_controls
            }],
            "brightness": 37,
            "layout_id": named_layout.id
        }
    }))
    .expect("profiles should serialize");
    fs::write(&profiles_path, &source).expect("profiles should write");

    let mut store = SceneStore::new(scenes_path.clone()).expect("scene store");
    let outcome = import_profiles(&profiles_path, &mut store, &layouts, &default_layout)
        .expect("profiles should import");
    let ProfileImportOutcome::Imported { profiles, backup } = outcome else {
        panic!("profile source should import");
    };

    assert_eq!(profiles, 1);
    assert!(!profiles_path.exists());
    assert_eq!(fs::read(&backup).expect("backup should read"), source);
    assert!(scenes_path.exists(), "canonical scene store must exist");

    let loaded = SceneStore::load(&scenes_path).expect("scene store should reload");
    let scene = loaded.list().next().expect("imported scene");
    assert_eq!(scene.name, "Aurora");
    assert_eq!(scene.description.as_deref(), Some("kept verbatim"));
    assert_eq!(
        scene.layout_id.as_ref().map(ToString::to_string),
        Some("desk-layout".to_owned())
    );
    assert_eq!(scene.activation_brightness, Some(0.37));
    assert_eq!(scene.mutation_mode, SceneMutationMode::Snapshot);

    let primary = scene
        .zones
        .iter()
        .find(|zone| zone.role == ZoneRole::Primary)
        .expect("primary zone");
    assert_eq!(primary.layout, named_layout);
    assert_eq!(primary.layers.len(), 1);
    assert!(matches!(
        &primary.layers[0].source,
        LayerSource::Effect {
            effect_id,
            controls,
            preset_id,
            ..
        } if *effect_id == primary_effect
            && controls == &primary_controls
            && *preset_id == Some(preset)
    ));

    let display = scene
        .zones
        .iter()
        .find(|zone| zone.role == ZoneRole::Display)
        .expect("display zone");
    assert_eq!(
        display
            .display_target
            .as_ref()
            .map(|target| target.device_id),
        Some(display_device)
    );
    assert_eq!(display.layout.canvas_width, 1);
    assert_eq!(display.layout.canvas_height, 1);
    assert!(display.layout.zones.is_empty());
    assert!(matches!(
        &display.layers[0].source,
        LayerSource::Effect {
            effect_id,
            controls,
            preset_id: None,
            ..
        } if *effect_id == display_effect && controls == &display_controls
    ));
}

#[test]
fn import_accepts_minimal_legacy_profile_and_retires_the_source() {
    let tempdir = TempDir::new().expect("tempdir");
    let scenes_path = tempdir.path().join("scenes.json");
    let profiles_path = tempdir.path().join("profiles.json");
    let default_layout = sample_layout("default");
    write_profiles(
        &profiles_path,
        [("legacy", json!({ "id": "profile-a", "name": "Desk" }))],
    );
    let mut store = SceneStore::new(scenes_path).expect("scene store");

    let outcome = import_profiles(&profiles_path, &mut store, &HashMap::new(), &default_layout)
        .expect("minimal legacy profile should import");

    let ProfileImportOutcome::Imported { profiles, backup } = outcome else {
        panic!("profile source should import");
    };
    assert_eq!(profiles, 1);
    assert!(!profiles_path.exists());
    assert!(backup.exists());
    let scene = store
        .list()
        .find(|scene| scene.name == "Desk")
        .expect("minimal profile should become a named scene");
    assert!(scene.description.is_none());
    assert!(scene.zones.is_empty());
}

#[test]
fn import_is_deterministic_and_crash_replay_preserves_destination_name() {
    let tempdir = TempDir::new().expect("tempdir");
    let scenes_path = tempdir.path().join("scenes.json");
    let profiles_path = tempdir.path().join("profiles.json");
    let default_layout = sample_layout("default");
    let mut first_store = SceneStore::new(scenes_path.clone()).expect("scene store");
    first_store.replace_named_scenes([make_scene("Focus"), make_scene("Focus (imported)")]);
    first_store.save().expect("seed scene store");
    write_profiles(
        &profiles_path,
        [
            ("later-map-key", profile("profile-b", "Focus")),
            ("earlier-map-key", profile("profile-a", "Focus")),
        ],
    );
    let original_source = fs::read(&profiles_path).expect("profiles should read");

    import_profiles(
        &profiles_path,
        &mut first_store,
        &HashMap::new(),
        &default_layout,
    )
    .expect("profiles should import");
    let first_import = SceneStore::load(&scenes_path).expect("scenes should reload");
    let mut imported = first_import
        .list()
        .filter(|scene| scene.name.starts_with("Focus (imported "))
        .map(|scene| (scene.name.clone(), scene.id, scene.zones.clone()))
        .collect::<Vec<_>>();
    imported.sort_by(|left, right| left.0.cmp(&right.0));
    assert_eq!(
        imported
            .iter()
            .map(|entry| entry.0.as_str())
            .collect::<Vec<_>>(),
        vec!["Focus (imported 2)", "Focus (imported 3)"]
    );

    let first_backup = backup_paths(tempdir.path())
        .into_iter()
        .next()
        .expect("first backup");
    fs::rename(&first_backup, &profiles_path).expect("simulate source retirement rollback");
    assert_eq!(
        fs::read(&profiles_path).expect("profiles should read"),
        original_source
    );

    let mut renamed_scenes = first_import.list().cloned().collect::<Vec<_>>();
    let imported_id = imported[0].1;
    assert_eq!(
        imported_id.to_string(),
        "e90412df-df69-54c3-80d3-f0c985802d19"
    );
    assert_eq!(
        imported[0].2[0].id.to_string(),
        "5ef516ca-09e6-52bf-9304-b8210ea6e1fd"
    );
    assert_eq!(
        imported[0].2[0].layers[0].id.to_string(),
        "185a2e55-ac0d-5286-b601-4b2c14ed41f9"
    );
    assert_eq!(
        imported[0].2[1].id.to_string(),
        "7e7150bc-f186-5d3e-9f34-e88ee5622d77"
    );
    assert_eq!(
        imported[0].2[1].layers[0].id.to_string(),
        "ba6da4ba-deba-57a2-88c6-055d380b0d0d"
    );
    renamed_scenes
        .iter_mut()
        .find(|scene| scene.id == imported_id)
        .expect("imported destination")
        .name = "Pinned imported name".to_owned();
    let mut replay_store = SceneStore::load(&scenes_path).expect("scene store should reload");
    replay_store.replace_named_scenes(renamed_scenes);
    replay_store.save().expect("rename should persist");
    import_profiles(
        &profiles_path,
        &mut replay_store,
        &HashMap::new(),
        &default_layout,
    )
    .expect("crash replay should import");

    let replayed = SceneStore::load(&scenes_path).expect("scenes should reload");
    assert_eq!(replayed.len(), 4, "replay must upsert, never duplicate");
    let pinned = replayed
        .list()
        .find(|scene| scene.id == imported_id)
        .expect("same deterministic destination");
    assert_eq!(pinned.name, "Pinned imported name");
    assert_eq!(pinned.zones, imported[0].2);
    assert_eq!(backup_paths(tempdir.path()).len(), 1);
}

#[test]
fn import_preserves_legacy_normalization_and_reserves_default_name() {
    let tempdir = TempDir::new().expect("tempdir");
    let scenes_path = tempdir.path().join("scenes.json");
    let profiles_path = tempdir.path().join("profiles.json");
    let default_layout = sample_layout("default");
    let legacy_layout = sample_layout("legacy-layout");
    let mut legacy = profile("profile-default", " Default ");
    legacy["description"] = json!("  normalized description  ");
    legacy["brightness"] = json!(255);
    legacy["layout_id"] = json!(legacy_layout.id);
    let duplicate_display = legacy["displays"][0].clone();
    legacy["displays"]
        .as_array_mut()
        .expect("profile displays should be an array")
        .push(duplicate_display);
    write_profiles(&profiles_path, [("legacy", legacy)]);
    let mut store = SceneStore::new(scenes_path).expect("scene store");

    import_profiles(
        &profiles_path,
        &mut store,
        &HashMap::from([(legacy_layout.id.clone(), legacy_layout.clone())]),
        &default_layout,
    )
    .expect("legacy-normalized profile should import");

    let scene = store.list().next().expect("imported scene");
    assert_eq!(scene.name, "Default (imported)");
    assert_eq!(scene.description.as_deref(), Some("normalized description"));
    assert_eq!(scene.activation_brightness, Some(1.0));
    assert_eq!(
        scene.layout_id.as_ref().map(ToString::to_string),
        Some("legacy-layout".to_owned())
    );
    assert_eq!(
        scene
            .zones
            .iter()
            .filter(|zone| zone.role == ZoneRole::Display)
            .count(),
        1
    );
    assert_eq!(
        scene
            .zones
            .iter()
            .find(|zone| zone.role == ZoneRole::Primary)
            .map(|zone| &zone.layout),
        Some(&legacy_layout)
    );
}

#[test]
fn conversion_failure_leaves_profiles_and_canonical_scenes_untouched() {
    let tempdir = TempDir::new().expect("tempdir");
    let scenes_path = tempdir.path().join("scenes.json");
    let profiles_path = tempdir.path().join("profiles.json");
    let default_layout = sample_layout("default");
    let mut store = SceneStore::new(scenes_path.clone()).expect("scene store");
    store.replace_named_scenes([make_scene("Existing")]);
    store.save().expect("seed scene store");
    let scenes_before = fs::read(&scenes_path).expect("scenes should read");
    let invalid = json!({
        "broken": {
            "id": "profile-broken",
            "name": "   ",
            "description": null,
            "primary": null,
            "displays": [],
            "brightness": 42,
            "layout_id": null
        }
    });
    let source = serde_json::to_vec_pretty(&invalid).expect("profile should serialize");
    fs::write(&profiles_path, &source).expect("profiles should write");

    let error = import_profiles(&profiles_path, &mut store, &HashMap::new(), &default_layout)
        .expect_err("empty normalized name should fail conversion");

    assert!(error.to_string().contains("name must not be empty"));
    assert_eq!(
        fs::read(&profiles_path).expect("profiles should read"),
        source
    );
    assert_eq!(
        fs::read(&scenes_path).expect("scenes should read"),
        scenes_before
    );
    assert!(backup_paths(tempdir.path()).is_empty());
}

#[test]
fn strict_parse_failure_leaves_profiles_untouched() {
    let tempdir = TempDir::new().expect("tempdir");
    let scenes_path = tempdir.path().join("scenes.json");
    let profiles_path = tempdir.path().join("profiles.json");
    let default_layout = sample_layout("default");
    let source = br#"{"legacy":{"id":"profile-a","name":"A","unknown":true}}"#;
    fs::write(&profiles_path, source).expect("profiles should write");
    let mut store = SceneStore::new(scenes_path.clone()).expect("scene store");

    import_profiles(&profiles_path, &mut store, &HashMap::new(), &default_layout)
        .expect_err("unknown legacy fields should fail strict parsing");

    assert_eq!(
        fs::read(&profiles_path).expect("profiles should read"),
        source
    );
    assert!(!scenes_path.exists());
    assert!(backup_paths(tempdir.path()).is_empty());
}

#[cfg(all(unix, feature = "persistence-test-hooks"))]
#[test]
fn non_durable_scene_write_leaves_profiles_source_untouched() {
    let tempdir = TempDir::new().expect("tempdir");
    let scenes_path = tempdir.path().join("scenes.json");
    let profiles_path = tempdir.path().join("profiles.json");
    let default_layout = sample_layout("default");
    write_profiles(&profiles_path, [("legacy", profile("profile-a", "Aurora"))]);
    let source = fs::read(&profiles_path).expect("profiles should read");
    let mut store = SceneStore::new(scenes_path.clone()).expect("scene store");
    let writer = AtomicFileWriter::new(&scenes_path).expect("atomic writer");
    writer.set_injected_directory_sync_failures(usize::MAX);

    let error = import_profiles(&profiles_path, &mut store, &HashMap::new(), &default_layout)
        .expect_err("non-durable replacement must fail the import");

    assert!(error.to_string().contains("not durable"));
    assert_eq!(
        fs::read(&profiles_path).expect("profiles should read"),
        source
    );
    assert!(backup_paths(tempdir.path()).is_empty());
}

#[cfg(feature = "persistence-test-hooks")]
#[test]
fn failed_scene_replacement_leaves_profiles_source_untouched() {
    let tempdir = TempDir::new().expect("tempdir");
    let scenes_path = tempdir.path().join("scenes.json");
    let profiles_path = tempdir.path().join("profiles.json");
    let default_layout = sample_layout("default");
    write_profiles(&profiles_path, [("legacy", profile("profile-a", "Aurora"))]);
    let source = fs::read(&profiles_path).expect("profiles should read");
    let mut store = SceneStore::new(scenes_path.clone()).expect("scene store");
    let writer = AtomicFileWriter::new(&scenes_path).expect("atomic writer");
    writer.set_injected_replace_failures(usize::MAX);

    let error = import_profiles(&profiles_path, &mut store, &HashMap::new(), &default_layout)
        .expect_err("failed replacement must fail the import");

    assert!(error.to_string().contains("did not replace"));
    assert_eq!(
        fs::read(&profiles_path).expect("profiles should read"),
        source
    );
    assert!(backup_paths(tempdir.path()).is_empty());
}
