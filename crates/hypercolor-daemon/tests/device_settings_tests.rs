#[cfg(feature = "persistence-test-hooks")]
use std::time::Duration;

use hypercolor_daemon::device_settings::DeviceSettingsStore;
use hypercolor_daemon::path_migration::MigrationOutcome;
#[cfg(feature = "persistence-test-hooks")]
use hypercolor_daemon::persistence::AtomicFileWriter;
use hypercolor_types::control::ControlValue;
use hypercolor_types::controls::ControlValueMap;
use serde_json::json;

fn v2_settings_payload(value: i64) -> Vec<u8> {
    serde_json::to_vec_pretty(&json!({
        "schema_version": 2,
        "global_brightness": 1.0,
        "devices": {},
        "driver_controls": {
            "net:desk-strip": {
                "dedup_threshold": { "kind": "integer", "value": value }
            }
        }
    }))
    .expect("v2 settings should serialize")
}

#[test]
fn device_settings_load_rejects_flat_legacy_snapshot_shape() {
    let tempdir = tempfile::tempdir().expect("tempdir should build");
    let path = tempdir.path().join("device-settings.json");
    std::fs::write(
        &path,
        r#"{
  "device:test": {
    "name": "Desk Strip",
    "disabled": false,
    "brightness": 0.5
  }
}"#,
    )
    .expect("legacy snapshot should write");

    let error = DeviceSettingsStore::load(&path).expect_err("legacy snapshot should fail");

    assert!(
        error
            .to_string()
            .contains("failed to parse device settings"),
        "error should point at the unsupported snapshot format: {error}"
    );
}

#[test]
fn device_settings_persists_driver_control_values() {
    let tempdir = tempfile::tempdir().expect("tempdir should build");
    let path = tempdir.path().join("device-settings.json");
    let mut store = DeviceSettingsStore::new(path.clone());
    let values = ControlValueMap::from([
        ("protocol".to_owned(), ControlValue::Enum("e131".to_owned())),
        ("dedup_threshold".to_owned(), ControlValue::Int(6)),
    ]);

    store
        .set_driver_control_values("net:desk-strip", values.clone())
        .expect("driver controls should canonicalize");
    store.save().expect("device settings should save");

    let loaded = DeviceSettingsStore::load(&path).expect("device settings should reload");

    assert_eq!(
        loaded
            .driver_control_values_for_key("net:desk-strip")
            .expect("driver controls should project"),
        values
    );
}

#[test]
fn device_settings_prunes_empty_driver_control_values() {
    let tempdir = tempfile::tempdir().expect("tempdir should build");
    let path = tempdir.path().join("device-settings.json");
    let mut store = DeviceSettingsStore::new(path);

    store
        .set_driver_control_values(
            "net:desk-strip",
            ControlValueMap::from([("protocol".to_owned(), ControlValue::Enum("e131".to_owned()))]),
        )
        .expect("driver controls should canonicalize");
    store
        .set_driver_control_values("net:desk-strip", ControlValueMap::new())
        .expect("empty driver controls should clear");

    assert!(
        store
            .driver_control_values_for_key("net:desk-strip")
            .expect("driver controls should project")
            .is_empty()
    );
}

#[test]
fn v2_driver_controls_migrate_to_the_canonical_wire() {
    let tempdir = tempfile::tempdir().expect("tempdir should build");
    let path = tempdir.path().join("device-settings.json");
    std::fs::write(
        &path,
        r#"{
  "schema_version": 2,
  "global_brightness": 1.0,
  "devices": {},
  "driver_controls": {
    "net:desk-strip": {
      "dedup_threshold": { "kind": "integer", "value": 6 },
      "target": { "kind": "ip_address", "value": "192.0.2.10" }
    }
  }
}"#,
    )
    .expect("v2 settings should write");

    let store = DeviceSettingsStore::load(&path).expect("v2 settings should migrate");
    let projected = store
        .driver_control_values_for_key("net:desk-strip")
        .expect("canonical values should project");
    assert_eq!(projected["dedup_threshold"], ControlValue::Int(6));
    assert_eq!(
        projected["target"],
        ControlValue::ip("192.0.2.10").expect("fixture IP should be valid")
    );

    let persisted: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).expect("migrated settings should read"))
            .expect("migrated settings should parse");
    assert_eq!(persisted["schema_version"], 3);
    assert_eq!(
        persisted["driver_controls"]["net:desk-strip"]["dedup_threshold"]["kind"],
        "int"
    );
    assert_eq!(
        persisted["driver_controls"]["net:desk-strip"]["target"]["kind"],
        "ip"
    );
    assert!(tempdir.path().join("device-settings.pre-v3.bak").exists());
}

#[test]
fn invalid_v2_control_names_the_device_and_control_without_rewrite() {
    let tempdir = tempfile::tempdir().expect("tempdir should build");
    let path = tempdir.path().join("device-settings.json");
    let payload = r#"{
  "schema_version": 2,
  "global_brightness": 1.0,
  "devices": {},
  "driver_controls": {
    "net:desk-strip": {
      "target": { "kind": "ip_address", "value": "not-an-address" }
    }
  }
}"#;
    std::fs::write(&path, payload).expect("invalid v2 settings should write");

    let error = DeviceSettingsStore::load(&path).expect_err("invalid control is refused");

    let message = format!("{error:#}");
    assert!(message.contains("net:desk-strip"));
    assert!(message.contains("target"));
    assert_eq!(
        std::fs::read_to_string(&path).expect("invalid settings survive"),
        payload
    );
    assert!(!tempdir.path().join("device-settings.pre-v3.bak").exists());
}

#[test]
fn v2_store_moves_to_state_as_canonical_v3() {
    let tempdir = tempfile::tempdir().expect("tempdir should build");
    let legacy = tempdir.path().join("data/device-settings.json");
    let canonical = tempdir.path().join("state/device-settings.json");
    std::fs::create_dir_all(legacy.parent().expect("legacy parent")).expect("legacy directory");
    std::fs::write(&legacy, v2_settings_payload(6)).expect("v2 settings should write");

    let (store, outcome) = DeviceSettingsStore::load_migrated(&legacy, &canonical)
        .expect("content and path migration should succeed");
    let MigrationOutcome::Imported {
        backup: Some(backup),
    } = outcome
    else {
        panic!("expected an imported backup, got {outcome:?}");
    };

    assert_eq!(
        store
            .driver_control_values_for_key("net:desk-strip")
            .expect("canonical controls should project")["dedup_threshold"],
        ControlValue::Int(6)
    );
    let persisted: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&canonical).expect("canonical settings should read"))
            .expect("canonical settings should parse");
    assert_eq!(persisted["schema_version"], 3);
    assert_eq!(
        persisted["driver_controls"]["net:desk-strip"]["dedup_threshold"]["kind"],
        "int"
    );
    assert!(!legacy.exists());
    assert!(backup.exists());

    let (_, second) = DeviceSettingsStore::load_migrated(&legacy, &canonical)
        .expect("restart should be idempotent");
    assert_eq!(second, MigrationOutcome::AlreadyMigrated);
}

#[test]
fn newer_legacy_schema_wins_over_existing_state() {
    let tempdir = tempfile::tempdir().expect("tempdir should build");
    let legacy = tempdir.path().join("data/device-settings.json");
    let canonical = tempdir.path().join("state/device-settings.json");
    std::fs::create_dir_all(legacy.parent().expect("legacy parent")).expect("legacy directory");
    std::fs::create_dir_all(canonical.parent().expect("canonical parent"))
        .expect("canonical directory");
    std::fs::write(&canonical, v2_settings_payload(2)).expect("v2 state should write");
    std::fs::write(
        &legacy,
        serde_json::to_vec_pretty(&json!({
            "schema_version": 3,
            "global_brightness": 1.0,
            "devices": {},
            "driver_controls": {
                "net:desk-strip": {
                    "dedup_threshold": { "kind": "int", "value": 3 }
                }
            }
        }))
        .expect("v3 settings should serialize"),
    )
    .expect("v3 legacy settings should write");

    let (store, outcome) = DeviceSettingsStore::load_migrated(&legacy, &canonical)
        .expect("newer legacy schema should import");

    assert!(matches!(outcome, MigrationOutcome::Imported { .. }));
    assert_eq!(
        store
            .driver_control_values_for_key("net:desk-strip")
            .expect("canonical controls should project")["dedup_threshold"],
        ControlValue::Int(3)
    );
}

#[test]
fn equal_v2_documents_prefer_state_then_upgrade_it_in_place() {
    let tempdir = tempfile::tempdir().expect("tempdir should build");
    let legacy = tempdir.path().join("data/device-settings.json");
    let canonical = tempdir.path().join("state/device-settings.json");
    std::fs::create_dir_all(legacy.parent().expect("legacy parent")).expect("legacy directory");
    std::fs::create_dir_all(canonical.parent().expect("canonical parent"))
        .expect("canonical directory");
    std::fs::write(&legacy, v2_settings_payload(1)).expect("legacy settings should write");
    std::fs::write(&canonical, v2_settings_payload(2)).expect("state settings should write");

    let (store, outcome) = DeviceSettingsStore::load_migrated(&legacy, &canonical)
        .expect("equal schemas should prefer state");

    assert_eq!(outcome, MigrationOutcome::AlreadyMigrated);
    assert_eq!(
        store
            .driver_control_values_for_key("net:desk-strip")
            .expect("canonical controls should project")["dedup_threshold"],
        ControlValue::Int(2)
    );
    assert!(legacy.exists());
    assert!(canonical.with_extension("pre-v3.bak").exists());
    let persisted: serde_json::Value =
        serde_json::from_slice(&std::fs::read(canonical).expect("canonical settings should read"))
            .expect("canonical settings should parse");
    assert_eq!(persisted["schema_version"], 3);
}

#[cfg(feature = "persistence-test-hooks")]
#[test]
fn failed_v2_import_keeps_legacy_until_canonical_v3_converges() {
    let tempdir = tempfile::tempdir().expect("tempdir should build");
    let legacy = tempdir.path().join("data/device-settings.json");
    let canonical = tempdir.path().join("state/device-settings.json");
    std::fs::create_dir_all(legacy.parent().expect("legacy parent")).expect("legacy directory");
    std::fs::write(&legacy, v2_settings_payload(6)).expect("v2 settings should write");
    let writer = AtomicFileWriter::new(&canonical).expect("canonical writer");
    writer.set_injected_replace_failures(usize::MAX);

    let (store, outcome) = DeviceSettingsStore::load_migrated(&legacy, &canonical)
        .expect("admitted import should stay authoritative");

    assert_eq!(outcome, MigrationOutcome::ImportRetrying);
    assert_eq!(
        store
            .driver_control_values_for_key("net:desk-strip")
            .expect("canonical controls should project")["dedup_threshold"],
        ControlValue::Int(6)
    );
    assert!(legacy.exists());

    writer.set_injected_replace_failures(0);
    writer.kick();
    writer
        .flush(Duration::from_secs(5))
        .expect("canonical v3 import should converge");
    let persisted: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&canonical).expect("canonical settings should read"))
            .expect("canonical settings should parse");
    assert_eq!(persisted["schema_version"], 3);

    let (_, second) = DeviceSettingsStore::load_migrated(&legacy, &canonical)
        .expect("restart should prefer canonical state");
    assert_eq!(second, MigrationOutcome::AlreadyMigrated);
    assert!(legacy.exists());
}
