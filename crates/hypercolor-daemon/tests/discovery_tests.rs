//! Integration tests for daemon discovery scan scoping.

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use hypercolor_core::attachment::AttachmentRegistry;
use hypercolor_core::bus::HypercolorBus;
use hypercolor_core::device::wled::WledKnownTarget;
use hypercolor_core::device::{
    BackendManager, DeviceLifecycleManager, DeviceRegistry, UsbProtocolConfigStore,
};
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_daemon::attachment_profiles::AttachmentProfileStore;
use hypercolor_daemon::device_settings::DeviceSettingsStore;
use hypercolor_daemon::discovery::{
    DiscoveryBackend, DiscoveryRuntime, execute_discovery_scan, resolve_wled_probe_ips,
    resolve_wled_probe_targets, sync_active_layout_for_renderable_devices,
};
use hypercolor_daemon::logical_devices::LogicalDevice;
use hypercolor_daemon::runtime_state;
use hypercolor_types::config::HypercolorConfig;
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceFingerprint,
    DeviceId, DeviceInfo, DeviceState, DeviceTopologyHint, ZoneInfo,
};
use hypercolor_types::spatial::{EdgeBehavior, SamplingMode, SpatialLayout};
use tokio::sync::{Mutex, RwLock};

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
        },
    }
}

fn make_runtime(
    device_registry: DeviceRegistry,
    lifecycle_manager: Arc<Mutex<DeviceLifecycleManager>>,
    layouts_path: std::path::PathBuf,
    runtime_state_path: std::path::PathBuf,
) -> DiscoveryRuntime {
    DiscoveryRuntime {
        device_registry,
        backend_manager: Arc::new(Mutex::new(BackendManager::new())),
        lifecycle_manager,
        reconnect_tasks: Arc::new(StdMutex::new(HashMap::new())),
        event_bus: Arc::new(HypercolorBus::new()),
        spatial_engine: Arc::new(RwLock::new(SpatialEngine::new(empty_layout()))),
        layouts: Arc::new(RwLock::new(HashMap::new())),
        layouts_path,
        layout_auto_exclusions: Arc::new(RwLock::new(HashMap::new())),
        logical_devices: Arc::new(RwLock::new(HashMap::<String, LogicalDevice>::new())),
        attachment_registry: Arc::new(RwLock::new(AttachmentRegistry::new())),
        attachment_profiles: Arc::new(RwLock::new(AttachmentProfileStore::new(
            std::path::PathBuf::from("attachment-profiles.json"),
        ))),
        device_settings: Arc::new(RwLock::new(DeviceSettingsStore::new(
            std::path::PathBuf::from("device-settings.json"),
        ))),
        runtime_state_path,
        usb_protocol_configs: UsbProtocolConfigStore::new(),
        in_progress: Arc::new(AtomicBool::new(true)),
        task_spawner: tokio::runtime::Handle::current(),
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
        runtime,
        Arc::new(config),
        vec![DiscoveryBackend::Wled],
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

    let resolved = resolve_wled_probe_ips(&registry, &config, &runtime_state_path).await;
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

    let resolved = resolve_wled_probe_targets(&registry, &config, &runtime_state_path).await;
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
