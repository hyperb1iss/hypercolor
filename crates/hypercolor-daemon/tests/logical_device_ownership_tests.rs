use std::collections::HashMap;

use hypercolor_daemon::logical_devices::{ensure_persisted_default, load_segments};
use hypercolor_types::device::DeviceId;

#[test]
fn default_ownership_survives_restart_and_noop_refresh() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let path = dir.path().join("logical-devices.json");
    let physical = DeviceId::new();
    let mut entries = HashMap::new();
    ensure_persisted_default(
        &path,
        &mut entries,
        physical,
        "fixture:controller",
        "Controller",
        8,
    )
    .expect("persist default ownership");
    let persisted = std::fs::read(&path).expect("read ownership");
    let mut loaded = load_segments(&path).expect("restore offline ownership");
    assert_eq!(loaded["fixture:controller"].physical_device_id, physical);
    ensure_persisted_default(
        &path,
        &mut loaded,
        physical,
        "fixture:controller",
        "Controller",
        8,
    )
    .expect("refresh unchanged ownership");
    assert_eq!(
        std::fs::read(&path).expect("read unchanged ownership"),
        persisted
    );
}

#[test]
fn failed_ownership_write_does_not_hide_needed_retry() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let path = dir.path().join("logical-devices.json");
    std::fs::create_dir(&path).expect("block file destination");
    let physical = DeviceId::new();
    let mut entries = HashMap::new();
    assert!(
        ensure_persisted_default(
            &path,
            &mut entries,
            physical,
            "fixture:controller",
            "Controller",
            8
        )
        .is_err()
    );
    assert!(
        entries.is_empty(),
        "failed publication must remain retryable"
    );
    std::fs::remove_dir(&path).expect("unblock destination");
    ensure_persisted_default(
        &path,
        &mut entries,
        physical,
        "fixture:controller",
        "Controller",
        8,
    )
    .expect("retry ownership persistence");
    assert_eq!(
        load_segments(&path).expect("read recovered ownership"),
        entries
    );
}
