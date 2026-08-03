use std::collections::BTreeMap;

use anyhow::{Result, bail};
use async_trait::async_trait;
use hypercolor_daemon::driver_inventory::DriverInventoryStore;
use hypercolor_daemon::runtime_state::{self, RuntimeSessionSnapshot};
use hypercolor_driver_api::{
    DriverCredentialStore, DriverDescriptor, DriverDiscoveryState, DriverHost, DriverModule,
    DriverRuntimeActions, DriverRuntimeCacheProvider, DriverTrackedDevice,
};
use hypercolor_network::DriverModuleRegistry;
use hypercolor_types::device::{DeviceId, DriverTransportKind};
use serde_json::json;
use tempfile::TempDir;

static EMPTY_DRIVER: DriverDescriptor = DriverDescriptor::new(
    "empty_inventory_test",
    "Empty Inventory Test",
    DriverTransportKind::Network,
    false,
    false,
);
static FAILING_DRIVER: DriverDescriptor = DriverDescriptor::new(
    "failing_inventory_test",
    "Failing Inventory Test",
    DriverTransportKind::Network,
    false,
    false,
);

struct EmptyInventoryDriver;

impl DriverModule for EmptyInventoryDriver {
    fn descriptor(&self) -> &'static DriverDescriptor {
        &EMPTY_DRIVER
    }

    fn runtime_cache(&self) -> Option<&dyn DriverRuntimeCacheProvider> {
        Some(self)
    }
}

#[async_trait]
impl DriverRuntimeCacheProvider for EmptyInventoryDriver {
    async fn snapshot(
        &self,
        _host: &dyn DriverHost,
    ) -> Result<BTreeMap<String, serde_json::Value>> {
        Ok(BTreeMap::new())
    }
}

struct FailingInventoryDriver;

impl DriverModule for FailingInventoryDriver {
    fn descriptor(&self) -> &'static DriverDescriptor {
        &FAILING_DRIVER
    }

    fn runtime_cache(&self) -> Option<&dyn DriverRuntimeCacheProvider> {
        Some(self)
    }
}

#[async_trait]
impl DriverRuntimeCacheProvider for FailingInventoryDriver {
    async fn snapshot(
        &self,
        _host: &dyn DriverHost,
    ) -> Result<BTreeMap<String, serde_json::Value>> {
        bail!("injected inventory snapshot failure")
    }
}

struct TestHost;

#[async_trait]
impl DriverCredentialStore for TestHost {
    async fn get_json(&self, _driver_id: &str, _key: &str) -> Result<Option<serde_json::Value>> {
        Ok(None)
    }

    async fn set_json(
        &self,
        _driver_id: &str,
        _key: &str,
        _value: serde_json::Value,
    ) -> Result<()> {
        Ok(())
    }

    async fn remove(&self, _driver_id: &str, _key: &str) -> Result<()> {
        Ok(())
    }
}

#[async_trait]
impl DriverRuntimeActions for TestHost {
    async fn activate_device(&self, _device_id: DeviceId, _backend_id: &str) -> Result<bool> {
        Ok(false)
    }

    async fn disconnect_device(
        &self,
        _device_id: DeviceId,
        _backend_id: &str,
        _will_retry: bool,
    ) -> Result<bool> {
        Ok(false)
    }
}

#[async_trait]
impl DriverDiscoveryState for TestHost {
    async fn tracked_devices(&self, _driver_id: &str) -> Vec<DriverTrackedDevice> {
        Vec::new()
    }

    fn load_cached_json(&self, _driver_id: &str, _key: &str) -> Result<Option<serde_json::Value>> {
        Ok(None)
    }
}

impl DriverHost for TestHost {
    fn credentials(&self) -> &dyn DriverCredentialStore {
        self
    }

    fn runtime(&self) -> &dyn DriverRuntimeActions {
        self
    }

    fn discovery_state(&self) -> &dyn DriverDiscoveryState {
        self
    }
}

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

#[tokio::test]
async fn empty_and_failed_refreshes_preserve_prior_inventory() {
    let tempdir = TempDir::new().expect("tempdir");
    let inventory_path = tempdir.path().join("driver-inventory.json");
    let runtime_path = tempdir.path().join("runtime-state.json");
    let store = DriverInventoryStore::open(inventory_path, &runtime_path).expect("open inventory");
    let prior = BTreeMap::from([("target".to_owned(), json!("10.4.22.69"))]);
    store
        .replace_driver(EMPTY_DRIVER.id, prior.clone())
        .expect("seed empty driver inventory");
    store
        .replace_driver(FAILING_DRIVER.id, prior.clone())
        .expect("seed failing driver inventory");
    let mut registry = DriverModuleRegistry::new();
    registry
        .register(EmptyInventoryDriver)
        .expect("register empty driver");
    registry
        .register(FailingInventoryDriver)
        .expect("register failing driver");

    store.refresh(&registry, &TestHost).await;

    assert_eq!(store.driver_cache(EMPTY_DRIVER.id), prior);
    assert_eq!(store.driver_cache(FAILING_DRIVER.id), prior);
}
