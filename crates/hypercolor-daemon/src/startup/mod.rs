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

use tokio::sync::{Mutex, RwLock, watch};
use tokio::task::JoinHandle;

use hypercolor_core::attachment::AttachmentRegistry;
use hypercolor_core::bus::HypercolorBus;
use hypercolor_core::config::ConfigManager;
use hypercolor_core::device::net::CredentialStore;
use hypercolor_core::device::{
    BackendManager, DeviceLifecycleManager, DeviceRegistry, UsbProtocolConfigStore,
};
use hypercolor_core::effect::EffectRegistry;
use hypercolor_core::engine::RenderLoop;
use hypercolor_core::input::InputManager;
use hypercolor_core::scene::SceneManager;
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_network::DriverRegistry;
use hypercolor_types::config::HypercolorConfig;
use hypercolor_types::device::DeviceId;
use hypercolor_types::server::ServerIdentity;
use hypercolor_types::spatial::SpatialLayout;

use crate::attachment_profiles::AttachmentProfileStore;
use crate::device_metrics::DeviceMetricsSnapshotStore;
use crate::device_settings::DeviceSettingsStore;
use crate::discovery;
use crate::display_output::DisplayOutputThread;
use crate::layout_auto_exclusions;
use crate::logical_devices::LogicalDevice;
use crate::network::DaemonDriverHost;
use crate::performance::PerformanceTracker;
use crate::preview_runtime::PreviewRuntime;
use crate::render_thread::RenderThread;
use crate::scene_store::SceneStore;
use crate::scene_transactions::SceneTransactionQueue;
use crate::session::{OutputPowerState, SessionController};
use crate::simulators::{SimulatedDisplayRuntime, SimulatedDisplayStore};

mod acceleration;
pub mod banner;
mod config;
mod discovery_worker;
mod lifecycle;
pub mod logging;
mod services;
mod signals;

pub(crate) use acceleration::{
    CompositorAccelerationResolution, cpu_compositor_acceleration_resolution,
    resolve_compositor_acceleration_mode,
};
pub use config::{default_config, load_config, parse_config_toml};
pub use discovery_worker::{
    collect_unmapped_driver_layout_targets, collect_unmapped_prefixed_layout_targets,
};
pub use signals::install_signal_handlers;

/// The top-level daemon state, holding all subsystems.
///
/// Each subsystem is wrapped in `Arc<Mutex<_>>` or `Arc<RwLock<_>>` so they
/// can be shared across the API server, render loop, MCP server, and event
/// handlers without contention.
///
/// Fields are `pub` because the API and MCP modules (built by other agents)
/// will need direct access to subsystems.
pub struct DaemonState {
    /// Live configuration manager (lock-free reads via `arc_swap`).
    pub config_manager: Arc<ConfigManager>,

    /// Device registry — tracks all known and connected devices.
    pub device_registry: DeviceRegistry,

    /// Effect catalog — metadata, search, categories for all known effects.
    pub effect_registry: Arc<RwLock<EffectRegistry>>,

    /// Scene manager — scene lifecycle, priority stack, transitions.
    pub scene_manager: Arc<RwLock<SceneManager>>,

    /// Persisted named-scene store.
    pub scene_store: Arc<RwLock<SceneStore>>,

    /// Event bus — broadcast events, frame data, spectrum data.
    pub event_bus: Arc<HypercolorBus>,

    /// Dedicated preview fanout for browser-facing canvas consumers.
    pub preview_runtime: Arc<PreviewRuntime>,

    /// Render loop — frame timing and FPS tier management.
    pub render_loop: Arc<RwLock<RenderLoop>>,

    /// Spatial sampling engine — maps canvas pixels to LED positions.
    pub spatial_engine: Arc<RwLock<SpatialEngine>>,

    /// Device backend router — pushes colors to hardware.
    pub backend_manager: Arc<Mutex<BackendManager>>,

    /// Shared per-device USB protocol configuration for dynamic topologies.
    pub usb_protocol_configs: UsbProtocolConfigStore,

    /// Shared credential store for network-authenticated device backends.
    pub credential_store: Arc<CredentialStore>,

    /// Narrow host adapter shared with built-in network drivers.
    pub driver_host: Arc<DaemonDriverHost>,

    /// Registry of compiled-in network drivers and capabilities.
    pub driver_registry: Arc<DriverRegistry>,

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

    /// Logical device segmentation store.
    pub logical_devices: Arc<RwLock<HashMap<String, LogicalDevice>>>,

    /// Persistent JSON file for user-defined logical segment devices.
    pub logical_devices_path: PathBuf,

    /// Attachment template registry (built-in plus user-defined).
    pub attachment_registry: Arc<RwLock<AttachmentRegistry>>,

    /// Persistent per-device attachment profiles.
    pub attachment_profiles: Arc<RwLock<AttachmentProfileStore>>,

    /// Persisted global and per-device output settings.
    pub device_settings: Arc<RwLock<DeviceSettingsStore>>,

    /// Persisted virtual display simulator definitions.
    pub simulated_displays: Arc<RwLock<SimulatedDisplayStore>>,

    /// Latest captured simulator frames for inspection surfaces.
    pub simulated_display_runtime: Arc<RwLock<SimulatedDisplayRuntime>>,

    /// Latest composited display frames captured per device for preview surfaces.
    pub display_frames: Arc<RwLock<crate::display_frames::DisplayFrameRuntime>>,

    /// Persisted effect -> layout association map.
    pub effect_layout_links: Arc<RwLock<HashMap<String, String>>>,

    /// Persistent JSON file for effect -> layout associations.
    pub effect_layout_links_path: PathBuf,

    /// Persistent JSON file for spatial layouts.
    pub layouts_path: PathBuf,

    /// In-memory layout store (shared with `AppState`).
    pub layouts: Arc<RwLock<HashMap<String, SpatialLayout>>>,

    /// Persisted layout-specific auto-sync exclusions.
    pub layout_auto_exclusions: Arc<RwLock<layout_auto_exclusions::LayoutAutoExclusionStore>>,

    /// Persistent JSON file for layout-specific auto-sync exclusions.
    pub layout_auto_exclusions_path: PathBuf,

    /// Persistent JSON file for startup runtime session state.
    pub runtime_state_path: PathBuf,

    /// Global discovery scan lock shared across startup and API-triggered scans.
    pub discovery_in_progress: Arc<AtomicBool>,

    /// Shared session-driven output power state for the render thread.
    pub power_state: watch::Sender<OutputPowerState>,

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

    /// Periodic discovery worker task.
    pub(super) discovery_task: Option<tokio::task::JoinHandle<()>>,

    /// Periodic per-device metrics collector task.
    pub(super) device_metrics_collector_task: Option<tokio::task::JoinHandle<()>>,

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

    pub(super) fn discovery_runtime(&self) -> discovery::DiscoveryRuntime {
        self.driver_host.discovery_runtime()
    }
}
