//! Subsystem initialization: bus, engines, managers, stores, and input sources.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use anyhow::{Context, Result};
use arc_swap::{ArcSwap, ArcSwapOption};
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use sysinfo::{MemoryRefreshKind, RefreshKind, System};
use tokio::sync::{Mutex, RwLock};
use tracing::{info, warn};

use hypercolor_core::asset::{AssetLibrary, StreamUrlPolicy};
use hypercolor_core::attachment::ComponentRegistry;
use hypercolor_core::bus::HypercolorBus;
use hypercolor_core::config::{
    BootConfig, CapturePersistenceEpoch, CapturePersistenceSource, ConfigManager,
};
use hypercolor_core::device::mock::MockDeviceBackend;
use hypercolor_core::device::{
    BackendManager, DeviceLifecycleManager, DeviceRegistry, UsbProtocolConfigStore,
};
use hypercolor_core::effect::builtin::register_builtin_effects;
use hypercolor_core::effect::{EffectRegistry, default_effect_search_paths, register_html_effects};
use hypercolor_core::engine::{FpsTier, RenderLoop};
#[cfg(target_os = "linux")]
use hypercolor_core::input::EvdevHostInput;
#[cfg(target_os = "macos")]
use hypercolor_core::input::MacosHostInput;
#[cfg(target_os = "windows")]
use hypercolor_core::input::WindowsHostInput;
use hypercolor_core::input::audio::AudioInput;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use hypercolor_core::input::screen::CaptureConfig as ScreenCaptureConfig;
#[cfg(target_os = "macos")]
use hypercolor_core::input::screen::MacosScreenCaptureInput;
#[cfg(target_os = "linux")]
use hypercolor_core::input::screen::WaylandScreenCaptureInput;
#[cfg(target_os = "windows")]
use hypercolor_core::input::screen::{
    CaptureSourceSink, ResolvedCaptureSource, WindowsScreenCaptureInput,
};
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
use hypercolor_core::input::screen::{ScreenAdmissionCapacity, ScreenAnalysisResourcePlan};
use hypercolor_core::input::{InputManager, SensorPoller, SourceStatusHandle};
use hypercolor_core::scene::SceneManager;
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_driver_api::CredentialStore;
use hypercolor_types::audio::{AudioPipelineConfig, AudioSourceType};
use hypercolor_types::config::HypercolorConfig;
use hypercolor_types::spatial::{EdgeBehavior, SamplingMode, SpatialLayout};

use crate::attachment_profiles::ComponentProfileStore;
use crate::device_metrics::DeviceMetricsSnapshot;
use crate::device_settings::DeviceSettingsStore;
use crate::domain::context::{
    DeviceContext, DomainContextResources, DomainContexts, RuntimeSessionService, SceneContext,
};
use crate::domain::layout::LayoutContext;
use crate::domain::output::OutputContext;
use crate::driver_inventory::{DRIVER_INVENTORY_FILENAME, DriverInventoryStore};
use crate::extensions::ExtensionRegistry;
use crate::interaction_routing::InteractionRoutingControl;
use crate::layout_auto_exclusions;
use crate::network::{self, DaemonDriverHost};
use crate::output_power::OutputPower;
use crate::performance::PerformanceTracker;
use crate::playlist_runtime::PlaylistRuntimeState;
use crate::preview_runtime::PreviewRuntime;
use crate::scene_store::SceneStore;
use crate::scene_transactions::SceneTransactionQueue;
use crate::simulators::{SimulatedDisplayBackend, SimulatedDisplayRuntime, SimulatedDisplayStore};
use crate::zone_layout_preview::ZoneLayoutPreviewStore;

use super::DaemonState;
use super::config::resolve_server_identity;
use super::resolve_compositor_acceleration_mode;
use crate::render_thread::ConfiguredFpsTier;

#[cfg(test)]
fn open_persisted_library_store(
    path: &std::path::Path,
) -> Result<(
    Arc<dyn crate::library::LibraryStore>,
    Arc<dyn crate::library::LibraryIdentityMigration>,
)> {
    let store = Arc::new(
        crate::library::JsonLibraryStore::open(path.to_owned()).with_context(|| {
            format!(
                "failed to open persisted library store at {}",
                path.display()
            )
        })?,
    );
    Ok((store.clone(), store))
}

fn open_persisted_library_store_with_effect_id_migrations(
    path: &std::path::Path,
    migrations: &crate::domain::effect::EffectIdMigrations,
) -> Result<(
    Arc<dyn crate::library::LibraryStore>,
    Arc<dyn crate::library::LibraryIdentityMigration>,
)> {
    let store = Arc::new(
        crate::library::JsonLibraryStore::open_with_effect_id_migrations(
            path.to_owned(),
            migrations,
        )
        .with_context(|| {
            format!(
                "failed to migrate persisted library store at {}",
                path.display()
            )
        })?,
    );
    Ok((store.clone(), store))
}

impl DaemonState {
    pub fn initialize(boot: BootConfig, config_manager: Arc<ConfigManager>) -> Result<Self> {
        Self::initialize_with_macos_owner(boot, config_manager, None)
    }

    pub fn initialize_with_macos_owner(
        boot: BootConfig,
        config_manager: Arc<ConfigManager>,
        macos_owner_snapshot: Option<crate::macos_owner::MacosOwnerSnapshot>,
    ) -> Result<Self> {
        Self::initialize_inner(boot, config_manager, macos_owner_snapshot)
    }

    /// Initialize all subsystems from a loaded configuration.
    ///
    /// `boot` is **consumed by value** (Spec 76 §3.2): subsystems freeze
    /// the boot values they need during construction, and the config dies
    /// with this call, so no live handle to a [`BootConfig`] can outlast
    /// initialization. `config_manager` is the live authority the load
    /// pipeline already built, so nothing here re-reads or re-parses the
    /// config file.
    ///
    /// This wires together the bus, registry, engines, and render loop
    /// but does **not** start any background tasks. Call [`start`](Self::start)
    /// to begin the render loop and device discovery.
    ///
    /// # Errors
    ///
    /// Returns an error if the configuration is invalid or a subsystem
    /// fails to construct.
    #[expect(
        clippy::too_many_lines,
        reason = "initialization is inherently sequential; splitting would scatter related setup across helpers"
    )]
    fn initialize_inner(
        boot: BootConfig,
        config_manager: Arc<ConfigManager>,
        macos_owner_snapshot: Option<crate::macos_owner::MacosOwnerSnapshot>,
    ) -> Result<Self> {
        let config: &HypercolorConfig = &boot;
        let data_dir = ConfigManager::data_dir();
        let state_dir = ConfigManager::state_dir();
        info!("Initializing daemon subsystems");
        #[cfg(not(target_os = "macos"))]
        let _ = macos_owner_snapshot;
        config
            .capture
            .validate()
            .context("invalid screen capture configuration")?;
        #[cfg(feature = "servo-gpu-import")]
        {
            hypercolor_core::effect::set_servo_gpu_import_mode(
                config.rendering.servo_gpu_import.mode,
            );
            info!(
                mode = ?hypercolor_core::effect::servo_gpu_import_mode(),
                "Servo GPU import mode configured"
            );
        }

        let render_acceleration = resolve_compositor_acceleration_mode(
            config.effect_engine.compositor_acceleration_mode,
            config.rendering.servo_gpu_import.mode,
        )
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
                adapter_device_type = probe.adapter_device_type,
                backend = probe.backend,
                texture_format = probe.texture_format,
                max_texture_dimension_2d = probe.max_texture_dimension_2d,
                max_storage_textures_per_shader_stage = probe.max_storage_textures_per_shader_stage,
                software_adapter_reason = probe.software_adapter_reason,
                servo_gpu_import_backend_compatible = probe.servo_gpu_import_backend_compatible,
                servo_gpu_import_backend_reason = probe.servo_gpu_import_backend_reason,
                linux_servo_gpu_import_backend_compatible = probe.linux_servo_gpu_import_backend_compatible,
                linux_servo_gpu_import_backend_reason = probe.linux_servo_gpu_import_backend_reason,
                "SparkleFlinger GPU probe succeeded"
            );
        }

        let server_identity =
            resolve_server_identity(config).context("failed to resolve server identity")?;
        let api_extensions = Vec::new();
        let lifecycle_extensions = Vec::new();

        // ── Event Bus ───────────────────────────────────────────────────
        let event_bus = Arc::new(HypercolorBus::new());
        let macos_daemon_ownership = Arc::new(ArcSwapOption::empty());
        #[cfg(target_os = "macos")]
        let mut pending_macos_owner_watch = macos_owner_snapshot
            .map(|snapshot| {
                super::macos_owner_watch::PendingMacosOwnerWatch::start(
                    ConfigManager::data_dir(),
                    Arc::clone(&macos_daemon_ownership),
                    Arc::clone(&event_bus),
                    snapshot,
                )
            })
            .transpose()?;
        let preview_runtime = Arc::new(PreviewRuntime::new(Arc::clone(&event_bus)));
        let zone_layout_previews = Arc::new(ZoneLayoutPreviewStore::default());
        info!("Event bus created");

        let asset_library_path = ConfigManager::config_dir().join("assets");
        let stream_url_policy = StreamUrlPolicy::from_private_network_allowlist(
            &config.media.stream_private_network_allowlist,
        );
        let asset_library = AssetLibrary::open_with_stream_url_policy(
            asset_library_path.clone(),
            stream_url_policy,
        )
        .with_context(|| {
            format!(
                "failed to open asset library at {}",
                asset_library_path.display()
            )
        })?;
        let asset_library = Arc::new(RwLock::new(asset_library));
        info!(path = %asset_library_path.display(), "Asset library ready");

        let scene_transactions = SceneTransactionQueue::default();

        // ── Device Registry ─────────────────────────────────────────────
        let device_registry = DeviceRegistry::new();
        info!("Device registry created");

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
        let effect_id_migrations = html_report.legacy_effect_ids;

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

        // ── Scene Manager / Store ──────────────────────────────────────
        let scenes_path = ConfigManager::data_dir().join("scenes.json");
        let profiles_path = ConfigManager::data_dir().join("profiles.json");
        let mut scene_store_inner = SceneStore::load(&scenes_path)
            .with_context(|| format!("failed to load scenes from {}", scenes_path.display()))?;
        match crate::profile_import::import_profiles(
            &profiles_path,
            &mut scene_store_inner,
            &persisted_layouts,
            &default_layout,
        )
        .context("failed to import legacy profiles")?
        {
            crate::profile_import::ProfileImportOutcome::NoSource => {}
            crate::profile_import::ProfileImportOutcome::Imported { profiles, backup } => {
                info!(profiles, backup = %backup.display(), "Imported legacy profiles as scenes");
            }
        }
        let migrated_scene_effect_ids = scene_store_inner
            .migrate_effect_ids(&effect_id_migrations)
            .context("failed to migrate persisted scene effect IDs")?;
        if migrated_scene_effect_ids > 0 {
            info!(
                migrated = migrated_scene_effect_ids,
                path = %scenes_path.display(),
                "Migrated persisted scene effect IDs"
            );
        }
        let mut scene_manager_inner = SceneManager::with_default_layout(default_layout.clone());
        for scene in scene_store_inner.list().cloned() {
            if let Err(error) = scene_manager_inner.create(scene) {
                warn!(%error, "Failed to install persisted named scene");
            }
        }
        let scene_store = Arc::new(RwLock::new(scene_store_inner));
        let scene_manager = crate::domain::scene::SceneService::new(
            scene_manager_inner,
            Arc::clone(&event_bus),
            Arc::clone(&scene_store),
            Arc::clone(&zone_layout_previews),
        );
        info!(path = %scenes_path.display(), "Scene manager created");

        // ── Render Loop ─────────────────────────────────────────────────
        let render_loop = RenderLoop::new(config.daemon.target_fps);
        let render_loop = Arc::new(RwLock::new(render_loop));
        let configured_max_fps_tier =
            ConfiguredFpsTier::new(FpsTier::from_fps(config.daemon.target_fps));
        info!(target_fps = config.daemon.target_fps, "Render loop created");

        let performance = Arc::new(RwLock::new(PerformanceTracker::default()));
        info!("Performance tracker created");
        let device_metrics = Arc::new(ArcSwap::from_pointee(DeviceMetricsSnapshot::default()));
        info!("Device metrics snapshot store created");

        // ── Spatial Engine ──────────────────────────────────────────────
        let spatial_engine = crate::domain::spatial::SpatialService::new(
            SpatialEngine::try_new(default_layout.clone())
                .context("failed to prepare the default spatial layout")?,
        );
        info!("Spatial engine created (empty default layout)");

        let driver_inventory_path = state_dir.join(DRIVER_INVENTORY_FILENAME);
        let (driver_inventory, driver_inventory_migration) = DriverInventoryStore::open_migrated(
            data_dir.join(DRIVER_INVENTORY_FILENAME),
            driver_inventory_path.clone(),
        )
        .context("failed to open driver inventory store")?;
        let driver_inventory = Arc::new(driver_inventory);
        info!(
            path = %driver_inventory_path.display(),
            migration = ?driver_inventory_migration,
            "Driver inventory store ready"
        );
        let credential_store = Arc::new(
            CredentialStore::open_blocking(&ConfigManager::data_dir())
                .context("failed to open driver credential store")?,
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
        #[cfg(target_os = "macos")]
        let macos_owner_publication =
            match (pending_macos_owner_watch.as_mut(), macos_owner_snapshot) {
                (Some(watch), Some(snapshot)) => {
                    Some(watch.reconcile_snapshot(snapshot).context(
                        "failed to reconcile macOS daemon ownership before source startup",
                    )?)
                }
                (None, snapshot) => {
                    snapshot.map(super::macos_owner_watch::MacosOwnerPublication::without_identity)
                }
                (Some(_), None) => None,
            };
        let (built_input_manager, browser_input) = build_input_manager(config, &config_manager)?;
        #[cfg(target_os = "macos")]
        let mut built_input_manager = built_input_manager;
        #[cfg(target_os = "macos")]
        if let Some(publication) = macos_owner_publication {
            super::macos_owner_watch::publish_owner_snapshot(
                &macos_daemon_ownership,
                &mut built_input_manager,
                &event_bus,
                publication,
            )?;
        }
        let interaction_routing = InteractionRoutingControl::new(
            browser_input.registry(),
            1,
            config.input.daemon_route,
            config.input.preview_route,
        );
        let input_status = built_input_manager.source_status_registry();
        let screen_capacity_status = built_input_manager.screen_capacity_status_handle();
        let input_manager = Arc::new(Mutex::new(built_input_manager));
        #[cfg(target_os = "macos")]
        let macos_owner_watch = pending_macos_owner_watch
            .map(|watch| watch.attach(Arc::clone(&input_manager)))
            .transpose()?;
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
        let mut attachment_registry_inner = ComponentRegistry::new();
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
        let attachment_profiles_inner = ComponentProfileStore::load(&attachment_profiles_path)
            .unwrap_or_else(|error| {
                warn!(
                    path = %attachment_profiles_path.display(),
                    %error,
                    "Failed to load attachment profiles; starting with empty store"
                );
                ComponentProfileStore::new(attachment_profiles_path)
            });
        let attachment_profiles = Arc::new(RwLock::new(attachment_profiles_inner));
        info!("Attachment profile store ready");

        // ── Display Preferences Store ─────────────────────────────
        let display_preferences_path = state_dir.join("display-preferences.json");
        let display_preferences_inner =
            match crate::display_preferences::DisplayPreferencesStore::load_migrated(
                &data_dir.join("display-preferences.json"),
                &display_preferences_path,
            ) {
                Ok((store, migration)) => {
                    info!(
                        path = %display_preferences_path.display(),
                        ?migration,
                        "Display preferences store ready"
                    );
                    let mut store = store;
                    let migrated = store
                        .migrate_effect_ids(&effect_id_migrations)
                        .context("failed to migrate display preference effect IDs")?;
                    if migrated > 0 {
                        info!(
                            migrated,
                            path = %display_preferences_path.display(),
                            "Migrated display preference effect IDs"
                        );
                    }
                    store
                }
                Err(error) => {
                    warn!(
                        path = %display_preferences_path.display(),
                        %error,
                        "Failed to load display preferences; starting with empty store"
                    );
                    crate::display_preferences::DisplayPreferencesStore::new(
                        display_preferences_path,
                    )
                    .context("failed to prepare empty display preference persistence")?
                }
            };
        let display_preferences = Arc::new(RwLock::new(display_preferences_inner));
        info!("Display preferences store ready");

        // ── Output Settings Store ───────────────────────────────────
        let device_settings_path = state_dir.join("device-settings.json");
        let device_settings_inner = DeviceSettingsStore::load_migrated(
            &data_dir.join("device-settings.json"),
            &device_settings_path,
        )
        .map(|(store, migration)| {
            info!(
                path = %device_settings_path.display(),
                ?migration,
                "Device settings store ready"
            );
            store
        })
        .unwrap_or_else(|error| {
            warn!(
                path = %device_settings_path.display(),
                %error,
                "Failed to load device settings; starting with defaults"
            );
            DeviceSettingsStore::new(device_settings_path)
        });
        let output_power = OutputPower::new(device_settings_inner);
        let device_settings = output_power.device_settings();
        info!("Device settings store ready");

        // ── Simulator Store ─────────────────────────────────────────
        let simulated_displays_path = ConfigManager::data_dir().join("simulated-displays.json");
        let simulated_displays_inner = SimulatedDisplayStore::load(&simulated_displays_path)
            .unwrap_or_else(|error| {
                warn!(
                    path = %simulated_displays_path.display(),
                    %error,
                    "Failed to load simulated displays; starting with empty store"
                );
                SimulatedDisplayStore::new(simulated_displays_path)
            });
        let simulated_displays = Arc::new(RwLock::new(simulated_displays_inner));
        let simulated_display_runtime = Arc::new(RwLock::new(SimulatedDisplayRuntime::new()));
        info!("Simulated display store ready");

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
        let runtime_state_path = state_dir.join("runtime-state.json");
        let mut startup_runtime_snapshot = match crate::runtime_state::load_migrated(
            &data_dir.join("runtime-state.json"),
            &runtime_state_path,
        ) {
            Ok((snapshot, migration)) => {
                info!(
                    path = %runtime_state_path.display(),
                    ?migration,
                    "Runtime session store ready"
                );
                snapshot
            }
            Err(error) => {
                warn!(
                    path = %runtime_state_path.display(),
                    %error,
                    "Failed to load runtime session snapshot"
                );
                None
            }
        };
        if let Some(snapshot) = startup_runtime_snapshot.as_mut() {
            let migrated = snapshot.migrate_effect_ids(&effect_id_migrations);
            if migrated > 0 {
                crate::runtime_state::save(&runtime_state_path, snapshot)
                    .context("failed to migrate runtime session effect IDs")?;
                info!(
                    migrated,
                    path = %runtime_state_path.display(),
                    "Migrated runtime session effect IDs"
                );
            }
        }

        let device_aliases_path = state_dir.join(crate::device_aliases::DEVICE_ALIASES_FILE);
        let startup_device_aliases = match crate::device_aliases::load_migrated(
            &data_dir.join(crate::device_aliases::DEVICE_ALIASES_FILE),
            &device_aliases_path,
        ) {
            Ok((aliases, migration)) => {
                info!(
                    path = %device_aliases_path.display(),
                    ?migration,
                    "Device alias store ready"
                );
                aliases
            }
            Err(error) => {
                warn!(
                    path = %device_aliases_path.display(),
                    %error,
                    "Failed to load device aliases; starting with an empty overlay"
                );
                crate::device_aliases::DeviceAliasFile::default()
            }
        };
        let discovery_in_progress = Arc::new(AtomicBool::new(false));
        let driver_registry = Arc::new(
            network::build_builtin_driver_module_registry(
                config,
                Arc::clone(&credential_store),
                usb_protocol_configs.clone(),
            )
            .context("failed to build driver module registry")?,
        );
        let discovery_runtime = crate::discovery::DiscoveryRuntime {
            device_registry: device_registry.clone(),
            backend_manager: Arc::clone(&backend_manager),
            lifecycle_manager: Arc::clone(&lifecycle_manager),
            reconnect_tasks: Arc::clone(&reconnect_tasks),
            event_bus: Arc::clone(&event_bus),
            spatial_engine: spatial_engine.clone(),
            scene_manager: scene_manager.clone(),
            layouts: Arc::clone(&layouts),
            layouts_path: layouts_path.clone(),
            layout_auto_exclusions: Arc::clone(&layout_auto_exclusions),
            logical_devices: Arc::clone(&logical_devices),
            attachment_registry: Arc::clone(&attachment_registry),
            attachment_profiles: Arc::clone(&attachment_profiles),
            device_settings: device_settings.clone(),
            scene_transactions: scene_transactions.clone(),
            runtime_state_path: runtime_state_path.clone(),
            device_aliases_path: device_aliases_path.clone(),
            usb_protocol_configs: usb_protocol_configs.clone(),
            credential_store: Arc::clone(&credential_store),
            in_progress: Arc::clone(&discovery_in_progress),
            pending_scans: Arc::default(),
            task_spawner: tokio::runtime::Handle::current(),
        };
        let driver_host = Arc::new(DaemonDriverHost::new(
            discovery_runtime,
            driver_inventory,
            Arc::clone(&driver_registry),
            Some(Arc::clone(&config_manager)),
        ));
        info!(
            drivers = ?driver_registry.ids(),
            "Driver module registry ready"
        );

        {
            // `initialize()` is invoked from `tokio::main` and `#[tokio::test]`,
            // so taking a blocking mutex guard here will panic inside the runtime.
            let mut backend_manager_inner = backend_manager.try_lock().map_err(|_| {
                anyhow::anyhow!(
                    "backend manager lock unexpectedly contended during daemon initialization"
                )
            })?;
            backend_manager_inner.register_backend(Arc::new(SimulatedDisplayBackend::new(
                Arc::clone(&simulated_displays),
                Arc::clone(&simulated_display_runtime),
            )));
            backend_manager_inner.register_backend(Arc::new(MockDeviceBackend::new()));
            network::register_enabled_device_backends(
                &mut backend_manager_inner,
                driver_registry.as_ref(),
                driver_host.as_ref(),
                config,
            )
            .context("failed to register enabled device backends")?;
        }
        info!("Device backends registered");

        let library_path = ConfigManager::data_dir().join("library.json");
        let (library_store, library_identity) =
            open_persisted_library_store_with_effect_id_migrations(
                &library_path,
                &effect_id_migrations,
            )?;
        let playlist_runtime = Arc::new(Mutex::new(PlaylistRuntimeState::new()));
        let start_time = Instant::now();
        let runtime_session = RuntimeSessionService::new(
            runtime_state_path.clone(),
            scene_manager.clone(),
            spatial_engine.clone(),
            output_power.clone(),
            Arc::clone(&driver_host),
            Arc::clone(&driver_registry),
        );
        let devices = DeviceContext::new(
            device_registry.clone(),
            Arc::clone(&lifecycle_manager),
            Arc::clone(&driver_host),
            Arc::clone(&driver_registry),
            Some(Arc::clone(&config_manager)),
            Arc::clone(&layout_auto_exclusions),
            layout_auto_exclusions_path.clone(),
        );
        let scene = SceneContext::new(
            scene_manager.clone(),
            runtime_session.clone(),
            Arc::clone(&asset_library),
            Some(Arc::clone(&config_manager)),
            Arc::clone(&render_loop),
            devices.clone(),
        );
        let layout = LayoutContext::new(
            Arc::clone(&layouts),
            spatial_engine.clone(),
            scene_manager.clone(),
            scene_transactions.clone(),
            runtime_session.clone(),
            devices.clone(),
        );
        let output = OutputContext::new(
            output_power.clone(),
            Arc::clone(&event_bus),
            runtime_session.clone(),
            Arc::clone(&performance),
            Arc::clone(&render_loop),
            spatial_engine.clone(),
            Arc::clone(&backend_manager),
            Arc::clone(&preview_runtime),
            devices.clone(),
            start_time,
        );
        let domains = DomainContexts::assemble(
            runtime_session,
            devices,
            scene,
            layout,
            output,
            DomainContextResources {
                effect_registry: Arc::clone(&effect_registry),
                spatial: spatial_engine.clone(),
                event_bus: Arc::clone(&event_bus),
            },
        );
        info!("All subsystems initialized");

        Ok(Self {
            domains,
            config_manager,
            extensions: ExtensionRegistry::default(),
            api_extensions,
            lifecycle_extensions,
            device_registry,
            effect_registry,
            scene_manager,
            scene_store,
            event_bus,
            macos_daemon_ownership,
            #[cfg(target_os = "macos")]
            _macos_owner_watch: macos_owner_watch,
            asset_library,
            library_store,
            library_identity,
            playlist_runtime,
            preview_runtime,
            zone_layout_previews,
            render_loop,
            configured_max_fps_tier,
            spatial_engine,
            backend_manager,
            usb_protocol_configs,
            credential_store,
            driver_host,
            driver_registry,
            performance,
            render_acceleration,
            device_metrics,
            lifecycle_manager,
            reconnect_tasks,
            input_manager,
            screen_capacity_status,
            input_status,
            browser_input,
            interaction_routing,
            logical_devices,
            logical_devices_path,
            attachment_registry,
            attachment_profiles,
            display_preferences,
            device_settings,
            simulated_displays,
            simulated_display_runtime,
            display_frames: Arc::new(RwLock::new(
                crate::display_frames::DisplayFrameRuntime::new(),
            )),
            layouts_path,
            layouts,
            layout_auto_exclusions,
            layout_auto_exclusions_path,
            runtime_state_path,
            device_aliases_path,
            startup_device_aliases: Some(startup_device_aliases),
            startup_runtime_snapshot,
            discovery_in_progress,
            output_power,
            scene_transactions,
            render_thread: None,
            display_output_thread: None,
            effect_watcher_task: None,
            effect_error_fallback_task: None,
            display_preference_sync_task: None,
            output_static_hold_task: None,
            discovery_task: None,
            device_metrics_collector_task: None,
            input_status_event_publisher: None,
            session_controller: None,
            start_time,
            server_identity,
        })
    }
}

pub(crate) fn build_input_manager(
    config: &HypercolorConfig,
    config_manager: &Arc<ConfigManager>,
) -> Result<(InputManager, hypercolor_core::input::BrowserInputHandle)> {
    let mut input_manager = InputManager::new();
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    let capacity_plan = screen_capacity_plan(&config.capture)?;
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    input_manager.set_screen_capacity_plan(
        capacity_plan.resource_capacity(),
        capacity_plan.total_capacity(),
        capacity_plan.total_capacity(),
    )?;
    input_manager.set_sensor_poller(SensorPoller::new());
    // Host input capture is consent-gated and platform-native: evdev on Linux,
    // Raw Input on Windows. Capture stays closed until an interactive effect
    // creates demand, so no window is created and no registration is taken
    // while nothing is listening.
    if let Some(source) = build_interaction_source(&config.input) {
        input_manager.add_source(source);
    }
    // Browser-preview injection is always registered: it has no hardware and
    // no privacy surface (the user drives their own browser), and its edges
    // only reach effects that declare input reactivity.
    let browser_source = hypercolor_core::input::BrowserInputSource::new();
    let browser_input = browser_source.handle();
    input_manager.add_source(Box::new(browser_source));
    input_manager.add_source(Box::new(hypercolor_core::input::MediaSource::new()));
    input_manager.add_source(Box::new(hypercolor_core::input::NetSource::new()));

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
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
        {
            let admission_coordinator = input_manager.screen_admission_coordinator();
            input_manager.add_source(build_platform_screen_capture_source(
                &config.capture,
                Arc::clone(config_manager),
                admission_coordinator,
                capacity_plan.total_capacity(),
            )?);
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        input_manager.add_source(build_platform_screen_capture_source(
            &config.capture,
            Arc::clone(config_manager),
        )?);
    }
    config_manager.mark_capture_runtime_applied(&config.capture);

    Ok((input_manager, browser_input))
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ScreenCapacityPlan {
    resource: ScreenAdmissionCapacity,
    total: ScreenAdmissionCapacity,
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
impl ScreenCapacityPlan {
    pub(crate) const fn resource_capacity(self) -> ScreenAdmissionCapacity {
        self.resource
    }

    pub(crate) const fn total_capacity(self) -> ScreenAdmissionCapacity {
        self.total
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub(crate) fn screen_capacity_plan(
    capture: &hypercolor_types::config::CaptureConfig,
) -> Result<ScreenCapacityPlan> {
    let backend_capacity = available_host_memory_bytes()?;
    screen_capacity_plan_for_backend(capture, backend_capacity)
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
pub(crate) fn screen_capacity_plan_for_backend(
    capture: &hypercolor_types::config::CaptureConfig,
    backend_capacity: u64,
) -> Result<ScreenCapacityPlan> {
    let byte_budget = capture.publication_memory_bytes.unwrap_or(backend_capacity);
    if byte_budget == 0 || backend_capacity == 0 {
        anyhow::bail!("screen publication memory budget must be non-zero");
    }
    let resource = ScreenAdmissionCapacity::new(backend_capacity, backend_capacity);
    let total = ScreenAdmissionCapacity::new(byte_budget, backend_capacity);
    Ok(ScreenCapacityPlan { resource, total })
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
pub(crate) fn screen_analysis_plan_for_demand(
    capture: &hypercolor_types::config::CaptureConfig,
    demand: hypercolor_core::input::screen::ScreenCaptureDemand,
    capacity: ScreenAdmissionCapacity,
) -> Result<Option<ScreenAnalysisResourcePlan>> {
    let Some(requested_extent) = demand.requested_extent() else {
        return Ok(None);
    };
    ScreenAnalysisResourcePlan::try_new_for_extent(
        capture.grid_cols,
        capture.grid_rows,
        capture.capture_fps,
        requested_extent,
        capacity.byte_budget().min(capacity.backend_capacity()),
    )
    .map(Some)
    .context("screen analysis demand exceeds configured steady capacity")
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn available_host_memory_bytes() -> Result<u64> {
    let mut system = System::new_with_specifics(
        RefreshKind::nothing().with_memory(MemoryRefreshKind::nothing().with_ram()),
    );
    system.refresh_memory();
    let available = system.available_memory();
    if available == 0 {
        anyhow::bail!("operating system reported no available host memory");
    }
    Ok(available)
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub(crate) fn build_platform_screen_capture_source(
    capture: &hypercolor_types::config::CaptureConfig,
    config_manager: Arc<ConfigManager>,
    admission_coordinator: hypercolor_core::input::screen::ScreenByteAdmissionCoordinator,
    capacity: ScreenAdmissionCapacity,
) -> Result<Box<dyn hypercolor_core::input::InputSource>> {
    let expected = Arc::clone(&config_manager.get());
    let persistence = CaptureConfigPersistenceGate::new(config_manager, &expected, true)?;
    build_platform_screen_capture_source_with_persistence(
        capture,
        persistence,
        admission_coordinator,
        capacity,
    )
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub(crate) fn build_platform_screen_capture_source(
    capture: &hypercolor_types::config::CaptureConfig,
    config_manager: Arc<ConfigManager>,
) -> Result<Box<dyn hypercolor_core::input::InputSource>> {
    let expected = Arc::clone(&config_manager.get());
    let persistence = CaptureConfigPersistenceGate::new(config_manager, &expected, true)?;
    build_platform_screen_capture_source_with_persistence(capture, persistence)
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub(crate) fn prepare_platform_screen_capture_source(
    capture: &hypercolor_types::config::CaptureConfig,
    config_manager: Arc<ConfigManager>,
    expected: &Arc<HypercolorConfig>,
    admission_coordinator: hypercolor_core::input::screen::ScreenByteAdmissionCoordinator,
    capacity: ScreenAdmissionCapacity,
) -> Result<(
    Box<dyn hypercolor_core::input::InputSource>,
    CaptureConfigPersistenceGate,
)> {
    let persistence = CaptureConfigPersistenceGate::new(config_manager, expected, false)?;
    let source = build_platform_screen_capture_source_with_persistence(
        capture,
        persistence.clone(),
        admission_coordinator,
        capacity,
    )?;
    Ok((source, persistence))
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub(crate) fn prepare_platform_screen_capture_source(
    capture: &hypercolor_types::config::CaptureConfig,
    config_manager: Arc<ConfigManager>,
    expected: &Arc<HypercolorConfig>,
) -> Result<(
    Box<dyn hypercolor_core::input::InputSource>,
    CaptureConfigPersistenceGate,
)> {
    let persistence = CaptureConfigPersistenceGate::new(config_manager, expected, false)?;
    let source =
        build_platform_screen_capture_source_with_persistence(capture, persistence.clone())?;
    Ok((source, persistence))
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn build_platform_screen_capture_source_with_persistence(
    capture: &hypercolor_types::config::CaptureConfig,
    persistence: CaptureConfigPersistenceGate,
    admission_coordinator: hypercolor_core::input::screen::ScreenByteAdmissionCoordinator,
    capacity: ScreenAdmissionCapacity,
) -> Result<Box<dyn hypercolor_core::input::InputSource>> {
    #[cfg(target_os = "windows")]
    let source = build_windows_screen_capture_source(
        capture,
        persistence.clone(),
        admission_coordinator,
        capacity,
    )?;
    #[cfg(target_os = "linux")]
    let source = build_screen_capture_source(
        capture,
        persistence.clone(),
        admission_coordinator,
        capacity,
    )?;
    #[cfg(target_os = "macos")]
    let source = build_macos_screen_capture_source(capture, admission_coordinator, capacity)?;
    let status = source
        .source_status_handle()
        .context("screen capture source must expose lifecycle status")?;
    persistence.bind_source(status);
    Ok(source)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn build_platform_screen_capture_source_with_persistence(
    capture: &hypercolor_types::config::CaptureConfig,
    persistence: CaptureConfigPersistenceGate,
) -> Result<Box<dyn hypercolor_core::input::InputSource>> {
    let _ = (capture, persistence);
    anyhow::bail!("screen capture is not supported on this platform")
}

#[derive(Clone)]
pub(crate) struct CaptureConfigPersistenceGate {
    inner: Arc<CaptureConfigPersistenceInner>,
}

struct CaptureConfigPersistenceInner {
    config_manager: Arc<ConfigManager>,
    state: StdMutex<CaptureConfigPersistenceState>,
}

impl Drop for CaptureConfigPersistenceInner {
    fn drop(&mut self) {
        let epoch = self
            .state
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .epoch;
        self.config_manager.revoke_capture_persistence(epoch);
    }
}

struct CaptureConfigPersistenceState {
    committed: bool,
    revoked: bool,
    epoch: CapturePersistenceEpoch,
    source_status: Option<SourceStatusHandle>,
    pending: Option<CaptureConfigPersistenceUpdate>,
}

enum CaptureConfigPersistenceUpdate {
    #[cfg(target_os = "macos")]
    MacosSource {
        configured: String,
        resolved: String,
    },
    #[cfg(target_os = "windows")]
    WindowsSource(ResolvedCaptureSource),
    #[cfg(target_os = "linux")]
    RestoreToken {
        configured: Option<String>,
        resolved: Option<String>,
    },
}

impl CaptureConfigPersistenceGate {
    fn new(
        config_manager: Arc<ConfigManager>,
        expected: &Arc<HypercolorConfig>,
        committed: bool,
    ) -> Result<Self> {
        let epoch = config_manager
            .reserve_capture_persistence(expected)
            .context("capture config changed before persistence authority was reserved")?;
        if committed && !config_manager.activate_capture_persistence(expected, epoch, None) {
            config_manager.revoke_capture_persistence(epoch);
            anyhow::bail!("capture config changed before persistence authority was activated");
        }
        Ok(Self {
            inner: Arc::new(CaptureConfigPersistenceInner {
                config_manager,
                state: StdMutex::new(CaptureConfigPersistenceState {
                    committed,
                    revoked: false,
                    epoch,
                    source_status: None,
                    pending: None,
                }),
            }),
        })
    }

    fn bind_source(&self, status: SourceStatusHandle) {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.source_status = Some(status);
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn for_macos_picker(
        config_manager: Arc<ConfigManager>,
        expected: &Arc<HypercolorConfig>,
        status: SourceStatusHandle,
    ) -> Result<Self> {
        let persistence = Self::new(config_manager, expected, true)?;
        persistence.bind_source(status);
        Ok(persistence)
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn publish_macos_selection(&self, configured: String, resolved: String) {
        self.publish(CaptureConfigPersistenceUpdate::MacosSource {
            configured,
            resolved,
        });
    }

    pub(crate) fn epoch(&self) -> CapturePersistenceEpoch {
        self.inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .epoch
    }

    pub(crate) fn source_identity(&self) -> Option<CapturePersistenceSource> {
        let state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        source_identity(&state)
    }

    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    fn publish(&self, update: CaptureConfigPersistenceUpdate) {
        let persistence = {
            let mut state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.revoked {
                None
            } else if !state.committed {
                state.pending = Some(update);
                None
            } else if !requires_source_identity(&update) {
                // A newer update supersedes anything parked earlier.
                state.pending = None;
                Some((state.epoch, None, update))
            } else if let Some(source) = source_identity(&state) {
                state.pending = None;
                Some((state.epoch, Some(source), update))
            } else {
                // The status snapshot has not caught up with the live
                // session yet. Losing the update here replays stale
                // state on the next reconnect; park it until an
                // identity-bearing publish or commit can flush it.
                warn!(
                    "capture persistence update parked: source identity \
                     not yet observable"
                );
                state.pending = Some(update);
                None
            }
        };
        if let Some((epoch, source, update)) = persistence {
            self.persist(epoch, source, update, false);
        }
    }

    pub(crate) fn commit(&self) {
        let persistence = {
            let mut state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.revoked {
                return;
            }
            state.committed = true;
            let identity = match state.pending.as_ref() {
                Some(update) if !requires_source_identity(update) => Some(None),
                Some(_) => source_identity(&state).map(Some),
                None => None,
            };
            if let Some(source) = identity {
                state
                    .pending
                    .take()
                    .map(|update| (state.epoch, source, update))
            } else {
                // Keep the parked update: taking it here without an
                // identity would silently drop a freshly rotated restore
                // token and force re-consent on the next reconnect.
                if state.pending.is_some() {
                    warn!(
                        "capture persistence commit deferred: source \
                         identity not yet observable"
                    );
                }
                None
            }
        };
        if let Some((epoch, source, update)) = persistence {
            self.persist(epoch, source, update, true);
        }
    }

    pub(crate) fn revoke(&self) {
        let epoch = {
            let mut state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.revoked = true;
            state.pending = None;
            state.epoch
        };
        self.inner.config_manager.revoke_capture_persistence(epoch);
    }

    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    fn persist(
        &self,
        epoch: CapturePersistenceEpoch,
        source: Option<CapturePersistenceSource>,
        update: CaptureConfigPersistenceUpdate,
        deferred: bool,
    ) {
        #[cfg(not(target_os = "linux"))]
        let _ = deferred;
        let config_manager = &self.inner.config_manager;
        let snapshot = Arc::clone(&config_manager.get());
        let should_persist = match &update {
            #[cfg(target_os = "macos")]
            CaptureConfigPersistenceUpdate::MacosSource { configured, .. } => {
                snapshot.capture.source == *configured
            }
            #[cfg(target_os = "windows")]
            CaptureConfigPersistenceUpdate::WindowsSource(resolved) => {
                snapshot.capture.source == resolved.configured_source
            }
            #[cfg(target_os = "linux")]
            CaptureConfigPersistenceUpdate::RestoreToken {
                configured,
                resolved,
                ..
            } => {
                if deferred {
                    snapshot.capture.restore_token == *configured
                } else {
                    snapshot.capture.restore_token != *resolved
                }
            }
        };
        if !should_persist {
            return;
        }

        let mutate = |capture: &mut hypercolor_types::config::CaptureConfig| match update {
            #[cfg(target_os = "macos")]
            CaptureConfigPersistenceUpdate::MacosSource { resolved, .. } => {
                capture.source = resolved;
            }
            #[cfg(target_os = "windows")]
            CaptureConfigPersistenceUpdate::WindowsSource(resolved) => {
                capture.source = resolved.stable_source;
            }
            #[cfg(target_os = "linux")]
            CaptureConfigPersistenceUpdate::RestoreToken { resolved, .. } => {
                capture.restore_token = resolved;
            }
        };
        let result = match source {
            Some(source) => config_manager.modify_capture_if_authorized(epoch, source, mutate),
            None => config_manager.modify_capture_if_epoch_current(epoch, mutate),
        };
        match result {
            Ok(Some(_)) => {}
            // A rejection here is a stale epoch or a superseded source, and
            // it means the update is gone; silence would look identical to
            // success and rot the persisted state.
            Ok(None) => warn!(
                "capture persistence rejected: epoch or source authority \
                 superseded"
            ),
            Err(error) => {
                warn!(%error, "Failed to persist resolved screen capture identity");
            }
        }
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    fn persist(
        &self,
        _epoch: CapturePersistenceEpoch,
        _source: Option<CapturePersistenceSource>,
        update: CaptureConfigPersistenceUpdate,
        _deferred: bool,
    ) {
        match update {}
    }
}

fn source_identity(state: &CaptureConfigPersistenceState) -> Option<CapturePersistenceSource> {
    let status = state.source_status.as_ref()?.snapshot();
    if status.source_graph_generation == 0 || status.session_generation == 0 {
        return None;
    }
    Some(CapturePersistenceSource::new(
        Arc::clone(&status.source_id),
        status.source_graph_generation,
        status.session_generation,
    ))
}

/// Whether an update's authorization must pin a source identity.
///
/// Restore tokens authorize by persistence epoch because their session
/// generations legitimately advance across reconnects. macOS picker updates
/// also authorize by epoch: the observer is bound to one exact status handle
/// and accepts only the first strictly newer native selection revision, even
/// while capture has no active session generation.
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn requires_source_identity(update: &CaptureConfigPersistenceUpdate) -> bool {
    match update {
        #[cfg(target_os = "macos")]
        CaptureConfigPersistenceUpdate::MacosSource { .. } => false,
        #[cfg(target_os = "windows")]
        CaptureConfigPersistenceUpdate::WindowsSource(_) => true,
        #[cfg(target_os = "linux")]
        CaptureConfigPersistenceUpdate::RestoreToken { .. } => false,
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
const fn requires_source_identity(_update: &CaptureConfigPersistenceUpdate) -> bool {
    false
}

#[cfg(target_os = "windows")]
fn windows_capture_source_sink(persistence: CaptureConfigPersistenceGate) -> CaptureSourceSink {
    Arc::new(move |resolved: ResolvedCaptureSource| {
        persistence.publish(CaptureConfigPersistenceUpdate::WindowsSource(resolved));
    })
}

#[cfg(target_os = "windows")]
pub(crate) fn build_windows_screen_capture_source(
    capture: &hypercolor_types::config::CaptureConfig,
    persistence: CaptureConfigPersistenceGate,
    admission_coordinator: hypercolor_core::input::screen::ScreenByteAdmissionCoordinator,
    capacity: ScreenAdmissionCapacity,
) -> Result<Box<dyn hypercolor_core::input::InputSource>> {
    Ok(Box::new(
        WindowsScreenCaptureInput::with_admission_coordinator(
            windows_screen_capture_config_from(capture, capacity)?,
            admission_coordinator,
        )
        .with_capture_source_sink(windows_capture_source_sink(persistence)),
    ))
}

#[cfg(target_os = "macos")]
pub(crate) fn build_macos_screen_capture_source(
    capture: &hypercolor_types::config::CaptureConfig,
    admission_coordinator: hypercolor_core::input::screen::ScreenByteAdmissionCoordinator,
    capacity: ScreenAdmissionCapacity,
) -> Result<Box<dyn hypercolor_core::input::InputSource>> {
    Ok(Box::new(MacosScreenCaptureInput::new(
        screen_capture_config_with_capacity_from(capture, capacity)?,
        admission_coordinator,
    )?))
}

/// Build the platform host-input capture source, when config allows one.
///
/// Every supported platform uses an event-driven native backend that reports
/// physical key positions. Returns `None` when input capture is disabled or
/// no source kind is enabled.
pub(crate) fn build_interaction_source(
    input: &hypercolor_types::config::InputConfig,
) -> Option<Box<dyn hypercolor_core::input::InputSource>> {
    if !input.enabled {
        return None;
    }

    #[cfg(target_os = "linux")]
    {
        (input.keyboard || input.mouse).then(|| {
            Box::new(EvdevHostInput::new(input.keyboard, input.mouse))
                as Box<dyn hypercolor_core::input::InputSource>
        })
    }

    // Consent is per-kind: declining the mouse means the process is never
    // registered for the mouse usage at all, so no pointer position can reach
    // the daemon even by accident.
    #[cfg(target_os = "windows")]
    {
        (input.keyboard || input.mouse).then(|| {
            Box::new(WindowsHostInput::new(input.keyboard, input.mouse))
                as Box<dyn hypercolor_core::input::InputSource>
        })
    }

    #[cfg(target_os = "macos")]
    {
        build_macos_host_input_source(input)
            .map(|source| Box::new(source) as Box<dyn hypercolor_core::input::InputSource>)
    }

    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    {
        let _ = input;
        None
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn build_macos_host_input_source(
    input: &hypercolor_types::config::InputConfig,
) -> Option<MacosHostInput> {
    (input.enabled && (input.keyboard || input.mouse))
        .then(|| MacosHostInput::new(input.keyboard, input.mouse))
}

/// Build the Wayland screen capture source with a restore-token sink that
/// persists the portal's source selection back into the daemon config.
#[cfg(target_os = "linux")]
pub(crate) fn build_screen_capture_source(
    capture: &hypercolor_types::config::CaptureConfig,
    persistence: CaptureConfigPersistenceGate,
    admission_coordinator: hypercolor_core::input::screen::ScreenByteAdmissionCoordinator,
    capacity: ScreenAdmissionCapacity,
) -> Result<Box<dyn hypercolor_core::input::InputSource>> {
    let capture_config = screen_capture_config_with_capacity_from(capture, capacity)?;
    let configured = capture.restore_token.clone();
    let sink = Arc::new(move |token: Option<String>| {
        persistence.publish(CaptureConfigPersistenceUpdate::RestoreToken {
            configured: configured.clone(),
            resolved: token,
        });
    });

    Ok(Box::new(
        WaylandScreenCaptureInput::with_admission_coordinator(
            capture_config,
            admission_coordinator,
        )
        .with_restore_token_sink(sink),
    ))
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub(crate) fn screen_capture_config_from(
    capture: &hypercolor_types::config::CaptureConfig,
) -> Result<ScreenCaptureConfig> {
    capture
        .validate()
        .context("invalid screen capture configuration")?;
    let acquisition_cadence = match capture.cadence {
        hypercolor_types::config::CaptureCadenceMode::Fixed => {
            hypercolor_core::input::screen::ScreenCaptureCadence::frames_per_second(
                capture.capture_fps,
            )
            .context("screen capture cadence is not representable by the runtime scheduler")?
        }
        hypercolor_types::config::CaptureCadenceMode::NativeRefresh => {
            hypercolor_core::input::screen::ScreenCaptureCadence::NativeRefresh
        }
    };
    Ok(ScreenCaptureConfig {
        target_fps: capture.capture_fps,
        acquisition_cadence,
        grid_cols: capture.grid_cols,
        grid_rows: capture.grid_rows,
        analysis_memory_bytes: u64::MAX,
        smoothing_alpha: capture.smoothing,
        scene_cut_threshold: capture.scene_cut_threshold,
        letterbox_threshold: capture.letterbox_threshold,
        letterbox_enabled: capture.letterbox,
        tuning: hypercolor_core::input::screen::ColorTuning {
            saturation: capture.saturation,
            brightness: capture.brightness,
            gamma: capture.gamma,
        },
        target_led_white_x: capture.target_led_white_x,
        target_led_white_y: capture.target_led_white_y,
        target_led_reference_white_nits: capture.target_led_reference_white_nits,
        target_led_peak_nits: capture.target_led_peak_nits,
        exposure_ev: capture.exposure_ev,
        restore_token: capture.restore_token.clone(),
        source: capture.source.clone(),
    })
}

#[cfg(target_os = "windows")]
fn windows_screen_capture_config_from(
    capture: &hypercolor_types::config::CaptureConfig,
    capacity: ScreenAdmissionCapacity,
) -> Result<ScreenCaptureConfig> {
    screen_capture_config_with_capacity_from(capture, capacity)
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn screen_capture_config_with_capacity_from(
    capture: &hypercolor_types::config::CaptureConfig,
    capacity: ScreenAdmissionCapacity,
) -> Result<ScreenCaptureConfig> {
    let analysis_memory_bytes = capacity.byte_budget().min(capacity.backend_capacity());
    ScreenAnalysisResourcePlan::try_new(
        capture.grid_cols,
        capture.grid_rows,
        capture.capture_fps,
        analysis_memory_bytes,
    )
    .context("screen analysis grid exceeds installed resource capacity")?;
    Ok(ScreenCaptureConfig {
        analysis_memory_bytes,
        ..screen_capture_config_from(capture)?
    })
}
fn audio_source_from_device(device: &str) -> AudioSourceType {
    let normalized = device.trim();
    if normalized.eq_ignore_ascii_case("none") {
        AudioSourceType::None
    } else if normalized.eq_ignore_ascii_case("default") {
        AudioSourceType::SystemMonitor
    } else if normalized.eq_ignore_ascii_case("microphone") {
        AudioSourceType::Microphone
    } else {
        AudioSourceType::Named(normalized.to_owned())
    }
}

fn noise_gate_to_db(noise_gate: f32) -> f32 {
    let linear = noise_gate.clamp(0.000_001, 1.0);
    20.0 * linear.log10()
}

#[cfg(all(
    test,
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
mod tests;

#[cfg(test)]
mod library_startup_tests {
    use super::*;

    #[test]
    fn corrupt_persisted_library_stops_store_initialization() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("library.json");
        std::fs::write(&path, b"not-json").expect("corrupt fixture");

        let Err(error) = open_persisted_library_store(&path) else {
            panic!("corrupt library must fail closed");
        };

        assert!(format!("{error:#}").contains("failed to open persisted library store"));
    }

    #[cfg(unix)]
    #[test]
    fn inaccessible_persisted_library_stops_store_initialization() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("library.json");
        symlink("library.json", &path).expect("self-referential symlink");

        let Err(error) = open_persisted_library_store(&path) else {
            panic!("inaccessible library must fail closed");
        };

        assert!(format!("{error:#}").contains("failed to open persisted library store"));
        assert!(matches!(
            error.downcast_ref::<crate::library::JsonLibraryStoreOpenError>(),
            Some(crate::library::JsonLibraryStoreOpenError::Read { .. })
        ));
    }
}

#[cfg(all(test, target_os = "macos"))]
mod macos_input_tests {
    use super::build_macos_host_input_source;

    #[test]
    fn startup_preserves_per_kind_consent() {
        let disabled = hypercolor_types::config::InputConfig::default();
        assert!(build_macos_host_input_source(&disabled).is_none());

        let keyboard = hypercolor_types::config::InputConfig {
            enabled: true,
            keyboard: true,
            mouse: false,
            ..Default::default()
        };
        let pointer = hypercolor_types::config::InputConfig {
            enabled: true,
            keyboard: false,
            mouse: true,
            ..Default::default()
        };

        assert_eq!(
            build_macos_host_input_source(&keyboard)
                .expect("keyboard source is configured")
                .capture_kinds(),
            (true, false)
        );
        assert_eq!(
            build_macos_host_input_source(&pointer)
                .expect("pointer source is configured")
                .capture_kinds(),
            (false, true)
        );
    }
}
