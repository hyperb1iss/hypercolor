use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::Result;
use hypercolor_driver_api::{
    BackendRebindActions, ControlApplyTarget, DeviceControlStore, DriverConfigProvider,
    DriverControlHost, DriverControlProvider, DriverControlStore, DriverCredentialStore,
    DriverDiscoveryState, DriverHost, DriverLifecycleActions, DriverModule, DriverRuntimeActions,
    DriverTrackedDevice, TrackedDeviceCtx, ValidatedControlChanges,
};
use hypercolor_driver_wled::{
    WledConfig, WledDriverModule, WledKnownTarget, WledProtocolConfig,
    resolve_wled_probe_ips_from_sources, resolve_wled_probe_targets_from_sources,
    wled_device_control_surface, wled_driver_control_surface,
};
use hypercolor_types::controls::{
    ApplyImpact, ControlAccess, ControlChange, ControlSurfaceEvent, ControlValue, ControlValueMap,
};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceClassHint, DeviceColorFormat, DeviceFamily,
    DeviceFeatures, DeviceFingerprint, DeviceId, DeviceInfo, DeviceOrigin, DeviceState,
    DeviceTopologyHint, ZoneInfo,
};

fn tracked_wled_device(ip: &str, hostname: &str, name: &str) -> DriverTrackedDevice {
    DriverTrackedDevice {
        info: DeviceInfo {
            id: DeviceId::new(),
            name: name.to_owned(),
            vendor: "WLED".to_owned(),
            family: DeviceFamily::new_static("wled", "WLED"),
            model: None,
            connection_type: ConnectionType::Network,
            origin: DeviceOrigin::native("wled", "wled", ConnectionType::Network),
            zones: vec![ZoneInfo {
                name: "Main".to_owned(),
                led_count: 60,
                topology: DeviceTopologyHint::Strip,
                color_format: DeviceColorFormat::Rgbw,
                layout_hint: None,
            }],
            firmware_version: Some("0.15.0".to_owned()),
            capabilities: DeviceCapabilities {
                led_count: 60,
                supports_direct: true,
                supports_brightness: true,
                has_display: false,
                display_resolution: None,
                max_fps: 55,
                color_space: hypercolor_types::device::DeviceColorSpace::default(),
                features: DeviceFeatures::default(),
            },
        },
        metadata: HashMap::from([
            ("ip".to_owned(), ip.to_owned()),
            ("hostname".to_owned(), hostname.to_owned()),
        ]),
        fingerprint: Some(DeviceFingerprint(format!("net:{hostname}"))),
        current_state: DeviceState::Known,
    }
}

#[test]
fn resolve_probe_ips_merges_all_sources() {
    let config = WledConfig {
        known_ips: vec![IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2))],
        ..WledConfig::default()
    };
    let tracked = vec![tracked_wled_device("10.0.0.5", "desk.local", "Desk Strip")];
    let cached_probe_ips = vec![IpAddr::V4(Ipv4Addr::new(10, 0, 0, 3))];
    let cached_targets = vec![WledKnownTarget::from_ip(IpAddr::V4(Ipv4Addr::new(
        10, 0, 0, 4,
    )))];

    let resolved =
        resolve_wled_probe_ips_from_sources(&config, &tracked, &cached_probe_ips, &cached_targets);

    assert_eq!(
        resolved,
        vec![
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)),
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 3)),
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 4)),
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5)),
        ]
    );
}

#[test]
fn resolve_probe_targets_prefers_tracked_metadata() {
    let tracked = vec![tracked_wled_device("10.0.0.5", "desk.local", "Desk Strip")];
    let resolved = resolve_wled_probe_targets_from_sources(
        &WledConfig::default(),
        &tracked,
        &[],
        &[WledKnownTarget {
            ip: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5)),
            hostname: None,
            fingerprint: None,
            name: None,
            led_count: None,
            firmware_version: None,
            max_fps: None,
            rgbw: None,
        }],
    );

    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].name.as_deref(), Some("Desk Strip"));
    assert_eq!(resolved[0].hostname.as_deref(), Some("desk.local"));
    assert_eq!(resolved[0].led_count, Some(60));
    assert_eq!(resolved[0].rgbw, Some(true));
}

#[test]
fn wled_module_advertises_control_surface_capability() {
    let descriptor = WledDriverModule::new(false).module_descriptor();

    assert!(descriptor.capabilities.controls);
    assert!(descriptor.capabilities.discovery);
    assert!(descriptor.capabilities.output_backend);
    assert!(descriptor.capabilities.presentation);
    assert!(descriptor.capabilities.runtime_cache);

    let presentation = WledDriverModule::new(false)
        .presentation()
        .expect("WLED should expose presentation metadata")
        .presentation();
    assert_eq!(presentation.label, "WLED");
    assert_eq!(presentation.accent_rgb, Some([255, 106, 193]));
    assert_eq!(
        presentation.default_device_class,
        Some(DeviceClassHint::Controller)
    );
}

#[test]
fn wled_config_validation_rejects_non_routable_known_ips() {
    let module = WledDriverModule::new(false);
    let mut config = module
        .config()
        .expect("WLED should expose config provider")
        .default_config();
    config
        .settings
        .insert("known_ips".to_owned(), serde_json::json!(["127.0.0.1"]));

    let error = module
        .validate_config(&config)
        .expect_err("loopback known IP should be rejected");
    assert!(error.to_string().contains("invalid WLED known IP"));
}

#[test]
fn wled_driver_control_surface_exposes_typed_config_fields() {
    let config = WledConfig {
        known_ips: vec![IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2))],
        default_protocol: WledProtocolConfig::E131,
        realtime_http_enabled: false,
        dedup_threshold: 7,
    };

    let surface = wled_driver_control_surface(&config);

    assert_eq!(surface.surface_id, "driver:wled");
    assert!(surface.revision > 0);
    assert_eq!(surface.fields.len(), 4);
    assert!(surface.fields.iter().any(|field| {
        field.id == "known_ips" && field.apply_impact == ApplyImpact::DiscoveryRescan
    }));
    assert!(surface.fields.iter().any(|field| {
        field.id == "default_protocol" && field.apply_impact == ApplyImpact::BackendRebind
    }));
    assert_eq!(
        surface.values["known_ips"],
        ControlValue::List(vec![ControlValue::IpAddress("10.0.0.2".to_owned())])
    );
    assert_eq!(
        surface.values["default_protocol"],
        ControlValue::Enum("e131".to_owned())
    );
    assert_eq!(
        surface.values["realtime_http_enabled"],
        ControlValue::Bool(false)
    );
    assert_eq!(surface.values["dedup_threshold"], ControlValue::Integer(7));

    let changed = wled_driver_control_surface(&WledConfig {
        dedup_threshold: 8,
        ..config
    });
    assert_ne!(surface.revision, changed.revision);
}

#[test]
fn wled_device_control_surface_exposes_tracked_metadata() {
    let tracked = tracked_wled_device("10.0.0.5", "desk.local", "Desk Strip");
    let device = TrackedDeviceCtx {
        device_id: tracked.info.id,
        info: &tracked.info,
        metadata: Some(&tracked.metadata),
        current_state: &tracked.current_state,
    };

    let driver_values = ControlValueMap::from([
        (
            "default_protocol".to_owned(),
            ControlValue::String("e131".to_owned()),
        ),
        ("dedup_threshold".to_owned(), ControlValue::Integer(9)),
    ]);
    let device_values = ControlValueMap::from([
        (
            "protocol".to_owned(),
            ControlValue::String("ddp".to_owned()),
        ),
        ("dedup_threshold".to_owned(), ControlValue::Integer(3)),
    ]);

    let surface = wled_device_control_surface(&device, &driver_values, &device_values);

    assert_eq!(
        surface.surface_id,
        format!("driver:wled:device:{}", tracked.info.id)
    );
    assert_eq!(
        surface.scope,
        hypercolor_types::controls::ControlSurfaceScope::Device {
            device_id: tracked.info.id,
            driver_id: "wled".to_owned(),
        }
    );
    assert!(surface.revision > 0);
    assert!(
        surface
            .fields
            .iter()
            .any(|field| { field.id == "protocol" && field.access == ControlAccess::ReadWrite })
    );
    assert!(
        !surface
            .fields
            .iter()
            .any(|field| { field.id == "dedup_threshold" })
    );
    assert!(
        surface
            .fields
            .iter()
            .any(|field| { field.id == "ip" && field.access == ControlAccess::ReadOnly })
    );
    assert_eq!(
        surface.values["protocol"],
        ControlValue::Enum("ddp".to_owned())
    );
    assert!(!surface.values.contains_key("dedup_threshold"));
    assert_eq!(
        surface.values["ip"],
        ControlValue::IpAddress("10.0.0.5".to_owned())
    );
    assert_eq!(
        surface.values["hostname"],
        ControlValue::String("desk.local".to_owned())
    );
    assert_eq!(
        surface.values["firmware_version"],
        ControlValue::String("0.15.0".to_owned())
    );
    assert_eq!(surface.values["led_count"], ControlValue::Integer(60));
    assert_eq!(surface.values["max_fps"], ControlValue::Integer(55));
    assert_eq!(surface.values["rgbw"], ControlValue::Bool(true));
}

#[tokio::test]
async fn wled_device_apply_persists_values_without_running_host_impacts() {
    let tracked = tracked_wled_device("10.0.0.5", "desk.local", "Desk Strip");
    let host = TestControlHost::default();
    let driver = WledDriverModule::new(false);
    let device = TrackedDeviceCtx {
        device_id: tracked.info.id,
        info: &tracked.info,
        metadata: Some(&tracked.metadata),
        current_state: &tracked.current_state,
    };
    let target = ControlApplyTarget::Device { device: &device };

    let response = DriverControlProvider::apply_changes(
        &driver,
        &host,
        &target,
        ValidatedControlChanges {
            changes: vec![ControlChange {
                field_id: "protocol".to_owned(),
                value: ControlValue::Enum("e131".to_owned()),
            }],
            impacts: vec![ApplyImpact::DeviceReconnect],
        },
    )
    .await
    .expect("WLED device control apply should persist values");

    assert_eq!(
        response.surface_id,
        format!("driver:wled:device:{}", tracked.info.id)
    );
    assert_eq!(
        response.values["protocol"],
        ControlValue::Enum("e131".to_owned())
    );
    assert_eq!(
        host.saved_device_values(tracked.info.id)["protocol"],
        ControlValue::Enum("e131".to_owned())
    );
    assert_eq!(host.reconnects.load(Ordering::Relaxed), 0);
}

#[derive(Default)]
struct TestControlHost {
    driver_values: Mutex<HashMap<String, ControlValueMap>>,
    device_values: Mutex<HashMap<DeviceId, ControlValueMap>>,
    reconnects: AtomicUsize,
}

impl TestControlHost {
    fn saved_device_values(&self, device_id: DeviceId) -> ControlValueMap {
        self.device_values
            .lock()
            .expect("test device values mutex should not be poisoned")
            .get(&device_id)
            .cloned()
            .expect("device values should be saved")
    }
}

#[async_trait::async_trait]
impl DriverCredentialStore for TestControlHost {
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

#[async_trait::async_trait]
impl DriverRuntimeActions for TestControlHost {
    async fn activate_device(&self, _device_id: DeviceId, _backend_id: &str) -> Result<bool> {
        Ok(true)
    }

    async fn disconnect_device(
        &self,
        _device_id: DeviceId,
        _backend_id: &str,
        _will_retry: bool,
    ) -> Result<bool> {
        Ok(true)
    }
}

#[async_trait::async_trait]
impl DriverDiscoveryState for TestControlHost {
    async fn tracked_devices(&self, _driver_id: &str) -> Vec<DriverTrackedDevice> {
        Vec::new()
    }

    fn load_cached_json(&self, _driver_id: &str, _key: &str) -> Result<Option<serde_json::Value>> {
        Ok(None)
    }
}

impl DriverHost for TestControlHost {
    fn credentials(&self) -> &dyn DriverCredentialStore {
        self
    }

    fn runtime(&self) -> &dyn DriverRuntimeActions {
        self
    }

    fn discovery_state(&self) -> &dyn DriverDiscoveryState {
        self
    }

    fn control_host(&self) -> Option<&dyn DriverControlHost> {
        Some(self)
    }
}

#[async_trait::async_trait]
impl DriverControlStore for TestControlHost {
    async fn load_driver_values(&self, driver_id: &str) -> Result<ControlValueMap> {
        Ok(self
            .driver_values
            .lock()
            .expect("test driver values mutex should not be poisoned")
            .get(driver_id)
            .cloned()
            .unwrap_or_default())
    }

    async fn save_driver_values(&self, driver_id: &str, values: ControlValueMap) -> Result<()> {
        self.driver_values
            .lock()
            .expect("test driver values mutex should not be poisoned")
            .insert(driver_id.to_owned(), values);
        Ok(())
    }
}

#[async_trait::async_trait]
impl DeviceControlStore for TestControlHost {
    async fn load_device_values(&self, device_id: DeviceId) -> Result<ControlValueMap> {
        Ok(self
            .device_values
            .lock()
            .expect("test device values mutex should not be poisoned")
            .get(&device_id)
            .cloned()
            .unwrap_or_default())
    }

    async fn save_device_values(&self, device_id: DeviceId, values: ControlValueMap) -> Result<()> {
        self.device_values
            .lock()
            .expect("test device values mutex should not be poisoned")
            .insert(device_id, values);
        Ok(())
    }
}

#[async_trait::async_trait]
impl DriverLifecycleActions for TestControlHost {
    async fn reconnect_device(&self, _device_id: DeviceId, _backend_id: &str) -> Result<bool> {
        self.reconnects.fetch_add(1, Ordering::Relaxed);
        Ok(true)
    }

    async fn rescan_driver(&self, _driver_id: &str) -> Result<()> {
        Ok(())
    }
}

#[async_trait::async_trait]
impl BackendRebindActions for TestControlHost {
    async fn rebind_backend(&self, _driver_id: &str) -> Result<()> {
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
        self
    }

    fn backend_rebind(&self) -> &dyn BackendRebindActions {
        self
    }

    fn publish_control_event(&self, _event: ControlSurfaceEvent) {}
}
