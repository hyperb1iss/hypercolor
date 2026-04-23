use hypercolor_daemon::device_settings::DeviceSettingsStore;

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
