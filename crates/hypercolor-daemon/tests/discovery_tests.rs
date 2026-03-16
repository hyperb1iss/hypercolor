//! Integration tests for daemon discovery scan scoping.

use anyhow::{Result, bail};
use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use hypercolor_core::attachment::AttachmentRegistry;
use hypercolor_core::bus::HypercolorBus;
use hypercolor_core::device::net::CredentialStore;
use hypercolor_core::device::wled::WledKnownTarget;
use hypercolor_core::device::{
    BackendInfo, BackendManager, DeviceBackend, DeviceLifecycleManager, DeviceRegistry,
    UsbProtocolConfigStore,
};
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_daemon::attachment_profiles::AttachmentProfileStore;
use hypercolor_daemon::device_settings::DeviceSettingsStore;
use hypercolor_daemon::discovery::{
    DiscoveryBackend, DiscoveryRuntime, execute_discovery_scan, resolve_hue_probe_bridges,
    resolve_wled_probe_ips, resolve_wled_probe_targets, sync_active_layout_connectivity,
    sync_active_layout_for_renderable_devices,
};
use hypercolor_daemon::logical_devices::LogicalDevice;
use hypercolor_daemon::network::{self, DaemonDriverHost};
use hypercolor_daemon::runtime_state;
use hypercolor_driver_hue::HueKnownBridge;
use hypercolor_network::DriverRegistry;
use hypercolor_types::config::HypercolorConfig;
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceFeatures,
    DeviceFingerprint, DeviceId, DeviceInfo, DeviceState, DeviceTopologyHint, ZoneInfo,
};
use hypercolor_types::spatial::{
    DeviceZone, EdgeBehavior, LedTopology, NormalizedPosition, SamplingMode, SpatialLayout,
    StripDirection,
};
use tokio::sync::{Mutex, RwLock};

struct TestDiscoveryRuntime {
    runtime: DiscoveryRuntime,
    driver_host: Arc<DaemonDriverHost>,
    driver_registry: Arc<DriverRegistry>,
}

impl std::ops::Deref for TestDiscoveryRuntime {
    type Target = DiscoveryRuntime;

    fn deref(&self) -> &Self::Target {
        &self.runtime
    }
}

#[derive(Clone)]
struct CountingBackend {
    expected_device_id: DeviceId,
    connect_count: Arc<std::sync::atomic::AtomicUsize>,
    disconnect_count: Arc<std::sync::atomic::AtomicUsize>,
}

#[async_trait::async_trait]
impl DeviceBackend for CountingBackend {
    fn info(&self) -> BackendInfo {
        BackendInfo {
            id: "mock".to_owned(),
            name: "Counting Backend".to_owned(),
            description: "Records connect/disconnect operations for discovery tests".to_owned(),
        }
    }

    async fn discover(&mut self) -> Result<Vec<DeviceInfo>> {
        Ok(Vec::new())
    }

    async fn connect(&mut self, id: &DeviceId) -> Result<()> {
        if *id != self.expected_device_id {
            bail!("unexpected device id {id}");
        }
        self.connect_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    async fn disconnect(&mut self, id: &DeviceId) -> Result<()> {
        if *id != self.expected_device_id {
            bail!("unexpected device id {id}");
        }
        self.disconnect_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    async fn write_colors(&mut self, id: &DeviceId, colors: &[[u8; 3]]) -> Result<()> {
        let _ = (id, colors);
        Ok(())
    }
}

fn empty_layout() -> SpatialLayout {
    SpatialLayout {
        id: "default".into(),
        name: "Default Layout".into(),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones: Vec::new(),
        groups: Vec::new(),
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    }
}

fn usb_device_info() -> DeviceInfo {
    DeviceInfo {
        id: DeviceId::new(),
        name: "USB Test Device".into(),
        vendor: "TestCorp".into(),
        family: DeviceFamily::PrismRgb,
        model: Some("test_prism".into()),
        connection_type: ConnectionType::Usb,
        zones: vec![ZoneInfo {
            name: "Channel 1".into(),
            led_count: 16,
            topology: DeviceTopologyHint::Strip,
            color_format: DeviceColorFormat::Rgb,
        }],
        firmware_version: Some("1.0.0".into()),
        capabilities: DeviceCapabilities {
            led_count: 16,
            supports_direct: true,
            supports_brightness: true,
            has_display: false,
            display_resolution: None,
            max_fps: 60,
            color_space: hypercolor_types::device::DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        },
    }
}

fn smbus_device_info(name: &str) -> DeviceInfo {
    DeviceInfo {
        id: DeviceId::new(),
        name: name.into(),
        vendor: "ASUS".into(),
        family: DeviceFamily::Asus,
        model: Some("asus_aura_smbus_dram".into()),
        connection_type: ConnectionType::SmBus,
        zones: vec![ZoneInfo {
            name: "Main".into(),
            led_count: 8,
            topology: DeviceTopologyHint::Strip,
            color_format: DeviceColorFormat::Rgb,
        }],
        firmware_version: Some("AUDA0-E6K5-0101".into()),
        capabilities: DeviceCapabilities {
            led_count: 8,
            supports_direct: true,
            supports_brightness: true,
            has_display: false,
            display_resolution: None,
            max_fps: 30,
            color_space: hypercolor_types::device::DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        },
    }
}

fn wled_device_info(name: &str) -> DeviceInfo {
    DeviceInfo {
        id: DeviceId::new(),
        name: name.into(),
        vendor: "WLED".into(),
        family: DeviceFamily::Wled,
        model: None,
        connection_type: ConnectionType::Network,
        zones: vec![ZoneInfo {
            name: "Main".into(),
            led_count: 30,
            topology: DeviceTopologyHint::Strip,
            color_format: DeviceColorFormat::Rgb,
        }],
        firmware_version: Some("0.15.3".into()),
        capabilities: DeviceCapabilities {
            led_count: 30,
            supports_direct: true,
            supports_brightness: true,
            has_display: false,
            display_resolution: None,
            max_fps: 60,
            color_space: hypercolor_types::device::DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        },
    }
}

fn mock_device_info() -> DeviceInfo {
    DeviceInfo {
        id: DeviceId::new(),
        name: "Mock Layout Device".into(),
        vendor: "Mock".into(),
        family: DeviceFamily::Custom("mock".into()),
        model: Some("mock_layout_device".into()),
        connection_type: ConnectionType::Network,
        zones: vec![ZoneInfo {
            name: "Main".into(),
            led_count: 16,
            topology: DeviceTopologyHint::Strip,
            color_format: DeviceColorFormat::Rgb,
        }],
        firmware_version: Some("1.0.0".into()),
        capabilities: DeviceCapabilities {
            led_count: 16,
            supports_direct: true,
            supports_brightness: true,
            has_display: false,
            display_resolution: None,
            max_fps: 60,
            color_space: hypercolor_types::device::DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        },
    }
}

fn layout_with_device(layout_device_id: &str) -> SpatialLayout {
    SpatialLayout {
        id: "default".into(),
        name: "Default Layout".into(),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones: vec![DeviceZone {
            id: "zone_main".into(),
            name: "Main".into(),
            device_id: layout_device_id.to_owned(),
            zone_name: None,
            group_id: None,
            position: NormalizedPosition { x: 0.5, y: 0.5 },
            size: NormalizedPosition { x: 1.0, y: 1.0 },
            rotation: 0.0,
            scale: 1.0,
            display_order: 0,
            orientation: None,
            topology: LedTopology::Strip {
                count: 16,
                direction: StripDirection::LeftToRight,
            },
            led_positions: Vec::new(),
            led_mapping: None,
            sampling_mode: None,
            edge_behavior: None,
            shape: None,
            shape_preset: None,
            attachment: None,
        }],
        groups: Vec::new(),
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    }
}

fn make_runtime(
    device_registry: DeviceRegistry,
    lifecycle_manager: Arc<Mutex<DeviceLifecycleManager>>,
    layouts_path: std::path::PathBuf,
    runtime_state_path: std::path::PathBuf,
) -> TestDiscoveryRuntime {
    let backend_manager = Arc::new(Mutex::new(BackendManager::new()));
    let reconnect_tasks = Arc::new(StdMutex::new(HashMap::new()));
    let event_bus = Arc::new(HypercolorBus::new());
    let spatial_engine = Arc::new(RwLock::new(SpatialEngine::new(empty_layout())));
    let layouts = Arc::new(RwLock::new(HashMap::new()));
    let layout_auto_exclusions = Arc::new(RwLock::new(HashMap::new()));
    let logical_devices = Arc::new(RwLock::new(HashMap::<String, LogicalDevice>::new()));
    let attachment_registry = Arc::new(RwLock::new(AttachmentRegistry::new()));
    let attachment_profiles = Arc::new(RwLock::new(AttachmentProfileStore::new(
        std::path::PathBuf::from("attachment-profiles.json"),
    )));
    let device_settings = Arc::new(RwLock::new(DeviceSettingsStore::new(
        std::path::PathBuf::from("device-settings.json"),
    )));
    let usb_protocol_configs = UsbProtocolConfigStore::new();
    let credential_store = Arc::new(
        CredentialStore::open_blocking(&std::env::temp_dir().join(format!(
            "hypercolor-test-credentials-{}",
            uuid::Uuid::now_v7()
        )))
        .expect("test credential store"),
    );
    let in_progress = Arc::new(AtomicBool::new(true));
    let runtime_state_path_for_host = runtime_state_path.clone();
    let runtime = DiscoveryRuntime {
        device_registry: device_registry.clone(),
        backend_manager: Arc::clone(&backend_manager),
        lifecycle_manager: Arc::clone(&lifecycle_manager),
        reconnect_tasks: Arc::clone(&reconnect_tasks),
        event_bus: Arc::clone(&event_bus),
        spatial_engine: Arc::clone(&spatial_engine),
        layouts: Arc::clone(&layouts),
        layouts_path,
        layout_auto_exclusions: Arc::clone(&layout_auto_exclusions),
        logical_devices: Arc::clone(&logical_devices),
        attachment_registry: Arc::clone(&attachment_registry),
        attachment_profiles: Arc::clone(&attachment_profiles),
        device_settings: Arc::clone(&device_settings),
        runtime_state_path,
        usb_protocol_configs: usb_protocol_configs.clone(),
        credential_store: Arc::clone(&credential_store),
        in_progress: Arc::clone(&in_progress),
        task_spawner: tokio::runtime::Handle::current(),
    };
    let driver_host = Arc::new(DaemonDriverHost::new(
        device_registry,
        backend_manager,
        lifecycle_manager,
        reconnect_tasks,
        event_bus,
        spatial_engine,
        layouts,
        runtime.layouts_path.clone(),
        layout_auto_exclusions,
        logical_devices,
        attachment_registry,
        attachment_profiles,
        device_settings,
        runtime_state_path_for_host,
        usb_protocol_configs,
        credential_store,
        in_progress,
    ));
    let driver_registry = Arc::new(
        network::build_builtin_driver_registry(
            &HypercolorConfig::default(),
            Arc::clone(&runtime.credential_store),
        )
        .expect("test driver registry"),
    );

    TestDiscoveryRuntime {
        runtime,
        driver_host,
        driver_registry,
    }
}

#[tokio::test]
async fn wled_only_scan_does_not_vanish_connected_usb_devices() {
    let device_registry = DeviceRegistry::new();
    let info = usb_device_info();
    let device_id = device_registry.add(info.clone()).await;
    assert_eq!(device_id, info.id);
    assert!(
        device_registry
            .set_state(&device_id, DeviceState::Connected)
            .await,
        "device registry state should update"
    );

    let lifecycle_manager = Arc::new(Mutex::new(DeviceLifecycleManager::new()));
    {
        let mut lifecycle = lifecycle_manager.lock().await;
        let _ = lifecycle.on_discovered(device_id, &info, "usb", None);
        lifecycle
            .on_connected(device_id)
            .expect("lifecycle should accept connected transition");
    }

    let temp_dir = tempfile::tempdir().expect("tempdir should be created");
    let runtime = make_runtime(
        device_registry.clone(),
        Arc::clone(&lifecycle_manager),
        temp_dir.path().join("layouts.json"),
        temp_dir.path().join("runtime-state.json"),
    );

    let mut config = HypercolorConfig::default();
    config.discovery.mdns_enabled = false;
    config.wled.known_ips.clear();

    let result = execute_discovery_scan(
        runtime.runtime.clone(),
        Arc::clone(&runtime.driver_registry),
        Arc::clone(&runtime.driver_host),
        Arc::new(config),
        vec![DiscoveryBackend::network("wled")],
        Duration::from_millis(50),
    )
    .await;

    assert!(
        result.vanished_devices.is_empty(),
        "WLED-only scans must not treat USB devices as vanished"
    );

    let tracked = device_registry
        .get(&device_id)
        .await
        .expect("USB device should remain in the registry");
    assert_eq!(tracked.state, DeviceState::Connected);

    let lifecycle_state = lifecycle_manager.lock().await.state(device_id);
    assert_eq!(lifecycle_state, Some(DeviceState::Connected));
}

#[tokio::test]
async fn smbus_scan_does_not_timeout_connected_smbus_devices_on_transient_miss() {
    let device_registry = DeviceRegistry::new();
    let info = smbus_device_info("ASUS Aura DRAM (SMBus 0x71)");
    let fingerprint = DeviceFingerprint("smbus:/dev/i2c-999:71".to_owned());
    let mut metadata = HashMap::new();
    metadata.insert("backend_id".to_owned(), "smbus".to_owned());
    metadata.insert("bus_path".to_owned(), "/dev/i2c-999".to_owned());
    metadata.insert("smbus_address".to_owned(), "0x71".to_owned());

    let device_id = device_registry
        .add_with_fingerprint_and_metadata(info.clone(), fingerprint.clone(), metadata)
        .await;
    assert_eq!(device_id, info.id);
    assert!(
        device_registry
            .set_state(&device_id, DeviceState::Connected)
            .await,
        "device registry state should update"
    );

    let lifecycle_manager = Arc::new(Mutex::new(DeviceLifecycleManager::new()));
    {
        let mut lifecycle = lifecycle_manager.lock().await;
        let _ = lifecycle.on_discovered(device_id, &info, "smbus", Some(&fingerprint));
        lifecycle
            .on_connected(device_id)
            .expect("lifecycle should accept connected transition");
    }

    let temp_dir = tempfile::tempdir().expect("tempdir should be created");
    let runtime = make_runtime(
        device_registry.clone(),
        Arc::clone(&lifecycle_manager),
        temp_dir.path().join("layouts.json"),
        temp_dir.path().join("runtime-state.json"),
    );

    let result = execute_discovery_scan(
        runtime.runtime.clone(),
        Arc::clone(&runtime.driver_registry),
        Arc::clone(&runtime.driver_host),
        Arc::new(HypercolorConfig::default()),
        vec![DiscoveryBackend::SmBus],
        Duration::from_millis(50),
    )
    .await;

    assert!(
        !result.vanished_devices.contains(&device_id.to_string()),
        "connected SMBus devices should not be timed out by a transient miss"
    );

    let tracked = device_registry
        .get(&device_id)
        .await
        .expect("SMBus device should remain in the registry");
    assert_eq!(tracked.state, DeviceState::Connected);

    let lifecycle_state = lifecycle_manager.lock().await.state(device_id);
    assert_eq!(lifecycle_state, Some(DeviceState::Connected));
}

#[tokio::test]
async fn resolve_wled_probe_ips_merges_config_runtime_state_and_registry_metadata() {
    let registry = DeviceRegistry::new();

    let configured_ip: IpAddr = "10.4.22.69".parse().expect("configured IP should parse");
    let cached_ip: IpAddr = "10.4.22.169".parse().expect("cached IP should parse");

    let mut config = HypercolorConfig::default();
    config.wled.known_ips = vec![configured_ip];

    let desk = wled_device_info("WLED Desk");
    let mut desk_metadata = HashMap::new();
    desk_metadata.insert("ip".to_owned(), cached_ip.to_string());
    registry
        .add_with_fingerprint_and_metadata(
            desk,
            DeviceFingerprint("net:aa:bb:cc:dd:ee:ff".to_owned()),
            desk_metadata,
        )
        .await;

    let studio = wled_device_info("WLED Studio");
    let mut studio_metadata = HashMap::new();
    studio_metadata.insert("ip".to_owned(), configured_ip.to_string());
    registry
        .add_with_fingerprint_and_metadata(
            studio,
            DeviceFingerprint("net:11:22:33:44:55:66".to_owned()),
            studio_metadata,
        )
        .await;

    let malformed = wled_device_info("WLED Broken");
    let mut malformed_metadata = HashMap::new();
    malformed_metadata.insert("ip".to_owned(), "definitely-not-an-ip".to_owned());
    registry
        .add_with_fingerprint_and_metadata(
            malformed,
            DeviceFingerprint("net:66:55:44:33:22:11".to_owned()),
            malformed_metadata,
        )
        .await;

    let usb = usb_device_info();
    let mut usb_metadata = HashMap::new();
    usb_metadata.insert("ip".to_owned(), "10.4.22.250".to_owned());
    registry
        .add_with_fingerprint_and_metadata(
            usb,
            DeviceFingerprint("usb:test-device".to_owned()),
            usb_metadata,
        )
        .await;

    let temp_dir = tempfile::tempdir().expect("tempdir should be created");
    let runtime_state_path = temp_dir.path().join("runtime-state.json");
    runtime_state::save(
        &runtime_state_path,
        &runtime_state::RuntimeSessionSnapshot {
            wled_probe_ips: vec![
                "10.4.22.42"
                    .parse()
                    .expect("cached runtime IP should parse"),
            ],
            ..runtime_state::RuntimeSessionSnapshot::default()
        },
    )
    .expect("runtime state should save");

    let resolved = resolve_wled_probe_ips(&registry, &config.wled, &runtime_state_path).await;
    assert_eq!(
        resolved,
        vec![
            "10.4.22.42"
                .parse()
                .expect("cached runtime IP should parse"),
            configured_ip,
            cached_ip,
        ]
    );
}

#[tokio::test]
async fn resolve_wled_probe_targets_preserves_cached_identity_hints() {
    let registry = DeviceRegistry::new();

    let configured_ip: IpAddr = "10.4.22.69".parse().expect("configured IP should parse");
    let cached_ip: IpAddr = "10.4.22.169".parse().expect("cached IP should parse");

    let mut config = HypercolorConfig::default();
    config.wled.known_ips = vec![configured_ip];

    let studio = wled_device_info("WLED Studio");
    let mut studio_metadata = HashMap::new();
    studio_metadata.insert("ip".to_owned(), configured_ip.to_string());
    studio_metadata.insert("hostname".to_owned(), "wled-studio".to_owned());
    registry
        .add_with_fingerprint_and_metadata(
            studio,
            DeviceFingerprint("net:11:22:33:44:55:66".to_owned()),
            studio_metadata,
        )
        .await;

    let temp_dir = tempfile::tempdir().expect("tempdir should be created");
    let runtime_state_path = temp_dir.path().join("runtime-state.json");
    runtime_state::save(
        &runtime_state_path,
        &runtime_state::RuntimeSessionSnapshot {
            wled_probe_targets: vec![WledKnownTarget {
                ip: cached_ip,
                hostname: Some("wled-desk".to_owned()),
                fingerprint: Some(DeviceFingerprint("net:wled:wled-desk".to_owned())),
                name: Some("Desk Strip".to_owned()),
                led_count: Some(120),
                firmware_version: Some("0.15.3".to_owned()),
                max_fps: Some(60),
                rgbw: Some(false),
            }],
            ..runtime_state::RuntimeSessionSnapshot::default()
        },
    )
    .expect("runtime state should save");

    let resolved = resolve_wled_probe_targets(&registry, &config.wled, &runtime_state_path).await;
    assert_eq!(resolved.len(), 2);

    let cached = resolved
        .iter()
        .find(|target| target.ip == cached_ip)
        .expect("cached target should be preserved");
    assert_eq!(cached.hostname.as_deref(), Some("wled-desk"));
    assert_eq!(cached.name.as_deref(), Some("Desk Strip"));
    assert_eq!(cached.led_count, Some(120));

    let configured = resolved
        .iter()
        .find(|target| target.ip == configured_ip)
        .expect("registry-backed target should be included");
    assert_eq!(configured.hostname.as_deref(), Some("wled-studio"));
    assert_eq!(configured.name.as_deref(), Some("WLED Studio"));
    assert_eq!(
        configured.fingerprint,
        Some(DeviceFingerprint("net:11:22:33:44:55:66".to_owned()))
    );
}

#[tokio::test]
async fn resolve_hue_probe_bridges_merges_config_and_registry_metadata() {
    let registry = DeviceRegistry::new();
    let configured_ip: IpAddr = "10.9.0.10".parse().expect("configured IP should parse");
    let cached_ip: IpAddr = "10.9.0.20".parse().expect("cached IP should parse");

    let mut config = HypercolorConfig::default();
    config.hue.bridge_ips = vec![configured_ip];

    let mut hue_info = wled_device_info("Studio Bridge");
    hue_info.family = DeviceFamily::Hue;
    hue_info.vendor = "Philips Hue".to_owned();
    hue_info.model = Some("BSB002".to_owned());
    hue_info.firmware_version = Some("1968096020".to_owned());

    let mut hue_metadata = HashMap::new();
    hue_metadata.insert("backend_id".to_owned(), "hue".to_owned());
    hue_metadata.insert("ip".to_owned(), cached_ip.to_string());
    hue_metadata.insert("api_port".to_owned(), "8443".to_owned());
    hue_metadata.insert("bridge_id".to_owned(), "bridge-cached".to_owned());
    registry
        .add_with_fingerprint_and_metadata(
            hue_info,
            DeviceFingerprint("hue:bridge-cached".to_owned()),
            hue_metadata,
        )
        .await;

    let resolved = resolve_hue_probe_bridges(&registry, &config.hue).await;
    assert_eq!(
        resolved,
        vec![
            HueKnownBridge {
                bridge_id: String::new(),
                ip: configured_ip,
                api_port: 443,
                name: String::new(),
                model_id: String::new(),
                sw_version: String::new(),
            },
            HueKnownBridge {
                bridge_id: "bridge-cached".to_owned(),
                ip: cached_ip,
                api_port: 8443,
                name: "Studio Bridge".to_owned(),
                model_id: "BSB002".to_owned(),
                sw_version: "1968096020".to_owned(),
            },
        ]
    );
}

#[tokio::test]
async fn sync_active_layout_for_renderable_devices_skips_excluded_devices() {
    let device_registry = DeviceRegistry::new();
    let info = usb_device_info();
    let device_id = device_registry.add(info.clone()).await;
    assert_eq!(device_id, info.id);
    assert!(
        device_registry
            .set_state(&device_id, DeviceState::Connected)
            .await,
        "device registry state should update"
    );

    let lifecycle_manager = Arc::new(Mutex::new(DeviceLifecycleManager::new()));
    let layout_device_id = {
        let mut lifecycle = lifecycle_manager.lock().await;
        let _ = lifecycle.on_discovered(device_id, &info, "usb", None);
        lifecycle
            .on_connected(device_id)
            .expect("lifecycle should accept connected transition");
        lifecycle
            .layout_device_id_for(device_id)
            .map(ToOwned::to_owned)
            .expect("connected device should have a canonical layout ID")
    };

    let temp_dir = tempfile::tempdir().expect("tempdir should be created");
    let runtime = make_runtime(
        device_registry,
        lifecycle_manager,
        temp_dir.path().join("layouts.json"),
        temp_dir.path().join("runtime-state.json"),
    );

    {
        let mut manager = runtime.backend_manager.lock().await;
        manager.map_device(layout_device_id.clone(), "usb", device_id);
    }
    {
        let mut exclusions = runtime.layout_auto_exclusions.write().await;
        exclusions.insert("default".to_owned(), HashSet::from([layout_device_id]));
    }

    sync_active_layout_for_renderable_devices(&runtime, None).await;

    let layout = {
        let spatial = runtime.spatial_engine.read().await;
        spatial.layout().as_ref().clone()
    };
    assert!(
        layout.zones.is_empty(),
        "excluded devices must not be reconciled back into the active layout"
    );

    let persisted_layouts = runtime.layouts.read().await;
    assert!(
        persisted_layouts.is_empty(),
        "skipping excluded devices should not persist any synthetic layout changes"
    );
}

#[tokio::test]
async fn sync_active_layout_for_renderable_devices_does_not_auto_adopt_new_devices() {
    let device_registry = DeviceRegistry::new();
    let info = usb_device_info();
    let device_id = device_registry.add(info.clone()).await;
    assert_eq!(device_id, info.id);
    assert!(
        device_registry
            .set_state(&device_id, DeviceState::Connected)
            .await,
        "device registry state should update"
    );

    let lifecycle_manager = Arc::new(Mutex::new(DeviceLifecycleManager::new()));
    let layout_device_id = {
        let mut lifecycle = lifecycle_manager.lock().await;
        let _ = lifecycle.on_discovered(device_id, &info, "usb", None);
        lifecycle
            .on_connected(device_id)
            .expect("lifecycle should accept connected transition");
        lifecycle
            .layout_device_id_for(device_id)
            .map(ToOwned::to_owned)
            .expect("connected device should have a canonical layout ID")
    };

    let temp_dir = tempfile::tempdir().expect("tempdir should be created");
    let runtime = make_runtime(
        device_registry,
        lifecycle_manager,
        temp_dir.path().join("layouts.json"),
        temp_dir.path().join("runtime-state.json"),
    );

    {
        let mut manager = runtime.backend_manager.lock().await;
        manager.map_device(layout_device_id, "usb", device_id);
    }

    sync_active_layout_for_renderable_devices(&runtime, None).await;

    let layout = {
        let spatial = runtime.spatial_engine.read().await;
        spatial.layout().as_ref().clone()
    };
    assert!(
        layout.zones.is_empty(),
        "newly discovered devices must not be auto-adopted into the active layout"
    );

    let persisted_layouts = runtime.layouts.read().await;
    assert!(
        persisted_layouts.is_empty(),
        "discovery should not persist layout changes for unmapped devices"
    );
}

#[tokio::test]
async fn sync_active_layout_connectivity_keeps_layout_inactive_devices_disconnected() {
    let device_registry = DeviceRegistry::new();
    let info = mock_device_info();
    let fingerprint = DeviceFingerprint("mock:layout-device".to_owned());
    let metadata = HashMap::from([("backend_id".to_owned(), "mock".to_owned())]);
    let device_id = device_registry
        .add_with_fingerprint_and_metadata(info.clone(), fingerprint, metadata)
        .await;
    assert_eq!(device_id, info.id);

    let lifecycle_manager = Arc::new(Mutex::new(DeviceLifecycleManager::new()));
    let temp_dir = tempfile::tempdir().expect("tempdir should be created");
    let runtime = make_runtime(
        device_registry,
        Arc::clone(&lifecycle_manager),
        temp_dir.path().join("layouts.json"),
        temp_dir.path().join("runtime-state.json"),
    );

    let connect_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let disconnect_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    {
        let mut manager = runtime.backend_manager.lock().await;
        manager.register_backend(Box::new(CountingBackend {
            expected_device_id: device_id,
            connect_count: Arc::clone(&connect_count),
            disconnect_count: Arc::clone(&disconnect_count),
        }));
    }

    sync_active_layout_connectivity(&runtime, None).await;

    assert_eq!(
        connect_count.load(std::sync::atomic::Ordering::Relaxed),
        0,
        "layout-inactive devices should not be connected"
    );
    assert_eq!(
        lifecycle_manager.lock().await.state(device_id),
        Some(DeviceState::Known)
    );
}

#[tokio::test]
async fn sync_active_layout_connectivity_disconnects_devices_removed_from_layout() {
    let device_registry = DeviceRegistry::new();
    let info = mock_device_info();
    let fingerprint = DeviceFingerprint("mock:layout-device".to_owned());
    let metadata = HashMap::from([("backend_id".to_owned(), "mock".to_owned())]);
    let device_id = device_registry
        .add_with_fingerprint_and_metadata(info.clone(), fingerprint.clone(), metadata)
        .await;
    assert_eq!(device_id, info.id);

    let lifecycle_manager = Arc::new(Mutex::new(DeviceLifecycleManager::new()));
    let temp_dir = tempfile::tempdir().expect("tempdir should be created");
    let runtime = make_runtime(
        device_registry,
        Arc::clone(&lifecycle_manager),
        temp_dir.path().join("layouts.json"),
        temp_dir.path().join("runtime-state.json"),
    );

    let connect_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let disconnect_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    {
        let mut manager = runtime.backend_manager.lock().await;
        manager.register_backend(Box::new(CountingBackend {
            expected_device_id: device_id,
            connect_count: Arc::clone(&connect_count),
            disconnect_count: Arc::clone(&disconnect_count),
        }));
    }

    let layout_device_id =
        DeviceLifecycleManager::canonical_layout_device_id("mock", &info, Some(&fingerprint));
    {
        let mut spatial = runtime.spatial_engine.write().await;
        spatial.update_layout(layout_with_device(&layout_device_id));
    }

    sync_active_layout_connectivity(&runtime, None).await;
    assert_eq!(
        connect_count.load(std::sync::atomic::Ordering::Relaxed),
        1,
        "active layout targets should connect the device"
    );
    assert_eq!(
        lifecycle_manager.lock().await.state(device_id),
        Some(DeviceState::Connected)
    );

    {
        let mut spatial = runtime.spatial_engine.write().await;
        spatial.update_layout(empty_layout());
    }

    sync_active_layout_connectivity(&runtime, None).await;
    assert_eq!(
        disconnect_count.load(std::sync::atomic::Ordering::Relaxed),
        1,
        "removing the device from the active layout should disconnect it"
    );
    assert_eq!(
        lifecycle_manager.lock().await.state(device_id),
        Some(DeviceState::Known)
    );
}
