//! Daemon startup orchestration, state management, and graceful shutdown.
//!
//! [`DaemonState`] is the top-level container for all subsystems. It wires
//! together configuration, the device registry, effect engine, spatial engine,
//! backend manager, scene manager, event bus, and render loop — then exposes
//! [`start`](DaemonState::start) and [`shutdown`](DaemonState::shutdown) for
//! lifecycle management.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use anyhow::{Context, Result};
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use hypercolor_core::bus::HypercolorBus;
use hypercolor_core::config::ConfigManager;
use hypercolor_core::device::mock::MockDeviceBackend;
use hypercolor_core::device::openrgb::{ClientConfig as OpenRgbClientConfig, OpenRgbBackend};
use hypercolor_core::device::wled::WledBackend;
use hypercolor_core::device::{
    BackendManager, DeviceLifecycleManager, DeviceRegistry, UsbBackend, UsbHotplugEvent,
    UsbHotplugMonitor,
};
use hypercolor_core::effect::builtin::register_builtin_effects;
use hypercolor_core::effect::{
    EffectEngine, EffectRegistry, default_effect_search_paths, register_html_effects,
};
use hypercolor_core::engine::RenderLoop;
use hypercolor_core::input::InputManager;
use hypercolor_core::input::audio::AudioInput;
use hypercolor_core::input::screen::{
    CaptureConfig as ScreenCaptureConfig, MonitorSelect, ScreenCaptureInput,
};
use hypercolor_core::scene::SceneManager;
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_types::audio::{AudioPipelineConfig, AudioSourceType};
use hypercolor_types::config::HypercolorConfig;
use hypercolor_types::device::DeviceId;
use hypercolor_types::spatial::{EdgeBehavior, SamplingMode, SpatialLayout};

use crate::effect_layouts;
use crate::logical_devices::LogicalDevice;
use crate::render_thread::{RenderThread, RenderThreadState};
use crate::{discovery, discovery::DiscoveryBackend};

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

    /// Effect catalog — metadata, search, categories for all known effects.
    pub effect_registry: Arc<RwLock<EffectRegistry>>,

    /// Scene manager — scene lifecycle, priority stack, transitions.
    pub scene_manager: Arc<RwLock<SceneManager>>,

    /// Event bus — broadcast events, frame data, spectrum data.
    pub event_bus: Arc<HypercolorBus>,

    /// Render loop — frame timing and FPS tier management.
    pub render_loop: Arc<RwLock<RenderLoop>>,

    /// Spatial sampling engine — maps canvas pixels to LED positions.
    pub spatial_engine: Arc<RwLock<SpatialEngine>>,

    /// Device backend router — pushes colors to hardware.
    pub backend_manager: Arc<Mutex<BackendManager>>,

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

    /// Persisted effect -> layout association map.
    pub effect_layout_links: Arc<RwLock<HashMap<String, String>>>,

    /// Persistent JSON file for effect -> layout associations.
    pub effect_layout_links_path: PathBuf,

    /// Global discovery scan lock shared across startup and API-triggered scans.
    pub discovery_in_progress: Arc<AtomicBool>,

    /// Handle to the running render thread (if started).
    render_thread: Option<RenderThread>,

    /// Periodic discovery worker task.
    discovery_task: Option<tokio::task::JoinHandle<()>>,

    /// Wall-clock reference for daemon uptime reporting.
    pub start_time: Instant,
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

        // ── Effect Registry ─────────────────────────────────────────────
        let effect_search_paths =
            default_effect_search_paths(&config.effect_engine.extra_effect_dirs);
        let mut effect_registry = EffectRegistry::new(effect_search_paths.clone());
        register_builtin_effects(&mut effect_registry);
        let builtin_count = effect_registry.len();
        let html_report = register_html_effects(&mut effect_registry, &effect_search_paths);
        let effect_registry = Arc::new(RwLock::new(effect_registry));
        info!(
            builtins = builtin_count,
            html_scanned = html_report.scanned_files,
            html_loaded = html_report.loaded_effects,
            html_replaced = html_report.replaced_effects,
            html_skipped = html_report.skipped_files,
            html_failed = html_report.failed_files(),
            "Effect registry created"
        );

        // ── Scene Manager ───────────────────────────────────────────────
        let scene_manager = Arc::new(RwLock::new(SceneManager::new()));
        info!("Scene manager created");

        // ── Render Loop ─────────────────────────────────────────────────
        let render_loop = RenderLoop::new(config.daemon.target_fps);
        let render_loop = Arc::new(RwLock::new(render_loop));
        info!(target_fps = config.daemon.target_fps, "Render loop created");

        // ── Spatial Engine ──────────────────────────────────────────────
        let default_layout = SpatialLayout {
            id: "default".into(),
            name: "Default Layout".into(),
            description: None,
            canvas_width: config.daemon.canvas_width,
            canvas_height: config.daemon.canvas_height,
            zones: Vec::new(),
            default_sampling_mode: SamplingMode::Bilinear,
            default_edge_behavior: EdgeBehavior::Clamp,
            spaces: None,
            version: 1,
        };
        let spatial_engine = Arc::new(RwLock::new(SpatialEngine::new(default_layout)));
        info!("Spatial engine created (empty default layout)");

        // ── Backend Manager ─────────────────────────────────────────────
        let mut backend_manager_inner = BackendManager::new();
        backend_manager_inner.register_backend(Box::new(MockDeviceBackend::new()));
        backend_manager_inner.register_backend(Box::new(OpenRgbBackend::new(
            OpenRgbClientConfig {
                host: config.discovery.openrgb_host.clone(),
                port: config.discovery.openrgb_port,
                ..OpenRgbClientConfig::default()
            },
        )));
        if config.discovery.wled_scan {
            backend_manager_inner
                .register_backend(Box::new(WledBackend::with_mdns_fallback(Vec::new(), true)));
        }
        backend_manager_inner.register_backend(Box::new(UsbBackend::new()));
        let backend_manager = Arc::new(Mutex::new(backend_manager_inner));
        info!("Backend manager created");

        // ── Device Lifecycle Manager ───────────────────────────────────
        let lifecycle_manager = Arc::new(Mutex::new(DeviceLifecycleManager::new()));
        let reconnect_tasks = Arc::new(StdMutex::new(HashMap::new()));
        info!("Device lifecycle manager created");

        // ── Input Manager ───────────────────────────────────────────────
        let input_manager = Arc::new(Mutex::new(build_input_manager(config)));
        info!(
            audio_enabled = config.audio.enabled,
            capture_enabled = config.capture.enabled,
            "Input manager created"
        );

        // ── Logical Device Store ─────────────────────────────────────
        let logical_devices_path = ConfigManager::data_dir().join("logical-devices.json");
        let persisted_segments = match crate::logical_devices::load_segments(&logical_devices_path)
        {
            Ok(entries) => entries,
            Err(error) => {
                warn!(
                    path = %logical_devices_path.display(),
                    %error,
                    "Failed to load persisted logical devices; starting with empty store"
                );
                HashMap::new()
            }
        };
        let logical_devices = Arc::new(RwLock::new(persisted_segments));
        info!(path = %logical_devices_path.display(), "Logical device store ready");

        // ── Effect/Layout Association Store ──────────────────────────
        let effect_layout_links_path = ConfigManager::data_dir().join("effect-layouts.json");
        let persisted_links = match effect_layouts::load(&effect_layout_links_path) {
            Ok(entries) => entries,
            Err(error) => {
                warn!(
                    path = %effect_layout_links_path.display(),
                    %error,
                    "Failed to load effect/layout associations; starting with empty store"
                );
                HashMap::new()
            }
        };
        let effect_layout_links = Arc::new(RwLock::new(persisted_links));
        info!(path = %effect_layout_links_path.display(), "Effect/layout association store ready");

        info!("All subsystems initialized");

        Ok(Self {
            config_manager,
            device_registry,
            effect_engine,
            effect_registry,
            scene_manager,
            event_bus,
            render_loop,
            spatial_engine,
            backend_manager,
            lifecycle_manager,
            reconnect_tasks,
            input_manager,
            logical_devices,
            logical_devices_path,
            effect_layout_links,
            effect_layout_links_path,
            discovery_in_progress: Arc::new(AtomicBool::new(false)),
            render_thread: None,
            discovery_task: None,
            start_time: Instant::now(),
        })
    }

    /// Read a snapshot of the current configuration.
    ///
    /// Lock-free via `arc_swap` — cheap to call from any context.
    pub fn config(&self) -> Arc<HypercolorConfig> {
        Arc::clone(&self.config_manager.get())
    }

    /// Start all subsystems — render loop, render thread, backend discovery.
    ///
    /// After this call the daemon is fully operational and processing frames.
    ///
    /// # Errors
    ///
    /// Returns an error if any subsystem fails to start.
    pub async fn start(&mut self) -> Result<()> {
        let config = self.config();
        info!(
            listen = %config.daemon.listen_address,
            port = config.daemon.port,
            target_fps = config.daemon.target_fps,
            "Starting daemon subsystems"
        );

        // Start configured input sources.
        {
            let mut input_manager = self.input_manager.lock().await;
            input_manager
                .start_all()
                .context("failed to start input sources")?;
        }

        // Start the render loop.
        {
            let mut loop_guard = self.render_loop.write().await;
            loop_guard.start();
        }

        // Spawn the render thread.
        let rt_state = RenderThreadState {
            effect_engine: Arc::clone(&self.effect_engine),
            spatial_engine: Arc::clone(&self.spatial_engine),
            backend_manager: Arc::clone(&self.backend_manager),
            event_bus: Arc::clone(&self.event_bus),
            render_loop: Arc::clone(&self.render_loop),
            input_manager: Arc::clone(&self.input_manager),
            canvas_width: config.daemon.canvas_width,
            canvas_height: config.daemon.canvas_height,
            screen_capture_enabled: config.capture.enabled,
        };
        self.render_thread = Some(RenderThread::spawn(rt_state));

        // Publish a startup event so subscribers know the daemon is alive.
        let device_count = self.device_registry.len().await;
        let effect_count = {
            let reg = self.effect_registry.read().await;
            reg.len()
        };
        self.event_bus
            .publish(hypercolor_types::event::HypercolorEvent::DaemonStarted {
                version: env!("CARGO_PKG_VERSION").to_string(),
                pid: std::process::id(),
                device_count: u32::try_from(device_count).unwrap_or(u32::MAX),
                effect_count: u32::try_from(effect_count).unwrap_or(u32::MAX),
            });

        self.spawn_discovery_worker(Arc::clone(&config));

        info!("Daemon is running");
        Ok(())
    }

    /// Graceful shutdown — stops all subsystems in reverse-dependency order.
    ///
    /// Sequence:
    /// 1. Stop render loop (no more frames produced)
    /// 2. Wait for render thread to exit
    /// 3. Deactivate the effect engine (release renderer resources)
    /// 4. Scene manager cleanup
    /// 5. Log final state
    ///
    /// # Errors
    ///
    /// Returns an error if any shutdown step fails critically. Non-critical
    /// failures are logged as warnings and do not prevent the rest of the
    /// sequence from completing.
    pub async fn shutdown(&mut self) -> Result<()> {
        info!("Beginning graceful shutdown");

        // 1. Stop render loop — next tick() will return false.
        {
            let mut loop_guard = self.render_loop.write().await;
            loop_guard.stop();
        }
        info!("Render loop stopped");

        // 2. Wait for render thread to exit.
        if let Some(mut rt) = self.render_thread.take() {
            if let Err(e) = rt.shutdown().await {
                warn!(error = %e, "render thread shutdown error");
            }
        }

        if let Some(handle) = self.discovery_task.take() {
            handle.abort();
        }

        {
            let mut reconnect_tasks = self
                .reconnect_tasks
                .lock()
                .expect("reconnect task map lock poisoned");
            for (_id, handle) in reconnect_tasks.drain() {
                handle.abort();
            }
        }

        // 3. Stop input sources.
        {
            let mut input_manager = self.input_manager.lock().await;
            input_manager.stop_all();
        }
        info!("Input sources stopped");

        // 4. Deactivate effect engine.
        {
            let mut engine_guard = self.effect_engine.lock().await;
            engine_guard.deactivate();
        }
        info!("Effect engine deactivated");

        // 5. Scene manager — deactivate current scene.
        {
            let mut scene_guard = self.scene_manager.write().await;
            scene_guard.deactivate_current();
        }
        info!("Scene manager cleaned up");

        // 6. Log final device count.
        let device_count = self.device_registry.len().await;
        info!(devices = device_count, "Device registry final state");

        // 7. Publish shutdown event.
        self.event_bus
            .publish(hypercolor_types::event::HypercolorEvent::DaemonShutdown {
                reason: "signal".to_string(),
            });

        info!("Graceful shutdown complete");
        Ok(())
    }

    fn spawn_discovery_worker(&mut self, config: Arc<HypercolorConfig>) {
        let worker = DiscoveryWorkerContext {
            device_registry: self.device_registry.clone(),
            backend_manager: Arc::clone(&self.backend_manager),
            lifecycle_manager: Arc::clone(&self.lifecycle_manager),
            reconnect_tasks: Arc::clone(&self.reconnect_tasks),
            event_bus: Arc::clone(&self.event_bus),
            config_manager: Arc::clone(&self.config_manager),
            logical_devices: Arc::clone(&self.logical_devices),
            in_progress: Arc::clone(&self.discovery_in_progress),
        };

        let initial_backends = match discovery::resolve_backends(None, &config) {
            Ok(backends) => backends,
            Err(error) => {
                warn!(error = %error, "Initial discovery backend resolution failed");
                Vec::<DiscoveryBackend>::new()
            }
        };
        let scan_interval =
            std::time::Duration::from_secs(config.discovery.scan_interval_secs.max(1));

        self.discovery_task = Some(tokio::spawn(async move {
            let hotplug_monitor = UsbHotplugMonitor::new(256);
            let mut hotplug_rx = hotplug_monitor.subscribe();
            let mut hotplug_task = match hotplug_monitor.start() {
                Ok(task) => {
                    info!("USB hotplug watcher started");
                    Some(task)
                }
                Err(error) => {
                    warn!(
                        error = %error,
                        "USB hotplug watcher failed to start; falling back to periodic scans"
                    );
                    None
                }
            };

            worker
                .run_scan_if_idle(
                    Arc::clone(&config),
                    initial_backends,
                    "Skipping initial discovery scan; scan already in progress",
                )
                .await;

            let mut ticker = tokio::time::interval(scan_interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            ticker.tick().await; // consume immediate tick

            loop {
                let run_periodic_scan = if hotplug_task.is_some() {
                    tokio::select! {
                        _ = ticker.tick() => true,
                        event = hotplug_rx.recv() => {
                            let run_usb_scan = match event {
                                Ok(UsbHotplugEvent::Arrived { vendor_id, product_id, descriptor }) => {
                                    info!(
                                        vendor_id,
                                        product_id,
                                        device = descriptor.name,
                                        "USB hotplug arrival detected"
                                    );
                                    true
                                }
                                Ok(UsbHotplugEvent::Removed { vendor_id, product_id }) => {
                                    info!(vendor_id, product_id, "USB hotplug removal detected");
                                    true
                                }
                                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                                    warn!(skipped, "USB hotplug receiver lagged");
                                    false
                                }
                                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                                    warn!("USB hotplug event channel closed; disabling hotplug-triggered scans");
                                    if let Some(task) = hotplug_task.take() {
                                        task.abort();
                                    }
                                    false
                                }
                            };

                            if run_usb_scan {
                                worker.run_usb_hotplug_scan().await;
                            }
                            false
                        }
                    }
                } else {
                    ticker.tick().await;
                    true
                };

                if !run_periodic_scan {
                    continue;
                }

                worker.run_periodic_scan().await;
            }
        }));
    }
}

#[derive(Clone)]
struct DiscoveryWorkerContext {
    device_registry: DeviceRegistry,
    backend_manager: Arc<Mutex<BackendManager>>,
    lifecycle_manager: Arc<Mutex<DeviceLifecycleManager>>,
    reconnect_tasks: Arc<StdMutex<HashMap<DeviceId, JoinHandle<()>>>>,
    event_bus: Arc<HypercolorBus>,
    config_manager: Arc<ConfigManager>,
    logical_devices: Arc<RwLock<HashMap<String, LogicalDevice>>>,
    in_progress: Arc<AtomicBool>,
}

impl DiscoveryWorkerContext {
    fn runtime(&self) -> discovery::DiscoveryRuntime {
        discovery::DiscoveryRuntime {
            device_registry: self.device_registry.clone(),
            backend_manager: Arc::clone(&self.backend_manager),
            lifecycle_manager: Arc::clone(&self.lifecycle_manager),
            reconnect_tasks: Arc::clone(&self.reconnect_tasks),
            event_bus: Arc::clone(&self.event_bus),
            logical_devices: Arc::clone(&self.logical_devices),
            in_progress: Arc::clone(&self.in_progress),
        }
    }

    async fn run_scan_if_idle(
        &self,
        config: Arc<HypercolorConfig>,
        backends: Vec<DiscoveryBackend>,
        busy_log: &'static str,
    ) {
        if backends.is_empty() {
            return;
        }

        if self
            .in_progress
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            debug!("{busy_log}");
            return;
        }

        let _ = discovery::execute_discovery_scan(
            self.runtime(),
            config,
            backends,
            discovery::default_timeout(),
        )
        .await;
    }

    async fn run_periodic_scan(&self) {
        let latest_config = Arc::clone(&self.config_manager.get());
        let backends = match discovery::resolve_backends(None, &latest_config) {
            Ok(backends) => backends,
            Err(error) => {
                warn!(
                    error = %error,
                    "Periodic discovery backend resolution failed; skipping interval"
                );
                return;
            }
        };

        self.run_scan_if_idle(
            latest_config,
            backends,
            "Skipping periodic discovery scan; scan already in progress",
        )
        .await;
    }

    async fn run_usb_hotplug_scan(&self) {
        self.run_scan_if_idle(
            Arc::clone(&self.config_manager.get()),
            vec![DiscoveryBackend::Usb],
            "Skipping USB hotplug scan; discovery already in progress",
        )
        .await;
    }
}

fn build_input_manager(config: &HypercolorConfig) -> InputManager {
    let mut input_manager = InputManager::new();

    if config.audio.enabled {
        let audio_pipeline_config = AudioPipelineConfig {
            source: audio_source_from_device(&config.audio.device),
            fft_size: usize::try_from(config.audio.fft_size).unwrap_or(1024),
            smoothing: config.audio.smoothing.clamp(0.0, 1.0),
            gain: 1.0,
            noise_floor: noise_gate_to_db(config.audio.noise_gate),
            beat_sensitivity: config.audio.beat_sensitivity.max(0.01),
        };
        let audio_input = AudioInput::new(&audio_pipeline_config)
            .with_name(format!("AudioInput({})", config.audio.device));
        input_manager.add_source(Box::new(audio_input));
    }

    if config.capture.enabled {
        let monitor = monitor_select_from_config(config.capture.monitor, &config.capture.source);
        let capture_config = ScreenCaptureConfig {
            monitor,
            target_fps: config.capture.capture_fps.max(1),
            ..ScreenCaptureConfig::default()
        };
        input_manager.add_source(Box::new(ScreenCaptureInput::new(capture_config)));
    }

    input_manager
}

fn audio_source_from_device(device: &str) -> AudioSourceType {
    let normalized = device.trim();
    if normalized.eq_ignore_ascii_case("none") {
        AudioSourceType::None
    } else if normalized.eq_ignore_ascii_case("auto") || normalized.eq_ignore_ascii_case("default")
    {
        AudioSourceType::SystemMonitor
    } else if normalized.eq_ignore_ascii_case("mic")
        || normalized.eq_ignore_ascii_case("microphone")
    {
        AudioSourceType::Microphone
    } else {
        AudioSourceType::Named(normalized.to_owned())
    }
}

fn monitor_select_from_config(monitor_index: u32, source: &str) -> MonitorSelect {
    let normalized = source.trim();
    if normalized.eq_ignore_ascii_case("auto") || normalized.eq_ignore_ascii_case("primary") {
        MonitorSelect::Primary
    } else if let Some(name) = normalized.strip_prefix("name:") {
        MonitorSelect::ByName(name.trim().to_owned())
    } else {
        MonitorSelect::ByIndex(monitor_index)
    }
}

fn noise_gate_to_db(noise_gate: f32) -> f32 {
    let linear = noise_gate.clamp(0.000_001, 1.0);
    20.0 * linear.log10()
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
