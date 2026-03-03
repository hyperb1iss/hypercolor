//! REST API and WebSocket server for the Hypercolor daemon.
//!
//! Assembles all route groups into a single [`axum::Router`] and provides
//! the shared [`AppState`] that every handler receives via Axum's
//! [`State`](axum::extract::State) extractor.

pub mod config;
pub mod devices;
pub mod diagnose;
pub mod effects;
pub mod envelope;
pub mod layouts;
pub mod profiles;
pub mod scenes;
pub mod security;
pub mod system;
pub mod ws;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use axum::Router;
use tokio::sync::{Mutex, RwLock};
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;

use hypercolor_core::bus::HypercolorBus;
use hypercolor_core::config::ConfigManager;
use hypercolor_core::device::{BackendManager, DeviceRegistry};
use hypercolor_core::effect::{EffectEngine, EffectRegistry};
use hypercolor_core::engine::RenderLoop;
use hypercolor_core::scene::SceneManager;
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_types::spatial::SpatialLayout;

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

    /// In-memory profile store.
    pub profiles: RwLock<HashMap<String, profiles::Profile>>,

    /// In-memory layout store.
    pub layouts: RwLock<HashMap<String, SpatialLayout>>,

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
            profiles: RwLock::new(HashMap::new()),
            layouts: RwLock::new(HashMap::new()),
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
            profiles: RwLock::new(HashMap::new()),
            layouts: RwLock::new(HashMap::new()),
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
pub fn build_router(state: Arc<AppState>) -> Router {
    let security_state = security::SecurityState::from_env();

    let api = Router::new()
        // ── Devices ──────────────────────────────────────────────────
        .route("/devices", axum::routing::get(devices::list_devices))
        .route(
            "/devices/discover",
            axum::routing::post(devices::discover_devices),
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
        .route(
            "/layouts/{id}",
            axum::routing::get(layouts::get_layout)
                .put(layouts::update_layout)
                .delete(layouts::delete_layout),
        )
        // ── System ───────────────────────────────────────────────────
        .route("/status", axum::routing::get(system::get_status))
        .route("/state", axum::routing::get(system::get_status))
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

    Router::new()
        .nest("/api/v1", api)
        .route("/health", axum::routing::get(system::health_check))
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
