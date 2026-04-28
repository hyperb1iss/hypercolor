use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use async_trait::async_trait;
use hypercolor_driver_api::CredentialStore;
use hypercolor_driver_api::{
    BackendRebindActions, DeviceControlStore, DriverConfigProvider, DriverControlHost,
    DriverControlStore, DriverCredentialStore, DriverDiscoveryState, DriverHost,
    DriverLifecycleActions, DriverModule, DriverRuntimeActions, DriverTrackedDevice,
    TrackedDeviceCtx,
};
use hypercolor_driver_nanoleaf::{
    NanoleafConfig, NanoleafDriverModule, nanoleaf_device_control_surface,
    nanoleaf_driver_control_surface, resolve_nanoleaf_probe_devices_from_sources,
};
use hypercolor_types::controls::{
    ApplyImpact, ControlAccess, ControlActionStatus, ControlSurfaceEvent, ControlValue,
    ControlValueMap,
};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceFeatures, DeviceId,
    DeviceInfo, DeviceOrigin, DeviceState, DeviceTopologyHint, ZoneInfo,
};
use serde_json::Value;
use tokio::sync::Mutex;

fn tracked_nanoleaf_device() -> DriverTrackedDevice {
    DriverTrackedDevice {
        info: DeviceInfo {
            id: DeviceId::new(),
            name: "Shapes".to_owned(),
            vendor: "Nanoleaf".to_owned(),
            family: DeviceFamily::new_static("nanoleaf", "Nanoleaf"),
            model: Some("NL42".to_owned()),
            connection_type: ConnectionType::Network,
            origin: DeviceOrigin::native("nanoleaf", "nanoleaf", ConnectionType::Network),
            zones: vec![ZoneInfo {
                name: "Panels".to_owned(),
                led_count: 1,
                topology: DeviceTopologyHint::Point,
                color_format: DeviceColorFormat::Rgb,
            }],
            firmware_version: Some("9.4.0".to_owned()),
            capabilities: DeviceCapabilities {
                led_count: 0,
                supports_direct: true,
                supports_brightness: true,
                has_display: false,
                display_resolution: None,
                max_fps: 30,
                color_space: hypercolor_types::device::DeviceColorSpace::default(),
                features: DeviceFeatures::default(),
            },
        },
        metadata: HashMap::from([
            ("ip".to_owned(), "10.0.0.30".to_owned()),
            ("api_port".to_owned(), "16021".to_owned()),
            ("device_key".to_owned(), "nanoleaf-shapes".to_owned()),
        ]),
        fingerprint: None,
        current_state: DeviceState::Known,
    }
}

#[test]
fn resolve_nanoleaf_probe_devices_merges_tracked_metadata() {
    let config = NanoleafConfig {
        device_ips: vec![IpAddr::V4(Ipv4Addr::new(10, 0, 0, 25))],
        ..NanoleafConfig::default()
    };

    let resolved =
        resolve_nanoleaf_probe_devices_from_sources(&config, &[tracked_nanoleaf_device()]);

    assert_eq!(resolved.len(), 2);
    let tracked = resolved
        .iter()
        .find(|device| device.ip == IpAddr::V4(Ipv4Addr::new(10, 0, 0, 30)))
        .expect("tracked device should be present");
    assert_eq!(tracked.port, 16021);
    assert_eq!(tracked.device_id, "nanoleaf-shapes");
    assert_eq!(tracked.name, "Shapes");
    assert_eq!(tracked.model, "NL42");
}

#[test]
fn nanoleaf_config_validation_rejects_non_routable_device_ips() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let module = NanoleafDriverModule::new(
        Arc::new(
            CredentialStore::open_blocking(tempdir.path()).expect("credential store should open"),
        ),
        false,
    );
    let mut config = module
        .config()
        .expect("Nanoleaf should expose config provider")
        .default_config();
    config
        .settings
        .insert("device_ips".to_owned(), serde_json::json!(["127.0.0.1"]));

    let error = module
        .validate_config(&config)
        .expect_err("loopback device IP should be rejected");
    assert!(error.to_string().contains("invalid Nanoleaf device IP"));
}

#[test]
fn nanoleaf_driver_control_surface_exposes_typed_config_fields() {
    let config = NanoleafConfig {
        device_ips: vec![IpAddr::V4(Ipv4Addr::new(10, 0, 0, 25))],
        transition_time: 4,
    };

    let surface = nanoleaf_driver_control_surface(&config);

    assert_eq!(surface.surface_id, "driver:nanoleaf");
    let ControlValue::List(device_ips) = &surface.values["device_ips"] else {
        panic!("device IPs should be a list");
    };
    assert_eq!(
        device_ips,
        &[ControlValue::IpAddress("10.0.0.25".to_owned())]
    );
    assert_eq!(surface.values["transition_time"], ControlValue::Integer(4));
    assert!(surface.fields.iter().any(
        |field| field.id == "device_ips" && field.apply_impact == ApplyImpact::DiscoveryRescan
    ));
    assert!(
        surface
            .fields
            .iter()
            .any(|field| field.id == "transition_time"
                && field.apply_impact == ApplyImpact::BackendRebind)
    );
}

#[test]
fn nanoleaf_device_control_surface_exposes_tracked_metadata() {
    let tracked = tracked_nanoleaf_device();
    let device = TrackedDeviceCtx {
        device_id: tracked.info.id,
        info: &tracked.info,
        metadata: Some(&tracked.metadata),
        current_state: &tracked.current_state,
    };

    let surface = nanoleaf_device_control_surface(&device);

    assert_eq!(
        surface.surface_id,
        format!("driver:nanoleaf:device:{}", tracked.info.id)
    );
    assert_eq!(
        surface.scope,
        hypercolor_types::controls::ControlSurfaceScope::Device {
            device_id: tracked.info.id,
            driver_id: "nanoleaf".to_owned(),
        }
    );
    assert!(surface.revision > 0);
    assert!(
        surface
            .fields
            .iter()
            .any(|field| { field.id == "ip" && field.access == ControlAccess::ReadOnly })
    );
    assert!(
        surface
            .fields
            .iter()
            .any(|field| { field.id == "api_port" && field.access == ControlAccess::ReadOnly })
    );
    assert_eq!(
        surface.values["ip"],
        ControlValue::IpAddress("10.0.0.30".to_owned())
    );
    assert_eq!(surface.values["api_port"], ControlValue::Integer(16021));
    assert_eq!(
        surface.values["device_key"],
        ControlValue::String("nanoleaf-shapes".to_owned())
    );
    assert_eq!(
        surface.values["model"],
        ControlValue::String("NL42".to_owned())
    );
    assert_eq!(
        surface.values["firmware_version"],
        ControlValue::String("9.4.0".to_owned())
    );
    assert_eq!(surface.values["led_count"], ControlValue::Integer(1));
    assert_eq!(surface.values["max_fps"], ControlValue::Integer(30));
    assert_eq!(
        surface.values["state"],
        ControlValue::String("Known".to_owned())
    );
    let refresh = surface
        .actions
        .iter()
        .find(|action| action.id == "refresh_topology")
        .expect("refresh topology action should be exposed");
    assert_eq!(refresh.apply_impact, ApplyImpact::DeviceReconnect);
    assert!(refresh.input_fields.is_empty());
    assert_eq!(
        surface.action_availability["refresh_topology"].state,
        hypercolor_types::controls::ControlAvailabilityState::Available
    );
}

#[tokio::test]
async fn nanoleaf_refresh_topology_action_schedules_device_reconnect() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let module = NanoleafDriverModule::new(
        Arc::new(
            CredentialStore::open_blocking(tempdir.path()).expect("credential store should open"),
        ),
        false,
    );
    let tracked = tracked_nanoleaf_device();
    let host = TestHost::default();
    let device = TrackedDeviceCtx {
        device_id: tracked.info.id,
        info: &tracked.info,
        metadata: Some(&tracked.metadata),
        current_state: &tracked.current_state,
    };

    let result = module
        .controls()
        .expect("Nanoleaf should expose controls")
        .invoke_action(
            &host,
            &hypercolor_driver_api::ControlApplyTarget::Device { device: &device },
            "refresh_topology",
            ControlValueMap::new(),
        )
        .await
        .expect("refresh topology action should invoke");

    assert_eq!(result.status, ControlActionStatus::Accepted);
    assert_eq!(result.result, Some(ControlValue::Bool(true)));
    assert!(host.control.lifecycle.reconnected.load(Ordering::SeqCst));
}

#[derive(Default)]
struct TestCredentialStore {
    values: Mutex<HashMap<String, Value>>,
}

#[async_trait]
impl DriverCredentialStore for TestCredentialStore {
    async fn get_json(&self, driver_id: &str, key: &str) -> Result<Option<Value>> {
        Ok(self
            .values
            .lock()
            .await
            .get(&format!("{driver_id}:{key}"))
            .cloned())
    }

    async fn set_json(&self, driver_id: &str, key: &str, value: Value) -> Result<()> {
        self.values
            .lock()
            .await
            .insert(format!("{driver_id}:{key}"), value);
        Ok(())
    }

    async fn remove(&self, driver_id: &str, key: &str) -> Result<()> {
        self.values
            .lock()
            .await
            .remove(&format!("{driver_id}:{key}"));
        Ok(())
    }
}

#[derive(Default)]
struct TestRuntimeActions;

#[async_trait]
impl DriverRuntimeActions for TestRuntimeActions {
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

#[derive(Default)]
struct TestDiscoveryState;

#[async_trait]
impl DriverDiscoveryState for TestDiscoveryState {
    async fn tracked_devices(&self, driver_id: &str) -> Vec<DriverTrackedDevice> {
        let _ = driver_id;
        Vec::new()
    }

    fn load_cached_json(&self, driver_id: &str, key: &str) -> Result<Option<serde_json::Value>> {
        let _ = (driver_id, key);
        Ok(None)
    }
}

#[derive(Default)]
struct TestLifecycleActions {
    reconnected: AtomicBool,
}

#[async_trait]
impl DriverLifecycleActions for TestLifecycleActions {
    async fn reconnect_device(&self, device_id: DeviceId, backend_id: &str) -> Result<bool> {
        let _ = (device_id, backend_id);
        self.reconnected.store(true, Ordering::SeqCst);
        Ok(true)
    }

    async fn rescan_driver(&self, driver_id: &str) -> Result<()> {
        let _ = driver_id;
        Ok(())
    }
}

#[derive(Default)]
struct TestControlHost {
    lifecycle: TestLifecycleActions,
}

#[async_trait]
impl DriverControlStore for TestControlHost {
    async fn load_driver_values(&self, driver_id: &str) -> Result<ControlValueMap> {
        let _ = driver_id;
        Ok(ControlValueMap::new())
    }

    async fn save_driver_values(&self, driver_id: &str, values: ControlValueMap) -> Result<()> {
        let _ = (driver_id, values);
        Ok(())
    }
}

#[async_trait]
impl DeviceControlStore for TestControlHost {
    async fn load_device_values(&self, device_id: DeviceId) -> Result<ControlValueMap> {
        let _ = device_id;
        Ok(ControlValueMap::new())
    }

    async fn save_device_values(&self, device_id: DeviceId, values: ControlValueMap) -> Result<()> {
        let _ = (device_id, values);
        Ok(())
    }
}

#[async_trait]
impl BackendRebindActions for TestControlHost {
    async fn rebind_backend(&self, driver_id: &str) -> Result<()> {
        let _ = driver_id;
        Ok(())
    }
}

impl DriverControlHost for TestControlHost {
    fn driver_config_store(&self) -> &dyn DriverControlStore {
        self
    }

    fn device_config_store(&self) -> &dyn DeviceControlStore {
        self
    }

    fn lifecycle(&self) -> &dyn DriverLifecycleActions {
        &self.lifecycle
    }

    fn backend_rebind(&self) -> &dyn BackendRebindActions {
        self
    }

    fn publish_control_event(&self, event: ControlSurfaceEvent) {
        let _ = event;
    }
}

struct TestHost {
    credentials: TestCredentialStore,
    runtime: TestRuntimeActions,
    discovery: TestDiscoveryState,
    control: TestControlHost,
}

impl Default for TestHost {
    fn default() -> Self {
        Self {
            credentials: TestCredentialStore::default(),
            runtime: TestRuntimeActions,
            discovery: TestDiscoveryState,
            control: TestControlHost::default(),
        }
    }
}

impl DriverHost for TestHost {
    fn credentials(&self) -> &dyn DriverCredentialStore {
        &self.credentials
    }

    fn runtime(&self) -> &dyn DriverRuntimeActions {
        &self.runtime
    }

    fn discovery_state(&self) -> &dyn DriverDiscoveryState {
        &self.discovery
    }

    fn control_host(&self) -> Option<&dyn DriverControlHost> {
        Some(&self.control)
    }
}
