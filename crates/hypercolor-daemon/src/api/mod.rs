//! REST API and WebSocket server for the Hypercolor daemon.
//!
//! Assembles all route groups into a single [`axum::Router`] and provides
//! the shared [`AppState`] that every handler receives via Axum's
//! [`State`](axum::extract::State) extractor.

pub mod devices;
pub mod effects;
pub mod envelope;
pub mod layouts;
pub mod profiles;
pub mod scenes;
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
use hypercolor_core::device::DeviceRegistry;
use hypercolor_core::effect::{EffectEngine, EffectRegistry};
use hypercolor_core::scene::SceneManager;
use hypercolor_types::spatial::SpatialLayout;

// ── AppState ─────────────────────────────────────────────────────────────

/// Shared application state injected into every API handler.
///
/// All fields are wrapped in `Arc` or interior-mutable containers so
/// the state can be cloned cheaply across Axum's task pool.
///
/// `EffectEngine` uses `Mutex` rather than `RwLock` because
/// `dyn EffectRenderer` is `Send` but not `Sync`.
pub struct AppState {
    /// Device tracking and lifecycle management.
    pub device_registry: DeviceRegistry,

    /// Effect catalog (metadata, search, categories).
    pub effect_registry: RwLock<EffectRegistry>,

    /// Active effect lifecycle and frame production.
    /// Uses `Mutex` because `EffectEngine` contains `dyn EffectRenderer`
    /// which is `Send` but not `Sync`.
    pub effect_engine: Mutex<EffectEngine>,

    /// Scene CRUD, priority stack, and transitions.
    pub scene_manager: RwLock<SceneManager>,

    /// System-wide event bus (broadcast + watch channels).
    pub event_bus: Arc<HypercolorBus>,

    /// In-memory profile store.
    pub profiles: RwLock<HashMap<String, profiles::Profile>>,

    /// In-memory layout store.
    pub layouts: RwLock<HashMap<String, SpatialLayout>>,

    /// Daemon start time for uptime calculation.
    pub start_time: Instant,
}

impl AppState {
    /// Create a new `AppState` with default empty subsystems.
    pub fn new() -> Self {
        Self {
            device_registry: DeviceRegistry::new(),
            effect_registry: RwLock::new(EffectRegistry::default()),
            effect_engine: Mutex::new(EffectEngine::new()),
            scene_manager: RwLock::new(SceneManager::new()),
            event_bus: Arc::new(HypercolorBus::new()),
            profiles: RwLock::new(HashMap::new()),
            layouts: RwLock::new(HashMap::new()),
            start_time: Instant::now(),
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
        // ── WebSocket ────────────────────────────────────────────────
        .route("/ws", axum::routing::get(ws::ws_handler));

    Router::new()
        .nest("/api/v1", api)
        .route("/health", axum::routing::get(system::health_check))
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
