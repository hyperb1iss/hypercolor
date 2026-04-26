use anyhow::Result;
use async_trait::async_trait;
use hypercolor_core::device::{BackendInfo, DeviceBackend};
use hypercolor_driver_api::{
    ClearPairingOutcome, ControlApplyTarget, DRIVER_API_SCHEMA_VERSION, DeviceAuthSummary,
    DiscoveryCapability, DiscoveryRequest, DiscoveryResult, DriverConfigView,
    DriverControlProvider, DriverCredentialStore, DriverDescriptor, DriverDiscoveryState,
    DriverHost, DriverRuntimeActions, DriverTransport, NetworkDriverFactory, PairDeviceOutcome,
    PairDeviceRequest, PairingCapability, TrackedDeviceCtx, ValidatedControlChanges,
};
use hypercolor_network::{DriverRegistry, DriverRegistryError};
use hypercolor_types::config::DriverConfigEntry;
use hypercolor_types::controls::{
    ApplyControlChangesResponse, ControlActionResult, ControlActionStatus, ControlChange,
    ControlSurfaceDocument, ControlSurfaceScope, ControlValueMap,
};
use hypercolor_types::device::{DeviceId, DeviceInfo, DriverModuleKind, DriverTransportKind};

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

struct NullHost {
    credentials: NullCredentialStore,
    runtime: NullRuntimeActions,
}

struct NullDiscoveryState;

#[async_trait]
impl DriverDiscoveryState for NullDiscoveryState {
    async fn tracked_devices(
        &self,
        backend_id: &str,
    ) -> Vec<hypercolor_driver_api::DriverTrackedDevice> {
        let _ = backend_id;
        Vec::new()
    }

    fn load_cached_json(&self, driver_id: &str, key: &str) -> Result<Option<serde_json::Value>> {
        let _ = (driver_id, key);
        Ok(None)
    }
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

struct TestBackend;

#[async_trait]
impl DeviceBackend for TestBackend {
    fn info(&self) -> BackendInfo {
        BackendInfo {
            id: "test".to_owned(),
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

struct DiscoveryOnlyCapability;

#[async_trait]
impl DiscoveryCapability for DiscoveryOnlyCapability {
    async fn discover(
        &self,
        host: &dyn DriverHost,
        request: &DiscoveryRequest,
        config: DriverConfigView<'_>,
    ) -> Result<DiscoveryResult> {
        let _ = (host, request, config);
        Ok(DiscoveryResult::default())
    }
}

struct PairingOnlyCapability;

#[async_trait]
impl PairingCapability for PairingOnlyCapability {
    async fn auth_summary(
        &self,
        host: &dyn DriverHost,
        device: &TrackedDeviceCtx<'_>,
    ) -> Option<DeviceAuthSummary> {
        let _ = (host, device);
        None
    }

    async fn pair(
        &self,
        host: &dyn DriverHost,
        device: &TrackedDeviceCtx<'_>,
        request: &PairDeviceRequest,
    ) -> Result<PairDeviceOutcome> {
        let _ = (host, device, request);
        unreachable!("pair is not exercised in registry tests")
    }

    async fn clear_credentials(
        &self,
        host: &dyn DriverHost,
        device: &TrackedDeviceCtx<'_>,
    ) -> Result<ClearPairingOutcome> {
        let _ = (host, device);
        unreachable!("clear_credentials is not exercised in registry tests")
    }
}

struct ControlOnlyCapability;

#[async_trait]
impl DriverControlProvider for ControlOnlyCapability {
    async fn driver_surface(
        &self,
        host: &dyn DriverHost,
        config: DriverConfigView<'_>,
    ) -> Result<Option<ControlSurfaceDocument>> {
        let _ = (host, config);
        Ok(Some(ControlSurfaceDocument::empty(
            "driver:control-only",
            ControlSurfaceScope::Driver {
                driver_id: "control-only".to_owned(),
            },
        )))
    }

    async fn device_surface(
        &self,
        host: &dyn DriverHost,
        device: &TrackedDeviceCtx<'_>,
    ) -> Result<Option<ControlSurfaceDocument>> {
        let _ = (host, device);
        Ok(None)
    }

    async fn validate_changes(
        &self,
        host: &dyn DriverHost,
        target: &ControlApplyTarget<'_>,
        changes: &[ControlChange],
    ) -> Result<ValidatedControlChanges> {
        let _ = (host, target);
        Ok(ValidatedControlChanges::new(changes.to_vec()))
    }

    async fn apply_changes(
        &self,
        host: &dyn DriverHost,
        target: &ControlApplyTarget<'_>,
        changes: ValidatedControlChanges,
    ) -> Result<ApplyControlChangesResponse> {
        let _ = (host, target, changes);
        unreachable!("apply is not exercised in registry tests")
    }

    async fn invoke_action(
        &self,
        host: &dyn DriverHost,
        target: &ControlApplyTarget<'_>,
        action_id: &str,
        input: ControlValueMap,
    ) -> Result<ControlActionResult> {
        let _ = (host, target, input);
        Ok(ControlActionResult {
            surface_id: "driver:control-only".to_owned(),
            action_id: action_id.to_owned(),
            status: ControlActionStatus::Completed,
            result: None,
            revision: 0,
        })
    }
}

struct DiscoveryOnlyDriver;

static DISCOVERY_ONLY_DESCRIPTOR: DriverDescriptor = DriverDescriptor::new(
    "discovery-only",
    "Discovery Only",
    DriverTransport::Network,
    true,
    false,
);

impl NetworkDriverFactory for DiscoveryOnlyDriver {
    fn descriptor(&self) -> &'static DriverDescriptor {
        &DISCOVERY_ONLY_DESCRIPTOR
    }

    fn build_backend(
        &self,
        host: &dyn DriverHost,
        config: DriverConfigView<'_>,
    ) -> Result<Option<Box<dyn DeviceBackend>>> {
        let _ = (host, config);
        Ok(Some(Box::new(TestBackend)))
    }

    fn discovery(&self) -> Option<&dyn DiscoveryCapability> {
        Some(&DiscoveryOnlyCapability)
    }
}

struct PairingOnlyDriver;

static PAIRING_ONLY_DESCRIPTOR: DriverDescriptor = DriverDescriptor::new(
    "pairing-only",
    "Pairing Only",
    DriverTransport::Network,
    false,
    true,
);

impl NetworkDriverFactory for PairingOnlyDriver {
    fn descriptor(&self) -> &'static DriverDescriptor {
        &PAIRING_ONLY_DESCRIPTOR
    }

    fn build_backend(
        &self,
        host: &dyn DriverHost,
        config: DriverConfigView<'_>,
    ) -> Result<Option<Box<dyn DeviceBackend>>> {
        let _ = (host, config);
        Ok(None)
    }

    fn pairing(&self) -> Option<&dyn PairingCapability> {
        Some(&PairingOnlyCapability)
    }
}

struct ControlOnlyDriver;

static CONTROL_ONLY_DESCRIPTOR: DriverDescriptor = DriverDescriptor::new(
    "control-only",
    "Control Only",
    DriverTransport::Network,
    false,
    false,
);

impl NetworkDriverFactory for ControlOnlyDriver {
    fn descriptor(&self) -> &'static DriverDescriptor {
        &CONTROL_ONLY_DESCRIPTOR
    }

    fn build_backend(
        &self,
        host: &dyn DriverHost,
        config: DriverConfigView<'_>,
    ) -> Result<Option<Box<dyn DeviceBackend>>> {
        let _ = (host, config);
        Ok(None)
    }

    fn controls(&self) -> Option<&dyn DriverControlProvider> {
        Some(&ControlOnlyCapability)
    }
}

#[test]
fn registry_rejects_duplicate_ids() {
    let mut registry = DriverRegistry::new();
    registry
        .register(DiscoveryOnlyDriver)
        .expect("first registration should succeed");
    let error = registry
        .register(DiscoveryOnlyDriver)
        .expect_err("duplicate id should fail");

    assert_eq!(
        error,
        DriverRegistryError::DuplicateDriverId {
            id: "discovery-only".to_owned()
        }
    );
}

#[test]
fn registry_lists_ids_in_deterministic_order() {
    let mut registry = DriverRegistry::new();
    registry
        .register(PairingOnlyDriver)
        .expect("pairing driver should register");
    registry
        .register(DiscoveryOnlyDriver)
        .expect("discovery driver should register");

    assert_eq!(
        registry.ids(),
        vec!["discovery-only".to_owned(), "pairing-only".to_owned()]
    );
}

#[test]
fn registry_lists_module_descriptors_in_deterministic_order() {
    let mut registry = DriverRegistry::new();
    registry
        .register(PairingOnlyDriver)
        .expect("pairing driver should register");
    registry
        .register(DiscoveryOnlyDriver)
        .expect("discovery driver should register");

    let descriptors = registry.module_descriptors();

    assert_eq!(descriptors[0].id, "discovery-only");
    assert_eq!(descriptors[0].module_kind, DriverModuleKind::Network);
    assert_eq!(
        descriptors[0].transports,
        vec![DriverTransportKind::Network]
    );
    assert!(descriptors[0].capabilities.discovery);
    assert!(descriptors[0].capabilities.backend_factory);
    assert_eq!(descriptors[1].id, "pairing-only");
    assert!(descriptors[1].capabilities.pairing);
    assert!(descriptors[1].capabilities.credentials);
}

#[test]
fn registry_filters_discovery_and_pairing_drivers() {
    let mut registry = DriverRegistry::new();
    registry
        .register(PairingOnlyDriver)
        .expect("pairing driver should register");
    registry
        .register(DiscoveryOnlyDriver)
        .expect("discovery driver should register");

    let discovery_ids = registry
        .discovery_drivers()
        .into_iter()
        .map(|driver| driver.descriptor().id.to_owned())
        .collect::<Vec<_>>();
    let pairing_ids = registry
        .pairing_drivers()
        .into_iter()
        .map(|driver| driver.descriptor().id.to_owned())
        .collect::<Vec<_>>();

    assert_eq!(discovery_ids, vec!["discovery-only".to_owned()]);
    assert_eq!(pairing_ids, vec!["pairing-only".to_owned()]);
}

#[test]
fn registry_filters_control_surface_drivers() {
    let mut registry = DriverRegistry::new();
    registry
        .register(PairingOnlyDriver)
        .expect("pairing driver should register");
    registry
        .register(ControlOnlyDriver)
        .expect("control driver should register");
    registry
        .register(DiscoveryOnlyDriver)
        .expect("discovery driver should register");

    let control_ids = registry
        .control_drivers()
        .into_iter()
        .map(|driver| driver.descriptor().id.to_owned())
        .collect::<Vec<_>>();
    let descriptors = registry.module_descriptors();
    let control_descriptor = descriptors
        .iter()
        .find(|descriptor| descriptor.id == "control-only")
        .expect("control descriptor should be present");

    assert_eq!(control_ids, vec!["control-only".to_owned()]);
    assert!(control_descriptor.capabilities.controls);
    assert!(!control_descriptor.capabilities.discovery);
    assert!(!control_descriptor.capabilities.pairing);
}

#[test]
fn registry_can_return_registered_driver() {
    let mut registry = DriverRegistry::new();
    registry
        .register(DiscoveryOnlyDriver)
        .expect("driver should register");

    let driver = registry
        .get("discovery-only")
        .expect("driver should be returned");
    assert_eq!(driver.descriptor().display_name, "Discovery Only");
}

struct MismatchedSchemaDriver;

static MISMATCHED_DESCRIPTOR: DriverDescriptor = DriverDescriptor::with_schema_version(
    "mismatch",
    "Schema Mismatch",
    DriverTransport::Network,
    true,
    false,
    u32::MAX,
);

impl NetworkDriverFactory for MismatchedSchemaDriver {
    fn descriptor(&self) -> &'static DriverDescriptor {
        &MISMATCHED_DESCRIPTOR
    }

    fn build_backend(
        &self,
        host: &dyn DriverHost,
        config: DriverConfigView<'_>,
    ) -> Result<Option<Box<dyn DeviceBackend>>> {
        let _ = (host, config);
        Ok(None)
    }
}

#[test]
fn registry_rejects_schema_version_mismatch() {
    let mut registry = DriverRegistry::new();
    let error = registry
        .register(MismatchedSchemaDriver)
        .expect_err("mismatched schema should fail");

    assert_eq!(
        error,
        DriverRegistryError::SchemaVersionMismatch {
            id: "mismatch".to_owned(),
            expected: DRIVER_API_SCHEMA_VERSION,
            found: u32::MAX,
        }
    );
}

#[test]
fn drivers_can_build_backends_through_registry_lookup() {
    let host = NullHost::new();
    let mut registry = DriverRegistry::new();
    registry
        .register(DiscoveryOnlyDriver)
        .expect("driver should register");

    let driver = registry
        .get("discovery-only")
        .expect("driver should be returned");
    let config = DriverConfigEntry::default();
    let backend = driver
        .build_backend(
            &host,
            DriverConfigView {
                driver_id: "discovery-only",
                entry: &config,
            },
        )
        .expect("backend build should succeed")
        .expect("driver should return a backend");

    assert_eq!(backend.info().id, "test");
}
