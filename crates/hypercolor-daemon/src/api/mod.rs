//! REST API and WebSocket server for the Hypercolor daemon.
//!
//! Assembles all route groups into a single [`axum::Router`] and provides
//! the shared [`AppState`] that every handler receives via Axum's
//! [`State`](axum::extract::State) extractor.

pub mod access_log;
pub mod assets;
pub mod attachments;
pub mod capture;
pub mod config;
pub mod control_values;
pub mod controls;
pub mod devices;
pub mod diagnose;
pub mod displays;
pub mod drivers;
pub mod effects;
pub mod envelope;
pub mod layouts;
pub mod library;
pub mod local;
#[cfg(all(target_os = "macos", feature = "wgpu", feature = "screen-capture"))]
mod macos_screen_parity;
pub mod openapi;
pub mod output;
mod routes;
pub mod scene;
pub mod scenes;
pub mod security;
pub mod simulators;
pub mod system;
pub mod ws;

use std::collections::HashMap;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::AtomicBool;
#[cfg(any(target_os = "macos", test))]
use std::sync::atomic::AtomicU64;
#[cfg(test)]
use std::sync::atomic::Ordering;
use std::time::Instant;

use arc_swap::{ArcSwap, ArcSwapOption};
use axum::Router;
use axum::http::{HeaderValue, Method, header};
use tokio::sync::{Mutex, RwLock, watch};
use tokio::task::JoinHandle;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};
use tracing::warn;
use utoipa_axum::router::OpenApiRouter;

use self::openapi::OperationDoc;
use crate::interaction_routing::InteractionRoutingControl;
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
use hypercolor_types::config::{
    EffectErrorFallbackPolicy, HypercolorConfig, McpConfig, RenderAccelerationMode, WebConfig,
};
use hypercolor_types::device::DeviceId;
use hypercolor_types::effect::EffectId;
use hypercolor_types::event::{EffectRef, EffectStopReason, HypercolorEvent, ZoneChangeKind};
use hypercolor_types::scene::{SceneId, Zone};
use hypercolor_types::server::ServerIdentity;
use hypercolor_types::spatial::SpatialLayout;
use uuid::Uuid;

use crate::attachment_profiles::ComponentProfileStore;
use crate::device_metrics::{DeviceMetricsSnapshot, DeviceMetricsSnapshotStore};
use crate::device_settings::DeviceSettingsStore;
use crate::display_frames::DisplayFrameRuntime;
use crate::display_preferences::DisplayPreferencesStore;
use crate::domain::context::{DeviceContext, RuntimeSessionService, SceneContext};
use crate::domain::effect::EffectContext;
use crate::domain::output::OutputContext;
use crate::domain::scene::SceneService;
use crate::domain::scene_tree::SceneTreeContext;
use crate::domain::spatial::SpatialService;
use crate::driver_inventory::{DRIVER_INVENTORY_FILENAME, DriverInventoryStore};
use crate::extensions::{ApiExtension, ExtensionRegistry};
use crate::layout_auto_exclusions;
use crate::library::{InMemoryLibraryStore, LibraryStore};
use crate::logical_devices::LogicalDevice;
use crate::network::{self, DaemonDriverHost};
use crate::performance::PerformanceTracker;
use crate::playlist_runtime::PlaylistRuntimeState;
use crate::preview_runtime::PreviewRuntime;
use crate::render_thread::{ConfiguredFpsTier, InputPublicationDemandHandle};
use crate::scene_store::SceneStore;
use crate::scene_transactions::SceneTransactionQueue;
use crate::session::OutputPowerState;
use crate::simulators::{SimulatedDisplayBackend, SimulatedDisplayRuntime, SimulatedDisplayStore};
use crate::zone_layout_preview::ZoneLayoutPreviewStore;

// ── AppState ─────────────────────────────────────────────────────────────

#[cfg(test)]
static APP_STATE_TEST_DATA_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

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
    /// Narrow scene transaction authority used by domain services.
    pub scene: SceneContext,

    /// Runtime-session snapshot and persistence authority.
    pub runtime_session: RuntimeSessionService,

    /// Device lifecycle and discovery-layout reconciliation authority.
    pub devices: DeviceContext,

    /// Global output power, brightness, and quiescence authority.
    pub output: OutputContext,

    /// Effect catalog, validation, and activation authority.
    pub effects: EffectContext,

    /// Live scene-tree read and mutation authority.
    pub scene_tree: SceneTreeContext,

    /// Device tracking and lifecycle management.
    pub device_registry: DeviceRegistry,

    /// Effect catalog (metadata, search, categories).
    pub effect_registry: Arc<RwLock<EffectRegistry>>,

    /// Scene CRUD, priority stack, and transitions.
    pub scene_manager: SceneService,

    /// Persisted named-scene store.
    pub scene_store: Arc<RwLock<SceneStore>>,

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
    pub device_settings: Arc<RwLock<DeviceSettingsStore>>,

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
    pub layout_mutation_test_hooks: layouts::LayoutMutationTestHooks,

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

    /// Shared user/session output brightness state.
    pub power_state: watch::Sender<OutputPowerState>,

    /// Serializes global output transitions and their persistence boundary.
    pub output_power_transition: Arc<Mutex<()>>,

    /// Frame-boundary scene changes mirrored into the render thread.
    pub scene_transactions: SceneTransactionQueue,

    /// Saved effect library storage (favorites, presets, playlists).
    pub library_store: Arc<dyn LibraryStore>,

    /// Active playlist runner state (single background worker at a time).
    pub playlist_runtime: Arc<Mutex<PlaylistRuntimeState>>,

    /// Daemon start time for uptime calculation.
    pub start_time: Instant,

    /// Stable network identity exposed by API and discovery surfaces.
    pub server_identity: ServerIdentity,

    /// Current daemon process session identifier, when one was attested.
    pub server_session_id: Option<String>,

    /// Shared API auth and rate-limiting state for HTTP and WS command dispatch.
    pub security_state: security::SecurityState,
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

#[cfg(test)]
mod tests {
    use hypercolor_types::config::RenderAccelerationMode;

    use super::effect_renderer_acceleration_mode;

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

    #[expect(
        clippy::too_many_lines,
        reason = "test-facing app state construction wires all shared subsystems in one place"
    )]
    #[doc(hidden)]
    pub fn new_with_runtime_overrides(
        data_dir: PathBuf,
        config_manager: Option<Arc<ConfigManager>>,
        driver_registry: Option<Arc<DriverModuleRegistry>>,
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
        let initial_global_brightness = device_settings.global_brightness();
        let (power_state, _) = watch::channel(OutputPowerState {
            global_brightness: initial_global_brightness,
            ..OutputPowerState::default()
        });
        let credential_store = Arc::new(
            CredentialStore::open_blocking(&data_dir)
                .expect("default app state should open credential store"),
        );
        let device_registry = DeviceRegistry::new();
        let effect_registry = Arc::new(RwLock::new(EffectRegistry::default()));
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
        let scene_manager = SceneService::new(scene_manager_inner, Arc::clone(&event_bus));
        let scene_store = Arc::new(RwLock::new(scene_store));
        let scene_transactions = SceneTransactionQueue::default();
        let asset_library = Arc::new(RwLock::new(
            AssetLibrary::open(data_dir.join("assets"))
                .expect("default app state should open asset library"),
        ));
        let preview_runtime = Arc::new(PreviewRuntime::new(Arc::clone(&event_bus)));
        let zone_layout_previews = Arc::new(ZoneLayoutPreviewStore::default());
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
        let device_settings = Arc::new(RwLock::new(device_settings));
        let simulated_displays = Arc::new(RwLock::new(simulated_displays));
        let simulated_display_runtime = Arc::new(RwLock::new(SimulatedDisplayRuntime::new()));
        let display_frames = Arc::new(RwLock::new(DisplayFrameRuntime::new()));
        let layouts = Arc::new(RwLock::new(HashMap::new()));
        let layouts_path = data_dir.join("layouts.json");
        let layout_auto_exclusions = Arc::new(RwLock::new(HashMap::new()));
        let layout_auto_exclusions_path = data_dir.join("layout-auto-exclusions.json");
        let logical_devices = Arc::new(RwLock::new(HashMap::new()));
        let logical_devices_path = data_dir.join("logical-devices.json");
        let runtime_state_path = data_dir.join("runtime-state.json");
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
        let driver_host = Arc::new(DaemonDriverHost::new(
            device_registry.clone(),
            Arc::clone(&backend_manager),
            Arc::clone(&lifecycle_manager),
            Arc::clone(&reconnect_tasks),
            Arc::clone(&event_bus),
            spatial_engine.clone(),
            scene_manager.clone(),
            Arc::clone(&layouts),
            layouts_path.clone(),
            Arc::clone(&layout_auto_exclusions),
            Arc::clone(&logical_devices),
            Arc::clone(&attachment_registry),
            Arc::clone(&attachment_profiles),
            Arc::clone(&device_settings),
            runtime_state_path.clone(),
            device_aliases_path,
            driver_inventory,
            usb_protocol_configs.clone(),
            Arc::clone(&credential_store),
            Arc::clone(&driver_registry),
            Arc::clone(&discovery_in_progress),
            scene_transactions.clone(),
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
            Arc::clone(&scene_store),
            spatial_engine.clone(),
            power_state.clone(),
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
            Arc::clone(&scene_store),
            Arc::clone(&zone_layout_previews),
            runtime_session.clone(),
            Arc::clone(&asset_library),
            config_manager.clone(),
            Arc::clone(&render_loop),
            devices.clone(),
        );
        let output_power_transition = Arc::new(Mutex::new(()));
        let start_time = Instant::now();
        let output = OutputContext::new(
            power_state.clone(),
            Arc::clone(&output_power_transition),
            Arc::clone(&device_settings),
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
        let effects = EffectContext::new(
            Arc::clone(&effect_registry),
            scene.clone(),
            spatial_engine.clone(),
            output.clone(),
        );
        let scene_tree = SceneTreeContext::new(
            scene.clone(),
            effects.clone(),
            devices.clone(),
            output.clone(),
        );

        Self {
            scene,
            runtime_session,
            devices,
            output,
            effects,
            scene_tree,
            device_registry,
            effect_registry,
            scene_manager,
            scene_store,
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
            layout_mutation_test_hooks: layouts::LayoutMutationTestHooks::default(),
            layouts_path,
            layout_auto_exclusions,
            layout_auto_exclusions_path,
            logical_devices,
            logical_devices_path,
            runtime_state_path,
            power_state,
            output_power_transition,
            scene_transactions,
            library_store: Arc::new(InMemoryLibraryStore::new()),
            playlist_runtime: Arc::new(Mutex::new(PlaylistRuntimeState::new())),
            start_time,
            server_identity: ServerIdentity {
                instance_id: "00000000-0000-7000-8000-000000000000".to_owned(),
                instance_name: "hypercolor".to_owned(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
            },
            server_session_id: None,
            security_state: security::SecurityState::from_config(&config),
        }
    }

    /// Create an `AppState` from a live [`DaemonState`](crate::startup::DaemonState).
    ///
    /// The device registry is cloned (it's internally `Arc`-wrapped).
    /// The scene manager, render loop, and event bus are
    /// shared by `Arc::clone` — the API operates on the exact same live
    /// instances as the daemon's render pipeline.
    pub fn from_daemon_state(daemon: &crate::startup::DaemonState) -> Self {
        // Stores are shared from the daemon, never reopened: every
        // AppState built from one daemon must see the same in-memory
        // copy, or a save through one silently clobbers writes made
        // through another.
        let data_dir = ConfigManager::data_dir();
        let library_store = Arc::clone(&daemon.library_store);
        let driver_host = Arc::clone(&daemon.driver_host);
        let driver_registry = Arc::clone(&daemon.driver_registry);
        let runtime_session = RuntimeSessionService::new(
            daemon.runtime_state_path.clone(),
            daemon.scene_manager.clone(),
            Arc::clone(&daemon.scene_store),
            daemon.spatial_engine.clone(),
            daemon.power_state.clone(),
            Arc::clone(&driver_host),
            Arc::clone(&driver_registry),
        );
        let devices = DeviceContext::new(
            daemon.device_registry.clone(),
            Arc::clone(&daemon.lifecycle_manager),
            Arc::clone(&driver_host),
            Arc::clone(&driver_registry),
            Some(Arc::clone(&daemon.config_manager)),
            Arc::clone(&daemon.layout_auto_exclusions),
            daemon.layout_auto_exclusions_path.clone(),
        );
        let scene = SceneContext::new(
            daemon.scene_manager.clone(),
            Arc::clone(&daemon.scene_store),
            Arc::clone(&daemon.zone_layout_previews),
            runtime_session.clone(),
            Arc::clone(&daemon.asset_library),
            Some(Arc::clone(&daemon.config_manager)),
            Arc::clone(&daemon.render_loop),
            devices.clone(),
        );
        let output = OutputContext::new(
            daemon.power_state.clone(),
            Arc::clone(&daemon.output_power_transition),
            Arc::clone(&daemon.device_settings),
            Arc::clone(&daemon.event_bus),
            runtime_session.clone(),
            Arc::clone(&daemon.performance),
            Arc::clone(&daemon.render_loop),
            daemon.spatial_engine.clone(),
            Arc::clone(&daemon.backend_manager),
            Arc::clone(&daemon.preview_runtime),
            devices.clone(),
            daemon.start_time,
        );
        let effects = EffectContext::new(
            Arc::clone(&daemon.effect_registry),
            scene.clone(),
            daemon.spatial_engine.clone(),
            output.clone(),
        );
        let scene_tree = SceneTreeContext::new(
            scene.clone(),
            effects.clone(),
            devices.clone(),
            output.clone(),
        );

        Self {
            scene,
            runtime_session,
            devices,
            output,
            effects,
            scene_tree,
            device_registry: daemon.device_registry.clone(),
            effect_registry: Arc::clone(&daemon.effect_registry),
            scene_manager: daemon.scene_manager.clone(),
            scene_store: Arc::clone(&daemon.scene_store),
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
            device_settings: Arc::clone(&daemon.device_settings),
            simulated_displays: Arc::clone(&daemon.simulated_displays),
            simulated_display_runtime: Arc::clone(&daemon.simulated_display_runtime),
            display_frames: Arc::clone(&daemon.display_frames),
            credential_store: Arc::clone(&daemon.credential_store),
            driver_host,
            driver_registry,
            layouts: Arc::clone(&daemon.layouts),
            #[cfg(feature = "persistence-test-hooks")]
            layout_mutation_test_hooks: layouts::LayoutMutationTestHooks::default(),
            layouts_path: daemon.layouts_path.clone(),
            layout_auto_exclusions: Arc::clone(&daemon.layout_auto_exclusions),
            layout_auto_exclusions_path: daemon.layout_auto_exclusions_path.clone(),
            logical_devices: Arc::clone(&daemon.logical_devices),
            logical_devices_path: daemon.logical_devices_path.clone(),
            runtime_state_path: daemon.runtime_state_path.clone(),
            power_state: daemon.power_state.clone(),
            output_power_transition: Arc::clone(&daemon.output_power_transition),
            scene_transactions: daemon.scene_transactions.clone(),
            library_store,
            playlist_runtime: Arc::new(Mutex::new(PlaylistRuntimeState::new())),
            start_time: daemon.start_time,
            server_identity: daemon.server_identity.clone(),
            server_session_id: None,
            security_state: security::SecurityState::from_config(&daemon.config_manager.get()),
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

/// Persist the spatial layout store to disk.
pub(crate) async fn persist_layouts(state: &Arc<AppState>) -> anyhow::Result<()> {
    let layouts = state.layouts.read().await;
    crate::layout_store::save(&state.layouts_path, &layouts)
}

pub(crate) async fn persist_layouts_best_effort(state: &Arc<AppState>) {
    if let Err(error) = persist_layouts(state).await {
        warn!(
            path = %state.layouts_path.display(),
            %error,
            "Failed to persist layout store"
        );
    }
}

pub(crate) async fn persist_simulated_displays(state: &Arc<AppState>) {
    let store = state.simulated_displays.read().await;
    if let Err(error) = store.save() {
        warn!(%error, "Failed to persist simulated display store");
    }
}

pub(crate) fn publish_render_group_changed(
    state: &AppState,
    scene_id: SceneId,
    group: &Zone,
    kind: ZoneChangeKind,
) {
    state.event_bus.publish(HypercolorEvent::ZoneChanged {
        scene_id,
        zone_id: group.id,
        role: group.role,
        kind,
    });
}

#[derive(Debug, Clone)]
pub(crate) struct EffectErrorFallbackApplied {
    pub effect: EffectRef,
    pub cleared_group_count: usize,
}

/// Unload an effect from every zone of the active scene that runs it,
/// as the configured error-fallback policy demands.
///
/// `Ok(None)` means the policy did nothing: either it is `None`, or no
/// zone was running the failed effect.
pub(crate) async fn apply_effect_error_fallback(
    state: &Arc<AppState>,
    effect_id: &str,
    policy: EffectErrorFallbackPolicy,
) -> Result<Option<EffectErrorFallbackApplied>, crate::domain::DomainError> {
    match policy {
        EffectErrorFallbackPolicy::None => Ok(None),
        EffectErrorFallbackPolicy::ClearGroups => {
            clear_active_scene_effect_groups(state, effect_id).await
        }
    }
}

async fn clear_active_scene_effect_groups(
    state: &Arc<AppState>,
    effect_id: &str,
) -> Result<Option<EffectErrorFallbackApplied>, crate::domain::DomainError> {
    let effect = resolve_effect_ref_for_fallback(state, effect_id).await;

    let mut mutation = state.scene_manager.begin_mutation().await;
    mutation.active_scene_for_runtime_mutation("applying an effect error fallback")?;
    let zone_ids = mutation
        .scenes()
        .active_scene()
        .map(|scene| {
            scene
                .zones
                .iter()
                .filter(|zone| {
                    zone.effect_ids()
                        .any(|candidate| candidate.to_string() == effect_id)
                })
                .map(|zone| zone.id)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if zone_ids.is_empty() {
        return Ok(None);
    }

    let cleared_zones = zone_ids
        .into_iter()
        .filter_map(|zone_id| {
            mutation.clear_zone_effect(zone_id, Some(effect.clone()), EffectStopReason::Error)
        })
        .collect::<Vec<_>>();
    if cleared_zones.is_empty() {
        return Ok(None);
    }

    crate::domain::scene::commit_scene(state.as_ref(), mutation)
        .await?
        .log_if_retrying("Failed to persist effect fallback");
    persist_runtime_session(state).await;

    Ok(Some(EffectErrorFallbackApplied {
        effect,
        cleared_group_count: cleared_zones.len(),
    }))
}

async fn resolve_effect_ref_for_fallback(state: &AppState, effect_id: &str) -> EffectRef {
    let parsed_id = Uuid::parse_str(effect_id).ok().map(EffectId::new);
    if let Some(parsed_id) = parsed_id {
        let registry = state.effect_registry.read().await;
        if let Some(entry) = registry.get(&parsed_id) {
            return crate::domain::effect::effect_ref(&entry.metadata);
        }
    }

    EffectRef {
        id: effect_id.to_owned(),
        name: effect_id.to_owned(),
        engine: "unknown".to_owned(),
    }
}

/// Remove every display assignment a deleted device leaves behind: its
/// scene-bound display groups, its runtime default face zone, and its
/// stored default-face preference. The default zone and preference are
/// pruned even when scene-store persistence fails — a deleted device must
/// never keep a live render group demanding face frames, and the deleted
/// device cannot be resolved later to clear them through the displays API.
pub(crate) async fn prune_scene_display_groups_for_device(
    state: &Arc<AppState>,
    device_id: DeviceId,
) {
    // The preference goes first and unconditionally. A deleted device
    // must never keep a stored default face, and it can no longer be
    // addressed through the displays API to clear one, so this must not
    // ride on whether the scene commit lands.
    let removed_preference = {
        let mut store = state.display_preferences.write().await;
        match store.remove(device_id) {
            Ok(removed) => removed.is_some(),
            Err(error) => {
                warn!(%error, %device_id, "Failed to prune display preference for deleted device");
                false
            }
        }
    };

    let pruned =
        match crate::domain::display::prune_display_zones_for_device(&state.scene, device_id).await
        {
            Ok(pruned) => pruned,
            Err(error) => {
                warn!(%error, %device_id, "Failed to prune display zones for deleted device");
                crate::domain::display::PrunedDisplayZones::empty()
            }
        };

    if pruned.removed_zones.is_empty() && pruned.removed_default.is_none() && !removed_preference {
        return;
    }
    persist_runtime_session(state).await;
}

/// Persist discovery auto-sync exclusions to disk.
pub(crate) async fn persist_layout_auto_exclusions(state: &AppState) {
    state.devices.persist_layout_auto_exclusions().await;
}

pub(crate) async fn save_runtime_session_snapshot(state: &AppState) {
    state.runtime_session.save().await;
}

pub(crate) async fn persist_runtime_session(state: &Arc<AppState>) {
    save_runtime_session_snapshot(state.as_ref()).await;
}

pub(crate) fn discovery_runtime(state: &AppState) -> crate::discovery::DiscoveryRuntime {
    state.driver_host.discovery_runtime()
}

/// Re-evaluate device connect behavior after a change to what the active
/// scene targets, so a device placed in a zone connects now instead of
/// whenever the next discovery sweep happens to run.
///
/// Awaited rather than detached, matching `apply_layout` and the logical
/// device endpoints. Reconciliation only performs I/O for devices whose
/// eligibility actually changed — a device already Connected and still
/// wanted, or still Known and still unwanted, yields no lifecycle actions
/// at all — so an edit that moves no device costs one in-memory walk.
/// Detaching it would buy nothing and cost ordering: concurrent runs could
/// apply stale eligibility out of order, and an in-flight connect could
/// outlive shutdown's disconnect sweep.
pub(crate) async fn sync_connectivity(state: &AppState) {
    state.devices.sync_connectivity().await;
}

// ── Router ───────────────────────────────────────────────────────────────

fn documented_api_routes(asset_upload_body_limit: usize) -> OpenApiRouter<Arc<AppState>> {
    routes::versioned(asset_upload_body_limit)
}

fn documented_root_routes() -> OpenApiRouter<Arc<AppState>> {
    OpenApiRouter::with_openapi(openapi::base_document()).routes(openapi::documented_route(
        "/health",
        axum::routing::get(system::health_check),
        [
            OperationDoc::get::<hypercolor_types::api::system::HealthResponse>(
                "health_check",
                "system",
                "Run daemon health check",
            )
            .also_status("503")
            .raw(),
        ],
    ))
}

pub(crate) fn openapi_document() -> utoipa::openapi::OpenApi {
    let asset_upload_body_limit =
        usize::try_from(assets::asset_upload_body_limit_bytes()).unwrap_or(usize::MAX);
    documented_root_routes().into_openapi().nest(
        "/api/v1",
        documented_api_routes(asset_upload_body_limit).into_openapi(),
    )
}

/// Build the complete Axum router with all API routes and middleware.
///
/// When `ui_dir` is provided, static files are served at `/` with SPA
/// fallback (all non-API, non-asset paths return `index.html`).
pub fn build_router(state: Arc<AppState>, ui_dir: Option<&Path>) -> Router {
    let security_state = state.security_state.clone();
    let (mcp_config, web_config): (McpConfig, WebConfig) =
        state.config_manager.as_ref().map_or_else(
            || (McpConfig::default(), WebConfig::default()),
            |manager| {
                let config = manager.get();
                (config.mcp.clone(), config.web.clone())
            },
        );
    let cors_origin = cors_origins(&web_config, security_state.security_enabled());
    // Sourced from the route's own ceiling so a 413 can never name a limit
    // this layer does not enforce.
    let asset_upload_body_limit =
        usize::try_from(assets::asset_upload_body_limit_bytes()).unwrap_or(usize::MAX);

    let api = documented_api_routes(asset_upload_body_limit);

    let mut api = api;
    for extension in &state.api_extensions {
        api = extension.mount_api_routes(api);
    }
    // A deleted route has to answer as one. Without a fallback scoped to
    // the API, an unmatched `/api/v1` path falls through to the SPA
    // fallback below and a browser-facing daemon answers `200 text/html`
    // for a route that no longer exists — every route-deletion fence in
    // the program is only as strong as this. Nesting resolves the inner
    // fallback first, so the SPA never sees an API path.
    let api = api.fallback(api_route_not_found);
    let (api, versioned_openapi) = api.split_for_parts();
    let api = api.method_not_allowed_fallback(api_route_not_found);
    let root = documented_root_routes();
    let (root, root_openapi) = root.split_for_parts();
    let document = root_openapi.nest("/api/v1", versioned_openapi);
    let mut router = root.nest("/api/v1", api);

    if mcp_config.enabled {
        router = router.merge(crate::mcp::build_router(Arc::clone(&state), &mcp_config));
    }

    router = router.merge(openapi::swagger(document));

    // Serve the web UI with SPA fallback when a UI directory is configured.
    //
    // Every dynamic mount above is named here so the middleware can tell
    // an asset request from an API one. The UI is the fallback, so its
    // surface is whatever those prefixes do not claim, and a browser
    // fetching a script or stylesheet attaches no bearer header.
    let mut static_assets = security::StaticAssetSurface::default();
    if let Some(ui_path) = ui_dir {
        let index = ui_path.join("index.html");
        router = router.fallback_service(ServeDir::new(ui_path).fallback(ServeFile::new(index)));
        static_assets = security::StaticAssetSurface::mounted(dynamic_route_prefixes(&mcp_config));
    }

    router
        .layer(axum::middleware::from_fn_with_state(
            security_state.with_static_assets(static_assets),
            security::enforce_security,
        ))
        .layer(
            CorsLayer::new()
                .allow_origin(cors_origin)
                .allow_methods([
                    Method::GET,
                    Method::HEAD,
                    Method::OPTIONS,
                    Method::POST,
                    Method::PUT,
                    Method::PATCH,
                    Method::DELETE,
                ])
                .allow_headers([header::ACCEPT, header::AUTHORIZATION, header::CONTENT_TYPE]),
        )
        .layer(axum::middleware::from_fn(access_log::log_access))
        .with_state(state)
}

/// The prefixes this router answers from a handler that
/// [`StaticAssetSurface`](security::StaticAssetSurface) does not already
/// protect.
///
/// The web UI mounts as the fallback, so the security layer identifies an
/// asset request by exclusion. `/api` and `/health` are seeded by the
/// surface itself; what varies per daemon is the MCP mount, whose base
/// path is configurable, so it is derived here next to the mount.
/// Render an unmatched `/api/v1` path as the canonical `DomainError`
/// envelope, so a retired route is indistinguishable from one that
/// never existed and distinguishable from a working page.
async fn api_route_not_found(
    axum::extract::OriginalUri(uri): axum::extract::OriginalUri,
) -> axum::response::Response {
    use axum::response::IntoResponse as _;
    // `OriginalUri`, not `Uri`: nesting strips `/api/v1` from the
    // request the fallback sees, and echoing the stripped path would
    // name an address the caller never asked for.
    crate::domain::DomainError::not_found(crate::domain::ResourceKind::Route, uri.path())
        .into_response()
}

fn dynamic_route_prefixes(mcp_config: &McpConfig) -> Vec<String> {
    let mut prefixes = Vec::new();
    if mcp_config.enabled {
        prefixes.push(crate::mcp::normalize_base_path(&mcp_config.base_path));
    }
    prefixes
}

fn cors_origins(web_config: &WebConfig, api_auth_required: bool) -> AllowOrigin {
    let configured_origins = configured_cors_origins(web_config, api_auth_required);
    AllowOrigin::predicate(move |origin: &HeaderValue, _| {
        is_allowed_cors_origin(origin, &configured_origins)
    })
}

fn configured_cors_origins(web_config: &WebConfig, api_auth_required: bool) -> Vec<HeaderValue> {
    if !api_auth_required {
        return Vec::new();
    }

    web_config
        .cors_origins
        .iter()
        .filter_map(|origin| configured_cors_origin(origin))
        .collect()
}

fn configured_cors_origin(origin: &str) -> Option<HeaderValue> {
    let origin = origin.trim();
    if !is_http_origin(origin) {
        warn!(origin, "Ignoring invalid configured CORS origin");
        return None;
    }

    match HeaderValue::from_str(origin) {
        Ok(value) => Some(value),
        Err(error) => {
            warn!(origin, %error, "Ignoring invalid configured CORS origin");
            None
        }
    }
}

fn is_allowed_cors_origin(origin: &HeaderValue, configured_origins: &[HeaderValue]) -> bool {
    is_loopback_origin(origin)
        || security::is_trusted_tauri_origin(origin)
        || configured_origins.iter().any(|allowed| allowed == origin)
}

fn is_http_origin(origin: &str) -> bool {
    let Ok(uri) = origin.parse::<axum::http::Uri>() else {
        return false;
    };
    matches!(uri.scheme_str(), Some("http" | "https"))
        && uri.host().is_some()
        && uri
            .path_and_query()
            .is_none_or(|path| matches!(path.as_str(), "" | "/"))
}

fn is_loopback_origin(origin: &HeaderValue) -> bool {
    let Ok(origin) = origin.to_str() else {
        return false;
    };
    let Ok(uri) = origin.parse::<axum::http::Uri>() else {
        return false;
    };
    if !matches!(uri.scheme_str(), Some("http" | "https")) {
        return false;
    }

    let Some(host) = uri.host() else {
        return false;
    };
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }

    host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

#[cfg(test)]
mod cors_tests {
    use axum::http::HeaderValue;
    use hypercolor_types::config::WebConfig;

    use super::{configured_cors_origins, is_allowed_cors_origin};

    fn origin(value: &str) -> HeaderValue {
        HeaderValue::from_str(value).expect("origin should be a valid header value")
    }

    #[test]
    fn loopback_origin_is_allowed_without_api_auth() {
        let configured = configured_cors_origins(&WebConfig::default(), false);

        assert!(is_allowed_cors_origin(
            &origin("http://localhost:9430"),
            &configured
        ));
        assert!(is_allowed_cors_origin(
            &origin("http://127.0.0.1:9430"),
            &configured
        ));
        for native_origin in [
            "tauri://localhost",
            "http://tauri.localhost",
            "https://tauri.localhost",
        ] {
            assert!(is_allowed_cors_origin(&origin(native_origin), &configured));
        }
        assert!(!is_allowed_cors_origin(
            &origin("tauri://attacker.example"),
            &configured
        ));
    }

    #[test]
    fn configured_origin_requires_api_auth() {
        let config = WebConfig {
            cors_origins: vec!["https://studio.example".to_owned()],
            ..WebConfig::default()
        };

        let unsecured = configured_cors_origins(&config, false);
        assert!(!is_allowed_cors_origin(
            &origin("https://studio.example"),
            &unsecured
        ));

        let secured = configured_cors_origins(&config, true);
        assert!(is_allowed_cors_origin(
            &origin("https://studio.example"),
            &secured
        ));
    }

    #[test]
    fn invalid_configured_origin_is_ignored() {
        let config = WebConfig {
            cors_origins: vec![
                "*".to_owned(),
                "https://studio.example/path".to_owned(),
                "https://studio.example".to_owned(),
            ],
            ..WebConfig::default()
        };

        let configured = configured_cors_origins(&config, true);

        assert_eq!(configured, vec![origin("https://studio.example")]);
    }
}

#[cfg(test)]
mod static_asset_surface_tests {
    use hypercolor_types::config::McpConfig;

    use super::dynamic_route_prefixes;

    #[test]
    fn the_mcp_mount_is_named() {
        let prefixes = dynamic_route_prefixes(&McpConfig {
            enabled: true,
            base_path: "/mcp".to_owned(),
            ..McpConfig::default()
        });

        assert_eq!(prefixes, vec!["/mcp"]);
    }

    #[test]
    fn a_relocated_mcp_mount_follows_its_configured_path() {
        // The exemption would hand an unauthenticated caller the MCP
        // surface if this tracked the default instead of the config.
        let prefixes = dynamic_route_prefixes(&McpConfig {
            enabled: true,
            base_path: "agents/".to_owned(),
            ..McpConfig::default()
        });

        assert_eq!(prefixes, vec!["/agents"]);
    }

    #[test]
    fn a_disabled_mcp_server_contributes_no_prefix() {
        let prefixes = dynamic_route_prefixes(&McpConfig {
            enabled: false,
            ..McpConfig::default()
        });

        assert!(prefixes.is_empty());
    }
}
