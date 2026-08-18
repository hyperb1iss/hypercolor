use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, bail};
use async_trait::async_trait;
use hypercolor_daemon::driver_inventory::DriverInventoryStore;
#[cfg(feature = "persistence-test-hooks")]
use hypercolor_daemon::persistence::AtomicFileWriter;
use hypercolor_driver_api::{
    DriverCredentialStore, DriverDescriptor, DriverDiscoveryState, DriverHost, DriverModule,
    DriverRuntimeActions, DriverRuntimeCacheProvider, DriverTrackedDevice,
};
use hypercolor_network::DriverModuleRegistry;
use hypercolor_types::device::{DeviceId, DriverTransportKind};
use serde_json::json;
use tempfile::TempDir;
use tokio::sync::Notify;

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
static REFRESHING_DRIVER: DriverDescriptor = DriverDescriptor::new(
    "refreshing_inventory_test",
    "Refreshing Inventory Test",
    DriverTransportKind::Network,
    false,
    false,
);
static BLOCKING_DRIVER: DriverDescriptor = DriverDescriptor::new(
    "blocking_inventory_test",
    "Blocking Inventory Test",
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

struct RefreshingInventoryDriver;

impl DriverModule for RefreshingInventoryDriver {
    fn descriptor(&self) -> &'static DriverDescriptor {
        &REFRESHING_DRIVER
    }

    fn runtime_cache(&self) -> Option<&dyn DriverRuntimeCacheProvider> {
        Some(self)
    }
}

#[async_trait]
impl DriverRuntimeCacheProvider for RefreshingInventoryDriver {
    async fn snapshot(
        &self,
        _host: &dyn DriverHost,
    ) -> Result<BTreeMap<String, serde_json::Value>> {
        Ok(BTreeMap::from([("known".to_owned(), json!("fresh"))]))
    }
}

struct BlockingInventoryDriver {
    entered: Arc<Notify>,
    release: Arc<Notify>,
}

impl DriverModule for BlockingInventoryDriver {
    fn descriptor(&self) -> &'static DriverDescriptor {
        &BLOCKING_DRIVER
    }

    fn runtime_cache(&self) -> Option<&dyn DriverRuntimeCacheProvider> {
        Some(self)
    }
}

#[async_trait]
impl DriverRuntimeCacheProvider for BlockingInventoryDriver {
    async fn snapshot(
        &self,
        _host: &dyn DriverHost,
    ) -> Result<BTreeMap<String, serde_json::Value>> {
        self.entered.notify_one();
        self.release.notified().await;
        Ok(BTreeMap::from([("targets".to_owned(), json!(["a", "b"]))]))
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

#[tokio::test]
async fn driver_inventory_round_trips_opaque_driver_payloads() {
    let tempdir = TempDir::new().expect("tempdir");
    let inventory_path = tempdir.path().join("driver-inventory.json");
    let store = DriverInventoryStore::open(inventory_path.clone()).expect("open inventory");

    store
        .replace_driver(
            "wled",
            BTreeMap::from([("probe_ips".to_owned(), json!(["10.4.22.69"]))]),
        )
        .await
        .expect("persist WLED inventory");

    let reopened = DriverInventoryStore::open(inventory_path).expect("reopen inventory");
    assert_eq!(
        reopened.load_cached_json("wled", "probe_ips"),
        Some(json!(["10.4.22.69"]))
    );
}

#[tokio::test]
async fn corrupt_inventory_is_quarantined_before_replacement() {
    let tempdir = TempDir::new().expect("tempdir");
    let inventory_path = tempdir.path().join("driver-inventory.json");
    std::fs::write(&inventory_path, b"not json").expect("write corrupt inventory");

    let store =
        DriverInventoryStore::open(inventory_path.clone()).expect("open quarantined inventory");
    store
        .replace_driver(
            "wled",
            BTreeMap::from([("probe_ips".to_owned(), json!(["10.4.22.192"]))]),
        )
        .await
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

#[tokio::test]
async fn updating_one_driver_preserves_opaque_sibling_payloads() {
    let tempdir = TempDir::new().expect("tempdir");
    let inventory_path = tempdir.path().join("driver-inventory.json");
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

    let store = DriverInventoryStore::open(inventory_path.clone()).expect("open inventory");
    store
        .replace_driver(
            "wled",
            BTreeMap::from([("probe_ips".to_owned(), json!(["10.4.22.192"]))]),
        )
        .await
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
    let store = DriverInventoryStore::open(inventory_path).expect("open inventory");
    let prior = BTreeMap::from([("target".to_owned(), json!("10.4.22.69"))]);
    store
        .replace_driver(EMPTY_DRIVER.id, prior.clone())
        .await
        .expect("seed empty driver inventory");
    store
        .replace_driver(FAILING_DRIVER.id, prior.clone())
        .await
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

#[tokio::test]
async fn refresh_preserves_unknown_driver_fields() {
    let tempdir = TempDir::new().expect("tempdir");
    let inventory_path = tempdir.path().join("driver-inventory.json");
    let store = DriverInventoryStore::open(inventory_path).expect("open inventory");
    store
        .replace_driver(
            REFRESHING_DRIVER.id,
            BTreeMap::from([
                ("known".to_owned(), json!("stale")),
                ("future_key".to_owned(), json!({"preserve": true})),
            ]),
        )
        .await
        .expect("seed refreshing inventory");
    let mut registry = DriverModuleRegistry::new();
    registry
        .register(RefreshingInventoryDriver)
        .expect("register refreshing driver");

    store.refresh(&registry, &TestHost).await;

    let cache = store.driver_cache(REFRESHING_DRIVER.id);
    assert_eq!(cache["known"], json!("fresh"));
    assert_eq!(cache["future_key"], json!({"preserve": true}));
}

#[tokio::test]
async fn refresh_and_driver_update_are_serialized_without_resurrection() {
    let tempdir = TempDir::new().expect("tempdir");
    let inventory_path = tempdir.path().join("driver-inventory.json");
    let store = Arc::new(DriverInventoryStore::open(inventory_path).expect("open inventory"));
    store
        .replace_driver(
            BLOCKING_DRIVER.id,
            BTreeMap::from([("targets".to_owned(), json!(["a", "b"]))]),
        )
        .await
        .expect("seed blocking inventory");
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let mut registry = DriverModuleRegistry::new();
    registry
        .register(BlockingInventoryDriver {
            entered: Arc::clone(&entered),
            release: Arc::clone(&release),
        })
        .expect("register blocking driver");
    let registry = Arc::new(registry);
    let refresh_store = Arc::clone(&store);
    let refresh_registry = Arc::clone(&registry);
    let refresh = tokio::spawn(async move {
        refresh_store.refresh(&refresh_registry, &TestHost).await;
    });
    entered.notified().await;

    let update_store = Arc::clone(&store);
    let mut update = tokio::spawn(async move {
        update_store
            .update_driver(BLOCKING_DRIVER.id, |current| {
                let mut updated = current.clone();
                updated.insert("targets".to_owned(), json!(["b"]));
                Ok(updated)
            })
            .await
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(25), &mut update)
            .await
            .is_err(),
        "driver update should wait for the in-flight refresh transaction"
    );

    release.notify_one();
    refresh.await.expect("refresh task");
    update
        .await
        .expect("update task")
        .expect("serialized driver update");
    assert_eq!(
        store.driver_cache(BLOCKING_DRIVER.id)["targets"],
        json!(["b"])
    );
}

#[tokio::test]
async fn concurrent_driver_updates_compose_from_latest_inventory() {
    let tempdir = TempDir::new().expect("tempdir");
    let inventory_path = tempdir.path().join("driver-inventory.json");
    let store = Arc::new(DriverInventoryStore::open(inventory_path).expect("open inventory"));
    store
        .replace_driver(
            "concurrent",
            BTreeMap::from([("targets".to_owned(), json!(["a", "b"]))]),
        )
        .await
        .expect("seed concurrent inventory");

    let updates = ["a", "b"].map(|forgotten| {
        let store = Arc::clone(&store);
        tokio::spawn(async move {
            store
                .update_driver("concurrent", |current| {
                    let mut updated = current.clone();
                    let retained = current["targets"]
                        .as_array()
                        .expect("target array")
                        .iter()
                        .filter(|target| target.as_str() != Some(forgotten))
                        .cloned()
                        .collect::<Vec<_>>();
                    updated.insert("targets".to_owned(), serde_json::Value::Array(retained));
                    Ok(updated)
                })
                .await
        })
    });
    for update in updates {
        update
            .await
            .expect("update task")
            .expect("concurrent inventory update");
    }

    assert_eq!(store.driver_cache("concurrent")["targets"], json!([]));
}

#[cfg(feature = "persistence-test-hooks")]
#[tokio::test]
async fn admitted_replacement_failure_keeps_live_inventory_authoritative() {
    let tempdir = TempDir::new().expect("tempdir");
    let inventory_path = tempdir.path().join("driver-inventory.json");
    let store = DriverInventoryStore::open(inventory_path.clone()).expect("open inventory");
    let writer = AtomicFileWriter::new(&inventory_path).expect("inventory writer");
    writer.set_injected_replace_failures(usize::MAX);

    store
        .replace_driver(
            "wled",
            BTreeMap::from([("probe_ips".to_owned(), json!(["10.4.22.69"]))]),
        )
        .await
        .expect("admitted intent should remain authoritative");
    assert_eq!(
        store.load_cached_json("wled", "probe_ips"),
        Some(json!(["10.4.22.69"]))
    );

    writer.set_injected_replace_failures(0);
    writer.kick();
    writer
        .flush(Duration::from_secs(5))
        .expect("inventory retry should converge");
    let reopened = DriverInventoryStore::open(inventory_path).expect("reopen inventory");
    assert_eq!(
        reopened.load_cached_json("wled", "probe_ips"),
        Some(json!(["10.4.22.69"]))
    );
}
