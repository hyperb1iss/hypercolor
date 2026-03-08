//! Integration tests for daemon discovery scan scoping.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use hypercolor_core::bus::HypercolorBus;
use hypercolor_core::device::{BackendManager, DeviceLifecycleManager, DeviceRegistry};
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_daemon::discovery::{DiscoveryBackend, DiscoveryRuntime, execute_discovery_scan};
use hypercolor_daemon::logical_devices::LogicalDevice;
use hypercolor_types::config::HypercolorConfig;
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceId, DeviceInfo,
    DeviceState, DeviceTopologyHint, ZoneInfo,
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

fn make_runtime(
    device_registry: DeviceRegistry,
    lifecycle_manager: Arc<Mutex<DeviceLifecycleManager>>,
    layouts_path: std::path::PathBuf,
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
        logical_devices: Arc::new(RwLock::new(HashMap::<String, LogicalDevice>::new())),
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
