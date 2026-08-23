//! Daemon application composition root.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
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
use hypercolor_core::input::InputManager;
use hypercolor_core::input::screen::ScreenCapacityStatusHandle;
use hypercolor_core::scene::SceneManager;
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_driver_support::CredentialStore;
use hypercolor_network::DriverModuleRegistry;
use hypercolor_types::config::{HypercolorConfig, RenderAccelerationMode};
use hypercolor_types::server::ServerIdentity;

use crate::attachment_profiles::ComponentProfileStore;
use crate::device_metrics::{DeviceMetricsSnapshot, DeviceMetricsSnapshotStore};
use crate::device_settings::{DeviceSettingsAccess, DeviceSettingsStore};
use crate::display_frames::DisplayFrameRuntime;
use crate::display_preferences::DisplayPreferencesStore;
use crate::domain::context::DomainContexts;
use crate::domain::effect::EffectIdentityResources;
use crate::domain::layout::LayoutContextResources;
use crate::domain::scene::SceneService;
use crate::domain::spatial::SpatialService;
use crate::driver_inventory::{DRIVER_INVENTORY_FILENAME, DriverInventoryStore};
use crate::extensions::{ApiExtension, ExtensionRegistry};
use crate::interaction_routing::InteractionRoutingControl;
use crate::library::{
    InMemoryLibraryStore, JsonLibraryStore, LibraryIdentityMigration, LibraryStore,
};
use crate::logical_devices::LogicalDevice;
use crate::network::{self, DaemonDriverHost};
use crate::output_power::OutputPower;
use crate::performance::PerformanceTracker;
use crate::playlist_runtime::PlaylistRuntimeState;
use crate::preview_runtime::PreviewRuntime;
use crate::render_thread::{ConfiguredFpsTier, InputPublicationDemandHandle};
use crate::scene_store::SceneStore;
#[cfg(feature = "persistence-test-hooks")]
use crate::scene_transactions::LayoutPublicationTestExecutor;
use crate::scene_transactions::SceneTransactionQueue;
use crate::simulators::{SimulatedDisplayBackend, SimulatedDisplayRuntime, SimulatedDisplayStore};
use crate::startup::services::{AssembledDomains, DomainAssemblyResources, assemble_domains};
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

    /// Configuration manager for config API endpoints.
    pub(crate) config_manager: Option<Arc<ConfigManager>>,

    /// Data directory backing state-owned stores and caches.
    pub data_dir: PathBuf,

    /// State directory backing the stores that survive as machine-local state.
    pub state_dir: PathBuf,

    /// Typed state owned by downstream daemon extensions.
    pub extensions: ExtensionRegistry,

    /// API route mounters owned by downstream daemon extensions.
    pub api_extensions: Vec<Arc<dyn ApiExtension>>,

    /// Live input graph shared with the daemon render thread.
    input_manager: Arc<Mutex<InputManager>>,

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

    /// Persistent per-device user settings store.
    pub device_settings: DeviceSettingsAccess,

    /// Persisted virtual display simulator definitions.
    pub simulated_displays: Arc<RwLock<SimulatedDisplayStore>>,

    /// Latest captured simulator frames for inspection surfaces.
    pub simulated_display_runtime: Arc<RwLock<SimulatedDisplayRuntime>>,

    /// Narrow host adapter shared with built-in driver modules.
    driver_host: Arc<DaemonDriverHost>,

    /// Registry of compiled-in driver modules and capabilities.
    driver_registry: Arc<DriverModuleRegistry>,

    /// Logical device segmentation store (physical device -> logical ranges).
    pub logical_devices: Arc<RwLock<HashMap<String, LogicalDevice>>>,

    /// Persistent path for user-defined logical segment devices.
    pub logical_devices_path: PathBuf,

    /// Persisted path for startup runtime-session restoration.
    pub runtime_state_path: PathBuf,

    /// Canonical global output power and brightness authority.
    pub output_power: OutputPower,

    /// Frame-boundary scene changes mirrored into the render thread.
    pub(crate) scene_transactions: SceneTransactionQueue,

    /// Saved effect library storage (favorites, presets, playlists).
    library_store: Arc<dyn LibraryStore>,

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

struct AppStateLibrary {
    store: Arc<dyn LibraryStore>,
    identity: Arc<dyn LibraryIdentityMigration>,
}

/// Test-facing composition builder for an isolated application state.
///
/// Every authority is selected before the domain graph is assembled, so
/// fixtures cannot leave cloned contexts pointing at superseded dependencies.
#[doc(hidden)]
pub struct AppStateBuilder {
    data_dir: PathBuf,
    state_dir: PathBuf,
    config_manager: Option<Arc<ConfigManager>>,
    driver_registry: Option<Arc<DriverModuleRegistry>>,
    runtime_state_path: Option<PathBuf>,
    library: Option<AppStateLibrary>,
    input_manager: Option<InputManager>,
}

impl AppStateBuilder {
    #[must_use]
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            state_dir: default_state_dir(&data_dir),
            data_dir,
            config_manager: None,
            driver_registry: None,
            runtime_state_path: None,
            library: None,
            input_manager: None,
        }
    }

    #[must_use]
    pub fn with_state_dir(mut self, state_dir: PathBuf) -> Self {
        self.state_dir = state_dir;
        self
    }

    #[must_use]
    pub fn with_config_manager(mut self, config_manager: Arc<ConfigManager>) -> Self {
        self.config_manager = Some(config_manager);
        self
    }

    #[must_use]
    pub fn with_driver_registry(mut self, driver_registry: Arc<DriverModuleRegistry>) -> Self {
        self.driver_registry = Some(driver_registry);
        self
    }

    #[must_use]
    pub fn with_runtime_state_path(mut self, runtime_state_path: PathBuf) -> Self {
        self.runtime_state_path = Some(runtime_state_path);
        self
    }

    #[must_use]
    pub fn with_library(mut self, library: Arc<JsonLibraryStore>) -> Self {
        let store: Arc<dyn LibraryStore> = library.clone();
        let identity: Arc<dyn LibraryIdentityMigration> = library;
        self.library = Some(AppStateLibrary { store, identity });
        self
    }

    #[must_use]
    pub fn with_input_manager(mut self, input_manager: InputManager) -> Self {
        self.input_manager = Some(input_manager);
        self
    }

    #[must_use]
    pub fn build(self) -> AppState {
        AppState::from_builder(self)
    }
}

/// The state tier is a sibling of the data tier for every fixture, so an
/// overridden data directory never reaches the real per-user state directory.
fn default_state_dir(data_dir: &Path) -> PathBuf {
    if data_dir == ConfigManager::data_dir() {
        ConfigManager::state_dir()
    } else {
        data_dir.join("state")
    }
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
        Self::builder().build()
    }

    #[doc(hidden)]
    #[must_use]
    pub fn builder() -> AppStateBuilder {
        #[cfg(test)]
        {
            let id = APP_STATE_TEST_DATA_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
            AppStateBuilder::new(
                std::env::temp_dir()
                    .join("hypercolor-app-state-tests")
                    .join(format!("{}-{id}", std::process::id())),
            )
        }

        #[cfg(not(test))]
        {
            AppStateBuilder::new(ConfigManager::data_dir())
        }
    }

    #[doc(hidden)]
    pub fn new_with_data_dir(data_dir: PathBuf) -> Self {
        AppStateBuilder::new(data_dir).build()
    }

    #[expect(
        clippy::too_many_lines,
        reason = "test-facing app state construction wires all shared subsystems in one place"
    )]
    fn from_builder(builder: AppStateBuilder) -> Self {
        use hypercolor_types::spatial::{EdgeBehavior, SamplingMode, SpatialLayout};

        let AppStateBuilder {
            data_dir,
            state_dir,
            config_manager,
            driver_registry,
            runtime_state_path,
            library,
            input_manager,
        } = builder;

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
        let device_settings_path = state_dir.join("device-settings.json");
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
        let event_bus = Arc::new(HypercolorBus::new());
        let zone_layout_previews = Arc::new(ZoneLayoutPreviewStore::default());
        let scene_manager = SceneService::new(
            scene_manager_inner,
            Arc::clone(&event_bus),
            scene_store,
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
            SpatialEngine::try_new(default_layout.clone())
                .expect("empty default spatial layout should always be addressable"),
        );
        let backend_manager = Arc::new(Mutex::new(BackendManager::new()));
        let usb_protocol_configs = UsbProtocolConfigStore::new();
        let performance = Arc::new(RwLock::new(PerformanceTracker::default()));
        let device_metrics = Arc::new(ArcSwap::from_pointee(DeviceMetricsSnapshot::default()));
        let lifecycle_manager = Arc::new(Mutex::new(DeviceLifecycleManager::new()));
        let reconnect_tasks = Arc::new(StdMutex::new(HashMap::new()));
        let browser_input = hypercolor_core::input::BrowserInputHandle::new();
        let interaction_routing = InteractionRoutingControl::new(
            browser_input.registry(),
            1,
            config.input.daemon_route,
            config.input.preview_route,
        );
        let standalone_input_manager = input_manager.unwrap_or_else(InputManager::new);
        let input_status = standalone_input_manager.source_status_registry();
        let screen_capacity_status = standalone_input_manager.screen_capacity_status_handle();
        let input_manager = Arc::new(Mutex::new(standalone_input_manager));
        let discovery_in_progress = Arc::new(AtomicBool::new(false));
        let attachment_registry = Arc::new(RwLock::new(attachment_registry));
        let attachment_profiles = Arc::new(RwLock::new(attachment_profiles));
        let display_preferences_path = state_dir.join("display-preferences.json");
        let display_preferences = Arc::new(RwLock::new(
            DisplayPreferencesStore::load(&display_preferences_path).unwrap_or_else(|_| {
                DisplayPreferencesStore::new(display_preferences_path)
                    .expect("display preference persistence should initialize")
            }),
        ));
        let simulated_displays = Arc::new(RwLock::new(simulated_displays));
        let simulated_display_runtime = Arc::new(RwLock::new(SimulatedDisplayRuntime::new()));
        let display_frames = Arc::new(RwLock::new(DisplayFrameRuntime::new()));
        let layouts = HashMap::from([(default_layout.id.clone(), default_layout)]);
        let layouts_path = data_dir.join("layouts.json");
        let layout_auto_exclusions = HashMap::new();
        let layout_auto_exclusions_path = data_dir.join("layout-auto-exclusions.json");
        let logical_devices = Arc::new(RwLock::new(HashMap::new()));
        let logical_devices_path = data_dir.join("logical-devices.json");
        let runtime_state_path =
            runtime_state_path.unwrap_or_else(|| state_dir.join("runtime-state.json"));
        let device_aliases_path = state_dir.join(crate::device_aliases::DEVICE_ALIASES_FILE);
        let driver_inventory = Arc::new(
            DriverInventoryStore::open(state_dir.join(DRIVER_INVENTORY_FILENAME))
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
        let AppStateLibrary {
            store: library_store,
            identity: library_identity,
        } = library.unwrap_or_else(|| {
            let library = Arc::new(InMemoryLibraryStore::new());
            let store: Arc<dyn LibraryStore> = library.clone();
            let identity: Arc<dyn LibraryIdentityMigration> = library;
            AppStateLibrary { store, identity }
        });
        let playlist_runtime = Arc::new(Mutex::new(PlaylistRuntimeState::new()));
        let start_time = Instant::now();
        let AssembledDomains {
            domains,
            driver_host,
        } = assemble_domains(
            DomainAssemblyResources {
                scene_manager: scene_manager.clone(),
                spatial_engine: spatial_engine.clone(),
                output_power: output_power.clone(),
                layout_resources: LayoutContextResources::new(
                    layouts,
                    layouts_path,
                    layout_auto_exclusions,
                    layout_auto_exclusions_path,
                ),
                scene_transactions: scene_transactions.clone(),
                runtime_state_path: runtime_state_path.clone(),
                config_manager: config_manager.clone(),
                driver_registry: Arc::clone(&driver_registry),
                asset_library: Arc::clone(&asset_library),
                render_loop: Arc::clone(&render_loop),
                event_bus: Arc::clone(&event_bus),
                performance: Arc::clone(&performance),
                backend_manager: Arc::clone(&backend_manager),
                preview_runtime: Arc::clone(&preview_runtime),
                start_time,
                input_status,
                effect_registry: Arc::clone(&effect_registry),
                effect_identity: EffectIdentityResources::new(
                    Arc::clone(&display_preferences),
                    Arc::clone(&library_identity),
                    Arc::clone(&playlist_runtime),
                ),
                display_preferences: Arc::clone(&display_preferences),
                display_frames: Arc::clone(&display_frames),
                device_metrics: Arc::clone(&device_metrics),
                input_manager: Arc::clone(&input_manager),
            },
            |layout| {
                let discovery_runtime = crate::discovery::DiscoveryRuntime {
                    device_registry: device_registry.clone(),
                    backend_manager: Arc::clone(&backend_manager),
                    lifecycle_manager: Arc::clone(&lifecycle_manager),
                    reconnect_tasks: Arc::clone(&reconnect_tasks),
                    event_bus: Arc::clone(&event_bus),
                    layout: layout.clone(),
                    logical_devices: Arc::clone(&logical_devices),
                    attachment_registry: Arc::clone(&attachment_registry),
                    attachment_profiles: Arc::clone(&attachment_profiles),
                    device_settings: device_settings.clone(),
                    runtime_state_path: runtime_state_path.clone(),
                    device_aliases_path,
                    usb_protocol_configs: usb_protocol_configs.clone(),
                    credential_store: Arc::clone(&credential_store),
                    in_progress: Arc::clone(&discovery_in_progress),
                    pending_scans: Arc::default(),
                    task_spawner: test_constructor_task_spawner(),
                };
                Ok(Arc::new(DaemonDriverHost::new(
                    discovery_runtime,
                    driver_inventory,
                    Arc::clone(&driver_registry),
                    config_manager.clone(),
                )))
            },
        )
        .expect("default app state should assemble the domain graph");
        {
            let mut manager = backend_manager.try_lock().expect(
                "default app state should register the simulator backend without contention",
            );
            manager.register_backend(Arc::new(SimulatedDisplayBackend::new(
                Arc::clone(&simulated_displays),
                Arc::clone(&simulated_display_runtime),
            )));
        }

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
            config_manager,
            data_dir,
            state_dir,
            extensions: ExtensionRegistry::default(),
            api_extensions: Vec::new(),
            input_manager,
            screen_capacity_status,
            #[cfg(target_os = "macos")]
            capture_picker_request_epoch: Arc::new(AtomicU64::new(0)),
            #[cfg(target_os = "macos")]
            capture_picker_persistence_task: Arc::new(StdMutex::new(None)),
            input_publication_demands: InputPublicationDemandHandle::new(),
            browser_input,
            interaction_routing,
            discovery_in_progress,
            attachment_registry,
            attachment_profiles,
            device_settings,
            simulated_displays,
            simulated_display_runtime,
            driver_host,
            driver_registry,
            logical_devices,
            logical_devices_path,
            runtime_state_path,
            output_power,
            scene_transactions,
            library_store,
            playlist_runtime,
            start_time,
            server_identity: ServerIdentity {
                instance_id: "00000000-0000-7000-8000-000000000000".to_owned(),
                instance_name: "hypercolor".to_owned(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
            },
            server_session_id: None,
            security_state: crate::api::security::SecurityState::unserved(),
        }
    }

    #[doc(hidden)]
    #[must_use]
    pub fn config_manager(&self) -> Option<&Arc<ConfigManager>> {
        self.config_manager.as_ref()
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn input_manager(&self) -> &Arc<Mutex<InputManager>> {
        &self.input_manager
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn driver_host(&self) -> &Arc<DaemonDriverHost> {
        &self.driver_host
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn driver_registry(&self) -> &Arc<DriverModuleRegistry> {
        &self.driver_registry
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn library_store(&self) -> &Arc<dyn LibraryStore> {
        &self.library_store
    }

    /// Create an `AppState` from a live [`DaemonState`](crate::startup::DaemonState).
    ///
    /// The device registry is cloned (it's internally `Arc`-wrapped).
    /// The scene manager, render loop, and event bus are
    /// shared by `Arc::clone` — the API operates on the exact same live
    /// instances as the daemon's render pipeline.
    pub fn from_daemon_state(daemon: &crate::startup::DaemonState) -> Self {
        // State-owned stores are shared from the daemon, never reopened, so
        // one API projection cannot silently clobber another projection's writes.
        let data_dir = ConfigManager::data_dir();
        let state_dir = ConfigManager::state_dir();
        let library_store = Arc::clone(daemon.library_store());
        let driver_host = Arc::clone(daemon.driver_host());
        let driver_registry = Arc::clone(daemon.driver_registry());
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
            config_manager: Some(Arc::clone(&daemon.config_manager)),
            data_dir,
            state_dir,
            extensions: daemon.extensions.clone(),
            api_extensions: daemon.api_extensions.clone(),
            input_manager: Arc::clone(daemon.input_manager()),
            screen_capacity_status: daemon.screen_capacity_status.clone(),
            #[cfg(target_os = "macos")]
            capture_picker_request_epoch: Arc::new(AtomicU64::new(0)),
            #[cfg(target_os = "macos")]
            capture_picker_persistence_task: Arc::new(StdMutex::new(None)),
            input_publication_demands: daemon
                .input_publication_demands()
                .expect("live API state requires a running input publication pump"),
            browser_input: daemon.browser_input.clone(),
            interaction_routing: daemon.interaction_routing.clone(),
            discovery_in_progress: Arc::clone(&daemon.discovery_in_progress),
            attachment_registry: Arc::clone(&daemon.attachment_registry),
            attachment_profiles: Arc::clone(&daemon.attachment_profiles),
            device_settings: daemon.device_settings.clone(),
            simulated_displays: Arc::clone(&daemon.simulated_displays),
            simulated_display_runtime: Arc::clone(&daemon.simulated_display_runtime),
            driver_host,
            driver_registry,
            logical_devices: Arc::clone(&daemon.logical_devices),
            logical_devices_path: daemon.logical_devices_path.clone(),
            runtime_state_path: daemon.runtime_state_path.clone(),
            output_power: daemon.output_power.clone(),
            scene_transactions: daemon.scene_transactions.clone(),
            library_store,
            playlist_runtime: Arc::clone(&daemon.playlist_runtime),
            start_time: daemon.start_time,
            server_identity: daemon.server_identity.clone(),
            server_session_id: None,
            security_state: crate::api::security::SecurityState::unserved(),
        }
    }

    #[cfg(feature = "persistence-test-hooks")]
    #[doc(hidden)]
    #[must_use]
    pub fn layout_publication_test_executor(&self) -> LayoutPublicationTestExecutor {
        self.domains.layout.layout_publication_test_executor()
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
