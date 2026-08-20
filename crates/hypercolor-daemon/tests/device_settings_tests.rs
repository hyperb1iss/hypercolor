use hypercolor_daemon::device_settings::DeviceSettingsStore;
use hypercolor_types::controls::{ControlValue, ControlValueMap};

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
        ("dedup_threshold".to_owned(), ControlValue::Integer(6)),
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
    assert_eq!(projected["dedup_threshold"], ControlValue::Integer(6));
    assert_eq!(
        projected["target"],
        ControlValue::IpAddress("192.0.2.10".to_owned())
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
