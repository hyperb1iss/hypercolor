//! Daemon application composition root.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::AtomicBool;
#[cfg(any(target_os = "macos", test))]
use std::sync::atomic::AtomicU64;
#[cfg(test)]
use std::sync::atomic::Ordering;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use arc_swap::{ArcSwap, ArcSwapOption};
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use tracing::warn;

use hypercolor_core::asset::AssetLibrary;
use hypercolor_core::attachment::ComponentRegistry;
use hypercolor_core::bus::HypercolorBus;
use hypercolor_core::config::ConfigManager;
use hypercolor_core::device::{
    BackendManager, DeviceLifecycleManager, DeviceRegistry, UsbProtocolConfigStore,
};
use hypercolor_core::effect::EffectRegistry;
use hypercolor_core::engine::{FpsTier, RenderLoop};
use hypercolor_core::input::screen::ScreenCapacityStatusHandle;
use hypercolor_core::input::{InputManager, SourceStatusRegistry};
use hypercolor_core::scene::SceneManager;
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_driver_api::CredentialStore;
use hypercolor_network::DriverModuleRegistry;
use hypercolor_types::config::{HypercolorConfig, RenderAccelerationMode};
use hypercolor_types::device::DeviceId;
use hypercolor_types::server::ServerIdentity;
use hypercolor_types::spatial::SpatialLayout;

use crate::attachment_profiles::ComponentProfileStore;
use crate::device_metrics::{DeviceMetricsSnapshot, DeviceMetricsSnapshotStore};
use crate::device_settings::{DeviceSettingsAccess, DeviceSettingsStore};
use crate::display_frames::DisplayFrameRuntime;
use crate::display_preferences::DisplayPreferencesStore;
use crate::domain::context::{
    DeviceContext, DomainContextResources, DomainContexts, RuntimeSessionService, SceneContext,
};
use crate::domain::layout::LayoutContext;
use crate::domain::output::OutputContext;
use crate::domain::scene::SceneService;
use crate::domain::spatial::SpatialService;
use crate::driver_inventory::{DRIVER_INVENTORY_FILENAME, DriverInventoryStore};
use crate::extensions::{ApiExtension, ExtensionRegistry};
use crate::interaction_routing::InteractionRoutingControl;
use crate::layout_auto_exclusions;
use crate::library::{InMemoryLibraryStore, LibraryIdentityMigration, LibraryStore};
use crate::logical_devices::LogicalDevice;
use crate::network::{self, DaemonDriverHost};
use crate::output_power::OutputPower;
use crate::performance::PerformanceTracker;
use crate::playlist_runtime::PlaylistRuntimeState;
use crate::preview_runtime::PreviewRuntime;
use crate::render_thread::{ConfiguredFpsTier, InputPublicationDemandHandle};
use crate::scene_store::SceneStore;
use crate::scene_transactions::SceneTransactionQueue;
use crate::simulators::{SimulatedDisplayBackend, SimulatedDisplayRuntime, SimulatedDisplayStore};
use crate::zone_layout_preview::ZoneLayoutPreviewStore;

// ── AppState ─────────────────────────────────────────────────────────────

#[cfg(test)]
static APP_STATE_TEST_DATA_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

fn test_constructor_task_spawner() -> tokio::runtime::Handle {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        return handle;
    }

    static FALLBACK_RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    FALLBACK_RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .thread_name("hypercolor-test-runtime")
                .enable_all()
                .build()
                .expect("test AppState runtime should initialize")
        })
        .handle()
        .clone()
}

#[cfg(target_os = "macos")]
type CapturePickerPersistenceTask = Arc<StdMutex<Option<(u64, JoinHandle<()>)>>>;

/// Shared application state injected into every API handler.
///
/// All fields are wrapped in `Arc` or interior-mutable containers so
/// the state can be cloned cheaply across Axum's task pool.
///
/// The `scene_manager`, `render_loop`, and `event_bus` fields are
/// `Arc`-wrapped so they can be shared with the daemon's live instances
/// via [`from_daemon_state`](Self::from_daemon_state). This guarantees
/// that API calls operate on the same subsystems as the render pipeline.
pub struct AppState {
    /// Complete domain service graph assembled by the composition root.
    pub domains: DomainContexts,

    /// Device tracking and lifecycle management.
    pub device_registry: DeviceRegistry,

    /// Scene CRUD, priority stack, and transitions.
    pub scene_manager: SceneService,

    /// System-wide event bus (broadcast + watch channels).
    pub event_bus: Arc<HypercolorBus>,

    /// Latest durable macOS daemon ownership state.
    pub macos_daemon_ownership: Arc<ArcSwapOption<crate::macos_owner::MacosOwnerSnapshot>>,

    /// Daemon-managed user media asset library.
    pub asset_library: Arc<RwLock<AssetLibrary>>,

    /// Dedicated preview fanout for browser-facing canvas consumers.
    pub preview_runtime: Arc<PreviewRuntime>,

    /// Transient per-zone layout overrides driven by Studio drag previews.
    pub zone_layout_previews: Arc<ZoneLayoutPreviewStore>,

    /// Render loop — frame timing and pipeline skeleton.
    pub render_loop: Arc<RwLock<RenderLoop>>,

    /// Configured render FPS ceiling shared with the render thread.
    pub configured_max_fps_tier: ConfiguredFpsTier,

    /// Spatial sampling engine — maps canvas pixels to LED positions.
    pub spatial_engine: SpatialService,

    /// Device backend router — pushes colors to hardware.
    pub backend_manager: Arc<Mutex<BackendManager>>,

    /// Shared per-device USB protocol configuration for dynamic topologies.
    pub usb_protocol_configs: UsbProtocolConfigStore,

    /// Rolling render-performance snapshot shared with metrics endpoints.
    pub performance: Arc<RwLock<PerformanceTracker>>,

    /// Resolved compositor acceleration path exposed through status surfaces.
    pub(crate) render_acceleration: crate::startup::CompositorAccelerationResolution,

    /// Rolling per-device metrics snapshot shared with device metrics endpoints.
    pub device_metrics: DeviceMetricsSnapshotStore,

    /// Device lifecycle state/action orchestration.
    pub lifecycle_manager: Arc<Mutex<DeviceLifecycleManager>>,

    /// Active reconnect tasks keyed by device ID.
    pub reconnect_tasks: Arc<StdMutex<HashMap<DeviceId, JoinHandle<()>>>>,

    /// Configuration manager for config API endpoints.
    pub config_manager: Option<Arc<ConfigManager>>,

    /// Data directory backing state-owned stores and caches.
    pub data_dir: PathBuf,

    /// Typed state owned by downstream daemon extensions.
    pub extensions: ExtensionRegistry,

    /// API route mounters owned by downstream daemon extensions.
    pub api_extensions: Vec<Arc<dyn ApiExtension>>,

    /// Live input graph shared with the daemon render thread.
    pub input_manager: Arc<Mutex<InputManager>>,

    /// Exact lock-free screen capacity policy and physical usage.
    pub screen_capacity_status: ScreenCapacityStatusHandle,

    /// Monotonic request order for macOS picker-persistence observers.
    #[cfg(target_os = "macos")]
    pub(crate) capture_picker_request_epoch: Arc<AtomicU64>,

    /// Latest macOS picker-persistence observer, fenced by request order.
    #[cfg(target_os = "macos")]
    pub(crate) capture_picker_persistence_task: CapturePickerPersistenceTask,

    /// Aggregate typed input demand shared with render and connection consumers.
    pub input_publication_demands: InputPublicationDemandHandle,

    /// Active-renderer mailbox for explicit macOS screen parity snapshots.
    #[cfg(all(target_os = "macos", feature = "wgpu", feature = "screen-capture"))]
    pub(crate) macos_screen_parity_diagnostics:
        Option<crate::render_thread::MacosScreenParityDiagnosticHandle>,

    /// Lock-free latest-value health for the live input graph.
    pub input_status: SourceStatusRegistry,

    /// Push handle for browser-preview input injection over WebSocket.
    pub browser_input: hypercolor_core::input::BrowserInputHandle,

    /// Coherent interaction policies and authoritative browser ownership.
    pub interaction_routing: InteractionRoutingControl,

    /// Global discovery scan lock flag shared across startup/API entrypoints.
    pub discovery_in_progress: Arc<AtomicBool>,

    /// Attachment template registry (built-in plus user templates).
    pub attachment_registry: Arc<RwLock<ComponentRegistry>>,

    /// Persistent per-device attachment profile store.
    pub attachment_profiles: Arc<RwLock<ComponentProfileStore>>,

    /// Per-display default face preferences (spec 69 §3.6).
    pub display_preferences: Arc<RwLock<DisplayPreferencesStore>>,

    /// Persistent per-device user settings store.
    pub device_settings: DeviceSettingsAccess,

    /// Persisted virtual display simulator definitions.
    pub simulated_displays: Arc<RwLock<SimulatedDisplayStore>>,

    /// Latest captured simulator frames for inspection surfaces.
    pub simulated_display_runtime: Arc<RwLock<SimulatedDisplayRuntime>>,

    /// Latest composited display frames captured per device for preview surfaces.
    pub display_frames: Arc<RwLock<DisplayFrameRuntime>>,

    /// Shared encrypted credential store for driver-authenticated backends.
    pub credential_store: Arc<CredentialStore>,

    /// Narrow host adapter shared with built-in driver modules.
    pub driver_host: Arc<DaemonDriverHost>,

    /// Registry of compiled-in driver modules and capabilities.
    pub driver_registry: Arc<DriverModuleRegistry>,

    /// In-memory layout store (shared with `DaemonState`, persisted to layouts.json).
    pub layouts: Arc<RwLock<HashMap<String, SpatialLayout>>>,

    #[cfg(feature = "persistence-test-hooks")]
    #[doc(hidden)]
    pub layout_mutation_test_hooks: crate::api::layouts::LayoutMutationTestHooks,

    /// Persistent path for spatial layouts.
    pub layouts_path: PathBuf,

    /// Discovery auto-sync exclusions keyed by legacy layout or scene zone.
    pub layout_auto_exclusions: Arc<RwLock<layout_auto_exclusions::LayoutAutoExclusionStore>>,

    /// Persistent path for discovery auto-sync exclusions.
    pub layout_auto_exclusions_path: PathBuf,

    /// Logical device segmentation store (physical device -> logical ranges).
    pub logical_devices: Arc<RwLock<HashMap<String, LogicalDevice>>>,

    /// Persistent path for user-defined logical segment devices.
    pub logical_devices_path: PathBuf,

    /// Persisted path for startup runtime-session restoration.
    pub runtime_state_path: PathBuf,

    /// Canonical global output power and brightness authority.
    pub output_power: OutputPower,

    /// Frame-boundary scene changes mirrored into the render thread.
    pub scene_transactions: SceneTransactionQueue,

    /// Saved effect library storage (favorites, presets, playlists).
    pub library_store: Arc<dyn LibraryStore>,

    pub(crate) library_identity: Arc<dyn LibraryIdentityMigration>,

    /// Active playlist runner state (single background worker at a time).
    pub playlist_runtime: Arc<Mutex<PlaylistRuntimeState>>,

    /// Daemon start time for uptime calculation.
    pub start_time: Instant,

    /// Stable network identity exposed by API and discovery surfaces.
    pub server_identity: ServerIdentity,

    /// Current daemon process session identifier, when one was attested.
    pub server_session_id: Option<String>,

    /// Shared API auth and rate-limiting state for HTTP and WS command dispatch.
    pub security_state: crate::api::security::SecurityState,
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) const fn effect_renderer_acceleration_mode(
    requested_mode: RenderAccelerationMode,
) -> RenderAccelerationMode {
    match requested_mode {
        RenderAccelerationMode::Gpu => RenderAccelerationMode::Cpu,
        mode => mode,
    }
}

impl AppState {
    /// Create a new `AppState` with default empty subsystems.
    ///
    /// Primarily useful for testing. In production, prefer
    /// [`from_daemon_state`](Self::from_daemon_state) to share subsystems
    /// with the daemon lifecycle.
    pub fn new() -> Self {
        #[cfg(test)]
        {
            let id = APP_STATE_TEST_DATA_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
            Self::new_with_data_dir(
                std::env::temp_dir()
                    .join("hypercolor-app-state-tests")
                    .join(format!("{}-{id}", std::process::id())),
            )
        }

        #[cfg(not(test))]
        {
            Self::new_with_data_dir(ConfigManager::data_dir())
        }
    }

    #[doc(hidden)]
    pub fn new_with_data_dir(data_dir: PathBuf) -> Self {
        Self::new_with_runtime_overrides(data_dir, None, None)
    }

    #[doc(hidden)]
    pub fn new_with_runtime_overrides(
        data_dir: PathBuf,
        config_manager: Option<Arc<ConfigManager>>,
        driver_registry: Option<Arc<DriverModuleRegistry>>,
    ) -> Self {
        Self::new_with_composition_overrides(data_dir, config_manager, driver_registry, None)
    }

    #[expect(
        clippy::too_many_lines,
        reason = "test-facing app state construction wires all shared subsystems in one place"
    )]
    #[doc(hidden)]
    pub fn new_with_composition_overrides(
        data_dir: PathBuf,
        config_manager: Option<Arc<ConfigManager>>,
        driver_registry: Option<Arc<DriverModuleRegistry>>,
        runtime_state_path: Option<PathBuf>,
    ) -> Self {
        use hypercolor_types::spatial::{EdgeBehavior, SamplingMode, SpatialLayout};

        let config = config_manager
            .as_ref()
            .map_or_else(HypercolorConfig::default, |manager| {
                manager.get().as_ref().clone()
            });

        let default_layout = SpatialLayout {
            id: "default".into(),
            name: "Default Layout".into(),
            description: None,
            canvas_width: 320,
            canvas_height: 200,
            zones: Vec::new(),
            default_sampling_mode: SamplingMode::Bilinear,
            default_edge_behavior: EdgeBehavior::Clamp,
            spaces: None,
            version: 1,
        };

        let mut attachment_registry = ComponentRegistry::new();
        if let Err(error) = attachment_registry.load_builtins() {
            warn!(%error, "Failed to load built-in attachment templates");
        }

        let attachment_templates_dir = data_dir.join("attachments");
        if let Err(error) = attachment_registry.load_user_dir(&attachment_templates_dir) {
            warn!(
                path = %attachment_templates_dir.display(),
                %error,
                "Failed to load user attachment templates"
            );
        }

        let attachment_profiles_path = data_dir.join("attachment-profiles.json");
        let attachment_profiles = ComponentProfileStore::load(&attachment_profiles_path)
            .unwrap_or_else(|error| {
                warn!(
                    path = %attachment_profiles_path.display(),
                    %error,
                    "Failed to load attachment profiles; starting with empty store"
                );
                ComponentProfileStore::new(attachment_profiles_path)
            });
        let device_settings_path = data_dir.join("device-settings.json");
        let device_settings =
            DeviceSettingsStore::load(&device_settings_path).unwrap_or_else(|error| {
                warn!(
                    path = %device_settings_path.display(),
                    %error,
                    "Failed to load device settings; starting with defaults"
                );
                DeviceSettingsStore::new(device_settings_path)
            });
        let output_power = OutputPower::new(device_settings);
        let device_settings = output_power.device_settings();
        let simulated_displays_path = data_dir.join("simulated-displays.json");
        let simulated_displays = SimulatedDisplayStore::load(&simulated_displays_path)
            .unwrap_or_else(|error| {
                warn!(
                    path = %simulated_displays_path.display(),
                    %error,
                    "Failed to load simulated displays; starting with empty store"
                );
                SimulatedDisplayStore::new(simulated_displays_path)
            });
        let credential_store = Arc::new(
            CredentialStore::open_blocking(&data_dir)
                .expect("default app state should open credential store"),
        );
        let device_registry = DeviceRegistry::new();
        let effect_registry = Arc::new(RwLock::new(EffectRegistry::new(vec![
            data_dir.join("effects"),
        ])));
        let scenes_path = data_dir.join("scenes.json");
        let scene_store = SceneStore::load(&scenes_path)
            .expect("default app state should load scene persistence");
        let mut scene_manager_inner = SceneManager::with_default_layout(default_layout.clone());
        for scene in scene_store.list().cloned() {
            if let Err(error) = scene_manager_inner.create(scene) {
                warn!(%error, "Failed to install persisted named scene into default app state");
            }
        }
        let scene_store = Arc::new(RwLock::new(scene_store));
        let event_bus = Arc::new(HypercolorBus::new());
        let zone_layout_previews = Arc::new(ZoneLayoutPreviewStore::default());
        let scene_manager = SceneService::new(
            scene_manager_inner,
            Arc::clone(&event_bus),
            Arc::clone(&scene_store),
            Arc::clone(&zone_layout_previews),
        );
        let scene_transactions = SceneTransactionQueue::default();
        let asset_library = Arc::new(RwLock::new(
            AssetLibrary::open(data_dir.join("assets"))
                .expect("default app state should open asset library"),
        ));
        let preview_runtime = Arc::new(PreviewRuntime::new(Arc::clone(&event_bus)));
        let render_loop = Arc::new(RwLock::new(RenderLoop::new(60)));
        let configured_max_fps_tier = ConfiguredFpsTier::new(FpsTier::Full);
        let spatial_engine = SpatialService::new(
            SpatialEngine::try_new(default_layout)
                .expect("empty default spatial layout should always be addressable"),
        );
        let backend_manager = Arc::new(Mutex::new(BackendManager::new()));
        let usb_protocol_configs = UsbProtocolConfigStore::new();
        let performance = Arc::new(RwLock::new(PerformanceTracker::default()));
        let device_metrics = Arc::new(ArcSwap::from_pointee(DeviceMetricsSnapshot::default()));
        let lifecycle_manager = Arc::new(Mutex::new(DeviceLifecycleManager::new()));
        let reconnect_tasks = Arc::new(StdMutex::new(HashMap::new()));
        let browser_input_source = hypercolor_core::input::BrowserInputSource::new();
        let browser_input = browser_input_source.handle();
        let interaction_routing = InteractionRoutingControl::new(
            browser_input.registry(),
            1,
            config.input.daemon_route,
            config.input.preview_route,
        );
        let mut standalone_input_manager = InputManager::new();
        standalone_input_manager.add_source(Box::new(browser_input_source));
        let input_status = standalone_input_manager.source_status_registry();
        let screen_capacity_status = standalone_input_manager.screen_capacity_status_handle();
        let input_manager = Arc::new(Mutex::new(standalone_input_manager));
        let discovery_in_progress = Arc::new(AtomicBool::new(false));
        let attachment_registry = Arc::new(RwLock::new(attachment_registry));
        let attachment_profiles = Arc::new(RwLock::new(attachment_profiles));
        let display_preferences_path = data_dir.join("display-preferences.json");
        let display_preferences = Arc::new(RwLock::new(
            DisplayPreferencesStore::load(&display_preferences_path).unwrap_or_else(|_| {
                DisplayPreferencesStore::new(display_preferences_path)
                    .expect("display preference persistence should initialize")
            }),
        ));
        let simulated_displays = Arc::new(RwLock::new(simulated_displays));
        let simulated_display_runtime = Arc::new(RwLock::new(SimulatedDisplayRuntime::new()));
        let display_frames = Arc::new(RwLock::new(DisplayFrameRuntime::new()));
        let layouts = Arc::new(RwLock::new(HashMap::new()));
        let layouts_path = data_dir.join("layouts.json");
        let layout_auto_exclusions = Arc::new(RwLock::new(HashMap::new()));
        let layout_auto_exclusions_path = data_dir.join("layout-auto-exclusions.json");
        let logical_devices = Arc::new(RwLock::new(HashMap::new()));
        let logical_devices_path = data_dir.join("logical-devices.json");
        let runtime_state_path =
            runtime_state_path.unwrap_or_else(|| data_dir.join("runtime-state.json"));
        let device_aliases_path = data_dir.join(crate::device_aliases::DEVICE_ALIASES_FILE);
        let driver_inventory = Arc::new(
            DriverInventoryStore::open(data_dir.join(DRIVER_INVENTORY_FILENAME))
                .expect("default app state should open driver inventory"),
        );
        let driver_registry = driver_registry.unwrap_or_else(|| {
            Arc::new(
                network::build_builtin_driver_module_registry(
                    &config,
                    Arc::clone(&credential_store),
                    usb_protocol_configs.clone(),
                )
                .expect("default app state should build driver module registry"),
            )
        });
        let discovery_runtime = crate::discovery::DiscoveryRuntime {
            device_registry: device_registry.clone(),
            backend_manager: Arc::clone(&backend_manager),
            lifecycle_manager: Arc::clone(&lifecycle_manager),
            reconnect_tasks: Arc::clone(&reconnect_tasks),
            event_bus: Arc::clone(&event_bus),
            spatial_engine: spatial_engine.clone(),
            scene_manager: scene_manager.clone(),
            layouts: Arc::clone(&layouts),
            layouts_path: layouts_path.clone(),
            layout_auto_exclusions: Arc::clone(&layout_auto_exclusions),
            logical_devices: Arc::clone(&logical_devices),
            attachment_registry: Arc::clone(&attachment_registry),
            attachment_profiles: Arc::clone(&attachment_profiles),
            device_settings: device_settings.clone(),
            scene_transactions: scene_transactions.clone(),
            runtime_state_path: runtime_state_path.clone(),
            device_aliases_path,
            usb_protocol_configs: usb_protocol_configs.clone(),
            credential_store: Arc::clone(&credential_store),
            in_progress: Arc::clone(&discovery_in_progress),
            pending_scans: Arc::default(),
            task_spawner: test_constructor_task_spawner(),
        };
        let driver_host = Arc::new(DaemonDriverHost::new(
            discovery_runtime,
            driver_inventory,
            Arc::clone(&driver_registry),
            config_manager.clone(),
        ));
        {
            let mut manager = backend_manager.try_lock().expect(
                "default app state should register the simulator backend without contention",
            );
            manager.register_backend(Arc::new(SimulatedDisplayBackend::new(
                Arc::clone(&simulated_displays),
                Arc::clone(&simulated_display_runtime),
            )));
        }
        let runtime_session = RuntimeSessionService::new(
            runtime_state_path.clone(),
            scene_manager.clone(),
            spatial_engine.clone(),
            output_power.clone(),
            Arc::clone(&driver_host),
            Arc::clone(&driver_registry),
        );
        let devices = DeviceContext::new(
            device_registry.clone(),
            Arc::clone(&lifecycle_manager),
            Arc::clone(&driver_host),
            Arc::clone(&driver_registry),
            config_manager.clone(),
            Arc::clone(&layout_auto_exclusions),
            layout_auto_exclusions_path.clone(),
        );
        let scene = SceneContext::new(
            scene_manager.clone(),
            runtime_session.clone(),
            Arc::clone(&asset_library),
            config_manager.clone(),
            Arc::clone(&render_loop),
            devices.clone(),
        );
        let layout = LayoutContext::new(
            Arc::clone(&layouts),
            spatial_engine.clone(),
            scene_manager.clone(),
            scene_transactions.clone(),
            runtime_session.clone(),
            devices.clone(),
        );
        let start_time = Instant::now();
        let output = OutputContext::new(
            output_power.clone(),
            Arc::clone(&event_bus),
            runtime_session.clone(),
            Arc::clone(&performance),
            Arc::clone(&render_loop),
            spatial_engine.clone(),
            Arc::clone(&backend_manager),
            Arc::clone(&preview_runtime),
            devices.clone(),
            start_time,
        );
        let domains = DomainContexts::assemble(
            runtime_session,
            devices,
            scene,
            layout,
            output,
            DomainContextResources {
                effect_registry: Arc::clone(&effect_registry),
                spatial: spatial_engine.clone(),
                event_bus: Arc::clone(&event_bus),
            },
        );

        let library = Arc::new(InMemoryLibraryStore::new());
        let library_store: Arc<dyn LibraryStore> = library.clone();
        let library_identity: Arc<dyn LibraryIdentityMigration> = library;

        Self {
            domains,
            device_registry,
            scene_manager,
            event_bus,
            macos_daemon_ownership: Arc::new(ArcSwapOption::empty()),
            asset_library,
            preview_runtime,
            zone_layout_previews,
            render_loop,
            configured_max_fps_tier,
            spatial_engine,
            backend_manager,
            usb_protocol_configs,
            performance,
            render_acceleration: crate::startup::cpu_compositor_acceleration_resolution(),
            device_metrics,
            lifecycle_manager,
            reconnect_tasks,
            config_manager,
            data_dir,
            extensions: ExtensionRegistry::default(),
            api_extensions: Vec::new(),
            input_manager,
            screen_capacity_status,
            #[cfg(target_os = "macos")]
            capture_picker_request_epoch: Arc::new(AtomicU64::new(0)),
            #[cfg(target_os = "macos")]
            capture_picker_persistence_task: Arc::new(StdMutex::new(None)),
            input_publication_demands: InputPublicationDemandHandle::new(),
            #[cfg(all(target_os = "macos", feature = "wgpu", feature = "screen-capture"))]
            macos_screen_parity_diagnostics: None,
            input_status,
            browser_input,
            interaction_routing,
            discovery_in_progress,
            attachment_registry,
            attachment_profiles,
            display_preferences,
            device_settings,
            simulated_displays,
            simulated_display_runtime,
            display_frames,
            credential_store,
            driver_host,
            driver_registry,
            layouts,
            #[cfg(feature = "persistence-test-hooks")]
            layout_mutation_test_hooks: crate::api::layouts::LayoutMutationTestHooks::default(),
            layouts_path,
            layout_auto_exclusions,
            layout_auto_exclusions_path,
            logical_devices,
            logical_devices_path,
            runtime_state_path,
            output_power,
            scene_transactions,
            library_store,
            library_identity,
            playlist_runtime: Arc::new(Mutex::new(PlaylistRuntimeState::new())),
            start_time,
            server_identity: ServerIdentity {
                instance_id: "00000000-0000-7000-8000-000000000000".to_owned(),
                instance_name: "hypercolor".to_owned(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
            },
            server_session_id: None,
            security_state: crate::api::security::SecurityState::from_config(&config),
        }
    }

    /// Create an `AppState` from a live [`DaemonState`](crate::startup::DaemonState).
    ///
    /// The device registry is cloned (it's internally `Arc`-wrapped).
    /// The scene manager, render loop, and event bus are
    /// shared by `Arc::clone` — the API operates on the exact same live
    /// instances as the daemon's render pipeline.
    pub fn from_daemon_state(daemon: &crate::startup::DaemonState) -> Self {
        let data_dir = ConfigManager::data_dir();
        let library_store = Arc::clone(&daemon.library_store);
        let library_identity = Arc::clone(&daemon.library_identity);
        let driver_host = Arc::clone(&daemon.driver_host);
        let driver_registry = Arc::clone(&daemon.driver_registry);
        let domains = daemon.domains.clone();

        Self {
            domains,
            device_registry: daemon.device_registry.clone(),
            scene_manager: daemon.scene_manager.clone(),
            event_bus: Arc::clone(&daemon.event_bus),
            macos_daemon_ownership: Arc::clone(&daemon.macos_daemon_ownership),
            asset_library: Arc::clone(&daemon.asset_library),
            preview_runtime: Arc::clone(&daemon.preview_runtime),
            zone_layout_previews: Arc::clone(&daemon.zone_layout_previews),
            render_loop: Arc::clone(&daemon.render_loop),
            configured_max_fps_tier: daemon.configured_max_fps_tier.clone(),
            spatial_engine: daemon.spatial_engine.clone(),
            backend_manager: Arc::clone(&daemon.backend_manager),
            usb_protocol_configs: daemon.usb_protocol_configs.clone(),
            performance: Arc::clone(&daemon.performance),
            render_acceleration: daemon.render_acceleration.clone(),
            device_metrics: Arc::clone(&daemon.device_metrics),
            lifecycle_manager: Arc::clone(&daemon.lifecycle_manager),
            reconnect_tasks: Arc::clone(&daemon.reconnect_tasks),
            config_manager: Some(Arc::clone(&daemon.config_manager)),
            data_dir,
            extensions: daemon.extensions.clone(),
            api_extensions: daemon.api_extensions.clone(),
            input_manager: Arc::clone(&daemon.input_manager),
            screen_capacity_status: daemon.screen_capacity_status.clone(),
            #[cfg(target_os = "macos")]
            capture_picker_request_epoch: Arc::new(AtomicU64::new(0)),
            #[cfg(target_os = "macos")]
            capture_picker_persistence_task: Arc::new(StdMutex::new(None)),
            input_publication_demands: daemon
                .input_publication_demands()
                .expect("live API state requires a running input publication pump"),
            #[cfg(all(target_os = "macos", feature = "wgpu", feature = "screen-capture"))]
            macos_screen_parity_diagnostics: daemon.macos_screen_parity_diagnostics(),
            input_status: daemon.input_status.clone(),
            browser_input: daemon.browser_input.clone(),
            interaction_routing: daemon.interaction_routing.clone(),
            discovery_in_progress: Arc::clone(&daemon.discovery_in_progress),
            attachment_registry: Arc::clone(&daemon.attachment_registry),
            attachment_profiles: Arc::clone(&daemon.attachment_profiles),
            display_preferences: Arc::clone(&daemon.display_preferences),
            device_settings: daemon.device_settings.clone(),
            simulated_displays: Arc::clone(&daemon.simulated_displays),
            simulated_display_runtime: Arc::clone(&daemon.simulated_display_runtime),
            display_frames: Arc::clone(&daemon.display_frames),
            credential_store: Arc::clone(&daemon.credential_store),
            driver_host,
            driver_registry,
            layouts: Arc::clone(&daemon.layouts),
            #[cfg(feature = "persistence-test-hooks")]
            layout_mutation_test_hooks: crate::api::layouts::LayoutMutationTestHooks::default(),
            layouts_path: daemon.layouts_path.clone(),
            layout_auto_exclusions: Arc::clone(&daemon.layout_auto_exclusions),
            layout_auto_exclusions_path: daemon.layout_auto_exclusions_path.clone(),
            logical_devices: Arc::clone(&daemon.logical_devices),
            logical_devices_path: daemon.logical_devices_path.clone(),
            runtime_state_path: daemon.runtime_state_path.clone(),
            output_power: daemon.output_power.clone(),
            scene_transactions: daemon.scene_transactions.clone(),
            library_store,
            library_identity,
            playlist_runtime: Arc::clone(&daemon.playlist_runtime),
            start_time: daemon.start_time,
            server_identity: daemon.server_identity.clone(),
            server_session_id: None,
            security_state: crate::api::security::SecurityState::from_config(
                &daemon.config_manager.get(),
            ),
        }
    }

    pub(crate) fn install_macos_daemon_session(
        &mut self,
        attestation: &crate::macos_owner::MacosDaemonSessionAttestation,
    ) {
        self.server_session_id = Some(attestation.server_session_id.as_str().to_owned());
        self.security_state
            .install_macos_daemon_session(attestation);
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use hypercolor_types::config::RenderAccelerationMode;

    use super::{AppState, effect_renderer_acceleration_mode};

    #[test]
    fn test_app_state_constructs_without_an_ambient_runtime() {
        let _state = AppState::new();
    }

    #[test]
    fn effect_renderer_mode_keeps_cpu_and_auto_requests() {
        assert_eq!(
            effect_renderer_acceleration_mode(RenderAccelerationMode::Cpu),
            RenderAccelerationMode::Cpu
        );
        assert_eq!(
            effect_renderer_acceleration_mode(RenderAccelerationMode::Auto),
            RenderAccelerationMode::Auto
        );
    }

    #[test]
    fn effect_renderer_mode_downgrades_gpu_requests_to_cpu() {
        assert_eq!(
            effect_renderer_acceleration_mode(RenderAccelerationMode::Gpu),
            RenderAccelerationMode::Cpu
        );
    }
}
