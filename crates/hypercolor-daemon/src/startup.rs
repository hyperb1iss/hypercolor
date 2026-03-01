//! Daemon startup orchestration, state management, and graceful shutdown.
//!
//! [`DaemonState`] is the top-level container for all subsystems. It wires
//! together configuration, the device registry, effect engine, scene manager,
//! event bus, and render loop — then exposes [`start`](DaemonState::start) and
//! [`shutdown`](DaemonState::shutdown) for lifecycle management.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::sync::{Mutex, RwLock};
use tracing::{info, warn};

use hypercolor_core::bus::HypercolorBus;
use hypercolor_core::config::ConfigManager;
use hypercolor_core::device::DeviceRegistry;
use hypercolor_core::effect::EffectEngine;
use hypercolor_core::engine::RenderLoop;
use hypercolor_core::scene::SceneManager;
use hypercolor_types::config::HypercolorConfig;

// ── Config File Name ────────────────────────────────────────────────────────

/// Default configuration file name within the config directory.
const CONFIG_FILE_NAME: &str = "hypercolor.toml";

// ── DaemonState ─────────────────────────────────────────────────────────────

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

    /// Effect engine — manages the active effect renderer.
    ///
    /// Uses `Mutex` rather than `RwLock` because `EffectEngine` contains a
    /// `Box<dyn EffectRenderer>` which is `Send` but not `Sync`.
    pub effect_engine: Arc<Mutex<EffectEngine>>,

    /// Scene manager — scene lifecycle, priority stack, transitions.
    pub scene_manager: Arc<RwLock<SceneManager>>,

    /// Event bus — broadcast events, frame data, spectrum data.
    pub event_bus: Arc<HypercolorBus>,

    /// Render loop — frame timing and pipeline skeleton.
    pub render_loop: Arc<RwLock<RenderLoop>>,
}

impl DaemonState {
    /// Initialize all subsystems from a loaded configuration.
    ///
    /// This wires together the bus, registry, engines, and render loop
    /// but does **not** start any background tasks. Call [`start`](Self::start)
    /// to begin the render loop and device discovery.
    ///
    /// # Errors
    ///
    /// Returns an error if the config manager cannot be created from the
    /// resolved config path.
    pub fn initialize(config: &HypercolorConfig, config_path: PathBuf) -> Result<Self> {
        info!("Initializing daemon subsystems");

        // ── Configuration ───────────────────────────────────────────────
        let config_manager =
            ConfigManager::new(config_path).context("failed to initialize config manager")?;
        let config_manager = Arc::new(config_manager);

        // ── Event Bus ───────────────────────────────────────────────────
        let event_bus = Arc::new(HypercolorBus::new());
        info!("Event bus created");

        // ── Device Registry ─────────────────────────────────────────────
        let device_registry = DeviceRegistry::new();
        info!("Device registry created");

        // ── Effect Engine ───────────────────────────────────────────────
        let effect_engine = EffectEngine::new()
            .with_canvas_size(config.daemon.canvas_width, config.daemon.canvas_height);
        let effect_engine = Arc::new(Mutex::new(effect_engine));
        info!(
            canvas = format_args!(
                "{}x{}",
                config.daemon.canvas_width, config.daemon.canvas_height
            ),
            "Effect engine created"
        );

        // ── Scene Manager ───────────────────────────────────────────────
        let scene_manager = Arc::new(RwLock::new(SceneManager::new()));
        info!("Scene manager created");

        // ── Render Loop ─────────────────────────────────────────────────
        let render_loop = RenderLoop::new(config.daemon.target_fps);
        let render_loop = Arc::new(RwLock::new(render_loop));
        info!(target_fps = config.daemon.target_fps, "Render loop created");

        info!("All subsystems initialized");

        Ok(Self {
            config_manager,
            device_registry,
            effect_engine,
            scene_manager,
            event_bus,
            render_loop,
        })
    }

    /// Read a snapshot of the current configuration.
    ///
    /// Lock-free via `arc_swap` — cheap to call from any context.
    pub fn config(&self) -> Arc<HypercolorConfig> {
        Arc::clone(&self.config_manager.get())
    }

    /// Start all subsystems — render loop, backend discovery, etc.
    ///
    /// After this call the daemon is fully operational and processing frames.
    ///
    /// # Errors
    ///
    /// Returns an error if any subsystem fails to start.
    pub async fn start(&self) -> Result<()> {
        let config = self.config();
        info!(
            listen = %config.daemon.listen_address,
            port = config.daemon.port,
            target_fps = config.daemon.target_fps,
            "Starting daemon subsystems"
        );

        // Start the render loop.
        {
            let mut loop_guard = self.render_loop.write().await;
            loop_guard.start();
        }

        // Publish a startup event so subscribers know the daemon is alive.
        let device_count = self.device_registry.len().await;
        self.event_bus
            .publish(hypercolor_types::event::HypercolorEvent::DaemonStarted {
                version: env!("CARGO_PKG_VERSION").to_string(),
                pid: std::process::id(),
                device_count: u32::try_from(device_count).unwrap_or(u32::MAX),
                effect_count: 0,
            });

        info!("Daemon is running");
        Ok(())
    }

    /// Graceful shutdown — stops all subsystems in reverse-dependency order.
    ///
    /// Sequence:
    /// 1. Stop render loop (no more frames produced)
    /// 2. Deactivate the effect engine (release renderer resources)
    /// 3. Scene manager cleanup
    /// 4. Log final state
    ///
    /// # Errors
    ///
    /// Returns an error if any shutdown step fails critically. Non-critical
    /// failures are logged as warnings and do not prevent the rest of the
    /// sequence from completing.
    pub async fn shutdown(&self) -> Result<()> {
        info!("Beginning graceful shutdown");

        // 1. Stop render loop.
        {
            let mut loop_guard = self.render_loop.write().await;
            loop_guard.stop();
        }
        info!("Render loop stopped");

        // 2. Deactivate effect engine.
        {
            let mut engine_guard = self.effect_engine.lock().await;
            engine_guard.deactivate();
        }
        info!("Effect engine deactivated");

        // 3. Scene manager — deactivate current scene.
        {
            let mut scene_guard = self.scene_manager.write().await;
            scene_guard.deactivate_current();
        }
        info!("Scene manager cleaned up");

        // 4. Log final device count.
        let device_count = self.device_registry.len().await;
        info!(devices = device_count, "Device registry final state");

        // 5. Publish shutdown event.
        self.event_bus
            .publish(hypercolor_types::event::HypercolorEvent::DaemonShutdown {
                reason: "signal".to_string(),
            });

        info!("Graceful shutdown complete");
        Ok(())
    }
}

// ── Config Loading ──────────────────────────────────────────────────────────

/// Load and validate configuration from the filesystem.
///
/// Resolution order:
/// 1. Explicit path from `--config` CLI argument
/// 2. Platform-specific config directory (`$XDG_CONFIG_HOME/hypercolor/hypercolor.toml`
///    on Linux, `%APPDATA%\hypercolor\hypercolor.toml` on Windows)
/// 3. Fall back to compile-time defaults (no file needed)
///
/// # Errors
///
/// Returns an error if an explicit config path is provided but the file
/// cannot be read or parsed. When falling back to defaults, this always
/// succeeds.
#[expect(
    clippy::unused_async,
    reason = "will be async when config loading gains network support"
)]
pub async fn load_config(config_path: Option<&Path>) -> Result<(HypercolorConfig, PathBuf)> {
    let resolved_path = resolve_config_path(config_path);

    info!(path = %resolved_path.display(), "Resolved config path");

    if resolved_path.exists() {
        let config = ConfigManager::load(&resolved_path)
            .with_context(|| format!("failed to load config from {}", resolved_path.display()))?;
        info!(
            schema_version = config.schema_version,
            "Configuration loaded from file"
        );
        Ok((config, resolved_path))
    } else if config_path.is_some() {
        // Explicit path was given but doesn't exist — that's an error.
        anyhow::bail!("config file not found: {}", resolved_path.display());
    } else {
        // No explicit path, no file found — use defaults.
        warn!("No config file found, using built-in defaults");
        let config = default_config();
        Ok((config, resolved_path))
    }
}

/// Resolve which config file path to use.
///
/// If an explicit path is provided, it is used directly. Otherwise the
/// platform-specific config directory is checked for `hypercolor.toml`.
fn resolve_config_path(explicit: Option<&Path>) -> PathBuf {
    explicit.map_or_else(
        || ConfigManager::config_dir().join(CONFIG_FILE_NAME),
        Path::to_path_buf,
    )
}

/// Construct a default configuration (all defaults, current schema version).
pub fn default_config() -> HypercolorConfig {
    HypercolorConfig::default()
}

// ── Signal Handling ─────────────────────────────────────────────────────────

/// Install OS signal handlers for graceful shutdown.
///
/// Returns a watch receiver that flips to `true` when a shutdown signal
/// (Ctrl+C / `SIGTERM`) is received. The spawned task is fire-and-forget;
/// it exits after the first signal.
#[must_use]
pub fn install_signal_handlers() -> tokio::sync::watch::Receiver<bool> {
    let (tx, rx) = tokio::sync::watch::channel(false);

    tokio::spawn(async move {
        if let Err(e) = tokio::signal::ctrl_c().await {
            tracing::error!(error = %e, "Failed to listen for shutdown signal");
            return;
        }
        info!("Shutdown signal received (Ctrl+C)");
        let _ = tx.send(true);
    });

    rx
}

/// Parse a TOML string into a [`HypercolorConfig`].
///
/// Convenience wrapper around `toml::from_str` for tests and tooling.
///
/// # Errors
///
/// Returns an error if the TOML is malformed or cannot be deserialized.
pub fn parse_config_toml(toml_str: &str) -> Result<HypercolorConfig> {
    toml::from_str(toml_str).context("failed to parse config TOML")
}
