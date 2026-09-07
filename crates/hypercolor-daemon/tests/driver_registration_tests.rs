//! Live driver registration (Spec 81 §2.3): enabling a driver registers its
//! output backend, disabling it disconnects its devices and unregisters.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use async_trait::async_trait;
use hypercolor_core::config::ConfigManager;
use hypercolor_daemon::app_state::AppState;
use hypercolor_daemon::network::{config_key_touches_drivers, reconcile_driver_output_backends};
use hypercolor_driver_api::{
    BackendInfo, DeviceBackend, DeviceBackendFactory, DiscoveredDevice, DiscoveryConnectBehavior,
    DriverConfigView, DriverDescriptor, DriverError, DriverHost, DriverModule, OutputBinding,
};
use hypercolor_network::DriverModuleRegistry;
use hypercolor_types::config::{DriverConfigEntry, HypercolorConfig};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceError, DeviceFamily,
    DeviceFingerprint, DeviceId, DeviceInfo, DeviceOrigin, DeviceState, DeviceTopologyHint,
    DriverTransportKind, FingerprintNamespace, SegmentInfo,
};
use hypercolor_types::event::HypercolorEvent;
use hypercolor_types::identity::BackendId;

static DATA_DIR_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

static FIXTURE_DESCRIPTOR: DriverDescriptor = DriverDescriptor::new(
    "fixture-bridge",
    "Fixture Bridge",
    DriverTransportKind::Bridge,
    true,
    false,
);

#[derive(Default)]
struct Counters {
    connects: AtomicUsize,
    disconnects: AtomicUsize,
}

struct FixtureDriver {
    counters: Arc<Counters>,
}

impl DriverModule for FixtureDriver {
    fn descriptor(&self) -> &'static DriverDescriptor {
        &FIXTURE_DESCRIPTOR
    }

    fn module_descriptor(&self) -> hypercolor_types::device::DriverModuleDescriptor {
        let mut descriptor = self.descriptor().module_descriptor();
        descriptor.default_enabled = false;
        descriptor
    }

    fn output(&self) -> OutputBinding<'_> {
        OutputBinding::Owned {
            id: BackendId::new("fixture-bridge").expect("valid backend id"),
            factory: self,
        }
    }
}

impl DeviceBackendFactory for FixtureDriver {
    fn build(
        &self,
        _host: &dyn DriverHost,
        _config: DriverConfigView<'_>,
    ) -> Result<Arc<dyn DeviceBackend>, DriverError> {
        Ok(Arc::new(FixtureBackend {
            counters: Arc::clone(&self.counters),
        }))
    }
}

struct FixtureBackend {
    counters: Arc<Counters>,
}

#[async_trait]
impl DeviceBackend for FixtureBackend {
    fn info(&self) -> BackendInfo {
        BackendInfo {
            id: "fixture-bridge".to_owned(),
            name: "Fixture Bridge".to_owned(),
            description: "Counts connects and disconnects".to_owned(),
        }
    }

    fn adopt_device(&self, _discovered: &DiscoveredDevice) -> Result<(), DeviceError> {
        Ok(())
    }

    async fn connect(&self, _id: &DeviceId) -> Result<(), DeviceError> {
        self.counters.connects.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    async fn disconnect(&self, _id: &DeviceId) -> Result<(), DeviceError> {
        self.counters.disconnects.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    async fn write_colors(&self, _id: &DeviceId, _colors: &[[u8; 3]]) -> Result<(), DeviceError> {
        Ok(())
    }
}

fn isolated_state(registry: DriverModuleRegistry) -> (AppState, tempfile::TempDir) {
    let _lock = DATA_DIR_LOCK
        .lock()
        .expect("data dir lock should not be poisoned");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let data_dir = tempdir.path().join("data");
    std::fs::create_dir_all(&data_dir).expect("temp data dir should be created");
    ConfigManager::set_data_dir_override(Some(data_dir));
    let state = AppState::builder()
        .with_driver_registry(Arc::new(registry))
        .build();
    ConfigManager::set_data_dir_override(None);
    (state, tempdir)
}

fn config_with_fixture(enabled: bool) -> HypercolorConfig {
    let mut config = HypercolorConfig::default();
    let entry = if enabled {
        DriverConfigEntry::enabled(BTreeMap::new())
    } else {
        DriverConfigEntry::disabled(BTreeMap::new())
    };
    config.drivers.insert("fixture-bridge".to_owned(), entry);
    config
}

fn fixture_device() -> DiscoveredDevice {
    let info = DeviceInfo {
        id: DeviceId::new(),
        name: "Fixture Controller".to_owned(),
        vendor: "Fixture".to_owned(),
        family: DeviceFamily::new_static("fixture-bridge", "Fixture"),
        model: None,
        connection_type: ConnectionType::Network,
        origin: DeviceOrigin::new(
            "fixture-bridge",
            "fixture-bridge",
            DriverTransportKind::Bridge,
        ),
        segments: vec![SegmentInfo {
            name: "Strip".to_owned(),
            led_count: 4,
            topology: DeviceTopologyHint::Strip,
            color_format: DeviceColorFormat::Rgb,
            layout_hint: None,
        }],
        firmware_version: None,
        capabilities: DeviceCapabilities::default(),
    };
    DiscoveredDevice {
        fingerprint: DeviceFingerprint::mint(FingerprintNamespace::Bridge, "fixture-bridge", "c0"),
        connect_behavior: DiscoveryConnectBehavior::Deferred,
        info,
        metadata: std::collections::HashMap::default(),
        claim: None,
    }
}

#[test]
fn driver_keys_select_reconciliation() {
    assert!(config_key_touches_drivers(""));
    assert!(config_key_touches_drivers("drivers"));
    assert!(config_key_touches_drivers("drivers.openrgb"));
    assert!(config_key_touches_drivers("drivers.openrgb.enabled"));
    assert!(!config_key_touches_drivers("daemon.fps"));
    assert!(!config_key_touches_drivers("driversx"));
}

#[tokio::test]
async fn flipping_a_driver_registers_then_unregisters_its_output_backend() {
    let counters = Arc::new(Counters::default());
    let mut registry = DriverModuleRegistry::new();
    registry
        .register(FixtureDriver {
            counters: Arc::clone(&counters),
        })
        .expect("fixture driver should register");
    let (state, _tmp) = isolated_state(registry);
    let runtime = state.driver_host().discovery_runtime();
    let registry = Arc::clone(state.driver_registry());
    let host = Arc::clone(state.driver_host());
    let mut events = state.event_bus.subscribe_all();

    let disabled = config_with_fixture(false);
    let report = reconcile_driver_output_backends(&runtime, &registry, host.as_ref(), &disabled)
        .await
        .expect("reconcile with the driver disabled");
    assert!(report.is_empty(), "nothing to do while the driver is off");
    {
        let manager = runtime.backend_manager.lock().await;
        assert!(!manager.backend_ids().contains(&"fixture-bridge"));
    }

    let enabled = config_with_fixture(true);
    let report = reconcile_driver_output_backends(&runtime, &registry, host.as_ref(), &enabled)
        .await
        .expect("reconcile with the driver enabled");
    assert_eq!(report.registered, vec!["fixture-bridge".to_owned()]);
    assert_eq!(
        report.registered_driver_ids,
        vec!["fixture-bridge".to_owned()]
    );
    assert!(report.unregistered.is_empty());
    {
        let manager = runtime.backend_manager.lock().await;
        assert!(manager.backend_ids().contains(&"fixture-bridge"));
    }
    assert!(
        runtime
            .unclaimed_devices
            .enabled_driver_ids()
            .expect("the reconciler hands the enabled set to the inventory")
            .contains("fixture-bridge")
    );

    // Same config again: idempotent.
    let report = reconcile_driver_output_backends(&runtime, &registry, host.as_ref(), &enabled)
        .await
        .expect("idempotent reconcile");
    assert!(report.is_empty());

    // A device rendering through the backend is disconnected on disable.
    let discovered = fixture_device();
    let info = discovered.info.clone();
    let device_id = runtime.device_registry.add_discovered(discovered).await;
    let mut tracked_info = info;
    tracked_info.id = device_id;
    {
        let mut lifecycle = runtime.lifecycle_manager.lock().await;
        lifecycle.on_discovered_with_behavior(
            device_id,
            &tracked_info,
            None,
            DiscoveryConnectBehavior::Deferred,
        );
        lifecycle
            .on_connected(device_id)
            .expect("fixture device should connect");
    }
    runtime
        .device_registry
        .set_state(&device_id, DeviceState::Connected)
        .await;

    let report = reconcile_driver_output_backends(&runtime, &registry, host.as_ref(), &disabled)
        .await
        .expect("reconcile after disabling the driver");
    assert_eq!(report.unregistered, vec!["fixture-bridge".to_owned()]);
    assert_eq!(report.disconnected_devices, vec![device_id]);
    assert!(report.registered.is_empty());
    assert_eq!(counters.disconnects.load(Ordering::Relaxed), 1);
    {
        let manager = runtime.backend_manager.lock().await;
        assert!(!manager.backend_ids().contains(&"fixture-bridge"));
    }
    let tracked = runtime
        .device_registry
        .get(&device_id)
        .await
        .expect("device stays tracked");
    assert_eq!(tracked.state, DeviceState::Known);

    let mut saw_disconnect = false;
    while let Ok(timestamped) = events.try_recv() {
        if let HypercolorEvent::DeviceDisconnected {
            device_id: id,
            will_retry,
            ..
        } = timestamped.event
            && id == device_id.to_string()
        {
            assert!(!will_retry);
            saw_disconnect = true;
        }
    }
    assert!(
        saw_disconnect,
        "disabling a driver publishes DeviceDisconnected"
    );
}

#[tokio::test]
async fn daemon_owned_backends_are_never_unregistered() {
    let (state, _tmp) = isolated_state(DriverModuleRegistry::new());
    let runtime = state.driver_host().discovery_runtime();
    let before: Vec<String> = {
        let manager = runtime.backend_manager.lock().await;
        manager
            .backend_ids()
            .into_iter()
            .map(ToOwned::to_owned)
            .collect()
    };
    assert!(
        !before.is_empty(),
        "the simulator backend registers regardless of drivers"
    );

    let report = reconcile_driver_output_backends(
        &runtime,
        state.driver_registry(),
        state.driver_host().as_ref(),
        &HypercolorConfig::default(),
    )
    .await
    .expect("reconcile with no drivers");
    assert!(report.is_empty());
    let after: Vec<String> = {
        let manager = runtime.backend_manager.lock().await;
        manager
            .backend_ids()
            .into_iter()
            .map(ToOwned::to_owned)
            .collect()
    };
    assert_eq!(before, after);
}
