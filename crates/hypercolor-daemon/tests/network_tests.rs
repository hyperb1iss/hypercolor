use std::collections::BTreeMap;
#[cfg(feature = "builtin-drivers")]
use std::collections::BTreeSet;
use std::sync::LazyLock;

use anyhow::Result;
use async_trait::async_trait;
use hypercolor_core::device::BackendManager;
#[cfg(feature = "builtin-drivers")]
use hypercolor_daemon::api::AppState;
use hypercolor_daemon::network;
use hypercolor_driver_api::{
    BackendInfo, DeviceBackend, DriverConfigView, DriverCredentialStore, DriverDescriptor,
    DriverDiscoveryState, DriverHost, DriverModule, DriverPresentationProvider,
    DriverProtocolCatalog, DriverRuntimeActions, DriverTransport,
};
use hypercolor_network::DriverModuleRegistry;
use hypercolor_types::config::{DriverConfigEntry, HypercolorConfig};
#[cfg(feature = "builtin-drivers")]
use hypercolor_types::device::DriverModuleKind;
use hypercolor_types::device::{
    DeviceClassHint, DeviceId, DeviceInfo, DriverPresentation, DriverProtocolDescriptor,
    DriverTransportKind,
};

#[test]
#[cfg(feature = "builtin-drivers")]
fn default_app_state_registers_builtin_network_drivers() {
    let state = AppState::new();
    let ids = state.driver_registry.ids();

    assert!(ids.contains(&"wled".to_owned()));
    assert!(ids.contains(&"hue".to_owned()));
    assert!(ids.contains(&"nanoleaf".to_owned()));
}

#[test]
#[cfg(feature = "builtin-drivers")]
fn builtin_pairing_drivers_expose_pairing_capabilities() {
    let state = AppState::new();

    assert!(
        state
            .driver_registry
            .get("hue")
            .expect("hue driver should be registered")
            .pairing()
            .is_some()
    );

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
#[cfg(feature = "builtin-drivers")]
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
    assert!(
        state
            .driver_registry
            .get("hue")
            .expect("hue driver should be registered")
            .discovery()
            .is_some()
    );
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
fn host_transport_scanner_factory_handles_known_and_unknown_targets() {
    let registry = DriverModuleRegistry::new();
    let config = HypercolorConfig::default();

    let usb = network::host_transport_scanner("usb", &registry, &config)
        .expect("usb host scanner should be built");
    assert_eq!(usb.name(), "USB HAL");

    let smbus = network::host_transport_scanner("smbus", &registry, &config)
        .expect("smbus host scanner should be built");
    assert_eq!(smbus.name(), "SMBus HAL");

    let blocks = network::host_transport_scanner("blocks", &registry, &config)
        .expect("blocks host scanner should be built");
    assert_eq!(blocks.name(), "ROLI Blocks (blocksd)");

    assert!(
        network::host_transport_scanner("unknown", &registry, &config).is_none(),
        "unknown host discovery targets should not build scanners"
    );
}

#[test]
#[cfg(feature = "builtin-drivers")]
fn enabled_module_ids_honor_driver_config_entries() {
    let state = AppState::new();
    let mut config = HypercolorConfig::default();
    config.drivers.insert(
        "nollie".to_owned(),
        DriverConfigEntry::disabled(BTreeMap::new()),
    );

    let enabled = network::enabled_module_ids(
        state.driver_registry.as_ref(),
        &config,
        DriverModuleKind::Hal,
    );

    assert!(!enabled.contains("nollie"));
    assert!(enabled.contains("prismrgb"));
    assert!(network::module_enabled_by_id(
        state.driver_registry.as_ref(),
        &config,
        "prismrgb"
    ));
    assert!(!network::module_enabled_by_id(
        state.driver_registry.as_ref(),
        &config,
        "nollie"
    ));
}

#[test]
#[cfg(feature = "builtin-drivers")]
fn enabled_module_ids_can_filter_by_transport() {
    let state = AppState::new();
    let mut config = HypercolorConfig::default();
    config.drivers.insert(
        "asus".to_owned(),
        DriverConfigEntry::disabled(BTreeMap::new()),
    );

    let enabled = network::enabled_module_ids_for_transport(
        state.driver_registry.as_ref(),
        &config,
        DriverModuleKind::Hal,
        &DriverTransportKind::Smbus,
    );

    assert!(!enabled.contains("asus"));
    assert!(enabled.is_empty());
}

#[test]
#[cfg(feature = "builtin-drivers")]
fn enabled_module_ids_include_default_enabled_hal_modules() {
    let state = AppState::new();
    let enabled = network::enabled_module_ids(
        state.driver_registry.as_ref(),
        &HypercolorConfig::default(),
        DriverModuleKind::Hal,
    );

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
    async fn get_json(&self, driver_id: &str, key: &str) -> Result<Option<serde_json::Value>> {
        let _ = (driver_id, key);
        Ok(None)
    }

    async fn set_json(&self, driver_id: &str, key: &str, value: serde_json::Value) -> Result<()> {
        let _ = (driver_id, key, value);
        Ok(())
    }

    async fn remove(&self, driver_id: &str, key: &str) -> Result<()> {
        let _ = (driver_id, key);
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

impl DriverModule for ConfiglessDriver {
    fn descriptor(&self) -> &'static DriverDescriptor {
        &CONFIGLESS_DESCRIPTOR
    }

    fn build_output_backend(
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

    fn has_output_backend(&self) -> bool {
        true
    }
}

struct CapabilityOnlyDriver;

static CAPABILITY_ONLY_DESCRIPTOR: DriverDescriptor = DriverDescriptor::new(
    "capability-only",
    "Capability Only",
    DriverTransport::Network,
    false,
    false,
);

impl DriverModule for CapabilityOnlyDriver {
    fn descriptor(&self) -> &'static DriverDescriptor {
        &CAPABILITY_ONLY_DESCRIPTOR
    }

    fn build_output_backend(
        &self,
        host: &dyn DriverHost,
        config: DriverConfigView<'_>,
    ) -> Result<Option<Box<dyn DeviceBackend>>> {
        let _ = (host, config);
        panic!("capability-only drivers should not build output backends");
    }

    fn has_output_backend(&self) -> bool {
        false
    }
}

struct ProtocolCatalogDriver;

static PROTOCOL_CATALOG_DESCRIPTOR: DriverDescriptor = DriverDescriptor::new(
    "protocol-catalog",
    "Protocol Catalog",
    DriverTransport::Usb,
    false,
    false,
);

static PROTOCOL_CATALOG_DESCRIPTORS: LazyLock<Vec<DriverProtocolDescriptor>> =
    LazyLock::new(|| {
        vec![DriverProtocolDescriptor {
            driver_id: "protocol-catalog".to_owned(),
            protocol_id: "protocol-catalog/example".to_owned(),
            display_name: "Protocol Catalog Example".to_owned(),
            vendor_id: Some(0x1234),
            product_id: Some(0x5678),
            family_id: "protocol-catalog".to_owned(),
            model_id: None,
            transport: DriverTransportKind::Usb,
            route_backend_id: "usb".to_owned(),
            presentation: None,
        }]
    });

impl DriverProtocolCatalog for ProtocolCatalogDriver {
    fn descriptors(&self) -> &[DriverProtocolDescriptor] {
        PROTOCOL_CATALOG_DESCRIPTORS.as_slice()
    }
}

impl DriverModule for ProtocolCatalogDriver {
    fn descriptor(&self) -> &'static DriverDescriptor {
        &PROTOCOL_CATALOG_DESCRIPTOR
    }

    fn build_output_backend(
        &self,
        host: &dyn DriverHost,
        config: DriverConfigView<'_>,
    ) -> Result<Option<Box<dyn DeviceBackend>>> {
        let _ = (host, config);
        Ok(None)
    }

    fn has_output_backend(&self) -> bool {
        false
    }

    fn protocol_catalog(&self) -> Option<&dyn DriverProtocolCatalog> {
        Some(self)
    }
}

struct PresentationDriver;

static PRESENTATION_DRIVER_DESCRIPTOR: DriverDescriptor = DriverDescriptor::new(
    "presentation-driver",
    "Presentation Driver",
    DriverTransport::Network,
    false,
    false,
);

impl DriverPresentationProvider for PresentationDriver {
    fn presentation(&self) -> DriverPresentation {
        DriverPresentation {
            label: "Driver-Owned Presentation".to_owned(),
            short_label: Some("DOP".to_owned()),
            accent_rgb: Some([128, 255, 234]),
            secondary_rgb: None,
            icon: Some("controller".to_owned()),
            default_device_class: Some(DeviceClassHint::Controller),
        }
    }
}

impl DriverModule for PresentationDriver {
    fn descriptor(&self) -> &'static DriverDescriptor {
        &PRESENTATION_DRIVER_DESCRIPTOR
    }

    fn build_output_backend(
        &self,
        host: &dyn DriverHost,
        config: DriverConfigView<'_>,
    ) -> Result<Option<Box<dyn DeviceBackend>>> {
        let _ = (host, config);
        Ok(None)
    }

    fn has_output_backend(&self) -> bool {
        false
    }

    fn presentation(&self) -> Option<&dyn DriverPresentationProvider> {
        Some(self)
    }
}

#[test]
fn protocol_descriptors_use_driver_catalog_before_hal_catalog() {
    let mut registry = DriverModuleRegistry::new();
    registry
        .register(ProtocolCatalogDriver)
        .expect("protocol catalog driver should register");

    let protocols = network::protocol_descriptors(&registry, "protocol-catalog");

    assert_eq!(protocols.len(), 1);
    assert_eq!(protocols[0].protocol_id, "protocol-catalog/example");
    assert_eq!(protocols[0].route_backend_id, "usb");
}

#[test]
fn module_presentation_prefers_driver_provider() {
    let mut registry = DriverModuleRegistry::new();
    registry
        .register(PresentationDriver)
        .expect("presentation driver should register");

    let presentation = network::module_presentation(&registry, "presentation-driver")
        .expect("presentation should resolve");

    assert_eq!(presentation.label, "Driver-Owned Presentation");
    assert_eq!(presentation.short_label.as_deref(), Some("DOP"));
    assert_eq!(
        presentation.default_device_class,
        Some(DeviceClassHint::Controller)
    );
}

#[test]
fn register_enabled_driver_output_backends_uses_default_config_for_configless_driver() {
    let host = NullHost::new();
    let mut registry = DriverModuleRegistry::new();
    registry
        .register(ConfiglessDriver)
        .expect("configless driver should register");
    let config = HypercolorConfig::default();
    let mut backend_manager = BackendManager::new();

    network::register_enabled_driver_output_backends(
        &mut backend_manager,
        &registry,
        &host,
        &config,
    )
    .expect("configless driver should register a backend");

    assert_eq!(backend_manager.backend_ids(), vec!["external-backend"]);
}

#[test]
fn register_enabled_driver_output_backends_skips_config_disabled_driver() {
    let host = NullHost::new();
    let mut registry = DriverModuleRegistry::new();
    registry
        .register(ConfiglessDriver)
        .expect("configless driver should register");
    let mut config = HypercolorConfig::default();
    config.drivers.insert(
        "external".to_owned(),
        DriverConfigEntry::disabled(BTreeMap::default()),
    );
    let mut backend_manager = BackendManager::new();

    network::register_enabled_driver_output_backends(
        &mut backend_manager,
        &registry,
        &host,
        &config,
    )
    .expect("disabled driver should be skipped cleanly");

    assert!(backend_manager.backend_ids().is_empty());
}

#[test]
fn register_enabled_driver_output_backends_skips_capability_only_driver() {
    let host = NullHost::new();
    let mut registry = DriverModuleRegistry::new();
    registry
        .register(CapabilityOnlyDriver)
        .expect("capability-only driver should register");
    let config = HypercolorConfig::default();
    let mut backend_manager = BackendManager::new();

    network::register_enabled_driver_output_backends(
        &mut backend_manager,
        &registry,
        &host,
        &config,
    )
    .expect("capability-only driver should be skipped cleanly");

    assert!(backend_manager.backend_ids().is_empty());
}
