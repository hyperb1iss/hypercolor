//! Integration tests for daemon discovery scan scoping.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use hypercolor_core::attachment::ComponentRegistry;
use hypercolor_core::bus::HypercolorBus;
use hypercolor_core::config::ConfigManager;
use hypercolor_core::device::{
    BackendManager, DeviceLifecycleManager, DeviceRegistry, UsbProtocolConfigStore,
};
use hypercolor_core::scene::SceneManager;
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_daemon::SceneTransactionQueue;
use hypercolor_daemon::attachment_profiles::ComponentProfileStore;
use hypercolor_daemon::device_settings::DeviceSettingsStore;
use hypercolor_daemon::discovery::{
    DiscoveryRuntime, DiscoveryTarget, execute_discovery_scan, execute_discovery_scan_if_idle,
    schedule_discovery_scan,
};
use hypercolor_daemon::display_preferences::DisplayPreferencesStore;
use hypercolor_daemon::domain::DeviceBindingMigrationContext;
use hypercolor_daemon::domain::layout::LayoutContext;
use hypercolor_daemon::domain::scene::SceneService;
use hypercolor_daemon::domain::spatial::SpatialService;
use hypercolor_daemon::driver_inventory::{DRIVER_INVENTORY_FILENAME, DriverInventoryStore};
use hypercolor_daemon::layout_auto_exclusions::LayoutAutoExclusionKey;
use hypercolor_daemon::logical_devices::{LogicalDevice, LogicalDeviceKind};
use hypercolor_daemon::network::{self, DaemonDriverHost};
use hypercolor_daemon::output_power::OutputPower;
use hypercolor_driver_api::{BackendInfo, DeviceBackend, DiscoveredDevice};
use hypercolor_driver_api::{
    DiscoveryCapability, DiscoveryConnectBehavior, DiscoveryRequest, DriverConfigView,
    DriverDescriptor, DriverError, DriverHost, DriverModule,
};
use hypercolor_driver_support::CredentialStore;
use hypercolor_network::DriverModuleRegistry;
use hypercolor_types::config::{DriverConfigEntry, HypercolorConfig};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceError, DeviceFamily,
    DeviceFeatures, DeviceFingerprint, DeviceId, DeviceInfo, DeviceOrigin, DeviceState,
    DeviceTopologyHint, DriverTransportKind, SegmentInfo,
};
use hypercolor_types::event::ZoneColors;
use hypercolor_types::scene::SceneId;
use hypercolor_types::spatial::{
    EdgeBehavior, LedTopology, NormalizedPosition, Output, SamplingMode, SpatialLayout,
    StripDirection,
};
use tokio::sync::{Mutex, RwLock, Semaphore};
use tokio::time::timeout;

struct TestDiscoveryRuntime {
    runtime: DiscoveryRuntime,
    driver_host: Arc<DaemonDriverHost>,
    driver_registry: Arc<DriverModuleRegistry>,
}

async fn sync_active_layout_for_renderable_devices(
    runtime: &DiscoveryRuntime,
    limit_to_devices: Option<&HashSet<DeviceId>>,
) {
    runtime
        .layout
        .test_workflows()
        .sync_active_layout_for_renderable_devices(runtime.clone(), limit_to_devices.cloned())
        .await;
}

async fn sync_active_layout_connectivity(
    runtime: &DiscoveryRuntime,
    limit_to_devices: Option<&HashSet<DeviceId>>,
) {
    runtime
        .layout
        .test_workflows()
        .sync_connectivity(runtime.clone(), limit_to_devices.cloned())
        .await;
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

struct CachePrimingBackend {
    expected_device_id: DeviceId,
    expected_fingerprint: DeviceFingerprint,
    cached: AtomicBool,
    adopt_count: Arc<std::sync::atomic::AtomicUsize>,
    connect_count: Arc<std::sync::atomic::AtomicUsize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DiscoveryConfigObservation {
    generation: u64,
    mdns_enabled: bool,
    timeout: Duration,
}

struct BlockingConfigDiscoveryDriver {
    observations: Arc<StdMutex<Vec<DiscoveryConfigObservation>>>,
    started: Arc<Semaphore>,
    release_first: Arc<Semaphore>,
}

#[derive(Clone)]
struct StaticAsusDiscoveryDriver {
    device: DiscoveredDevice,
}

static STATIC_ASUS_DISCOVERY_DRIVER: DriverDescriptor = DriverDescriptor::new(
    "asus",
    "Static ASUS Discovery",
    DriverTransportKind::Smbus,
    true,
    false,
);

impl DriverModule for StaticAsusDiscoveryDriver {
    fn descriptor(&self) -> &'static DriverDescriptor {
        &STATIC_ASUS_DISCOVERY_DRIVER
    }

    fn discovery(&self) -> Option<&dyn DiscoveryCapability> {
        Some(self)
    }
}

#[async_trait::async_trait]
impl DiscoveryCapability for StaticAsusDiscoveryDriver {
    async fn discover(
        &self,
        _host: &dyn DriverHost,
        _request: &DiscoveryRequest,
        _config: DriverConfigView<'_>,
    ) -> Result<Vec<DiscoveredDevice>, DriverError> {
        Ok(vec![self.device.clone()])
    }
}

static BLOCKING_CONFIG_DISCOVERY_DRIVER: DriverDescriptor = DriverDescriptor::new(
    "blocking_config_discovery",
    "Blocking Config Discovery",
    DriverTransportKind::Network,
    true,
    false,
);

impl DriverModule for BlockingConfigDiscoveryDriver {
    fn descriptor(&self) -> &'static DriverDescriptor {
        &BLOCKING_CONFIG_DISCOVERY_DRIVER
    }

    fn discovery(&self) -> Option<&dyn DiscoveryCapability> {
        Some(self)
    }
}

#[async_trait::async_trait]
impl DiscoveryCapability for BlockingConfigDiscoveryDriver {
    async fn discover(
        &self,
        _host: &dyn DriverHost,
        request: &DiscoveryRequest,
        config: DriverConfigView<'_>,
    ) -> Result<Vec<DiscoveredDevice>, DriverError> {
        let generation = config
            .entry
            .settings
            .get("generation")
            .and_then(serde_json::Value::as_u64)
            .expect("test discovery generation should be configured");
        let call_index = {
            let mut observations = self
                .observations
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            observations.push(DiscoveryConfigObservation {
                generation,
                mdns_enabled: request.mdns_enabled,
                timeout: request.timeout,
            });
            observations.len()
        };
        self.started.add_permits(1);

        if call_index == 1 {
            Arc::clone(&self.release_first)
                .acquire_owned()
                .await
                .expect("first discovery release semaphore should stay open")
                .forget();
        }

        Ok(Vec::new())
    }
}

#[derive(Clone)]
struct FailingDisconnectBackend {
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

    fn adopt_device(&self, _discovered: &DiscoveredDevice) -> Result<(), DeviceError> {
        Ok(())
    }

    async fn connect(&self, id: &DeviceId) -> Result<(), DeviceError> {
        if *id != self.expected_device_id {
            return Err(DeviceError::NotFound {
                device: id.to_string(),
            });
        }
        self.connect_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    async fn disconnect(&self, id: &DeviceId) -> Result<(), DeviceError> {
        if *id != self.expected_device_id {
            return Err(DeviceError::NotFound {
                device: id.to_string(),
            });
        }
        self.disconnect_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    async fn write_colors(&self, id: &DeviceId, colors: &[[u8; 3]]) -> Result<(), DeviceError> {
        let _ = (id, colors);
        Ok(())
    }
}

#[async_trait::async_trait]
impl DeviceBackend for FailingDisconnectBackend {
    fn info(&self) -> BackendInfo {
        BackendInfo {
            id: "mock".to_owned(),
            name: "Failing Disconnect Backend".to_owned(),
            description: "Fails disconnect after accepting routed writes".to_owned(),
        }
    }

    fn adopt_device(&self, _discovered: &DiscoveredDevice) -> Result<(), DeviceError> {
        Ok(())
    }

    async fn connect(&self, id: &DeviceId) -> Result<(), DeviceError> {
        if *id != self.expected_device_id {
            return Err(DeviceError::NotFound {
                device: id.to_string(),
            });
        }
        self.connect_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    async fn disconnect(&self, id: &DeviceId) -> Result<(), DeviceError> {
        if *id != self.expected_device_id {
            return Err(DeviceError::NotFound {
                device: id.to_string(),
            });
        }
        self.disconnect_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Err(DeviceError::connection(id, "simulated disconnect failure"))
    }

    async fn write_colors(&self, id: &DeviceId, colors: &[[u8; 3]]) -> Result<(), DeviceError> {
        let _ = (id, colors);
        Ok(())
    }
}

#[async_trait::async_trait]
impl DeviceBackend for CachePrimingBackend {
    fn info(&self) -> BackendInfo {
        BackendInfo {
            id: "mock".to_owned(),
            name: "Cache Priming Backend".to_owned(),
            description: "Requires scanner metadata before connect".to_owned(),
        }
    }

    fn adopt_device(&self, discovered: &DiscoveredDevice) -> Result<(), DeviceError> {
        self.adopt_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.cached.store(
            discovered.info.id == self.expected_device_id
                && discovered.fingerprint == self.expected_fingerprint
                && discovered
                    .metadata
                    .get("descriptor")
                    .is_some_and(|value| value == "cached"),
            Ordering::Release,
        );
        Ok(())
    }

    async fn connect(&self, id: &DeviceId) -> Result<(), DeviceError> {
        if *id != self.expected_device_id {
            return Err(DeviceError::NotFound {
                device: id.to_string(),
            });
        }
        if !self.cached.load(Ordering::Acquire) {
            return Err(DeviceError::NotAdopted { device_id: *id });
        }
        self.connect_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    async fn disconnect(&self, id: &DeviceId) -> Result<(), DeviceError> {
        if *id != self.expected_device_id {
            return Err(DeviceError::NotFound {
                device: id.to_string(),
            });
        }
        Ok(())
    }

    async fn write_colors(&self, id: &DeviceId, colors: &[[u8; 3]]) -> Result<(), DeviceError> {
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

        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        version: 1,
    }
}

fn usb_device_info() -> DeviceInfo {
    DeviceInfo {
        id: DeviceId::new(),
        name: "USB Test Device".into(),
        vendor: "TestCorp".into(),
        family: DeviceFamily::new_static("prismrgb", "PrismRGB"),
        model: Some("test_prism".into()),
        connection_type: ConnectionType::Usb,
        origin: DeviceOrigin::native("prismrgb", "usb", ConnectionType::Usb)
            .with_protocol_id("prismrgb/test-prism"),
        segments: vec![SegmentInfo {
            name: "Channel 1".into(),
            led_count: 16,
            topology: DeviceTopologyHint::Strip,
            color_format: DeviceColorFormat::Rgb,
            layout_hint: None,
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
        family: DeviceFamily::new_static("asus", "ASUS"),
        model: Some("asus_aura_smbus_dram".into()),
        connection_type: ConnectionType::SmBus,
        origin: DeviceOrigin::native("asus", "smbus", ConnectionType::SmBus)
            .with_protocol_id("asus/aura-smbus"),
        segments: vec![SegmentInfo {
            name: "Main".into(),
            led_count: 8,
            topology: DeviceTopologyHint::Strip,
            color_format: DeviceColorFormat::Rgb,
            layout_hint: None,
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

fn prism_s_device_info_with_backend(backend_id: &str) -> DeviceInfo {
    DeviceInfo {
        id: DeviceId::new(),
        name: "PrismRGB Prism S".to_owned(),
        vendor: "PrismRGB".to_owned(),
        family: DeviceFamily::new_static("prismrgb", "PrismRGB"),
        model: Some("prism_s".to_owned()),
        connection_type: ConnectionType::Usb,
        origin: DeviceOrigin::native("prismrgb", backend_id, ConnectionType::Usb)
            .with_protocol_id("prismrgb/prism-s"),
        segments: vec![
            SegmentInfo {
                name: "ATX Strimer".to_owned(),
                led_count: 120,
                topology: DeviceTopologyHint::Matrix { rows: 6, cols: 20 },
                color_format: DeviceColorFormat::Rgb,
                layout_hint: None,
            },
            SegmentInfo {
                name: "GPU Strimer".to_owned(),
                led_count: 162,
                topology: DeviceTopologyHint::Matrix { rows: 6, cols: 27 },
                color_format: DeviceColorFormat::Rgb,
                layout_hint: None,
            },
        ],
        firmware_version: None,
        capabilities: DeviceCapabilities::default(),
    }
}

fn mock_device_info() -> DeviceInfo {
    DeviceInfo {
        id: DeviceId::new(),
        name: "Mock Layout Device".into(),
        vendor: "Mock".into(),
        family: DeviceFamily::named("mock"),
        model: Some("mock_layout_device".into()),
        connection_type: ConnectionType::Network,
        origin: DeviceOrigin::native("mock", "mock", ConnectionType::Network),
        segments: vec![SegmentInfo {
            name: "Main".into(),
            led_count: 16,
            topology: DeviceTopologyHint::Strip,
            color_format: DeviceColorFormat::Rgb,
            layout_hint: None,
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
        zones: vec![Output {
            id: "zone_main".into(),
            name: "Main".into(),
            device_id: layout_device_id.to_owned(),
            zone_name: None,

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
            brightness: None,
        }],

        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        version: 1,
    }
}

fn legacy_asus_dram_layout() -> SpatialLayout {
    let mut layout = layout_with_device("asus:dev-i2c-9:73");
    let output = layout
        .zones
        .first_mut()
        .expect("test layout should contain one output");
    output.name = "ASUS Aura DRAM (SMBus 0x73) · Lighting".to_owned();
    output.zone_name = Some("Lighting".to_owned());
    output.topology = LedTopology::Strip {
        count: 8,
        direction: StripDirection::LeftToRight,
    };
    layout
}

fn make_runtime(
    device_registry: DeviceRegistry,
    lifecycle_manager: Arc<Mutex<DeviceLifecycleManager>>,
    layouts_path: std::path::PathBuf,
    runtime_state_path: std::path::PathBuf,
) -> TestDiscoveryRuntime {
    make_runtime_with_layout(
        device_registry,
        lifecycle_manager,
        layouts_path,
        runtime_state_path,
        empty_layout(),
        HashSet::new(),
    )
}

fn make_runtime_with_layout(
    device_registry: DeviceRegistry,
    lifecycle_manager: Arc<Mutex<DeviceLifecycleManager>>,
    layouts_path: std::path::PathBuf,
    runtime_state_path: std::path::PathBuf,
    active_layout: SpatialLayout,
    excluded_device_ids: HashSet<String>,
) -> TestDiscoveryRuntime {
    make_runtime_with_registry_and_layout(
        device_registry,
        lifecycle_manager,
        layouts_path,
        runtime_state_path,
        None,
        None,
        active_layout,
        excluded_device_ids,
    )
}

fn make_runtime_with_registry(
    device_registry: DeviceRegistry,
    lifecycle_manager: Arc<Mutex<DeviceLifecycleManager>>,
    layouts_path: std::path::PathBuf,
    runtime_state_path: std::path::PathBuf,
    driver_registry: Option<DriverModuleRegistry>,
    config_manager: Option<Arc<ConfigManager>>,
) -> TestDiscoveryRuntime {
    make_runtime_with_registry_and_layout(
        device_registry,
        lifecycle_manager,
        layouts_path,
        runtime_state_path,
        driver_registry,
        config_manager,
        empty_layout(),
        HashSet::new(),
    )
}

#[allow(clippy::too_many_arguments)]
fn make_runtime_with_registry_and_layout(
    device_registry: DeviceRegistry,
    lifecycle_manager: Arc<Mutex<DeviceLifecycleManager>>,
    layouts_path: std::path::PathBuf,
    runtime_state_path: std::path::PathBuf,
    driver_registry: Option<DriverModuleRegistry>,
    config_manager: Option<Arc<ConfigManager>>,
    active_layout: SpatialLayout,
    excluded_device_ids: HashSet<String>,
) -> TestDiscoveryRuntime {
    let backend_manager = Arc::new(Mutex::new(BackendManager::new()));
    let reconnect_tasks = Arc::new(StdMutex::new(HashMap::new()));
    let event_bus = Arc::new(HypercolorBus::new());
    let spatial_engine = SpatialService::new(SpatialEngine::new(active_layout.clone()));
    let layouts = HashMap::new();
    let logical_devices = Arc::new(RwLock::new(HashMap::<String, LogicalDevice>::new()));
    let logical_devices_path = runtime_state_path.with_file_name("logical-devices.json");
    let attachment_registry = Arc::new(RwLock::new(ComponentRegistry::new()));
    let attachment_profiles = Arc::new(RwLock::new(ComponentProfileStore::new(
        std::path::PathBuf::from("attachment-profiles.json"),
    )));
    let device_settings = OutputPower::new(DeviceSettingsStore::new(std::path::PathBuf::from(
        "device-settings.json",
    )))
    .device_settings();
    let display_preferences = Arc::new(RwLock::new(
        DisplayPreferencesStore::new(runtime_state_path.with_file_name("display-preferences.json"))
            .expect("display preference store"),
    ));
    let usb_protocol_configs = UsbProtocolConfigStore::new();
    let credential_store = Arc::new(
        CredentialStore::open_blocking(&std::env::temp_dir().join(format!(
            "hypercolor-test-credentials-{}",
            uuid::Uuid::now_v7()
        )))
        .expect("test credential store"),
    );
    let in_progress = Arc::new(AtomicBool::new(true));
    let driver_inventory = Arc::new(
        DriverInventoryStore::open(runtime_state_path.with_file_name(DRIVER_INVENTORY_FILENAME))
            .expect("test driver inventory"),
    );
    let scene_transactions = SceneTransactionQueue::default();
    let scene_manager_inner = SceneManager::with_default_layout(active_layout);
    let primary_zone_id = scene_manager_inner
        .active_scene()
        .and_then(hypercolor_types::scene::Scene::primary_zone)
        .map(|zone| zone.id)
        .expect("default scene should have a primary zone");
    let layout_auto_exclusions = if excluded_device_ids.is_empty() {
        HashMap::new()
    } else {
        HashMap::from([(
            LayoutAutoExclusionKey::zone(SceneId::DEFAULT, primary_zone_id),
            excluded_device_ids,
        )])
    };
    let scene_manager = SceneService::in_memory(scene_manager_inner, Arc::clone(&event_bus));
    let driver_registry = Arc::new(driver_registry.unwrap_or_else(|| {
        network::build_builtin_driver_module_registry(
            &HypercolorConfig::default(),
            Arc::clone(&credential_store),
            usb_protocol_configs.clone(),
        )
        .expect("test driver registry")
    }));
    let layout = LayoutContext::new_test_context(
        layouts,
        layouts_path,
        layout_auto_exclusions,
        runtime_state_path.with_file_name("layout-auto-exclusions.json"),
        spatial_engine,
        scene_manager,
        scene_transactions,
        runtime_state_path.clone(),
    );
    let runtime = DiscoveryRuntime {
        device_registry: device_registry.clone(),
        backend_manager: Arc::clone(&backend_manager),
        lifecycle_manager: Arc::clone(&lifecycle_manager),
        reconnect_tasks: Arc::clone(&reconnect_tasks),
        event_bus: Arc::clone(&event_bus),
        layout: layout.clone(),
        binding_migration: Arc::new(DeviceBindingMigrationContext::new(
            layout.clone(),
            Arc::clone(&logical_devices),
            logical_devices_path,
            Arc::clone(&attachment_profiles),
            device_settings.clone(),
            display_preferences,
            runtime_state_path.with_file_name("device-binding-migration.json"),
        )),
        logical_devices: Arc::clone(&logical_devices),
        attachment_registry: Arc::clone(&attachment_registry),
        attachment_profiles: Arc::clone(&attachment_profiles),
        device_settings: device_settings.clone(),
        runtime_state_path: runtime_state_path.clone(),
        device_aliases_path: runtime_state_path.with_file_name("device-aliases.json"),
        usb_protocol_configs: usb_protocol_configs.clone(),
        credential_store: Arc::clone(&credential_store),
        in_progress: Arc::clone(&in_progress),
        pending_scans: Arc::default(),
        task_spawner: tokio::runtime::Handle::current(),
    };
    let driver_host = Arc::new(DaemonDriverHost::new(
        runtime.clone(),
        driver_inventory,
        Arc::clone(&driver_registry),
        config_manager,
    ));
    TestDiscoveryRuntime {
        runtime,
        driver_host,
        driver_registry,
    }
}

fn install_active_layout(
    runtime: &mut TestDiscoveryRuntime,
    active_layout: SpatialLayout,
    state_dir: &std::path::Path,
) {
    let spatial = SpatialService::new(SpatialEngine::new(active_layout.clone()));
    let scenes = SceneService::in_memory(
        SceneManager::with_default_layout(active_layout),
        Arc::clone(&runtime.event_bus),
    );
    runtime.runtime.layout = LayoutContext::new_test_context(
        HashMap::new(),
        state_dir.join("layouts.json"),
        HashMap::new(),
        state_dir.join("layout-auto-exclusions.json"),
        spatial,
        scenes,
        SceneTransactionQueue::default(),
        state_dir.join("runtime-state.json"),
    );
    runtime.runtime.binding_migration = Arc::new(DeviceBindingMigrationContext::new(
        runtime.runtime.layout.clone(),
        Arc::clone(&runtime.runtime.logical_devices),
        state_dir.join("logical-devices.json"),
        Arc::clone(&runtime.runtime.attachment_profiles),
        runtime.runtime.device_settings.clone(),
        Arc::new(RwLock::new(
            DisplayPreferencesStore::new(state_dir.join("display-preferences.json"))
                .expect("display preference store"),
        )),
        state_dir.join("device-binding-migration.json"),
    ));
}

fn discovery_config(generation: u64, mdns_enabled: bool) -> HypercolorConfig {
    let mut config = HypercolorConfig::default();
    config.discovery.mdns_enabled = mdns_enabled;
    config.drivers.insert(
        BLOCKING_CONFIG_DISCOVERY_DRIVER.id.to_owned(),
        DriverConfigEntry::enabled(BTreeMap::from([(
            "generation".to_owned(),
            serde_json::json!(generation),
        )])),
    );
    config
}

#[tokio::test]
async fn queued_scan_uses_newest_config_when_active_owner_finishes() {
    let temp_dir = tempfile::tempdir().expect("tempdir should be created");
    let config_manager = Arc::new(
        ConfigManager::new(temp_dir.path().join("config.toml"))
            .expect("test config manager should load"),
    );
    config_manager.update(discovery_config(1, false));

    let observations = Arc::new(StdMutex::new(Vec::new()));
    let started = Arc::new(Semaphore::new(0));
    let release_first = Arc::new(Semaphore::new(0));
    let mut driver_registry = DriverModuleRegistry::new();
    driver_registry
        .register(BlockingConfigDiscoveryDriver {
            observations: Arc::clone(&observations),
            started: Arc::clone(&started),
            release_first: Arc::clone(&release_first),
        })
        .expect("test discovery driver should register");

    let runtime = make_runtime_with_registry(
        DeviceRegistry::new(),
        Arc::new(Mutex::new(DeviceLifecycleManager::new())),
        temp_dir.path().join("layouts.json"),
        temp_dir.path().join("runtime-state.json"),
        Some(driver_registry),
        Some(Arc::clone(&config_manager)),
    );
    runtime.runtime.in_progress.store(false, Ordering::Release);
    let target = DiscoveryTarget::driver(BLOCKING_CONFIG_DISCOVERY_DRIVER.id);
    let initial_config = Arc::clone(&config_manager.get());

    schedule_discovery_scan(
        runtime.runtime.clone(),
        Arc::clone(&runtime.driver_registry),
        Arc::clone(&runtime.driver_host),
        Arc::clone(&initial_config),
        vec![target.clone()],
        Duration::from_secs(2),
    );
    timeout(Duration::from_secs(1), started.acquire())
        .await
        .expect("first scan should start")
        .expect("discovery start semaphore should stay open")
        .forget();

    schedule_discovery_scan(
        runtime.runtime.clone(),
        Arc::clone(&runtime.driver_registry),
        Arc::clone(&runtime.driver_host),
        initial_config,
        vec![target.clone(), target],
        Duration::from_millis(700),
    );
    config_manager.update(discovery_config(2, true));
    release_first.add_permits(1);

    timeout(Duration::from_secs(1), started.acquire())
        .await
        .expect("queued scan should start after ownership transfer")
        .expect("discovery start semaphore should stay open")
        .forget();
    timeout(Duration::from_secs(1), async {
        while runtime.runtime.in_progress.load(Ordering::Acquire) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("queued scan should complete and release discovery ownership");

    let observations = observations
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    assert_eq!(
        observations,
        vec![
            DiscoveryConfigObservation {
                generation: 1,
                mdns_enabled: false,
                timeout: Duration::from_secs(2),
            },
            DiscoveryConfigObservation {
                generation: 2,
                mdns_enabled: true,
                timeout: Duration::from_millis(700),
            },
        ]
    );
}

#[tokio::test]
async fn execute_discovery_scan_if_idle_respects_existing_scan_owner() {
    let device_registry = DeviceRegistry::new();
    let lifecycle_manager = Arc::new(Mutex::new(DeviceLifecycleManager::new()));
    let temp_dir = tempfile::tempdir().expect("tempdir should be created");
    let runtime = make_runtime(
        device_registry,
        lifecycle_manager,
        temp_dir.path().join("layouts.json"),
        temp_dir.path().join("runtime-state.json"),
    );

    runtime.runtime.in_progress.store(false, Ordering::Release);
    let result = execute_discovery_scan_if_idle(
        runtime.runtime.clone(),
        Arc::clone(&runtime.driver_registry),
        Arc::clone(&runtime.driver_host),
        Arc::new(HypercolorConfig::default()),
        Vec::new(),
        Duration::from_millis(50),
    )
    .await;
    assert!(result.is_some(), "idle scan should be allowed to run");
    assert!(
        !runtime.runtime.in_progress.load(Ordering::Acquire),
        "completed scan should release the in-progress flag"
    );

    runtime.runtime.in_progress.store(true, Ordering::Release);
    let skipped = execute_discovery_scan_if_idle(
        runtime.runtime.clone(),
        Arc::clone(&runtime.driver_registry),
        Arc::clone(&runtime.driver_host),
        Arc::new(HypercolorConfig::default()),
        Vec::new(),
        Duration::from_millis(50),
    )
    .await;
    assert!(skipped.is_none(), "overlapping scan should be skipped");
    assert!(
        runtime.runtime.in_progress.load(Ordering::Acquire),
        "skipped scan must not clear another caller's in-progress flag"
    );
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
        let _ = lifecycle.on_discovered(device_id, &info, None);
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

    let result = execute_discovery_scan(
        runtime.runtime.clone(),
        Arc::clone(&runtime.driver_registry),
        Arc::clone(&runtime.driver_host),
        Arc::new(config),
        vec![DiscoveryTarget::driver("wled")],
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
async fn session_resume_targets_are_available_host_recovery_targets() {
    let temp_dir = tempfile::tempdir().expect("tempdir should be created");
    let runtime = make_runtime(
        DeviceRegistry::new(),
        Arc::new(Mutex::new(DeviceLifecycleManager::new())),
        temp_dir.path().join("layouts.json"),
        temp_dir.path().join("runtime-state.json"),
    );
    let targets = DiscoveryTarget::session_resume_targets(
        &HypercolorConfig::default(),
        runtime.driver_registry.as_ref(),
    )
    .expect("resume targets should resolve");
    let ids = targets
        .iter()
        .map(DiscoveryTarget::as_str)
        .collect::<Vec<_>>();

    #[cfg(any(target_os = "linux", target_os = "windows"))]
    assert_eq!(ids, vec!["smbus", "usb"]);
    #[cfg(target_os = "macos")]
    assert_eq!(ids, vec!["usb"]);
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    assert!(ids.is_empty());
    assert!(
        targets
            .iter()
            .find(|target| target.as_str() == "usb")
            .is_some_and(|target| !target.preserves_renderable_on_discovery_miss()),
        "USB resume scans should retire vanished devices after a clean scan"
    );
}

#[tokio::test]
async fn smbus_scan_does_not_timeout_connected_smbus_devices_on_transient_miss() {
    let device_registry = DeviceRegistry::new();
    let info = smbus_device_info("ASUS Aura DRAM (SMBus 0x71)");
    let fingerprint = DeviceFingerprint::from_persisted("smbus:/dev/i2c-999:71".to_owned());
    let mut metadata = HashMap::new();
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
        let _ = lifecycle.on_discovered(device_id, &info, Some(&fingerprint));
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
        vec![DiscoveryTarget::smbus()],
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
async fn complete_sweep_connects_device_after_cross_host_binding_migration() {
    let mut info = smbus_device_info("ASUS Aura DRAM (SMBus 0x73)");
    info.segments[0].name = "Lighting".to_owned();
    info.origin.backend_id = "mock".to_owned();
    let fingerprint = DeviceFingerprint::from_persisted("smbus:asus:pawnio:i801:73".to_owned());
    let current_layout_device_id =
        DeviceLifecycleManager::canonical_layout_device_id(&info, Some(&fingerprint));
    assert_eq!(current_layout_device_id, "asus:pawnio:i801:73");

    let discovered = DiscoveredDevice {
        fingerprint,
        connect_behavior: DiscoveryConnectBehavior::AutoConnect,
        info,
        metadata: HashMap::new(),
        claim: None,
    };
    let device_id = discovered.info.id;
    let mut driver_registry = DriverModuleRegistry::new();
    driver_registry
        .register(StaticAsusDiscoveryDriver { device: discovered })
        .expect("static ASUS discovery driver should register");

    let lifecycle_manager = Arc::new(Mutex::new(DeviceLifecycleManager::new()));
    let temp_dir = tempfile::tempdir().expect("tempdir should be created");
    let runtime = make_runtime_with_registry_and_layout(
        DeviceRegistry::new(),
        Arc::clone(&lifecycle_manager),
        temp_dir.path().join("layouts.json"),
        temp_dir.path().join("runtime-state.json"),
        Some(driver_registry),
        None,
        legacy_asus_dram_layout(),
        HashSet::new(),
    );
    let connect_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    {
        let mut manager = runtime.backend_manager.lock().await;
        manager.register_backend(Arc::new(CountingBackend {
            expected_device_id: device_id,
            connect_count: Arc::clone(&connect_count),
            disconnect_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }));
    }

    let scan = execute_discovery_scan(
        runtime.runtime.clone(),
        Arc::clone(&runtime.driver_registry),
        Arc::clone(&runtime.driver_host),
        Arc::new(HypercolorConfig::default()),
        vec![DiscoveryTarget::driver("asus")],
        Duration::from_millis(50),
    );
    tokio::pin!(scan);
    let renderer = runtime.layout.layout_publication_test_executor();
    let result = timeout(Duration::from_secs(5), async {
        loop {
            tokio::select! {
                result = &mut scan => break result,
                () = tokio::time::sleep(Duration::from_millis(1)) => {
                    renderer
                        .execute_next_layout_publication()
                        .await
                        .expect("layout publication should succeed");
                }
            }
        }
    })
    .await
    .expect("discovery scan should not deadlock on layout publication");

    assert_eq!(result.new_devices.len(), 1);
    assert_eq!(
        runtime.layout.current().zones[0].device_id,
        current_layout_device_id,
        "the complete sweep should migrate the active layout binding"
    );
    assert_eq!(
        connect_count.load(std::sync::atomic::Ordering::Relaxed),
        1,
        "the migrated active layout should connect the device in the same sweep"
    );
    assert_eq!(
        lifecycle_manager.lock().await.state(device_id),
        Some(DeviceState::Connected)
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
        let _ = lifecycle.on_discovered(device_id, &info, None);
        lifecycle
            .on_connected(device_id)
            .expect("lifecycle should accept connected transition");
        lifecycle
            .layout_device_id_for(device_id)
            .map(ToOwned::to_owned)
            .expect("connected device should have a canonical layout ID")
    };

    let temp_dir = tempfile::tempdir().expect("tempdir should be created");
    let layouts_path = temp_dir.path().join("layouts.json");
    let runtime = make_runtime_with_layout(
        device_registry,
        lifecycle_manager,
        layouts_path.clone(),
        temp_dir.path().join("runtime-state.json"),
        empty_layout(),
        HashSet::from([layout_device_id.clone()]),
    );

    {
        let mut manager = runtime.backend_manager.lock().await;
        manager.map_device(layout_device_id.clone(), "usb", device_id);
    }
    sync_active_layout_for_renderable_devices(&runtime, None).await;

    let layout = runtime.layout.current();
    assert!(
        layout.zones.is_empty(),
        "excluded devices must not be reconciled back into the active layout"
    );

    let persisted_layouts = hypercolor_daemon::layout_store::load(&layouts_path)
        .expect("persisted layouts should remain readable");
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
        let _ = lifecycle.on_discovered(device_id, &info, None);
        lifecycle
            .on_connected(device_id)
            .expect("lifecycle should accept connected transition");
        lifecycle
            .layout_device_id_for(device_id)
            .map(ToOwned::to_owned)
            .expect("connected device should have a canonical layout ID")
    };

    let temp_dir = tempfile::tempdir().expect("tempdir should be created");
    let layouts_path = temp_dir.path().join("layouts.json");
    let runtime = make_runtime(
        device_registry,
        lifecycle_manager,
        layouts_path.clone(),
        temp_dir.path().join("runtime-state.json"),
    );

    {
        let mut manager = runtime.backend_manager.lock().await;
        manager.map_device(layout_device_id, "usb", device_id);
    }

    sync_active_layout_for_renderable_devices(&runtime, None).await;

    let layout = runtime.layout.current();
    assert!(
        layout.zones.is_empty(),
        "newly discovered devices must not be auto-adopted into the active layout"
    );

    let persisted_layouts = hypercolor_daemon::layout_store::load(&layouts_path)
        .expect("persisted layouts should remain readable");
    assert!(
        persisted_layouts.is_empty(),
        "discovery should not persist layout changes for unmapped devices"
    );
}

#[tokio::test]
async fn sync_active_layout_connectivity_keeps_layout_inactive_devices_disconnected() {
    let device_registry = DeviceRegistry::new();
    let info = mock_device_info();
    let fingerprint = DeviceFingerprint::from_persisted("mock:layout-device".to_owned());
    let device_id = device_registry
        .add_with_fingerprint(info.clone(), fingerprint)
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
        manager.register_backend(Arc::new(CountingBackend {
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
async fn sync_active_layout_connectivity_primes_backend_from_registry_metadata() {
    let device_registry = DeviceRegistry::new();
    let info = mock_device_info();
    let fingerprint = DeviceFingerprint::from_persisted("mock:cache-primed-device".to_owned());
    let metadata = HashMap::from([("descriptor".to_owned(), "cached".to_owned())]);
    let device_id = device_registry
        .add_with_fingerprint_and_metadata(info.clone(), fingerprint.clone(), metadata)
        .await;
    assert_eq!(device_id, info.id);

    let lifecycle_manager = Arc::new(Mutex::new(DeviceLifecycleManager::new()));
    let temp_dir = tempfile::tempdir().expect("tempdir should be created");
    let layout_device_id =
        DeviceLifecycleManager::canonical_layout_device_id(&info, Some(&fingerprint));
    let runtime = make_runtime_with_layout(
        device_registry,
        Arc::clone(&lifecycle_manager),
        temp_dir.path().join("layouts.json"),
        temp_dir.path().join("runtime-state.json"),
        layout_with_device(&layout_device_id),
        HashSet::new(),
    );

    let adopt_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let connect_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    {
        let mut manager = runtime.backend_manager.lock().await;
        manager.register_backend(Arc::new(CachePrimingBackend {
            expected_device_id: device_id,
            expected_fingerprint: fingerprint.clone(),
            cached: AtomicBool::new(false),
            adopt_count: Arc::clone(&adopt_count),
            connect_count: Arc::clone(&connect_count),
        }));
    }

    sync_active_layout_connectivity(&runtime, None).await;

    assert_eq!(
        adopt_count.load(std::sync::atomic::Ordering::Relaxed),
        1,
        "backend should adopt canonical discovery metadata before connect"
    );
    assert_eq!(
        connect_count.load(std::sync::atomic::Ordering::Relaxed),
        1,
        "primed backend should connect once"
    );
    assert_eq!(
        lifecycle_manager.lock().await.state(device_id),
        Some(DeviceState::Connected)
    );
}

#[tokio::test]
async fn sync_active_layout_connectivity_disconnects_devices_removed_from_layout() {
    let device_registry = DeviceRegistry::new();
    let info = mock_device_info();
    let fingerprint = DeviceFingerprint::from_persisted("mock:layout-device".to_owned());
    let device_id = device_registry
        .add_with_fingerprint(info.clone(), fingerprint.clone())
        .await;
    assert_eq!(device_id, info.id);

    let lifecycle_manager = Arc::new(Mutex::new(DeviceLifecycleManager::new()));
    let temp_dir = tempfile::tempdir().expect("tempdir should be created");
    let layout_device_id =
        DeviceLifecycleManager::canonical_layout_device_id(&info, Some(&fingerprint));
    let mut runtime = make_runtime_with_layout(
        device_registry,
        Arc::clone(&lifecycle_manager),
        temp_dir.path().join("layouts.json"),
        temp_dir.path().join("runtime-state.json"),
        layout_with_device(&layout_device_id),
        HashSet::new(),
    );

    let connect_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let disconnect_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    {
        let mut manager = runtime.backend_manager.lock().await;
        manager.register_backend(Arc::new(CountingBackend {
            expected_device_id: device_id,
            connect_count: Arc::clone(&connect_count),
            disconnect_count: Arc::clone(&disconnect_count),
        }));
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

    install_active_layout(&mut runtime, empty_layout(), temp_dir.path());

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

#[tokio::test]
async fn sync_active_layout_connectivity_cleans_logical_routes_when_disconnect_fails() {
    let device_registry = DeviceRegistry::new();
    let info = mock_device_info();
    let fingerprint = DeviceFingerprint::from_persisted("mock:segmented-device".to_owned());
    let device_id = device_registry
        .add_with_fingerprint(info.clone(), fingerprint.clone())
        .await;
    assert_eq!(device_id, info.id);

    let lifecycle_manager = Arc::new(Mutex::new(DeviceLifecycleManager::new()));
    let temp_dir = tempfile::tempdir().expect("tempdir should be created");
    let physical_layout_id =
        DeviceLifecycleManager::canonical_layout_device_id(&info, Some(&fingerprint));
    let segment_layout_id = format!("{physical_layout_id}:segment");
    let mut runtime = make_runtime_with_layout(
        device_registry,
        Arc::clone(&lifecycle_manager),
        temp_dir.path().join("layouts.json"),
        temp_dir.path().join("runtime-state.json"),
        layout_with_device(&segment_layout_id),
        HashSet::new(),
    );

    let connect_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let disconnect_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    {
        let mut manager = runtime.backend_manager.lock().await;
        manager.register_backend(Arc::new(FailingDisconnectBackend {
            expected_device_id: device_id,
            connect_count: Arc::clone(&connect_count),
            disconnect_count: Arc::clone(&disconnect_count),
        }));
    }

    {
        let mut logical_devices = runtime.logical_devices.write().await;
        logical_devices.insert(
            segment_layout_id.clone(),
            LogicalDevice {
                id: segment_layout_id.clone(),
                physical_device_id: device_id,
                name: "Segment".to_owned(),
                led_start: 0,
                led_count: info.total_led_count(),
                enabled: true,
                kind: LogicalDeviceKind::Segment,
            },
        );
    }
    sync_active_layout_connectivity(&runtime, None).await;

    assert_eq!(
        connect_count.load(std::sync::atomic::Ordering::Relaxed),
        1,
        "active logical segment should connect the physical device"
    );

    {
        let layout = runtime.layout.current();
        let zone_colors = vec![ZoneColors {
            zone_id: "zone_main".to_owned(),
            colors: vec![[12, 34, 56]; usize::try_from(info.total_led_count()).unwrap_or_default()],
        }];
        let mut manager = runtime.backend_manager.lock().await;
        let stats = manager.write_frame(&zone_colors, &layout);
        assert_eq!(stats.devices_written, 1);
        assert_eq!(manager.mapped_device_count(), 1);
        assert_eq!(manager.debug_snapshot().queue_count, 1);
    }

    install_active_layout(&mut runtime, empty_layout(), temp_dir.path());

    sync_active_layout_connectivity(&runtime, None).await;

    assert_eq!(
        disconnect_count.load(std::sync::atomic::Ordering::Relaxed),
        1,
        "removing the logical segment from the active layout should disconnect the device"
    );
    assert_eq!(
        lifecycle_manager.lock().await.state(device_id),
        Some(DeviceState::Known)
    );
    {
        let manager = runtime.backend_manager.lock().await;
        assert_eq!(manager.mapped_device_count(), 0);
        assert_eq!(manager.debug_snapshot().queue_count, 0);
    }
}

#[tokio::test]
async fn sync_active_layout_connectivity_only_applies_host_attachment_profiles_for_opt_in_backends()
{
    let device_registry = DeviceRegistry::new();
    let info = prism_s_device_info_with_backend("mock");
    let fingerprint = DeviceFingerprint::from_persisted("usb:external-prism".to_owned());
    let device_id = device_registry
        .add_with_fingerprint(info.clone(), fingerprint.clone())
        .await;
    assert_eq!(device_id, info.id);

    let lifecycle_manager = Arc::new(Mutex::new(DeviceLifecycleManager::new()));
    let temp_dir = tempfile::tempdir().expect("tempdir should be created");
    let layout_device_id =
        DeviceLifecycleManager::canonical_layout_device_id(&info, Some(&fingerprint));
    let runtime = make_runtime_with_layout(
        device_registry,
        Arc::clone(&lifecycle_manager),
        temp_dir.path().join("layouts.json"),
        temp_dir.path().join("runtime-state.json"),
        layout_with_device(&layout_device_id),
        HashSet::new(),
    );

    let connect_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let disconnect_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    {
        let mut manager = runtime.backend_manager.lock().await;
        manager.register_backend(Arc::new(CountingBackend {
            expected_device_id: device_id,
            connect_count: Arc::clone(&connect_count),
            disconnect_count: Arc::clone(&disconnect_count),
        }));
    }

    sync_active_layout_connectivity(&runtime, None).await;

    assert_eq!(
        connect_count.load(std::sync::atomic::Ordering::Relaxed),
        1,
        "active layout targets should still connect through the custom backend"
    );
    assert!(
        runtime
            .usb_protocol_configs
            .config(device_id)
            .await
            .is_none(),
        "HAL USB protocol config must only be applied by backends that opt in"
    );
}
