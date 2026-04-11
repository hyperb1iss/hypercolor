//! Subsystem initialization: bus, engines, managers, stores, and input sources.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use anyhow::{Context, Result};
use tokio::sync::{Mutex, RwLock, watch};
use tracing::{info, warn};

use hypercolor_core::attachment::AttachmentRegistry;
use hypercolor_core::bus::HypercolorBus;
use hypercolor_core::config::ConfigManager;
use hypercolor_core::device::mock::MockDeviceBackend;
use hypercolor_core::device::net::CredentialStore;
use hypercolor_core::device::{
    BackendManager, DeviceLifecycleManager, DeviceRegistry, SmBusBackend, UsbBackend,
    UsbProtocolConfigStore,
};
use hypercolor_core::effect::builtin::register_builtin_effects;
use hypercolor_core::effect::{
    EffectEngine, EffectRegistry, default_effect_search_paths, register_html_effects,
};
use hypercolor_core::engine::RenderLoop;
#[cfg(target_os = "linux")]
use hypercolor_core::input::EvdevKeyboardInput;
use hypercolor_core::input::audio::AudioInput;
#[cfg(target_os = "linux")]
use hypercolor_core::input::screen::WaylandScreenCaptureInput;
#[cfg(target_os = "linux")]
use hypercolor_core::input::screen::{CaptureConfig as ScreenCaptureConfig, MonitorSelect};
use hypercolor_core::input::{InputManager, InteractionInput, SensorPoller};
use hypercolor_core::scene::SceneManager;
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_types::audio::{AudioPipelineConfig, AudioSourceType};
use hypercolor_types::config::HypercolorConfig;
use hypercolor_types::spatial::{EdgeBehavior, SamplingMode, SpatialLayout};

use crate::attachment_profiles::AttachmentProfileStore;
use crate::device_settings::DeviceSettingsStore;
use crate::display_overlays::{DisplayOverlayRegistry, DisplayOverlayRuntimeRegistry};
use crate::effect_layouts;
use crate::layout_auto_exclusions;
use crate::network::{self, DaemonDriverHost};
use crate::performance::PerformanceTracker;
use crate::preview_runtime::PreviewRuntime;
use crate::scene_transactions::SceneTransactionQueue;
use crate::session::{OutputPowerState, set_global_brightness};

use super::DaemonState;
use super::config::resolve_server_identity;
use super::resolve_compositor_acceleration_mode;

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
    #[expect(
        clippy::too_many_lines,
        reason = "initialization is inherently sequential; splitting would scatter related setup across helpers"
    )]
    pub fn initialize(config: &HypercolorConfig, config_path: PathBuf) -> Result<Self> {
        info!("Initializing daemon subsystems");
        let render_acceleration =
            resolve_compositor_acceleration_mode(config.effect_engine.render_acceleration_mode)
                .context("failed to resolve compositor acceleration mode")?;
        if let Some(reason) = render_acceleration.fallback_reason {
            warn!(
                requested_mode = ?render_acceleration.requested_mode,
                effective_mode = ?render_acceleration.effective_mode,
                reason,
                "Requested compositor acceleration is unavailable; using CPU path"
            );
        } else {
            info!(
                effective_mode = ?render_acceleration.effective_mode,
                "Compositor acceleration resolved"
            );
        }
        if let Some(probe) = &render_acceleration.gpu_probe {
            info!(
                adapter = %probe.adapter_name,
                backend = probe.backend,
                texture_format = probe.texture_format,
                max_texture_dimension_2d = probe.max_texture_dimension_2d,
                max_storage_textures_per_shader_stage = probe.max_storage_textures_per_shader_stage,
                "SparkleFlinger GPU probe succeeded"
            );
        }

        let server_identity =
            resolve_server_identity(config).context("failed to resolve server identity")?;

        // ── Configuration ───────────────────────────────────────────────
        let config_manager =
            ConfigManager::new(config_path).context("failed to initialize config manager")?;
        let config_manager = Arc::new(config_manager);

        // ── Event Bus ───────────────────────────────────────────────────
        let event_bus = Arc::new(HypercolorBus::new());
        let preview_runtime = Arc::new(PreviewRuntime::new(Arc::clone(&event_bus)));
        info!("Event bus created");

        let (power_state, _) = watch::channel(OutputPowerState::default());
        let scene_transactions = SceneTransactionQueue::default();
        info!("Session power state channel created");

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
            render_acceleration = ?config.effect_engine.render_acceleration_mode,
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

        let performance = Arc::new(RwLock::new(PerformanceTracker::default()));
        info!("Performance tracker created");

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
        let spatial_engine = Arc::new(RwLock::new(SpatialEngine::new(default_layout.clone())));
        info!("Spatial engine created (empty default layout)");

        let runtime_state_path = ConfigManager::data_dir().join("runtime-state.json");
        let credential_store = Arc::new(
            CredentialStore::open_blocking(&ConfigManager::data_dir())
                .context("failed to open network credential store")?,
        );

        // ── Backend Manager ─────────────────────────────────────────────
        let usb_protocol_configs = UsbProtocolConfigStore::new();
        let backend_manager = Arc::new(Mutex::new(BackendManager::new()));
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

        // ── Attachment Template Registry ─────────────────────────────
        let attachment_templates_dir = ConfigManager::data_dir().join("attachments");
        let mut attachment_registry_inner = AttachmentRegistry::new();
        let builtin_count = attachment_registry_inner
            .load_builtins()
            .unwrap_or_else(|error| {
                warn!(%error, "Failed to load built-in attachment templates");
                0
            });
        let user_count = attachment_registry_inner
            .load_user_dir(&attachment_templates_dir)
            .unwrap_or_else(|error| {
                warn!(
                    path = %attachment_templates_dir.display(),
                    %error,
                    "Failed to load user attachment templates; starting without them"
                );
                0
            });
        let attachment_registry = Arc::new(RwLock::new(attachment_registry_inner));
        info!(
            builtin = builtin_count,
            user = user_count,
            "Attachment template registry ready"
        );

        // ── Attachment Profile Store ─────────────────────────────────
        let attachment_profiles_path = ConfigManager::data_dir().join("attachment-profiles.json");
        let attachment_profiles_inner = AttachmentProfileStore::load(&attachment_profiles_path)
            .unwrap_or_else(|error| {
                warn!(
                    path = %attachment_profiles_path.display(),
                    %error,
                    "Failed to load attachment profiles; starting with empty store"
                );
                AttachmentProfileStore::new(attachment_profiles_path)
            });
        let attachment_profiles = Arc::new(RwLock::new(attachment_profiles_inner));
        info!("Attachment profile store ready");

        // ── Output Settings Store ───────────────────────────────────
        let device_settings_path = ConfigManager::data_dir().join("device-settings.json");
        let device_settings_inner = DeviceSettingsStore::load(&device_settings_path)
            .unwrap_or_else(|error| {
                warn!(
                    path = %device_settings_path.display(),
                    %error,
                    "Failed to load device settings; starting with defaults"
                );
                DeviceSettingsStore::new(device_settings_path)
            });
        let initial_global_brightness = device_settings_inner.global_brightness();
        let device_settings = Arc::new(RwLock::new(device_settings_inner));
        set_global_brightness(&power_state, initial_global_brightness);
        info!("Device settings store ready");

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

        // ── Layout Store ─────────────────────────────────────────────
        let layouts_path = ConfigManager::data_dir().join("layouts.json");
        let mut persisted_layouts = match crate::layout_store::load(&layouts_path) {
            Ok(entries) => entries,
            Err(error) => {
                warn!(
                    path = %layouts_path.display(),
                    %error,
                    "Failed to load persisted layouts; starting with empty store"
                );
                HashMap::new()
            }
        };
        if crate::layout_store::ensure_default_layout(&mut persisted_layouts, &default_layout) {
            if let Err(error) = crate::layout_store::save(&layouts_path, &persisted_layouts) {
                warn!(
                    path = %layouts_path.display(),
                    %error,
                    "Failed to persist inserted default layout"
                );
            } else {
                info!(
                    path = %layouts_path.display(),
                    "Inserted missing default layout into persisted layout store"
                );
            }
        }
        let layout_count = persisted_layouts.len();
        let layouts = Arc::new(RwLock::new(persisted_layouts));
        info!(
            path = %layouts_path.display(),
            count = layout_count,
            "Layout store ready"
        );

        // ── Layout Auto-Exclusion Store ─────────────────────────────
        let layout_auto_exclusions_path =
            ConfigManager::data_dir().join("layout-auto-exclusions.json");
        let persisted_layout_auto_exclusions =
            match layout_auto_exclusions::load(&layout_auto_exclusions_path) {
                Ok(entries) => entries,
                Err(error) => {
                    warn!(
                        path = %layout_auto_exclusions_path.display(),
                        %error,
                        "Failed to load layout auto-exclusions; starting with empty store"
                    );
                    HashMap::new()
                }
            };
        let layout_auto_exclusions = Arc::new(RwLock::new(persisted_layout_auto_exclusions));
        info!(
            path = %layout_auto_exclusions_path.display(),
            "Layout auto-exclusion store ready"
        );

        // ── Runtime Session Store ───────────────────────────────────
        info!(
            path = %runtime_state_path.display(),
            "Runtime session store ready"
        );

        let discovery_in_progress = Arc::new(AtomicBool::new(false));
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
            network::build_builtin_driver_registry(config, Arc::clone(&credential_store))
                .context("failed to build network driver registry")?,
        );
        info!(
            drivers = ?driver_registry.ids(),
            "Network driver registry ready"
        );

        {
            // `initialize()` is invoked from `tokio::main` and `#[tokio::test]`,
            // so taking a blocking mutex guard here will panic inside the runtime.
            let mut backend_manager_inner = backend_manager.try_lock().map_err(|_| {
                anyhow::anyhow!(
                    "backend manager lock unexpectedly contended during daemon initialization"
                )
            })?;
            backend_manager_inner.register_backend(Box::new(MockDeviceBackend::new()));
            network::register_enabled_backends(
                &mut backend_manager_inner,
                driver_registry.as_ref(),
                driver_host.as_ref(),
                config,
            )
            .context("failed to register built-in network backends")?;
            if config.discovery.blocks_scan {
                let socket_path = config.discovery.blocks_socket_path.as_ref().map_or_else(
                    hypercolor_core::device::BlocksBackend::default_socket_path,
                    std::path::PathBuf::from,
                );
                backend_manager_inner.register_backend(Box::new(
                    hypercolor_core::device::BlocksBackend::new(socket_path),
                ));
            }
            backend_manager_inner.register_backend(Box::new(SmBusBackend::new()));
            backend_manager_inner.register_backend(Box::new(
                UsbBackend::with_protocol_config_store(usb_protocol_configs.clone()),
            ));
        }
        info!("Device backends registered");

        info!("All subsystems initialized");

        Ok(Self {
            config_manager,
            device_registry,
            effect_engine,
            effect_registry,
            scene_manager,
            event_bus,
            preview_runtime,
            render_loop,
            spatial_engine,
            backend_manager,
            usb_protocol_configs,
            credential_store,
            driver_host,
            driver_registry,
            performance,
            lifecycle_manager,
            reconnect_tasks,
            input_manager,
            logical_devices,
            logical_devices_path,
            attachment_registry,
            attachment_profiles,
            device_settings,
            display_overlays: Arc::new(DisplayOverlayRegistry::new()),
            display_overlay_runtime: Arc::new(DisplayOverlayRuntimeRegistry::new()),
            effect_layout_links,
            effect_layout_links_path,
            layouts_path,
            layouts,
            layout_auto_exclusions,
            layout_auto_exclusions_path,
            runtime_state_path,
            discovery_in_progress,
            power_state,
            scene_transactions,
            render_thread: None,
            display_output_thread: None,
            effect_watcher_task: None,
            discovery_task: None,
            session_controller: None,
            start_time: Instant::now(),
            server_identity,
        })
    }
}

pub(crate) fn build_input_manager(config: &HypercolorConfig) -> InputManager {
    let mut input_manager = InputManager::new();
    input_manager.set_sensor_poller(SensorPoller::new());
    input_manager.add_source(Box::new(InteractionInput::new()));
    #[cfg(target_os = "linux")]
    input_manager.add_source(Box::new(EvdevKeyboardInput::new()));

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

    #[cfg(target_os = "linux")]
    if config.capture.enabled {
        let monitor = monitor_select_from_config(config.capture.monitor);
        let capture_config = ScreenCaptureConfig {
            monitor,
            target_fps: config.capture.capture_fps.max(1),
            ..ScreenCaptureConfig::default()
        };
        input_manager.add_source(Box::new(WaylandScreenCaptureInput::new(capture_config)));
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

#[cfg(target_os = "linux")]
fn monitor_select_from_config(monitor_index: u32) -> MonitorSelect {
    if monitor_index == 0 {
        MonitorSelect::Primary
    } else {
        MonitorSelect::ByIndex(monitor_index)
    }
}

fn noise_gate_to_db(noise_gate: f32) -> f32 {
    let linear = noise_gate.clamp(0.000_001, 1.0);
    20.0 * linear.log10()
}
