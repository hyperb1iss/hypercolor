//! Daemon startup orchestration, state management, and graceful shutdown.
//!
//! [`DaemonState`] is the top-level container for all subsystems. It wires
//! together configuration, the device registry, effect registry, spatial engine,
//! backend manager, scene manager, event bus, and render loop — then exposes
//! [`start`](DaemonState::start) and [`shutdown`](DaemonState::shutdown) for
//! lifecycle management.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;

use hypercolor_core::asset::AssetLibrary;
use hypercolor_core::attachment::ComponentRegistry;
use hypercolor_core::bus::HypercolorBus;
use hypercolor_core::config::ConfigManager;
use hypercolor_core::device::{
    BackendManager, DeviceLifecycleManager, DeviceRegistry, UsbProtocolConfigStore,
};
use hypercolor_core::effect::EffectRegistry;
use hypercolor_core::engine::RenderLoop;
use hypercolor_core::input::screen::ScreenCapacityStatusHandle;
use hypercolor_core::input::{InputManager, SourceStatusRegistry};
use hypercolor_driver_support::CredentialStore;
use hypercolor_network::DriverModuleRegistry;
use hypercolor_types::config::HypercolorConfig;
use hypercolor_types::device::DeviceId;
use hypercolor_types::server::ServerIdentity;

use crate::attachment_profiles::ComponentProfileStore;
use crate::device_metrics::DeviceMetricsSnapshotStore;
use crate::device_settings::DeviceSettingsAccess;
use crate::discovery;
use crate::display_output::DisplayOutputThread;
use crate::display_preferences::DisplayPreferencesStore;
use crate::domain::context::DomainContexts;
use crate::domain::scene::SceneService;
use crate::domain::spatial::SpatialService;
use crate::extensions::{ApiExtension, DaemonLifecycleExtension, ExtensionRegistry};
use crate::interaction_routing::InteractionRoutingControl;
use crate::logical_devices::LogicalDevice;
use crate::network::DaemonDriverHost;
use crate::output_power::OutputPower;
use crate::performance::PerformanceTracker;
use crate::preview_runtime::PreviewRuntime;
use crate::render_thread::{ConfiguredFpsTier, InputPublicationDemandHandle, RenderThread};
use crate::scene_store::SceneStore;
use crate::scene_transactions::SceneTransactionQueue;
use crate::session::SessionController;
use crate::simulators::{SimulatedDisplayRuntime, SimulatedDisplayStore};
use crate::zone_layout_preview::ZoneLayoutPreviewStore;

mod acceleration;
pub mod banner;
mod config;
mod discovery_worker;
pub(crate) mod input_status_events;
mod lifecycle;
pub mod logging;
#[cfg(target_os = "macos")]
mod macos_owner_watch;
pub(crate) mod services;
mod signals;

pub(crate) use acceleration::{
    CompositorAccelerationResolution, cpu_compositor_acceleration_resolution,
    resolve_compositor_acceleration_mode,
};
pub(crate) use config::normalize_daemon_driver_configs;
pub use config::{config_sources, default_config, parse_config_toml};
pub use discovery_worker::{
    collect_unmapped_driver_layout_targets, collect_unmapped_prefixed_layout_targets,
};
pub use signals::{SUPERVISED_PARENT_PID_ENV, install_signal_handlers};

/// The top-level daemon state, holding all subsystems.
///
/// Each subsystem is wrapped in `Arc<Mutex<_>>` or `Arc<RwLock<_>>` so they
/// can be shared across the API server, render loop, MCP server, and event
/// handlers without contention.
///
/// Fields are `pub` because the API and MCP modules (built by other agents)
/// will need direct access to subsystems.
pub struct DaemonState {
    /// Complete domain service graph shared by every transport.
    pub domains: DomainContexts,

    /// Live configuration manager (lock-free reads via `arc_swap`).
    pub config_manager: Arc<ConfigManager>,

    /// Typed state owned by downstream daemon extensions.
    pub extensions: ExtensionRegistry,

    /// API route mounters owned by downstream daemon extensions.
    pub api_extensions: Vec<Arc<dyn ApiExtension>>,

    /// Startup and shutdown hooks owned by downstream daemon extensions.
    pub lifecycle_extensions: Vec<Arc<dyn DaemonLifecycleExtension>>,

    /// Device registry — tracks all known and connected devices.
    pub device_registry: DeviceRegistry,

    /// Effect catalog — metadata, search, categories for all known effects.
    pub effect_registry: Arc<RwLock<EffectRegistry>>,

    /// Scene manager — scene lifecycle, priority stack, transitions.
    pub scene_manager: SceneService,

    /// Persisted named-scene store.
    pub scene_store: Arc<RwLock<SceneStore>>,

    /// Event bus — broadcast events, frame data, spectrum data.
    pub event_bus: Arc<HypercolorBus>,

    /// Latest durable macOS daemon ownership state.
    pub macos_daemon_ownership:
        Arc<arc_swap::ArcSwapOption<crate::macos_owner::MacosOwnerSnapshot>>,

    #[cfg(target_os = "macos")]
    _macos_owner_watch: Option<macos_owner_watch::MacosOwnerWatch>,

    /// Daemon-managed user media asset library.
    pub asset_library: Arc<RwLock<AssetLibrary>>,

    /// Saved effect library storage (favorites, presets, playlists).
    /// One instance for the whole process: every `AppState` built from
    /// this daemon shares it, so a write through any surface is visible
    /// to all of them and none can clobber another's in-memory copy.
    pub library_store: Arc<dyn crate::library::LibraryStore>,

    /// Dedicated preview fanout for browser-facing canvas consumers.
    pub preview_runtime: Arc<PreviewRuntime>,

    /// Transient per-zone layout overrides driven by Studio drag previews.
    pub zone_layout_previews: Arc<ZoneLayoutPreviewStore>,

    /// Render loop — frame timing and FPS tier management.
    pub render_loop: Arc<RwLock<RenderLoop>>,

    /// Configured render FPS ceiling shared with the render thread.
    pub configured_max_fps_tier: ConfiguredFpsTier,

    /// Spatial sampling engine — maps canvas pixels to LED positions.
    pub spatial_engine: SpatialService,

    /// Device backend router — pushes colors to hardware.
    pub backend_manager: Arc<Mutex<BackendManager>>,

    /// Shared per-device USB protocol configuration for dynamic topologies.
    pub usb_protocol_configs: UsbProtocolConfigStore,

    /// Shared credential store for driver-authenticated device backends.
    pub credential_store: Arc<CredentialStore>,

    /// Narrow host adapter shared with built-in driver modules.
    pub driver_host: Arc<DaemonDriverHost>,

    /// Registry of compiled-in driver modules and capabilities.
    pub driver_registry: Arc<DriverModuleRegistry>,

    /// Rolling render-performance snapshot shared with the API.
    pub performance: Arc<RwLock<PerformanceTracker>>,

    /// Resolved compositor acceleration path used by the render thread.
    pub(crate) render_acceleration: CompositorAccelerationResolution,

    /// Rolling per-device metrics snapshot shared with the API.
    pub device_metrics: DeviceMetricsSnapshotStore,

    /// Device lifecycle state/action orchestration.
    pub lifecycle_manager: Arc<Mutex<DeviceLifecycleManager>>,

    /// Active reconnect tasks keyed by device ID.
    pub reconnect_tasks: Arc<StdMutex<HashMap<DeviceId, JoinHandle<()>>>>,

    /// Input orchestrator — audio and screen capture sampling sources.
    pub input_manager: Arc<Mutex<InputManager>>,

    /// Exact lock-free screen capacity policy and physical usage.
    pub screen_capacity_status: ScreenCapacityStatusHandle,

    /// Lock-free latest-value health for the live input graph.
    pub input_status: SourceStatusRegistry,

    /// Push handle for browser-preview input injection over WebSocket.
    pub browser_input: hypercolor_core::input::BrowserInputHandle,

    /// Coherent interaction policies and authoritative browser ownership.
    pub interaction_routing: InteractionRoutingControl,

    /// Logical device segmentation store.
    pub logical_devices: Arc<RwLock<HashMap<String, LogicalDevice>>>,

    /// Persistent JSON file for user-defined logical segment devices.
    pub logical_devices_path: PathBuf,

    /// Attachment template registry (built-in plus user-defined).
    pub attachment_registry: Arc<RwLock<ComponentRegistry>>,

    /// Persistent per-device attachment profiles.
    pub attachment_profiles: Arc<RwLock<ComponentProfileStore>>,

    /// Per-display default face preferences (spec 69 §3.6).
    pub display_preferences: Arc<RwLock<DisplayPreferencesStore>>,

    /// Persisted global and per-device output settings.
    pub device_settings: DeviceSettingsAccess,

    /// Persisted virtual display simulator definitions.
    pub simulated_displays: Arc<RwLock<SimulatedDisplayStore>>,

    /// Latest captured simulator frames for inspection surfaces.
    pub simulated_display_runtime: Arc<RwLock<SimulatedDisplayRuntime>>,

    /// Latest composited display frames captured per device for preview surfaces.
    pub display_frames: Arc<RwLock<crate::display_frames::DisplayFrameRuntime>>,

    /// Persistent JSON file for startup runtime session state.
    pub runtime_state_path: PathBuf,

    /// Persistent portable identity overlay in the machine-local state tier.
    pub device_aliases_path: PathBuf,

    pub(super) startup_device_aliases: Option<crate::device_aliases::DeviceAliasFile>,
    pub(super) startup_runtime_snapshot: Option<crate::runtime_state::RuntimeSessionSnapshot>,

    /// Global discovery scan lock shared across startup and API-triggered scans.
    pub discovery_in_progress: Arc<AtomicBool>,

    /// Canonical global output power and brightness authority.
    pub output_power: OutputPower,

    /// Frame-boundary scene changes mirrored into the render thread.
    pub scene_transactions: SceneTransactionQueue,

    /// Handle to the running render thread (if started).
    pub(super) render_thread: Option<RenderThread>,

    /// Handle to the automatic display output task (if started).
    pub(super) display_output_thread: Option<DisplayOutputThread>,

    /// Effect file watcher for hot-reload.
    pub(super) effect_watcher_task: Option<tokio::task::JoinHandle<()>>,

    /// Effect-error fallback worker driven by the event bus.
    pub(super) effect_error_fallback_task: Option<tokio::task::JoinHandle<()>>,
    pub(super) display_preference_sync_task: Option<tokio::task::JoinHandle<()>>,
    pub(super) output_static_hold_task: Option<tokio::task::JoinHandle<()>>,

    /// Periodic discovery worker task.
    pub(super) discovery_task: Option<tokio::task::JoinHandle<()>>,

    /// Periodic per-device metrics collector task.
    pub(super) device_metrics_collector_task: Option<tokio::task::JoinHandle<()>>,

    /// Single daemon-owned source-status to event-bus publisher.
    pub(super) input_status_event_publisher: Option<input_status_events::InputStatusEventPublisher>,

    /// Session/power-awareness watcher and policy controller.
    pub(super) session_controller: Option<SessionController>,

    /// Wall-clock reference for daemon uptime reporting.
    pub start_time: Instant,

    /// Stable network identity exposed by discovery and API responses.
    pub server_identity: ServerIdentity,
}

impl DaemonState {
    /// Read a snapshot of the current configuration.
    ///
    /// Lock-free via `arc_swap` — cheap to call from any context.
    pub fn config(&self) -> Arc<HypercolorConfig> {
        Arc::clone(&self.config_manager.get())
    }

    /// Clone the live input-publication demand handle after subsystem startup.
    pub fn input_publication_demands(&self) -> Option<InputPublicationDemandHandle> {
        self.render_thread
            .as_ref()
            .map(RenderThread::input_publication_demands)
    }

    #[cfg(all(target_os = "macos", feature = "wgpu", feature = "screen-capture"))]
    pub(crate) fn macos_screen_parity_diagnostics(
        &self,
    ) -> Option<crate::render_thread::MacosScreenParityDiagnosticHandle> {
        self.render_thread
            .as_ref()
            .map(RenderThread::macos_screen_parity_diagnostics)
    }

    pub(super) fn discovery_runtime(&self) -> discovery::DiscoveryRuntime {
        self.driver_host.discovery_runtime()
    }

    pub fn register_extension_state<T>(
        &self,
        value: Arc<T>,
    ) -> std::result::Result<(), crate::extensions::ExtensionRegistryError>
    where
        T: Send + Sync + 'static,
    {
        self.extensions.insert(value)
    }

    pub fn register_api_extension(&mut self, extension: Arc<dyn ApiExtension>) {
        self.api_extensions.push(extension);
    }

    pub fn register_lifecycle_extension(&mut self, extension: Arc<dyn DaemonLifecycleExtension>) {
        self.lifecycle_extensions.push(extension);
    }
}
