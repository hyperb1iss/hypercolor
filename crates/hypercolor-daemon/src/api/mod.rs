//! REST API and WebSocket server for the Hypercolor daemon.
//!
//! Assembles all route groups into a single [`axum::Router`] and provides
//! the shared [`AppState`] that every handler receives via Axum's
//! [`State`](axum::extract::State) extractor.

pub mod attachments;
pub mod config;
pub mod control_values;
pub mod devices;
pub mod diagnose;
pub mod effects;
pub mod envelope;
pub mod layouts;
pub mod library;
pub mod preview;
pub mod profiles;
pub mod scenes;
pub mod security;
pub mod settings;
pub mod system;
pub mod ws;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use axum::Router;
use tokio::sync::{Mutex, RwLock, watch};
use tokio::task::JoinHandle;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;
use tracing::warn;

use hypercolor_core::attachment::AttachmentRegistry;
use hypercolor_core::bus::HypercolorBus;
use hypercolor_core::config::ConfigManager;
use hypercolor_core::device::net::CredentialStore;
use hypercolor_core::device::{
    BackendManager, DeviceLifecycleManager, DeviceRegistry, UsbProtocolConfigStore,
};
use hypercolor_core::effect::{EffectEngine, EffectRegistry};
use hypercolor_core::engine::RenderLoop;
use hypercolor_core::input::InputManager;
use hypercolor_core::scene::SceneManager;
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_types::config::McpConfig;
use hypercolor_types::device::DeviceId;
use hypercolor_types::server::ServerIdentity;
use hypercolor_types::spatial::SpatialLayout;

use crate::attachment_profiles::AttachmentProfileStore;
use crate::device_settings::DeviceSettingsStore;
use crate::layout_auto_exclusions;
use crate::library::{InMemoryLibraryStore, JsonLibraryStore, LibraryStore};
use crate::logical_devices::LogicalDevice;
use crate::performance::PerformanceTracker;
use crate::playlist_runtime::PlaylistRuntimeState;
use crate::profile_store::ProfileStore;
use crate::runtime_state;
use crate::session::{OutputPowerState, current_global_brightness};

// ── AppState ─────────────────────────────────────────────────────────────

/// Shared application state injected into every API handler.
///
/// All fields are wrapped in `Arc` or interior-mutable containers so
/// the state can be cloned cheaply across Axum's task pool.
///
/// `EffectEngine` uses `Mutex` rather than `RwLock` because
/// `dyn EffectRenderer` is `Send` but not `Sync`.
///
/// The `effect_engine`, `scene_manager`, and `render_loop` fields are
/// `Arc`-wrapped so they can be shared with the daemon's live instances
/// via [`from_daemon_state`](Self::from_daemon_state). This guarantees
/// that API calls operate on the same subsystems as the render pipeline.
pub struct AppState {
    /// Device tracking and lifecycle management.
    pub device_registry: DeviceRegistry,

    /// Effect catalog (metadata, search, categories).
    pub effect_registry: Arc<RwLock<EffectRegistry>>,

    /// Active effect lifecycle and frame production.
    /// Uses `Mutex` because `EffectEngine` contains `dyn EffectRenderer`
    /// which is `Send` but not `Sync`.
    pub effect_engine: Arc<Mutex<EffectEngine>>,

    /// Scene CRUD, priority stack, and transitions.
    pub scene_manager: Arc<RwLock<SceneManager>>,

    /// System-wide event bus (broadcast + watch channels).
    pub event_bus: Arc<HypercolorBus>,

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

    /// Shared encrypted credential store for network-authenticated backends.
    pub credential_store: Arc<CredentialStore>,

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

    /// Saved effect library storage (favorites, presets, playlists).
    pub library_store: Arc<dyn LibraryStore>,

    /// Active playlist runner state (single background worker at a time).
    pub playlist_runtime: Arc<Mutex<PlaylistRuntimeState>>,

    /// Daemon start time for uptime calculation.
    pub start_time: Instant,

    /// Stable network identity exposed by API and discovery surfaces.
    pub server_identity: ServerIdentity,
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
            groups: Vec::new(),
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
        let initial_global_brightness = device_settings.global_brightness();
        let (power_state, _) = watch::channel(OutputPowerState {
            global_brightness: initial_global_brightness,
            ..OutputPowerState::default()
        });
        let credential_store = Arc::new(
            CredentialStore::open_blocking(&ConfigManager::data_dir())
                .expect("default app state should open credential store"),
        );

        Self {
            device_registry: DeviceRegistry::new(),
            effect_registry: Arc::new(RwLock::new(EffectRegistry::default())),
            effect_engine: Arc::new(Mutex::new(EffectEngine::new())),
            scene_manager: Arc::new(RwLock::new(SceneManager::new())),
            event_bus: Arc::new(HypercolorBus::new()),
            render_loop: Arc::new(RwLock::new(RenderLoop::new(60))),
            spatial_engine: Arc::new(RwLock::new(SpatialEngine::new(default_layout))),
            backend_manager: Arc::new(Mutex::new(BackendManager::new())),
            usb_protocol_configs: UsbProtocolConfigStore::new(),
            performance: Arc::new(RwLock::new(PerformanceTracker::default())),
            lifecycle_manager: Arc::new(Mutex::new(DeviceLifecycleManager::new())),
            reconnect_tasks: Arc::new(StdMutex::new(HashMap::new())),
            config_manager: None,
            input_manager: Arc::new(Mutex::new(InputManager::new())),
            discovery_in_progress: Arc::new(AtomicBool::new(false)),
            profiles: Arc::new(RwLock::new(profiles)),
            attachment_registry: Arc::new(RwLock::new(attachment_registry)),
            attachment_profiles: Arc::new(RwLock::new(attachment_profiles)),
            device_settings: Arc::new(RwLock::new(device_settings)),
            credential_store,
            layouts: Arc::new(RwLock::new(HashMap::new())),
            layouts_path: ConfigManager::data_dir().join("layouts.json"),
            layout_auto_exclusions: Arc::new(RwLock::new(HashMap::new())),
            layout_auto_exclusions_path: ConfigManager::data_dir()
                .join("layout-auto-exclusions.json"),
            logical_devices: Arc::new(RwLock::new(HashMap::new())),
            logical_devices_path: ConfigManager::data_dir().join("logical-devices.json"),
            effect_layout_links: Arc::new(RwLock::new(HashMap::new())),
            effect_layout_links_path: ConfigManager::data_dir().join("effect-layouts.json"),
            runtime_state_path: ConfigManager::data_dir().join("runtime-state.json"),
            power_state,
            library_store: Arc::new(InMemoryLibraryStore::new()),
            playlist_runtime: Arc::new(Mutex::new(PlaylistRuntimeState::new())),
            start_time: Instant::now(),
            server_identity: ServerIdentity {
                instance_id: "00000000-0000-7000-8000-000000000000".to_owned(),
                instance_name: "hypercolor".to_owned(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
            },
        }
    }

    /// Create an `AppState` from a live [`DaemonState`](crate::startup::DaemonState).
    ///
    /// The device registry is cloned (it's internally `Arc`-wrapped).
    /// The effect engine, scene manager, render loop, and event bus are
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
                "Failed to load profiles; starting with empty store"
            );
            ProfileStore::new(profiles_path)
        });

        Self {
            device_registry: daemon.device_registry.clone(),
            effect_registry: Arc::clone(&daemon.effect_registry),
            effect_engine: Arc::clone(&daemon.effect_engine),
            scene_manager: Arc::clone(&daemon.scene_manager),
            event_bus: Arc::clone(&daemon.event_bus),
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
            credential_store: Arc::clone(&daemon.credential_store),
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
            library_store,
            playlist_runtime: Arc::new(Mutex::new(PlaylistRuntimeState::new())),
            start_time: daemon.start_time,
            server_identity: daemon.server_identity.clone(),
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

/// Persist the current runtime session snapshot (active effect/preset/controls/layout).
pub(crate) async fn persist_runtime_session(state: &Arc<AppState>) {
    let mut snapshot = {
        let engine = state.effect_engine.lock().await;
        runtime_state::snapshot_from_engine(&engine)
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

    if let Err(error) = runtime_state::save(&state.runtime_state_path, &snapshot) {
        warn!(
            path = %state.runtime_state_path.display(),
            %error,
            "Failed to persist runtime session snapshot"
        );
    }
}

pub(crate) fn discovery_runtime(state: &AppState) -> crate::discovery::DiscoveryRuntime {
    crate::discovery::DiscoveryRuntime {
        device_registry: state.device_registry.clone(),
        backend_manager: Arc::clone(&state.backend_manager),
        lifecycle_manager: Arc::clone(&state.lifecycle_manager),
        reconnect_tasks: Arc::clone(&state.reconnect_tasks),
        event_bus: Arc::clone(&state.event_bus),
        spatial_engine: Arc::clone(&state.spatial_engine),
        layouts: Arc::clone(&state.layouts),
        layouts_path: state.layouts_path.clone(),
        layout_auto_exclusions: Arc::clone(&state.layout_auto_exclusions),
        logical_devices: Arc::clone(&state.logical_devices),
        attachment_registry: Arc::clone(&state.attachment_registry),
        attachment_profiles: Arc::clone(&state.attachment_profiles),
        device_settings: Arc::clone(&state.device_settings),
        runtime_state_path: state.runtime_state_path.clone(),
        usb_protocol_configs: state.usb_protocol_configs.clone(),
        credential_store: Arc::clone(&state.credential_store),
        in_progress: Arc::clone(&state.discovery_in_progress),
        task_spawner: tokio::runtime::Handle::current(),
    }
}

// ── Router ───────────────────────────────────────────────────────────────

/// Build the complete Axum router with all API routes and middleware.
///
/// When `ui_dir` is provided, static files are served at `/` with SPA
/// fallback (all non-API, non-asset paths return `index.html`).
#[expect(clippy::too_many_lines)]
pub fn build_router(state: Arc<AppState>, ui_dir: Option<&Path>) -> Router {
    let security_state = security::SecurityState::from_env();
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
    #[cfg(feature = "hue")]
    let api = api.route(
        "/devices/pair/hue",
        axum::routing::post(devices::pair_hue_device),
    );
    #[cfg(feature = "nanoleaf")]
    let api = api.route(
        "/devices/pair/nanoleaf",
        axum::routing::post(devices::pair_nanoleaf_device),
    );

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
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
