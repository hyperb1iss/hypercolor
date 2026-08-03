use std::collections::BTreeMap;

use hypercolor_daemon::driver_inventory::DriverInventoryStore;
use hypercolor_daemon::runtime_state::{self, RuntimeSessionSnapshot};
use serde_json::json;
use tempfile::TempDir;

#[test]
fn driver_inventory_round_trips_opaque_driver_payloads() {
    let tempdir = TempDir::new().expect("tempdir");
    let inventory_path = tempdir.path().join("driver-inventory.json");
    let runtime_path = tempdir.path().join("runtime-state.json");
    let store =
        DriverInventoryStore::open(inventory_path.clone(), &runtime_path).expect("open inventory");

    store
        .replace_driver(
            "wled",
            BTreeMap::from([("probe_ips".to_owned(), json!(["10.4.22.69"]))]),
        )
        .expect("persist WLED inventory");

    let reopened =
        DriverInventoryStore::open(inventory_path, &runtime_path).expect("reopen inventory");
    assert_eq!(
        reopened.load_cached_json("wled", "probe_ips"),
        Some(json!(["10.4.22.69"]))
    );
}

#[test]
fn driver_inventory_migrates_legacy_runtime_cache_once() {
    let tempdir = TempDir::new().expect("tempdir");
    let inventory_path = tempdir.path().join("driver-inventory.json");
    let runtime_path = tempdir.path().join("runtime-state.json");
    let snapshot = RuntimeSessionSnapshot {
        driver_runtime_cache: BTreeMap::from([(
            "wled".to_owned(),
            BTreeMap::from([("probe_ips".to_owned(), json!(["10.4.22.169"]))]),
        )]),
        ..RuntimeSessionSnapshot::default()
    };
    runtime_state::save(&runtime_path, &snapshot).expect("save legacy runtime cache");

    let store = DriverInventoryStore::open(inventory_path.clone(), &runtime_path)
        .expect("migrate inventory");
    assert_eq!(
        store.load_cached_json("wled", "probe_ips"),
        Some(json!(["10.4.22.169"]))
    );
    assert!(inventory_path.exists());

    runtime_state::save(&runtime_path, &RuntimeSessionSnapshot::default())
        .expect("clear legacy runtime cache");
    let reopened =
        DriverInventoryStore::open(inventory_path, &runtime_path).expect("reopen inventory");
    assert_eq!(
        reopened.load_cached_json("wled", "probe_ips"),
        Some(json!(["10.4.22.169"]))
    );
}

#[test]
fn corrupt_inventory_is_quarantined_before_replacement() {
    let tempdir = TempDir::new().expect("tempdir");
    let inventory_path = tempdir.path().join("driver-inventory.json");
    let runtime_path = tempdir.path().join("runtime-state.json");
    std::fs::write(&inventory_path, b"not json").expect("write corrupt inventory");

    let store = DriverInventoryStore::open(inventory_path.clone(), &runtime_path)
        .expect("open quarantined inventory");
    store
        .replace_driver(
            "wled",
            BTreeMap::from([("probe_ips".to_owned(), json!(["10.4.22.192"]))]),
        )
        .expect("write replacement inventory");

    let quarantined = std::fs::read_dir(tempdir.path())
        .expect("list tempdir")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .find(|name| name.starts_with("driver-inventory.json.corrupt-"))
        .expect("quarantined inventory");
    assert_eq!(
        std::fs::read(tempdir.path().join(quarantined)).expect("read quarantine"),
        b"not json"
    );
    assert!(inventory_path.exists());
}

#[test]
fn updating_one_driver_preserves_opaque_sibling_payloads() {
    let tempdir = TempDir::new().expect("tempdir");
    let inventory_path = tempdir.path().join("driver-inventory.json");
    let runtime_path = tempdir.path().join("runtime-state.json");
    std::fs::write(
        &inventory_path,
        serde_json::to_vec_pretty(&json!({
            "schema_version": 1,
            "drivers": {
                "future-driver": ["opaque", "payload"],
                "wled": {"probe_ips": ["10.4.22.69"]}
            },
            "future_top_level": {"enabled": true}
        }))
        .expect("serialize fixture"),
    )
    .expect("write fixture");

    let store =
        DriverInventoryStore::open(inventory_path.clone(), &runtime_path).expect("open inventory");
    store
        .replace_driver(
            "wled",
            BTreeMap::from([("probe_ips".to_owned(), json!(["10.4.22.192"]))]),
        )
        .expect("update WLED inventory");

    let saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(inventory_path).expect("read updated inventory"))
            .expect("parse updated inventory");
    assert_eq!(
        saved["drivers"]["future-driver"],
        json!(["opaque", "payload"])
    );
    assert_eq!(saved["future_top_level"], json!({"enabled": true}));
}
