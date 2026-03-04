//! REST API and WebSocket server for the Hypercolor daemon.
//!
//! Assembles all route groups into a single [`axum::Router`] and provides
//! the shared [`AppState`] that every handler receives via Axum's
//! [`State`](axum::extract::State) extractor.

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
pub mod system;
pub mod ws;

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use axum::Router;
use tokio::sync::{Mutex, RwLock};
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;
use tracing::warn;

use hypercolor_core::bus::HypercolorBus;
use hypercolor_core::config::ConfigManager;
use hypercolor_core::device::{BackendManager, DeviceRegistry};
use hypercolor_core::effect::{EffectEngine, EffectRegistry};
use hypercolor_core::engine::RenderLoop;
use hypercolor_core::scene::SceneManager;
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_types::spatial::SpatialLayout;

use crate::library::{InMemoryLibraryStore, JsonLibraryStore, LibraryStore};
use crate::playlist_runtime::PlaylistRuntimeState;

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

    /// Configuration manager for config API endpoints.
    pub config_manager: Option<Arc<ConfigManager>>,

    /// Global discovery scan lock flag shared across startup/API entrypoints.
    pub discovery_in_progress: Arc<AtomicBool>,

    /// In-memory profile store.
    pub profiles: RwLock<HashMap<String, profiles::Profile>>,

    /// In-memory layout store.
    pub layouts: RwLock<HashMap<String, SpatialLayout>>,

    /// Saved effect library storage (favorites, presets, playlists).
    pub library_store: Arc<dyn LibraryStore>,

    /// Active playlist runner state (single background worker at a time).
    pub playlist_runtime: Arc<Mutex<PlaylistRuntimeState>>,

    /// Daemon start time for uptime calculation.
    pub start_time: Instant,
}

impl AppState {
    /// Create a new `AppState` with default empty subsystems.
    ///
    /// Primarily useful for testing. In production, prefer
    /// [`from_daemon_state`](Self::from_daemon_state) to share subsystems
    /// with the daemon lifecycle.
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

        Self {
            device_registry: DeviceRegistry::new(),
            effect_registry: Arc::new(RwLock::new(EffectRegistry::default())),
            effect_engine: Arc::new(Mutex::new(EffectEngine::new())),
            scene_manager: Arc::new(RwLock::new(SceneManager::new())),
            event_bus: Arc::new(HypercolorBus::new()),
            render_loop: Arc::new(RwLock::new(RenderLoop::new(60))),
            spatial_engine: Arc::new(RwLock::new(SpatialEngine::new(default_layout))),
            backend_manager: Arc::new(Mutex::new(BackendManager::new())),
            config_manager: None,
            discovery_in_progress: Arc::new(AtomicBool::new(false)),
            profiles: RwLock::new(HashMap::new()),
            layouts: RwLock::new(HashMap::new()),
            library_store: Arc::new(InMemoryLibraryStore::new()),
            playlist_runtime: Arc::new(Mutex::new(PlaylistRuntimeState::new())),
            start_time: Instant::now(),
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

        Self {
            device_registry: daemon.device_registry.clone(),
            effect_registry: Arc::clone(&daemon.effect_registry),
            effect_engine: Arc::clone(&daemon.effect_engine),
            scene_manager: Arc::clone(&daemon.scene_manager),
            event_bus: Arc::clone(&daemon.event_bus),
            render_loop: Arc::clone(&daemon.render_loop),
            spatial_engine: Arc::clone(&daemon.spatial_engine),
            backend_manager: Arc::clone(&daemon.backend_manager),
            config_manager: Some(Arc::clone(&daemon.config_manager)),
            discovery_in_progress: Arc::clone(&daemon.discovery_in_progress),
            profiles: RwLock::new(HashMap::new()),
            layouts: RwLock::new(HashMap::new()),
            library_store,
            playlist_runtime: Arc::new(Mutex::new(PlaylistRuntimeState::new())),
            start_time: daemon.start_time,
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
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
            "/devices/{id}",
            axum::routing::get(devices::get_device)
                .put(devices::update_device)
                .delete(devices::delete_device),
        )
        .route(
            "/devices/{id}/identify",
            axum::routing::post(devices::identify_device),
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
        .route("/effects/stop", axum::routing::post(effects::stop_effect))
        .route("/effects/{id}", axum::routing::get(effects::get_effect))
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
        .route("/layouts/active", axum::routing::get(layouts::get_active_layout))
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
        .route("/status", axum::routing::get(system::get_status))
        .route("/state", axum::routing::get(system::get_status))
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

    // Serve the web UI with SPA fallback when a UI directory is configured.
    if let Some(ui_path) = ui_dir {
        let index = ui_path.join("index.html");
        router = router
            .fallback_service(ServeDir::new(ui_path).not_found_service(ServeFile::new(index)));
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
