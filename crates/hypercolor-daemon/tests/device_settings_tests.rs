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

    store.set_driver_control_values("net:desk-strip", values.clone());
    store.save().expect("device settings should save");

    let loaded = DeviceSettingsStore::load(&path).expect("device settings should reload");

    assert_eq!(
        loaded.driver_control_values_for_key("net:desk-strip"),
        values
    );
}

#[test]
fn device_settings_prunes_empty_driver_control_values() {
    let tempdir = tempfile::tempdir().expect("tempdir should build");
    let path = tempdir.path().join("device-settings.json");
    let mut store = DeviceSettingsStore::new(path);

    store.set_driver_control_values(
        "net:desk-strip",
        ControlValueMap::from([("protocol".to_owned(), ControlValue::Enum("e131".to_owned()))]),
    );
    store.set_driver_control_values("net:desk-strip", ControlValueMap::new());

    assert!(
        store
            .driver_control_values_for_key("net:desk-strip")
            .is_empty()
    );
}
