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
    built_settings: Mutex<Vec<BTreeMap<String, serde_json::Value>>>,
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
        config: DriverConfigView<'_>,
    ) -> Result<Arc<dyn DeviceBackend>, DriverError> {
        if config.entry.settings.contains_key("fail_build") {
            return Err(DriverError::Configuration {
                message: "fixture build rejected".to_owned(),
            });
        }
        self.counters
            .built_settings
            .lock()
            .expect("settings lock")
            .push(config.entry.settings.clone());
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

    async fn connected_device_metadata(
        &self,
        _id: &DeviceId,
    ) -> Result<Option<std::collections::HashMap<String, String>>, DeviceError> {
        Ok(Some(std::collections::HashMap::from([(
            "output_enabled".to_owned(),
            "true".to_owned(),
        )])))
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

#[tokio::test(start_paused = true)]
async fn retained_runtime_reconnects_idle_devices_without_a_frame_failure() {
    let counters = Arc::new(Counters::default());
    let mut registry = DriverModuleRegistry::new();
    registry
        .register(FixtureDriver {
            counters: Arc::clone(&counters),
        })
        .expect("register fixture");
    let (state, _tmp) = isolated_state(registry);
    let runtime = state.driver_host().discovery_runtime();
    reconcile_driver_output_backends(
        &runtime,
        state.driver_registry(),
        state.driver_host().as_ref(),
        &config_with_fixture(true),
        None,
    )
    .await
    .expect("enable fixture");
    let discovered = fixture_device();
    let mut info = discovered.info.clone();
    let device_id = runtime.device_registry.add_discovered(discovered).await;
    info.id = device_id;
    {
        let mut lifecycle = runtime.lifecycle_manager.lock().await;
        lifecycle.on_discovered_with_behavior(
            device_id,
            &info,
            None,
            DiscoveryConnectBehavior::Deferred,
        );
        lifecycle
            .on_connected(device_id)
            .expect("connected fixture");
    }
    runtime
        .device_registry
        .set_state(&device_id, DeviceState::Connected)
        .await;
    let retained = state
        .driver_host()
        .runtime_handle()
        .expect("owned runtime handle");

    assert!(
        retained
            .request_reconnect(device_id, "unrelated-backend", None)
            .await
            .is_err()
    );
    assert_eq!(counters.disconnects.load(Ordering::Relaxed), 0);
    let mut refreshed = fixture_device();
    refreshed.info.id = device_id;
    refreshed.info.segments[0].led_count = 0;
    refreshed
        .metadata
        .insert("output_enabled".to_owned(), "false".to_owned());
    refreshed.metadata.insert(
        "disabled_reason".to_owned(),
        "zone shape changed (was 4, now 0); rescan".to_owned(),
    );
    assert!(
        retained
            .request_reconnect(device_id, "fixture-bridge", Some(refreshed))
            .await
            .expect("request reconnect")
    );
    assert_eq!(counters.disconnects.load(Ordering::Relaxed), 1);
    assert_eq!(
        runtime
            .device_registry
            .get(&device_id)
            .await
            .expect("refreshed device")
            .info
            .total_led_count(),
        0
    );
    assert_eq!(
        runtime
            .device_registry
            .metadata_for_id(&device_id)
            .await
            .expect("updated metadata")["output_enabled"],
        "false"
    );
    assert_eq!(
        runtime
            .device_registry
            .get(&device_id)
            .await
            .expect("tracked device")
            .state,
        DeviceState::Reconnecting
    );
    assert!(
        !retained
            .request_reconnect(device_id, "fixture-bridge", None)
            .await
            .expect("duplicate is a no-op")
    );

    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if runtime
                .device_registry
                .get(&device_id)
                .await
                .expect("tracked device")
                .state
                .is_renderable()
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("lifecycle reconnects without output frames");
    assert_eq!(counters.connects.load(Ordering::Relaxed), 1);
    let metadata = runtime
        .device_registry
        .metadata_for_id(&device_id)
        .await
        .expect("connected metadata");
    assert_eq!(metadata["output_enabled"], "true");
    assert!(!metadata.contains_key("disabled_reason"));
}

#[tokio::test]
async fn retained_runtime_does_not_keep_daemon_state_alive() {
    let (state, _tmp) = isolated_state(DriverModuleRegistry::new());
    let host = Arc::clone(state.driver_host());
    let retained = host.runtime_handle().expect("owned runtime handle");
    let backend_manager = Arc::downgrade(&host.discovery_runtime().backend_manager);
    drop(state);
    drop(host);
    assert!(backend_manager.upgrade().is_none());
    assert!(
        retained
            .request_reconnect(DeviceId::new(), "fixture-bridge", None)
            .await
            .is_err()
    );
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
    let report =
        reconcile_driver_output_backends(&runtime, &registry, host.as_ref(), &disabled, None)
            .await
            .expect("reconcile with the driver disabled");
    assert!(report.is_empty(), "nothing to do while the driver is off");
    {
        let manager = runtime.backend_manager.lock().await;
        assert!(!manager.backend_ids().contains(&"fixture-bridge"));
    }

    let enabled = config_with_fixture(true);
    let report =
        reconcile_driver_output_backends(&runtime, &registry, host.as_ref(), &enabled, None)
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
    let report =
        reconcile_driver_output_backends(&runtime, &registry, host.as_ref(), &enabled, None)
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

    let mut bridge = fixture_device();
    bridge.info.id = DeviceId::new();
    bridge.info.origin = DeviceOrigin::new("openrgb", "openrgb", DriverTransportKind::Bridge);
    bridge.fingerprint =
        DeviceFingerprint::mint(FingerprintNamespace::Bridge, "openrgb", "guarded-route");
    let mut bridge_info = bridge.info.clone();
    let bridge_id = runtime.device_registry.add_discovered(bridge).await;
    bridge_info.id = bridge_id;
    {
        let mut lifecycle = runtime.lifecycle_manager.lock().await;
        lifecycle.on_discovered_with_behavior(
            bridge_id,
            &bridge_info,
            None,
            DiscoveryConnectBehavior::Deferred,
        );
        lifecycle.on_user_disable(bridge_id).expect("guard disable");
    }
    runtime
        .device_registry
        .set_state(&bridge_id, DeviceState::Disabled)
        .await;
    runtime.bridge_output_locks.insert(
        bridge_id,
        hypercolor_daemon::discovery::BridgeOutputLock {
            native_device_id: device_id,
            native_driver_id: "fixture-bridge".to_owned(),
            reason: "native owns fixture".to_owned(),
        },
    );
    let report =
        reconcile_driver_output_backends(&runtime, &registry, host.as_ref(), &disabled, None)
            .await
            .expect("reconcile after disabling the driver");
    assert!(!runtime.bridge_output_locks.is_locked(&bridge_id));
    assert_eq!(
        runtime
            .device_registry
            .get(&bridge_id)
            .await
            .expect("released bridge")
            .state,
        DeviceState::Known
    );
    assert!(
        runtime
            .device_registry
            .get(&device_id)
            .await
            .expect("native settings")
            .user_settings
            .enabled
    );
    // Releasing an ownership lock must preserve an explicit bridge disable.
    runtime
        .device_registry
        .update_user_settings(&bridge_id, None, Some(false), None, None)
        .await
        .expect("bridge user settings");
    runtime
        .lifecycle_manager
        .lock()
        .await
        .on_user_disable(bridge_id)
        .expect("user disable");
    runtime
        .device_registry
        .set_state(&bridge_id, DeviceState::Disabled)
        .await;
    runtime.bridge_output_locks.insert(
        bridge_id,
        hypercolor_daemon::discovery::BridgeOutputLock {
            native_device_id: device_id,
            native_driver_id: "fixture-bridge".to_owned(),
            reason: "native owns fixture".to_owned(),
        },
    );
    reconcile_driver_output_backends(
        &runtime,
        &registry,
        host.as_ref(),
        &disabled,
        Some(&disabled),
    )
    .await
    .expect("release user-disabled bridge");
    assert!(!runtime.bridge_output_locks.is_locked(&bridge_id));
    let bridge = runtime
        .device_registry
        .get(&bridge_id)
        .await
        .expect("bridge retained");
    assert_eq!(bridge.state, DeviceState::Disabled);
    assert!(!bridge.user_settings.enabled);

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
        None,
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

#[tokio::test]
async fn changing_enabled_driver_settings_rebuilds_output_and_requests_discovery() {
    let counters = Arc::new(Counters::default());
    let mut registry = DriverModuleRegistry::new();
    registry
        .register(FixtureDriver {
            counters: Arc::clone(&counters),
        })
        .expect("fixture driver");
    let (state, _tmp) = isolated_state(registry);
    let runtime = state.driver_host().discovery_runtime();
    let original = config_with_fixture(true);
    reconcile_driver_output_backends(
        &runtime,
        state.driver_registry(),
        state.driver_host().as_ref(),
        &original,
        None,
    )
    .await
    .expect("initial provider");
    let discovered = fixture_device();
    let mut info = discovered.info.clone();
    let id = runtime.device_registry.add_discovered(discovered).await;
    info.id = id;
    {
        let mut lifecycle = runtime.lifecycle_manager.lock().await;
        lifecycle.on_discovered_with_behavior(id, &info, None, DiscoveryConnectBehavior::Deferred);
        lifecycle.on_connected(id).expect("connected fixture");
    }
    runtime
        .device_registry
        .set_state(&id, DeviceState::Connected)
        .await;
    let mut rejected = original.clone();
    rejected
        .drivers
        .get_mut("fixture-bridge")
        .expect("entry")
        .settings
        .insert("fail_build".to_owned(), serde_json::json!(true));
    assert!(
        reconcile_driver_output_backends(
            &runtime,
            state.driver_registry(),
            state.driver_host().as_ref(),
            &rejected,
            Some(&original),
        )
        .await
        .is_err()
    );
    assert_eq!(counters.disconnects.load(Ordering::Relaxed), 0);
    assert_eq!(
        runtime
            .device_registry
            .get(&id)
            .await
            .expect("working route")
            .state,
        DeviceState::Connected
    );
    assert!(
        runtime
            .backend_manager
            .lock()
            .await
            .backend_ids()
            .contains(&"fixture-bridge")
    );

    let mut updated = original.clone();
    updated
        .drivers
        .get_mut("fixture-bridge")
        .expect("entry")
        .settings
        .insert(
            "zone_sizes".to_owned(),
            serde_json::json!({"controller": {"strip": 16}}),
        );
    let report = reconcile_driver_output_backends(
        &runtime,
        state.driver_registry(),
        state.driver_host().as_ref(),
        &updated,
        Some(&original),
    )
    .await
    .expect("settings rebuild");
    assert_eq!(report.disconnected_devices, vec![id]);
    assert_eq!(report.registered_driver_ids, vec!["fixture-bridge"]);
    assert_eq!(report.unregistered, vec!["fixture-bridge"]);
    assert_eq!(report.registered, vec!["fixture-bridge"]);
    assert_eq!(counters.disconnects.load(Ordering::Relaxed), 1);
    {
        let settings = counters.built_settings.lock().expect("settings lock");
        assert_eq!(settings.len(), 2);
        assert_eq!(
            settings[1]["zone_sizes"],
            serde_json::json!({"controller": {"strip": 16}})
        );
    }
    let tracked = runtime
        .device_registry
        .get(&id)
        .await
        .expect("tracked fixture");
    assert!(tracked.user_settings.enabled);
    assert_eq!(tracked.state, DeviceState::Known);
    let repeated = reconcile_driver_output_backends(
        &runtime,
        state.driver_registry(),
        state.driver_host().as_ref(),
        &updated,
        Some(&updated),
    )
    .await
    .expect("unchanged settings");
    assert!(repeated.is_empty());
    assert_eq!(
        counters.built_settings.lock().expect("settings lock").len(),
        2
    );
}

#[tokio::test]
async fn reconnect_snapshot_mismatch_still_reconnects_and_known_refresh_publishes() {
    let (state, _tmp) = isolated_state(DriverModuleRegistry::new());
    let runtime = state.driver_host().discovery_runtime();
    let discovery = fixture_device();
    let id = runtime
        .device_registry
        .add_discovered(discovery.clone())
        .await;
    let mut info = discovery.info.clone();
    info.id = id;
    runtime
        .lifecycle_manager
        .lock()
        .await
        .on_discovered_with_behavior(id, &info, None, DiscoveryConnectBehavior::Deferred);
    let handle = state
        .driver_host()
        .runtime_handle()
        .expect("runtime handle");
    let mut events = state.event_bus.subscribe_all();
    let mut refresh = discovery;
    refresh.info.id = id;
    refresh.info.segments[0].led_count = 0;
    assert!(
        !handle
            .request_reconnect(id, "fixture-bridge", Some(refresh.clone()))
            .await
            .expect("refresh Known")
    );
    let event = events.try_recv().expect("refresh publishes");
    assert!(matches!(
        event.event,
        HypercolorEvent::DeviceStateChanged { .. }
    ));
    runtime
        .lifecycle_manager
        .lock()
        .await
        .on_connected(id)
        .expect("connected fixture");
    runtime
        .device_registry
        .set_state(&id, DeviceState::Connected)
        .await;
    refresh.fingerprint =
        DeviceFingerprint::mint(FingerprintNamespace::Bridge, "fixture-bridge", "mismatch");
    assert!(
        handle
            .request_reconnect(id, "fixture-bridge", Some(refresh))
            .await
            .expect("identity mismatch still requests reconnect")
    );
    assert_eq!(
        runtime
            .device_registry
            .get(&id)
            .await
            .expect("tracked")
            .state,
        DeviceState::Reconnecting
    );
}
