//! REST API and WebSocket server for the Hypercolor daemon.
//!
//! Assembles all route groups into a single [`axum::Router`] and provides
//! the shared [`AppState`] that every handler receives via Axum's
//! [`State`](axum::extract::State) extractor.

pub mod access_log;
pub mod attachments;
pub mod config;
pub mod control_values;
pub mod devices;
pub mod diagnose;
pub mod displays;
pub mod effects;
pub mod envelope;
pub mod layouts;
pub mod library;
pub mod overlays;
pub mod preview;
pub mod profiles;
pub mod scenes;
pub mod security;
pub mod settings;
pub mod simulators;
pub mod system;
pub mod ws;

use std::collections::HashMap;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use axum::Router;
use axum::http::{HeaderValue, Method, header};
use tokio::sync::{Mutex, RwLock, watch};
use tokio::task::JoinHandle;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};
use tracing::warn;

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
use hypercolor_types::config::{HypercolorConfig, McpConfig, RenderAccelerationMode};
use hypercolor_types::device::DeviceId;
use hypercolor_types::event::{HypercolorEvent, RenderGroupChangeKind, SceneChangeReason};
use hypercolor_types::scene::{RenderGroup, SceneId};
use hypercolor_types::server::ServerIdentity;
use hypercolor_types::spatial::SpatialLayout;

use crate::attachment_profiles::AttachmentProfileStore;
use crate::api::envelope::ApiError;
use crate::device_settings::DeviceSettingsStore;
use crate::display_frames::DisplayFrameRuntime;
use crate::display_overlays::{DisplayOverlayRegistry, DisplayOverlayRuntimeRegistry};
use crate::layout_auto_exclusions;
use crate::library::{InMemoryLibraryStore, JsonLibraryStore, LibraryStore};
use crate::logical_devices::LogicalDevice;
use crate::network::{self, DaemonDriverHost};
use crate::performance::PerformanceTracker;
use crate::playlist_runtime::PlaylistRuntimeState;
use crate::preview_runtime::PreviewRuntime;
use crate::profile_store::ProfileStore;
use crate::runtime_state;
use crate::scene_store::SceneStore;
use crate::scene_transactions::SceneTransactionQueue;
use crate::session::{OutputPowerState, current_global_brightness};
use crate::simulators::{SimulatedDisplayBackend, SimulatedDisplayRuntime, SimulatedDisplayStore};

// ── AppState ─────────────────────────────────────────────────────────────

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
    /// Device tracking and lifecycle management.
    pub device_registry: DeviceRegistry,

    /// Effect catalog (metadata, search, categories).
    pub effect_registry: Arc<RwLock<EffectRegistry>>,

    /// Scene CRUD, priority stack, and transitions.
    pub scene_manager: Arc<RwLock<SceneManager>>,

    /// Persisted named-scene store.
    pub scene_store: Arc<RwLock<SceneStore>>,

    /// System-wide event bus (broadcast + watch channels).
    pub event_bus: Arc<HypercolorBus>,

    /// Dedicated preview fanout for browser-facing canvas consumers.
    pub preview_runtime: Arc<PreviewRuntime>,

    /// Render loop — frame timing and pipeline skeleton.
    pub render_loop: Arc<RwLock<RenderLoop>>,

    /// Spatial sampling engine — maps canvas pixels to LED positions.
    pub spatial_engine: Arc<RwLock<SpatialEngine>>,

    /// Device backend router — pushes colors to hardware.
    pub backend_manager: Arc<Mutex<BackendManager>>,

    /// Shared per-device USB protocol configuration for dynamic topologies.
    pub usb_protocol_configs: UsbProtocolConfigStore,

    /// Rolling render-performance snapshot shared with metrics endpoints.
    pub performance: Arc<RwLock<PerformanceTracker>>,

    /// Device lifecycle state/action orchestration.
    pub lifecycle_manager: Arc<Mutex<DeviceLifecycleManager>>,

    /// Active reconnect tasks keyed by device ID.
    pub reconnect_tasks: Arc<StdMutex<HashMap<DeviceId, JoinHandle<()>>>>,

    /// Configuration manager for config API endpoints.
    pub config_manager: Option<Arc<ConfigManager>>,

    /// Live input graph shared with the daemon render thread.
    pub input_manager: Arc<Mutex<InputManager>>,

    /// Global discovery scan lock flag shared across startup/API entrypoints.
    pub discovery_in_progress: Arc<AtomicBool>,

    /// Persistent lighting profile store.
    pub profiles: Arc<RwLock<ProfileStore>>,

    /// Attachment template registry (built-in plus user templates).
    pub attachment_registry: Arc<RwLock<AttachmentRegistry>>,

    /// Persistent per-device attachment profile store.
    pub attachment_profiles: Arc<RwLock<AttachmentProfileStore>>,

    /// Persistent per-device user settings store.
    pub device_settings: Arc<RwLock<DeviceSettingsStore>>,

    /// Persisted virtual display simulator definitions.
    pub simulated_displays: Arc<RwLock<SimulatedDisplayStore>>,

    /// Latest captured simulator frames for inspection surfaces.
    pub simulated_display_runtime: Arc<RwLock<SimulatedDisplayRuntime>>,

    /// Live per-display overlay configs shared with the display-output workers.
    pub display_overlays: Arc<DisplayOverlayRegistry>,

    /// Live per-slot overlay runtime state published by display workers.
    pub display_overlay_runtime: Arc<DisplayOverlayRuntimeRegistry>,

    /// Latest composited display frames captured per device for preview surfaces.
    pub display_frames: Arc<RwLock<DisplayFrameRuntime>>,

    /// Shared encrypted credential store for network-authenticated backends.
    pub credential_store: Arc<CredentialStore>,

    /// Narrow host adapter shared with built-in network drivers.
    pub driver_host: Arc<DaemonDriverHost>,

    /// Registry of compiled-in network drivers and capabilities.
    pub driver_registry: Arc<DriverRegistry>,

    /// In-memory layout store (shared with `DaemonState`, persisted to layouts.json).
    pub layouts: Arc<RwLock<HashMap<String, SpatialLayout>>>,

    /// Persistent path for spatial layouts.
    pub layouts_path: PathBuf,

    /// Layout-specific exclusions for discovery auto-sync.
    pub layout_auto_exclusions: Arc<RwLock<layout_auto_exclusions::LayoutAutoExclusionStore>>,

    /// Persistent path for layout-specific discovery auto-sync exclusions.
    pub layout_auto_exclusions_path: PathBuf,

    /// Logical device segmentation store (physical device -> logical ranges).
    pub logical_devices: Arc<RwLock<HashMap<String, LogicalDevice>>>,

    /// Persistent path for user-defined logical segment devices.
    pub logical_devices_path: PathBuf,

    /// Persisted effect -> layout associations.
    pub effect_layout_links: Arc<RwLock<HashMap<String, String>>>,

    /// Persistent path for effect -> layout associations.
    pub effect_layout_links_path: PathBuf,

    /// Persisted path for startup runtime-session restoration.
    pub runtime_state_path: PathBuf,

    /// Shared user/session output brightness state.
    pub power_state: watch::Sender<OutputPowerState>,

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

    /// Shared API auth and rate-limiting state for HTTP and WS command dispatch.
    pub security_state: security::SecurityState,
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn configured_render_acceleration_mode(
    config_manager: Option<&Arc<ConfigManager>>,
) -> RenderAccelerationMode {
    config_manager.map_or(RenderAccelerationMode::Cpu, |manager| {
        manager.get().effect_engine.render_acceleration_mode
    })
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

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn configured_effect_renderer_acceleration_mode(
    config_manager: Option<&Arc<ConfigManager>>,
) -> RenderAccelerationMode {
    effect_renderer_acceleration_mode(configured_render_acceleration_mode(config_manager))
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
    #[expect(
        clippy::too_many_lines,
        reason = "test-facing app state construction wires all shared subsystems in one place"
    )]
    pub fn new() -> Self {
        use hypercolor_types::spatial::{EdgeBehavior, SamplingMode, SpatialLayout};

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

        let mut attachment_registry = AttachmentRegistry::new();
        if let Err(error) = attachment_registry.load_builtins() {
            warn!(%error, "Failed to load built-in attachment templates");
        }

        let attachment_templates_dir = ConfigManager::data_dir().join("attachments");
        if let Err(error) = attachment_registry.load_user_dir(&attachment_templates_dir) {
            warn!(
                path = %attachment_templates_dir.display(),
                %error,
                "Failed to load user attachment templates"
            );
        }

        let attachment_profiles_path = ConfigManager::data_dir().join("attachment-profiles.json");
        let attachment_profiles = AttachmentProfileStore::load(&attachment_profiles_path)
            .unwrap_or_else(|error| {
                warn!(
                    path = %attachment_profiles_path.display(),
                    %error,
                    "Failed to load attachment profiles; starting with empty store"
                );
                AttachmentProfileStore::new(attachment_profiles_path)
            });
        let profiles_path = ConfigManager::data_dir().join("profiles.json");
        let profiles = ProfileStore::load(&profiles_path).unwrap_or_else(|error| {
            warn!(
                path = %profiles_path.display(),
                %error,
                cause = %error.root_cause(),
                "Failed to load profiles; starting with empty store"
            );
            ProfileStore::new(profiles_path)
        });
        let device_settings_path = ConfigManager::data_dir().join("device-settings.json");
        let device_settings =
            DeviceSettingsStore::load(&device_settings_path).unwrap_or_else(|error| {
                warn!(
                    path = %device_settings_path.display(),
                    %error,
                    "Failed to load device settings; starting with defaults"
                );
                DeviceSettingsStore::new(device_settings_path)
            });
        let simulated_displays_path = ConfigManager::data_dir().join("simulated-displays.json");
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
        let scene_transactions = SceneTransactionQueue::default();
        let credential_store = Arc::new(
            CredentialStore::open_blocking(&ConfigManager::data_dir())
                .expect("default app state should open credential store"),
        );
        let device_registry = DeviceRegistry::new();
        let effect_registry = Arc::new(RwLock::new(EffectRegistry::default()));
        let scenes_path = ConfigManager::data_dir().join("scenes.json");
        let scene_store = SceneStore::load(&scenes_path).unwrap_or_else(|error| {
            warn!(
                path = %scenes_path.display(),
                %error,
                cause = %error.root_cause(),
                "Failed to load scenes; starting with empty store"
            );
            SceneStore::new(scenes_path)
        });
        let mut scene_manager_inner = SceneManager::with_default();
        for scene in scene_store.list().cloned() {
            if let Err(error) = scene_manager_inner.create(scene) {
                warn!(%error, "Failed to install persisted named scene into default app state");
            }
        }
        let scene_manager = Arc::new(RwLock::new(scene_manager_inner));
        let scene_store = Arc::new(RwLock::new(scene_store));
        let event_bus = Arc::new(HypercolorBus::new());
        let preview_runtime = Arc::new(PreviewRuntime::new(Arc::clone(&event_bus)));
        let render_loop = Arc::new(RwLock::new(RenderLoop::new(60)));
        let spatial_engine = Arc::new(RwLock::new(SpatialEngine::new(default_layout)));
        let backend_manager = Arc::new(Mutex::new(BackendManager::new()));
        let usb_protocol_configs = UsbProtocolConfigStore::new();
        let performance = Arc::new(RwLock::new(PerformanceTracker::default()));
        let lifecycle_manager = Arc::new(Mutex::new(DeviceLifecycleManager::new()));
        let reconnect_tasks = Arc::new(StdMutex::new(HashMap::new()));
        let input_manager = Arc::new(Mutex::new(InputManager::new()));
        let discovery_in_progress = Arc::new(AtomicBool::new(false));
        let attachment_registry = Arc::new(RwLock::new(attachment_registry));
        let attachment_profiles = Arc::new(RwLock::new(attachment_profiles));
        let device_settings = Arc::new(RwLock::new(device_settings));
        let simulated_displays = Arc::new(RwLock::new(simulated_displays));
        let simulated_display_runtime = Arc::new(RwLock::new(SimulatedDisplayRuntime::new()));
        let display_overlays = Arc::new(DisplayOverlayRegistry::new());
        let display_overlay_runtime = Arc::new(DisplayOverlayRuntimeRegistry::new());
        let display_frames = Arc::new(RwLock::new(DisplayFrameRuntime::new()));
        let layouts = Arc::new(RwLock::new(HashMap::new()));
        let layouts_path = ConfigManager::data_dir().join("layouts.json");
        let layout_auto_exclusions = Arc::new(RwLock::new(HashMap::new()));
        let layout_auto_exclusions_path =
            ConfigManager::data_dir().join("layout-auto-exclusions.json");
        let logical_devices = Arc::new(RwLock::new(HashMap::new()));
        let logical_devices_path = ConfigManager::data_dir().join("logical-devices.json");
        let effect_layout_links = Arc::new(RwLock::new(HashMap::new()));
        let effect_layout_links_path = ConfigManager::data_dir().join("effect-layouts.json");
        let runtime_state_path = ConfigManager::data_dir().join("runtime-state.json");
        let driver_host = Arc::new(DaemonDriverHost::new(
            device_registry.clone(),
            Arc::clone(&backend_manager),
            Arc::clone(&lifecycle_manager),
            Arc::clone(&reconnect_tasks),
            Arc::clone(&event_bus),
            Arc::clone(&spatial_engine),
            Arc::clone(&layouts),
            layouts_path.clone(),
            Arc::clone(&layout_auto_exclusions),
            Arc::clone(&logical_devices),
            Arc::clone(&attachment_registry),
            Arc::clone(&attachment_profiles),
            Arc::clone(&device_settings),
            runtime_state_path.clone(),
            usb_protocol_configs.clone(),
            Arc::clone(&credential_store),
            Arc::clone(&discovery_in_progress),
            scene_transactions.clone(),
        ));
        let driver_registry = Arc::new(
            network::build_builtin_driver_registry(
                &HypercolorConfig::default(),
                Arc::clone(&credential_store),
            )
            .expect("default app state should build network driver registry"),
        );
        {
            let mut manager = backend_manager.try_lock().expect(
                "default app state should register the simulator backend without contention",
            );
            manager.register_backend(Box::new(SimulatedDisplayBackend::new(
                Arc::clone(&simulated_displays),
                Arc::clone(&simulated_display_runtime),
            )));
        }

        Self {
            device_registry,
            effect_registry,
            scene_manager,
            scene_store,
            event_bus,
            preview_runtime,
            render_loop,
            spatial_engine,
            backend_manager,
            usb_protocol_configs,
            performance,
            lifecycle_manager,
            reconnect_tasks,
            config_manager: None,
            input_manager,
            discovery_in_progress,
            profiles: Arc::new(RwLock::new(profiles)),
            attachment_registry,
            attachment_profiles,
            device_settings,
            simulated_displays,
            simulated_display_runtime,
            display_overlays,
            display_overlay_runtime,
            display_frames,
            credential_store,
            driver_host,
            driver_registry,
            layouts,
            layouts_path,
            layout_auto_exclusions,
            layout_auto_exclusions_path,
            logical_devices,
            logical_devices_path,
            effect_layout_links,
            effect_layout_links_path,
            runtime_state_path,
            power_state,
            scene_transactions,
            library_store: Arc::new(InMemoryLibraryStore::new()),
            playlist_runtime: Arc::new(Mutex::new(PlaylistRuntimeState::new())),
            start_time: Instant::now(),
            server_identity: ServerIdentity {
                instance_id: "00000000-0000-7000-8000-000000000000".to_owned(),
                instance_name: "hypercolor".to_owned(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
            },
            security_state: security::SecurityState::from_env(),
        }
    }

    /// Create an `AppState` from a live [`DaemonState`](crate::startup::DaemonState).
    ///
    /// The device registry is cloned (it's internally `Arc`-wrapped).
    /// The scene manager, render loop, and event bus are
    /// shared by `Arc::clone` — the API operates on the exact same live
    /// instances as the daemon's render pipeline.
    pub fn from_daemon_state(daemon: &crate::startup::DaemonState) -> Self {
        let library_path = ConfigManager::data_dir().join("library.json");
        let library_store: Arc<dyn LibraryStore> =
            match JsonLibraryStore::open(library_path.clone()) {
                Ok(store) => Arc::new(store),
                Err(error) => {
                    warn!(
                        path = %library_path.display(),
                        %error,
                        "Failed to load persisted library store; falling back to in-memory store"
                    );
                    Arc::new(InMemoryLibraryStore::new())
                }
            };
        let profiles_path = ConfigManager::data_dir().join("profiles.json");
        let profiles = ProfileStore::load(&profiles_path).unwrap_or_else(|error| {
            warn!(
                path = %profiles_path.display(),
                %error,
                cause = %error.root_cause(),
                "Failed to load profiles; starting with empty store"
            );
            ProfileStore::new(profiles_path)
        });
        let driver_host = Arc::clone(&daemon.driver_host);
        let driver_registry = Arc::clone(&daemon.driver_registry);

        Self {
            device_registry: daemon.device_registry.clone(),
            effect_registry: Arc::clone(&daemon.effect_registry),
            scene_manager: Arc::clone(&daemon.scene_manager),
            scene_store: Arc::clone(&daemon.scene_store),
            event_bus: Arc::clone(&daemon.event_bus),
            preview_runtime: Arc::clone(&daemon.preview_runtime),
            render_loop: Arc::clone(&daemon.render_loop),
            spatial_engine: Arc::clone(&daemon.spatial_engine),
            backend_manager: Arc::clone(&daemon.backend_manager),
            usb_protocol_configs: daemon.usb_protocol_configs.clone(),
            performance: Arc::clone(&daemon.performance),
            lifecycle_manager: Arc::clone(&daemon.lifecycle_manager),
            reconnect_tasks: Arc::clone(&daemon.reconnect_tasks),
            config_manager: Some(Arc::clone(&daemon.config_manager)),
            input_manager: Arc::clone(&daemon.input_manager),
            discovery_in_progress: Arc::clone(&daemon.discovery_in_progress),
            profiles: Arc::new(RwLock::new(profiles)),
            attachment_registry: Arc::clone(&daemon.attachment_registry),
            attachment_profiles: Arc::clone(&daemon.attachment_profiles),
            device_settings: Arc::clone(&daemon.device_settings),
            simulated_displays: Arc::clone(&daemon.simulated_displays),
            simulated_display_runtime: Arc::clone(&daemon.simulated_display_runtime),
            display_overlays: Arc::clone(&daemon.display_overlays),
            display_overlay_runtime: Arc::clone(&daemon.display_overlay_runtime),
            display_frames: Arc::clone(&daemon.display_frames),
            credential_store: Arc::clone(&daemon.credential_store),
            driver_host,
            driver_registry,
            layouts: Arc::clone(&daemon.layouts),
            layouts_path: daemon.layouts_path.clone(),
            layout_auto_exclusions: Arc::clone(&daemon.layout_auto_exclusions),
            layout_auto_exclusions_path: daemon.layout_auto_exclusions_path.clone(),
            logical_devices: Arc::clone(&daemon.logical_devices),
            logical_devices_path: daemon.logical_devices_path.clone(),
            effect_layout_links: Arc::clone(&daemon.effect_layout_links),
            effect_layout_links_path: daemon.effect_layout_links_path.clone(),
            runtime_state_path: daemon.runtime_state_path.clone(),
            power_state: daemon.power_state.clone(),
            scene_transactions: daemon.scene_transactions.clone(),
            library_store,
            playlist_runtime: Arc::new(Mutex::new(PlaylistRuntimeState::new())),
            start_time: daemon.start_time,
            server_identity: daemon.server_identity.clone(),
            security_state: security::SecurityState::from_env(),
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

/// Persist the spatial layout store to disk.
pub(crate) async fn persist_layouts(state: &Arc<AppState>) {
    let layouts = state.layouts.read().await;
    if let Err(error) = crate::layout_store::save(&state.layouts_path, &layouts) {
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

pub(crate) async fn save_scene_store_snapshot(state: &AppState) -> anyhow::Result<()> {
    let scenes = {
        let manager = state.scene_manager.read().await;
        manager.list().into_iter().cloned().collect::<Vec<_>>()
    };

    let mut store = state.scene_store.write().await;
    store.replace_named_scenes(scenes);
    store.save()
}

pub(crate) fn publish_render_group_changed(
    state: &AppState,
    scene_id: SceneId,
    group: &RenderGroup,
    kind: RenderGroupChangeKind,
) {
    state
        .event_bus
        .publish(HypercolorEvent::RenderGroupChanged {
            scene_id,
            group_id: group.id,
            role: group.role,
            kind,
        });
}

#[derive(Debug, Clone)]
pub(crate) enum ActiveSceneMutationError {
    NoActiveScene,
    SnapshotLocked { scene_name: String },
}

impl ActiveSceneMutationError {
    #[must_use]
    pub(crate) fn message(&self, action: &str) -> String {
        match self {
            Self::NoActiveScene => "No active scene available".to_owned(),
            Self::SnapshotLocked { scene_name } => format!(
                "Active scene '{scene_name}' is in snapshot mode; return to Default or deactivate it before {action}"
            ),
        }
    }

    pub(crate) fn api_response(&self, action: &str) -> axum::response::Response {
        match self {
            Self::NoActiveScene => ApiError::internal(self.message(action)),
            Self::SnapshotLocked { .. } => ApiError::conflict(self.message(action)),
        }
    }
}

pub(crate) fn active_scene_id_for_runtime_mutation(
    scene_manager: &SceneManager,
) -> Result<SceneId, ActiveSceneMutationError> {
    let active_scene = scene_manager
        .active_scene()
        .ok_or(ActiveSceneMutationError::NoActiveScene)?;
    if active_scene.blocks_runtime_mutation() {
        return Err(ActiveSceneMutationError::SnapshotLocked {
            scene_name: active_scene.name.clone(),
        });
    }
    Ok(active_scene.id)
}

pub(crate) async fn prune_scene_display_groups_for_device(
    state: &Arc<AppState>,
    device_id: DeviceId,
) {
    let removed_groups = {
        let mut scene_manager = state.scene_manager.write().await;
        scene_manager.remove_display_groups_for_device(device_id)
    };
    if removed_groups.is_empty() {
        return;
    }

    for (scene_id, group) in &removed_groups {
        publish_render_group_changed(
            state.as_ref(),
            *scene_id,
            group,
            RenderGroupChangeKind::Removed,
        );
    }
    persist_runtime_session(state).await;
}

pub(crate) fn publish_active_scene_changed(
    state: &AppState,
    previous: Option<SceneId>,
    current: SceneId,
    reason: SceneChangeReason,
) {
    state
        .event_bus
        .publish(HypercolorEvent::ActiveSceneChanged {
            previous,
            current,
            reason,
        });
}

/// Persist layout-specific discovery auto-sync exclusions to disk.
pub(crate) async fn persist_layout_auto_exclusions(state: &Arc<AppState>) {
    let exclusions = state.layout_auto_exclusions.read().await;
    if let Err(error) =
        crate::layout_auto_exclusions::save(&state.layout_auto_exclusions_path, &exclusions)
    {
        warn!(
            path = %state.layout_auto_exclusions_path.display(),
            %error,
            "Failed to persist layout auto-exclusion store"
        );
    }
}

/// Persist the current runtime session snapshot (active scene, layout, brightness, and discovery state).
pub(crate) async fn save_runtime_session_snapshot(state: &AppState) {
    let mut snapshot = {
        let scene_manager = state.scene_manager.read().await;
        runtime_state::snapshot_from_scene_manager(&scene_manager)
    };

    // Capture active layout ID from the spatial engine.
    {
        let spatial = state.spatial_engine.read().await;
        snapshot.active_layout_id = Some(spatial.layout().id.clone());
    }
    snapshot.global_brightness = current_global_brightness(&state.power_state);
    snapshot.wled_probe_ips = runtime_state::collect_wled_probe_ips(&state.device_registry).await;
    snapshot.wled_probe_targets =
        runtime_state::collect_wled_probe_targets(&state.device_registry).await;

    if let Err(error) = save_scene_store_snapshot(state).await {
        warn!(%error, "Failed to persist scene store before runtime snapshot save");
    }

    if let Err(error) = runtime_state::save(&state.runtime_state_path, &snapshot) {
        warn!(
            path = %state.runtime_state_path.display(),
            %error,
            "Failed to persist runtime session snapshot"
        );
    }
}

pub(crate) async fn persist_runtime_session(state: &Arc<AppState>) {
    save_runtime_session_snapshot(state.as_ref()).await;
}

pub(crate) fn discovery_runtime(state: &AppState) -> crate::discovery::DiscoveryRuntime {
    state.driver_host.discovery_runtime()
}

// ── Router ───────────────────────────────────────────────────────────────

/// Build the complete Axum router with all API routes and middleware.
///
/// When `ui_dir` is provided, static files are served at `/` with SPA
/// fallback (all non-API, non-asset paths return `index.html`).
#[expect(clippy::too_many_lines)]
pub fn build_router(state: Arc<AppState>, ui_dir: Option<&Path>) -> Router {
    let security_state = state.security_state.clone();
    let mcp_config: McpConfig = state
        .config_manager
        .as_ref()
        .map(|manager| manager.get().mcp.clone())
        .unwrap_or_default();

    let api = Router::new()
        // ── Devices ──────────────────────────────────────────────────
        .route("/devices", axum::routing::get(devices::list_devices))
        .route(
            "/devices/discover",
            axum::routing::post(devices::discover_devices),
        )
        .route(
            "/devices/debug/queues",
            axum::routing::get(devices::debug_output_queues),
        )
        .route(
            "/devices/debug/routing",
            axum::routing::get(devices::debug_device_routing),
        )
        .route(
            "/devices/{id}",
            axum::routing::get(devices::get_device)
                .put(devices::update_device)
                .delete(devices::delete_device),
        )
        .route(
            "/devices/{id}/attachments",
            axum::routing::get(devices::get_attachments)
                .put(devices::update_attachments)
                .delete(devices::delete_attachments),
        )
        .route(
            "/devices/{id}/attachments/preview",
            axum::routing::post(devices::preview_attachments),
        )
        .route(
            "/devices/{id}/logical-devices",
            axum::routing::get(devices::list_device_logical_devices)
                .post(devices::create_logical_device),
        )
        .route(
            "/devices/{id}/identify",
            axum::routing::post(devices::identify_device),
        )
        .route(
            "/devices/{id}/zones/{zone_id}/identify",
            axum::routing::post(devices::identify_zone),
        )
        .route(
            "/devices/{id}/attachments/{slot_id}/identify",
            axum::routing::post(devices::identify_attachment),
        )
        .route(
            "/devices/{id}/pair",
            axum::routing::post(devices::pair_device).delete(devices::delete_pairing),
        )
        // ── Displays ─────────────────────────────────────────────────
        .route(
            "/overlays/catalog",
            axum::routing::get(overlays::get_overlay_catalog),
        )
        .route("/displays", axum::routing::get(displays::list_displays))
        .route(
            "/displays/{id}/preview.jpg",
            axum::routing::get(displays::get_display_preview),
        )
        .route(
            "/displays/{id}/face",
            axum::routing::get(displays::get_display_face)
                .put(displays::set_display_face)
                .delete(displays::delete_display_face),
        )
        .route(
            "/displays/{id}/face/controls",
            axum::routing::patch(displays::patch_display_face_controls),
        )
        .route(
            "/displays/{id}/overlays",
            axum::routing::get(displays::list_overlays)
                .put(displays::replace_overlays)
                .post(displays::add_overlay),
        )
        .route(
            "/displays/{id}/overlays/runtime",
            axum::routing::get(displays::list_overlay_runtimes),
        )
        .route(
            "/displays/{id}/overlays/reorder",
            axum::routing::post(displays::reorder_overlays),
        )
        .route(
            "/displays/{id}/overlays/{slot_id}",
            axum::routing::get(displays::get_overlay)
                .patch(displays::patch_overlay)
                .delete(displays::delete_overlay),
        )
        .route(
            "/simulators/displays",
            axum::routing::get(simulators::list_simulated_displays)
                .post(simulators::create_simulated_display),
        )
        .route(
            "/simulators/displays/{id}",
            axum::routing::get(simulators::get_simulated_display)
                .patch(simulators::patch_simulated_display)
                .delete(simulators::delete_simulated_display),
        )
        .route(
            "/simulators/displays/{id}/frame",
            axum::routing::get(simulators::get_simulated_display_frame),
        )
        .route(
            "/logical-devices",
            axum::routing::get(devices::list_logical_devices),
        )
        .route(
            "/logical-devices/{id}",
            axum::routing::get(devices::get_logical_device)
                .put(devices::update_logical_device)
                .delete(devices::delete_logical_device),
        )
        // ── Attachments ──────────────────────────────────────────────
        .route(
            "/attachments/templates",
            axum::routing::get(attachments::list_templates)
                .post(attachments::create_template),
        )
        .route(
            "/attachments/templates/{id}",
            axum::routing::get(attachments::get_template)
                .put(attachments::update_template)
                .delete(attachments::delete_template),
        )
        .route(
            "/attachments/categories",
            axum::routing::get(attachments::list_categories),
        )
        .route(
            "/attachments/vendors",
            axum::routing::get(attachments::list_vendors),
        )
        // ── Effects ──────────────────────────────────────────────────
        .route("/effects", axum::routing::get(effects::list_effects))
        .route(
            "/effects/active",
            axum::routing::get(effects::get_active_effect),
        )
        .route(
            "/effects/current/controls",
            axum::routing::patch(effects::update_current_controls),
        )
        .route(
            "/effects/current/controls/{name}/binding",
            axum::routing::put(effects::set_current_control_binding),
        )
        .route(
            "/effects/current/reset",
            axum::routing::post(effects::reset_controls),
        )
        .route("/effects/stop", axum::routing::post(effects::stop_effect))
        .route(
            "/effects/rescan",
            axum::routing::post(effects::rescan_effects),
        )
        .route("/effects/{id}", axum::routing::get(effects::get_effect))
        .route(
            "/effects/{id}/layout",
            axum::routing::get(effects::get_effect_layout)
                .put(effects::set_effect_layout)
                .delete(effects::delete_effect_layout),
        )
        .route(
            "/effects/{id}/apply",
            axum::routing::post(effects::apply_effect),
        )
        // ── Scenes ───────────────────────────────────────────────────
        .route(
            "/scenes",
            axum::routing::get(scenes::list_scenes).post(scenes::create_scene),
        )
        .route("/scenes/active", axum::routing::get(scenes::get_active_scene))
        .route(
            "/scenes/deactivate",
            axum::routing::post(scenes::deactivate_scene),
        )
        .route(
            "/scenes/{id}",
            axum::routing::get(scenes::get_scene)
                .put(scenes::update_scene)
                .delete(scenes::delete_scene),
        )
        .route(
            "/scenes/{id}/activate",
            axum::routing::post(scenes::activate_scene),
        )
        // ── Profiles ─────────────────────────────────────────────────
        .route(
            "/profiles",
            axum::routing::get(profiles::list_profiles).post(profiles::create_profile),
        )
        .route(
            "/profiles/{id}",
            axum::routing::get(profiles::get_profile)
                .put(profiles::update_profile)
                .delete(profiles::delete_profile),
        )
        .route(
            "/profiles/{id}/apply",
            axum::routing::post(profiles::apply_profile),
        )
        // ── Layouts ──────────────────────────────────────────────────
        .route(
            "/layouts",
            axum::routing::get(layouts::list_layouts).post(layouts::create_layout),
        )
        .route(
            "/layouts/active",
            axum::routing::get(layouts::get_active_layout),
        )
        .route(
            "/layouts/active/preview",
            axum::routing::put(layouts::preview_layout),
        )
        .route(
            "/layouts/{id}",
            axum::routing::get(layouts::get_layout)
                .put(layouts::update_layout)
                .delete(layouts::delete_layout),
        )
        .route(
            "/layouts/{id}/apply",
            axum::routing::post(layouts::apply_layout),
        )
        // ── Library ──────────────────────────────────────────────────
        .route(
            "/library/favorites",
            axum::routing::get(library::list_favorites).post(library::add_favorite),
        )
        .route(
            "/library/favorites/{effect}",
            axum::routing::delete(library::remove_favorite),
        )
        .route(
            "/library/presets",
            axum::routing::get(library::list_presets).post(library::create_preset),
        )
        .route(
            "/library/presets/{id}",
            axum::routing::get(library::get_preset)
                .put(library::update_preset)
                .delete(library::delete_preset),
        )
        .route(
            "/library/presets/{id}/apply",
            axum::routing::post(library::apply_preset),
        )
        .route(
            "/library/playlists",
            axum::routing::get(library::list_playlists).post(library::create_playlist),
        )
        .route(
            "/library/playlists/active",
            axum::routing::get(library::get_active_playlist),
        )
        .route(
            "/library/playlists/stop",
            axum::routing::post(library::stop_playlist),
        )
        .route(
            "/library/playlists/{id}",
            axum::routing::get(library::get_playlist)
                .put(library::update_playlist)
                .delete(library::delete_playlist),
        )
        .route(
            "/library/playlists/{id}/activate",
            axum::routing::post(library::activate_playlist),
        )
        // ── System ───────────────────────────────────────────────────
        .route("/server", axum::routing::get(system::get_server))
        .route("/status", axum::routing::get(system::get_status))
        .route("/state", axum::routing::get(system::get_status))
        .route("/system/sensors", axum::routing::get(system::get_sensors))
        .route(
            "/system/sensors/{label}",
            axum::routing::get(system::get_sensor),
        )
        .route("/audio/devices", axum::routing::get(settings::list_audio_devices))
        .route(
            "/settings/brightness",
            axum::routing::get(settings::get_brightness).put(settings::set_brightness),
        )
        // ── Preview ──────────────────────────────────────────────────
        .route("/preview", axum::routing::get(preview::preview_page))
        // ── Config ───────────────────────────────────────────────────
        .route("/config", axum::routing::get(config::show_config))
        .route("/config/get", axum::routing::get(config::get_config_value))
        .route("/config/set", axum::routing::post(config::set_config_value))
        .route(
            "/config/reset",
            axum::routing::post(config::reset_config_value),
        )
        // ── Diagnostics ──────────────────────────────────────────────
        .route("/diagnose", axum::routing::post(diagnose::run_diagnostics))
        // ── WebSocket ────────────────────────────────────────────────
        .route("/ws", axum::routing::get(ws::ws_handler));
    let mut router = Router::new()
        .nest("/api/v1", api)
        // Compatibility alias for clients still using the legacy top-level WS path.
        .route("/ws", axum::routing::get(ws::ws_handler))
        .route("/preview", axum::routing::get(preview::preview_page))
        .route("/health", axum::routing::get(system::health_check));

    if mcp_config.enabled {
        router = router.merge(crate::mcp::build_router(Arc::clone(&state), &mcp_config));
    }

    // Serve the web UI with SPA fallback when a UI directory is configured.
    if let Some(ui_path) = ui_dir {
        let index = ui_path.join("index.html");
        router = router.fallback_service(ServeDir::new(ui_path).fallback(ServeFile::new(index)));
    }

    router
        .layer(axum::middleware::from_fn_with_state(
            security_state,
            security::enforce_security,
        ))
        .layer(
            CorsLayer::new()
                .allow_origin(loopback_cors_origins())
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

fn loopback_cors_origins() -> AllowOrigin {
    AllowOrigin::predicate(|origin: &HeaderValue, _| is_loopback_origin(origin))
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
