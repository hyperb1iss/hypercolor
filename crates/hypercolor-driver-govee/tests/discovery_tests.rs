use std::collections::{BTreeMap, HashMap};
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::Result;
use hypercolor_driver_api::{
    BackendRebindActions, ControlApplyTarget, DeviceControlStore, DriverConfigView,
    DriverControlHost, DriverControlProvider, DriverControlStore, DriverCredentialStore,
    DriverDiscoveryState, DriverHost, DriverLifecycleActions, DriverRuntimeActions,
    DriverTrackedDevice, TrackedDeviceCtx, ValidatedControlChanges,
};
use hypercolor_driver_govee::cloud::V1Device;
use hypercolor_driver_govee::{
    GoveeDriverModule, GoveeKnownDevice, build_cloud_discovered_device, build_device_info,
    govee_device_control_surface, govee_driver_control_surface, merge_cloud_inventory,
    parse_scan_response, resolve_govee_probe_devices, resolve_govee_probe_devices_from_sources,
};
use hypercolor_types::config::{DriverConfigEntry, GoveeConfig};
use hypercolor_types::controls::{
    ApplyImpact, ControlAccess, ControlChange, ControlPersistence, ControlSurfaceEvent,
    ControlSurfaceScope, ControlValue, ControlValueMap,
};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceFeatures,
    DeviceFingerprint, DeviceId, DeviceInfo, DeviceOrigin, DeviceState, DeviceTopologyHint,
    ZoneInfo,
};

#[test]
fn parses_lan_scan_response_into_stable_govee_identity() {
    let response = br#"{
        "msg": {
            "cmd": "scan",
            "data": {
                "ip": "192.168.1.44",
                "device": "AA:BB:CC:DD:EE:FF",
                "sku": "H6163",
                "wifiVersionSoft": "1.02.03"
            }
        }
    }"#;

    let device = parse_scan_response(response, IpAddr::V4(Ipv4Addr::new(192, 168, 1, 99)))
        .expect("scan response should parse");

    assert_eq!(device.ip, IpAddr::V4(Ipv4Addr::new(192, 168, 1, 44)));
    assert_eq!(device.sku, "H6163");
    assert_eq!(device.mac, "aabbccddeeff");
    assert_eq!(device.firmware_version.as_deref(), Some("1.02.03"));
}

#[test]
fn build_device_info_uses_validated_razer_count_only() {
    let device = parse_scan_response(
        br#"{"msg":{"data":{"ip":"10.0.0.8","device":"001122334455","sku":"H619A"}}}"#,
        IpAddr::V4(Ipv4Addr::new(10, 0, 0, 8)),
    )
    .expect("scan response should parse");

    let info = build_device_info(&device);

    assert_eq!(info.family, DeviceFamily::new_static("govee", "Govee"));
    assert_eq!(info.model.as_deref(), Some("H619A"));
    assert_eq!(info.total_led_count(), 20);
    assert_eq!(info.capabilities.max_fps, 25);
}

#[test]
fn resolve_probe_devices_merges_config_and_tracked_metadata() {
    let config = GoveeConfig {
        known_ips: vec![IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2))],
        ..GoveeConfig::default()
    };
    let tracked = vec![tracked_govee_device("10.0.0.5", "H6163", "aabbccddeeff")];

    let resolved = resolve_govee_probe_devices_from_sources(&config, &tracked);

    assert_eq!(resolved.len(), 2);
    assert_eq!(resolved[0].ip, IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)));
    assert_eq!(resolved[1].ip, IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5)));
    assert_eq!(resolved[1].sku.as_deref(), Some("H6163"));
    assert_eq!(resolved[1].mac.as_deref(), Some("aabbccddeeff"));
}

#[test]
fn resolve_probe_devices_merges_cached_runtime_hints() {
    let config = GoveeConfig {
        known_ips: vec![IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2))],
        ..GoveeConfig::default()
    };
    let cached = vec![GoveeKnownDevice {
        ip: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)),
        sku: Some("H619A".to_owned()),
        mac: Some("001122334455".to_owned()),
    }];

    let resolved = resolve_govee_probe_devices(&config, &[], &cached);

    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].ip, IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)));
    assert_eq!(resolved[0].sku.as_deref(), Some("H619A"));
    assert_eq!(resolved[0].mac.as_deref(), Some("001122334455"));
}

#[test]
fn cloud_inventory_device_uses_mac_fingerprint_when_device_id_is_mac() {
    let discovered = build_cloud_discovered_device(V1Device {
        device: "AA:BB:CC:DD:EE:FF".to_owned(),
        model: "H6163".to_owned(),
        device_name: "Desk Strip".to_owned(),
        controllable: true,
        retrievable: true,
        support_cmds: vec!["turn".to_owned(), "brightness".to_owned()],
        properties: None,
    });

    assert_eq!(discovered.fingerprint.0, "net:govee:aabbccddeeff");
    assert_eq!(discovered.info.name, "Desk Strip");
    assert_eq!(
        discovered.metadata.get("mac"),
        Some(&"aabbccddeeff".to_owned())
    );
    assert_eq!(
        discovered.metadata.get("cloud_device_id"),
        Some(&"AA:BB:CC:DD:EE:FF".to_owned())
    );
    assert!(!discovered.connect_behavior.should_auto_connect());
}

#[test]
fn cloud_inventory_merges_with_lan_device_without_overriding_lan_metadata() {
    let lan_device = parse_scan_response(
        br#"{"msg":{"data":{"ip":"10.0.0.8","device":"001122334455","sku":"H619A"}}}"#,
        IpAddr::V4(Ipv4Addr::new(10, 0, 0, 8)),
    )
    .expect("scan response should parse");
    let mut devices = vec![hypercolor_driver_api::DriverDiscoveredDevice::from(
        hypercolor_driver_api::DiscoveredDevice {
            fingerprint: DeviceFingerprint("net:govee:001122334455".to_owned()),
            connect_behavior: hypercolor_driver_api::DiscoveryConnectBehavior::AutoConnect,
            info: build_device_info(&lan_device),
            metadata: HashMap::from([
                ("ip".to_owned(), "10.0.0.8".to_owned()),
                ("sku".to_owned(), "H619A".to_owned()),
                ("mac".to_owned(), "001122334455".to_owned()),
            ]),
        },
    )];

    merge_cloud_inventory(
        &mut devices,
        vec![V1Device {
            device: "00:11:22:33:44:55".to_owned(),
            model: "H619A".to_owned(),
            device_name: "Cloud Name".to_owned(),
            controllable: true,
            retrievable: true,
            support_cmds: vec!["color".to_owned()],
            properties: None,
        }],
    );

    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].metadata.get("ip"), Some(&"10.0.0.8".to_owned()));
    assert_eq!(devices[0].info.name, "RGBIC Pro Strip H619A");
    assert_eq!(
        devices[0].metadata.get("cloud_device_id"),
        Some(&"00:11:22:33:44:55".to_owned())
    );
    assert_eq!(
        devices[0].metadata.get("cloud_support_cmds"),
        Some(&"color".to_owned())
    );
    assert!(devices[0].connect_behavior.should_auto_connect());
}

#[test]
fn govee_driver_control_surface_exposes_config_fields() {
    let config = GoveeConfig {
        known_ips: vec![IpAddr::V4(Ipv4Addr::new(10, 0, 0, 9))],
        power_off_on_disconnect: true,
        lan_state_fps: 7,
        razer_fps: 25,
    };

    let surface = govee_driver_control_surface(&config);

    assert_eq!(surface.surface_id, "driver:govee");
    assert!(surface.revision > 0);
    assert_eq!(
        surface.scope,
        ControlSurfaceScope::Driver {
            driver_id: "govee".to_owned(),
        }
    );
    assert!(surface.fields.iter().any(|field| {
        field.id == "known_ips"
            && field.access == ControlAccess::ReadWrite
            && field.persistence == ControlPersistence::DriverConfig
    }));
    assert!(
        surface
            .fields
            .iter()
            .any(|field| field.id == "razer_fps" && field.access == ControlAccess::ReadWrite)
    );
    assert_eq!(
        surface.values["known_ips"],
        ControlValue::List(vec![ControlValue::IpAddress("10.0.0.9".to_owned())])
    );
    assert_eq!(
        surface.values["power_off_on_disconnect"],
        ControlValue::Bool(true)
    );
    assert_eq!(surface.values["lan_state_fps"], ControlValue::Integer(7));
    assert_eq!(surface.values["razer_fps"], ControlValue::Integer(25));

    let changed = govee_driver_control_surface(&GoveeConfig {
        razer_fps: 26,
        ..config
    });
    assert_ne!(surface.revision, changed.revision);
}

#[tokio::test]
async fn govee_apply_persists_values_without_running_host_impacts() {
    let host = TestControlHost::default();
    let driver = GoveeDriverModule::new(GoveeConfig::default());
    let entry = DriverConfigEntry::enabled(BTreeMap::new());
    let config = DriverConfigView {
        driver_id: "govee",
        entry: &entry,
    };
    let target = ControlApplyTarget::Driver {
        driver_id: "govee",
        config,
    };

    let response = DriverControlProvider::apply_changes(
        &driver,
        &host,
        &target,
        ValidatedControlChanges {
            changes: vec![ControlChange {
                field_id: "lan_state_fps".to_owned(),
                value: ControlValue::Integer(8),
            }],
            impacts: vec![ApplyImpact::BackendRebind, ApplyImpact::DiscoveryRescan],
        },
    )
    .await
    .expect("govee control apply should persist values");

    assert_eq!(response.surface_id, "driver:govee");
    assert_eq!(response.values["lan_state_fps"], ControlValue::Integer(8));
    assert_eq!(
        host.saved_driver_values("govee")["lan_state_fps"],
        ControlValue::Integer(8)
    );
    assert_eq!(host.rebinds.load(Ordering::Relaxed), 0);
    assert_eq!(host.rescans.load(Ordering::Relaxed), 0);
}

#[test]
fn govee_device_control_surface_exposes_lan_metadata() {
    let tracked = tracked_govee_device("10.0.0.5", "H619A", "001122334455");
    let device = TrackedDeviceCtx {
        device_id: tracked.info.id,
        info: &tracked.info,
        metadata: Some(&tracked.metadata),
        current_state: &tracked.current_state,
    };

    let surface = govee_device_control_surface(&device);

    assert_eq!(
        surface.surface_id,
        format!("driver:govee:device:{}", tracked.info.id)
    );
    assert_eq!(
        surface.scope,
        ControlSurfaceScope::Device {
            device_id: tracked.info.id,
            driver_id: "govee".to_owned(),
        }
    );
    assert!(surface.revision > 0);
    assert!(
        surface
            .fields
            .iter()
            .any(|field| field.id == "ip" && field.access == ControlAccess::ReadOnly)
    );
    assert!(
        surface
            .fields
            .iter()
            .any(|field| field.id == "razer_streaming" && field.access == ControlAccess::ReadOnly)
    );
    assert_eq!(
        surface.values["ip"],
        ControlValue::IpAddress("10.0.0.5".to_owned())
    );
    assert_eq!(
        surface.values["sku"],
        ControlValue::String("H619A".to_owned())
    );
    assert_eq!(
        surface.values["mac"],
        ControlValue::MacAddress("001122334455".to_owned())
    );
    assert_eq!(surface.values["razer_streaming"], ControlValue::Bool(true));
    assert_eq!(surface.values["led_count"], ControlValue::Integer(1));
    assert_eq!(surface.values["max_fps"], ControlValue::Integer(10));
}

#[test]
fn govee_device_control_surface_exposes_cloud_metadata() {
    let discovered = build_cloud_discovered_device(V1Device {
        device: "AA:BB:CC:DD:EE:FF".to_owned(),
        model: "H6163".to_owned(),
        device_name: "Cloud Strip".to_owned(),
        controllable: true,
        retrievable: false,
        support_cmds: vec!["turn".to_owned(), "brightness".to_owned()],
        properties: None,
    });
    let device = TrackedDeviceCtx {
        device_id: discovered.info.id,
        info: &discovered.info,
        metadata: Some(&discovered.metadata),
        current_state: &DeviceState::Known,
    };

    let surface = govee_device_control_surface(&device);

    assert_eq!(
        surface.values["cloud_device_id"],
        ControlValue::String("AA:BB:CC:DD:EE:FF".to_owned())
    );
    assert_eq!(
        surface.values["cloud_controllable"],
        ControlValue::Bool(true)
    );
    assert_eq!(
        surface.values["cloud_retrievable"],
        ControlValue::Bool(false)
    );
    assert_eq!(
        surface.values["cloud_support_cmds"],
        ControlValue::List(vec![
            ControlValue::String("turn".to_owned()),
            ControlValue::String("brightness".to_owned()),
        ])
    );
}

fn tracked_govee_device(ip: &str, sku: &str, mac: &str) -> DriverTrackedDevice {
    DriverTrackedDevice {
        info: DeviceInfo {
            id: DeviceId::new(),
            name: "Desk Govee".to_owned(),
            vendor: "Govee".to_owned(),
            family: DeviceFamily::new_static("govee", "Govee"),
            model: Some(sku.to_owned()),
            connection_type: ConnectionType::Network,
            origin: DeviceOrigin::native("govee", "govee", ConnectionType::Network),
            zones: vec![ZoneInfo {
                name: "Main".to_owned(),
                led_count: 1,
                topology: DeviceTopologyHint::Strip,
                color_format: DeviceColorFormat::Rgb,
                layout_hint: None,
            }],
            firmware_version: None,
            capabilities: DeviceCapabilities {
                led_count: 1,
                supports_direct: true,
                supports_brightness: true,
                has_display: false,
                display_resolution: None,
                max_fps: 10,
                color_space: hypercolor_types::device::DeviceColorSpace::default(),
                features: DeviceFeatures::default(),
            },
        },
        metadata: HashMap::from([
            ("ip".to_owned(), ip.to_owned()),
            ("sku".to_owned(), sku.to_owned()),
            ("mac".to_owned(), mac.to_owned()),
        ]),
        fingerprint: Some(DeviceFingerprint(format!("net:govee:{mac}"))),
        current_state: DeviceState::Known,
    }
}

#[derive(Default)]
struct TestControlHost {
    driver_values: Mutex<HashMap<String, ControlValueMap>>,
    rebinds: AtomicUsize,
    rescans: AtomicUsize,
}

impl TestControlHost {
    fn saved_driver_values(&self, driver_id: &str) -> ControlValueMap {
        self.driver_values
            .lock()
            .expect("test driver values mutex should not be poisoned")
            .get(driver_id)
            .cloned()
            .expect("driver values should be saved")
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
    async fn load_device_values(&self, _device_id: DeviceId) -> Result<ControlValueMap> {
        Ok(ControlValueMap::new())
    }

    async fn save_device_values(
        &self,
        _device_id: DeviceId,
        _values: ControlValueMap,
    ) -> Result<()> {
        Ok(())
    }
}

#[async_trait::async_trait]
impl DriverLifecycleActions for TestControlHost {
    async fn reconnect_device(&self, _device_id: DeviceId, _backend_id: &str) -> Result<bool> {
        Ok(true)
    }

    async fn rescan_driver(&self, _driver_id: &str) -> Result<()> {
        self.rescans.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

#[async_trait::async_trait]
impl BackendRebindActions for TestControlHost {
    async fn rebind_backend(&self, _driver_id: &str) -> Result<()> {
        self.rebinds.fetch_add(1, Ordering::Relaxed);
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
