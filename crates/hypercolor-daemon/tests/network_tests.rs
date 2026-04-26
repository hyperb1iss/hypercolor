use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use async_trait::async_trait;
use hypercolor_core::device::{BackendInfo, BackendManager, DeviceBackend};
use hypercolor_daemon::api::AppState;
use hypercolor_daemon::network;
use hypercolor_driver_api::{
    DriverConfigView, DriverCredentialStore, DriverDescriptor, DriverDiscoveryState, DriverHost,
    DriverRuntimeActions, DriverTransport, NetworkDriverFactory,
};
use hypercolor_network::DriverRegistry;
use hypercolor_types::config::{DriverConfigEntry, HypercolorConfig};
use hypercolor_types::device::{DeviceId, DeviceInfo};

#[test]
fn default_app_state_registers_builtin_network_drivers() {
    let state = AppState::new();
    let ids = state.driver_registry.ids();

    assert!(ids.contains(&"wled".to_owned()));
    #[cfg(feature = "hue")]
    assert!(ids.contains(&"hue".to_owned()));
    #[cfg(feature = "nanoleaf")]
    assert!(ids.contains(&"nanoleaf".to_owned()));
}

#[test]
fn builtin_pairing_drivers_expose_pairing_capabilities() {
    let state = AppState::new();
    #[cfg(not(any(feature = "hue", feature = "nanoleaf")))]
    let _ = &state;

    #[cfg(feature = "hue")]
    assert!(
        state
            .driver_registry
            .get("hue")
            .expect("hue driver should be registered")
            .pairing()
            .is_some()
    );

    #[cfg(feature = "nanoleaf")]
    assert!(
        state
            .driver_registry
            .get("nanoleaf")
            .expect("nanoleaf driver should be registered")
            .pairing()
            .is_some()
    );
}

#[test]
fn builtin_network_drivers_expose_discovery_capabilities() {
    let state = AppState::new();

    assert!(
        state
            .driver_registry
            .get("wled")
            .expect("wled driver should be registered")
            .discovery()
            .is_some()
    );
    #[cfg(feature = "hue")]
    assert!(
        state
            .driver_registry
            .get("hue")
            .expect("hue driver should be registered")
            .discovery()
            .is_some()
    );
    #[cfg(feature = "nanoleaf")]
    assert!(
        state
            .driver_registry
            .get("nanoleaf")
            .expect("nanoleaf driver should be registered")
            .discovery()
            .is_some()
    );
}

#[test]
fn enabled_hal_driver_ids_honor_driver_config_entries() {
    let mut config = HypercolorConfig::default();
    config.drivers.insert(
        "nollie".to_owned(),
        DriverConfigEntry::disabled(BTreeMap::new()),
    );

    let enabled = network::enabled_hal_driver_ids(&config);

    assert!(!enabled.contains("nollie"));
    assert!(enabled.contains("prismrgb"));
    assert!(network::hal_driver_enabled(&config, "prismrgb"));
    assert!(!network::hal_driver_enabled(&config, "nollie"));
}

#[test]
fn enabled_hal_driver_ids_include_default_enabled_hal_modules() {
    let enabled = network::enabled_hal_driver_ids(&HypercolorConfig::default());

    assert!(enabled.is_superset(&BTreeSet::from([
        "asus".to_owned(),
        "nollie".to_owned(),
        "prismrgb".to_owned(),
        "razer".to_owned(),
    ])));
}

struct NullCredentialStore;

#[async_trait]
impl DriverCredentialStore for NullCredentialStore {
    async fn get_json(&self, key: &str) -> Result<Option<serde_json::Value>> {
        let _ = key;
        Ok(None)
    }

    async fn set_json(&self, key: &str, value: serde_json::Value) -> Result<()> {
        let _ = (key, value);
        Ok(())
    }

    async fn remove(&self, key: &str) -> Result<()> {
        let _ = key;
        Ok(())
    }
}

struct NullRuntimeActions;

#[async_trait]
impl DriverRuntimeActions for NullRuntimeActions {
    async fn activate_device(&self, device_id: DeviceId, backend_id: &str) -> Result<bool> {
        let _ = (device_id, backend_id);
        Ok(false)
    }

    async fn disconnect_device(
        &self,
        device_id: DeviceId,
        backend_id: &str,
        will_retry: bool,
    ) -> Result<bool> {
        let _ = (device_id, backend_id, will_retry);
        Ok(false)
    }
}

struct NullDiscoveryState;

#[async_trait]
impl DriverDiscoveryState for NullDiscoveryState {
    async fn tracked_devices(
        &self,
        driver_id: &str,
    ) -> Vec<hypercolor_driver_api::DriverTrackedDevice> {
        let _ = driver_id;
        Vec::new()
    }

    fn load_cached_json(&self, driver_id: &str, key: &str) -> Result<Option<serde_json::Value>> {
        let _ = (driver_id, key);
        Ok(None)
    }
}

struct NullHost {
    credentials: NullCredentialStore,
    runtime: NullRuntimeActions,
}

impl NullHost {
    fn new() -> Self {
        Self {
            credentials: NullCredentialStore,
            runtime: NullRuntimeActions,
        }
    }
}

impl DriverHost for NullHost {
    fn credentials(&self) -> &dyn DriverCredentialStore {
        &self.credentials
    }

    fn runtime(&self) -> &dyn DriverRuntimeActions {
        &self.runtime
    }

    fn discovery_state(&self) -> &dyn DriverDiscoveryState {
        static DISCOVERY_STATE: NullDiscoveryState = NullDiscoveryState;
        &DISCOVERY_STATE
    }
}

struct TestBackend {
    id: &'static str,
}

#[async_trait]
impl DeviceBackend for TestBackend {
    fn info(&self) -> BackendInfo {
        BackendInfo {
            id: self.id.to_owned(),
            name: "Test Backend".to_owned(),
            description: "Test backend".to_owned(),
        }
    }

    async fn discover(&mut self) -> Result<Vec<DeviceInfo>> {
        Ok(Vec::new())
    }

    async fn connect(&mut self, id: &DeviceId) -> Result<()> {
        let _ = id;
        Ok(())
    }

    async fn disconnect(&mut self, id: &DeviceId) -> Result<()> {
        let _ = id;
        Ok(())
    }

    async fn write_colors(&mut self, id: &DeviceId, colors: &[[u8; 3]]) -> Result<()> {
        let _ = (id, colors);
        Ok(())
    }
}

struct ConfiglessDriver;

static CONFIGLESS_DESCRIPTOR: DriverDescriptor = DriverDescriptor::new(
    "external",
    "External Driver",
    DriverTransport::Network,
    true,
    false,
);

impl NetworkDriverFactory for ConfiglessDriver {
    fn descriptor(&self) -> &'static DriverDescriptor {
        &CONFIGLESS_DESCRIPTOR
    }

    fn build_backend(
        &self,
        host: &dyn DriverHost,
        config: DriverConfigView<'_>,
    ) -> Result<Option<Box<dyn DeviceBackend>>> {
        let _ = host;
        assert_eq!(config.driver_id, "external");
        assert!(config.enabled());
        assert!(config.entry.settings.is_empty());
        Ok(Some(Box::new(TestBackend {
            id: "external-backend",
        })))
    }
}

#[test]
fn register_enabled_backends_uses_default_config_for_configless_driver() {
    let host = NullHost::new();
    let mut registry = DriverRegistry::new();
    registry
        .register(ConfiglessDriver)
        .expect("configless driver should register");
    let config = HypercolorConfig::default();
    let mut backend_manager = BackendManager::new();

    network::register_enabled_backends(&mut backend_manager, &registry, &host, &config)
        .expect("configless driver should register a backend");

    assert_eq!(backend_manager.backend_ids(), vec!["external-backend"]);
}

#[test]
fn register_enabled_backends_skips_config_disabled_driver() {
    let host = NullHost::new();
    let mut registry = DriverRegistry::new();
    registry
        .register(ConfiglessDriver)
        .expect("configless driver should register");
    let mut config = HypercolorConfig::default();
    config.drivers.insert(
        "external".to_owned(),
        DriverConfigEntry::disabled(BTreeMap::default()),
    );
    let mut backend_manager = BackendManager::new();

    network::register_enabled_backends(&mut backend_manager, &registry, &host, &config)
        .expect("disabled driver should be skipped cleanly");

    assert!(backend_manager.backend_ids().is_empty());
}
