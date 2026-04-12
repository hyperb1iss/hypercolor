//! Daemon lifecycle: start, shutdown, runtime session persistence, and background workers.

use std::sync::Arc;

use anyhow::{Context, Result};
use tracing::{debug, info, warn};

use hypercolor_core::device::{UsbHotplugEvent, UsbHotplugMonitor};
use hypercolor_core::effect::{
    EffectRegistry, EffectWatchEvent, EffectWatcher, create_renderer_for_metadata_with_mode,
};
use hypercolor_core::engine::FpsTier;
use hypercolor_types::config::HypercolorConfig;
use hypercolor_types::effect::{EffectId, EffectMetadata};

use crate::discovery::{self, DiscoveryBackend};
use crate::display_output::overlay::DefaultOverlayRendererFactory;
use crate::display_output::{
    DEFAULT_STATIC_HOLD_REFRESH_INTERVAL, DisplayOutputState, DisplayOutputThread,
};
use crate::render_thread::{CanvasDims, RenderThread, RenderThreadState};
use crate::runtime_state::{self, RuntimeSessionSnapshot};
use crate::scene_transactions::apply_layout_update;
use crate::session::{SessionController, current_global_brightness, set_global_brightness};
use crate::simulators::activate_simulated_displays;

use super::DaemonState;
use super::discovery_worker::DiscoveryWorkerContext;
use super::resolve_compositor_acceleration_mode;

impl DaemonState {
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

        // Restore persisted runtime session (last active effect/preset/controls)
        // before the render loop begins producing frames.
        self.restore_runtime_session_if_configured(&config).await;

        self.session_controller = Some(SessionController::start(
            Arc::clone(&self.config_manager),
            Arc::clone(&self.event_bus),
            Arc::clone(&self.effect_engine),
            self.power_state.clone(),
            self.discovery_runtime(),
            Arc::clone(&self.driver_host),
            Arc::clone(&self.driver_registry),
        ));

        activate_simulated_displays(&self.discovery_runtime(), &self.simulated_displays)
            .await
            .context("failed to activate virtual display simulators")?;

        // Start the render loop.
        {
            let mut loop_guard = self.render_loop.write().await;
            loop_guard.start();
        }

        // Spawn the render thread.
        let render_acceleration =
            resolve_compositor_acceleration_mode(config.effect_engine.render_acceleration_mode)
                .context("failed to resolve compositor acceleration mode while starting daemon")?;
        let rt_state = RenderThreadState {
            effect_engine: Arc::clone(&self.effect_engine),
            effect_registry: Arc::clone(&self.effect_registry),
            spatial_engine: Arc::clone(&self.spatial_engine),
            backend_manager: Arc::clone(&self.backend_manager),
            performance: Arc::clone(&self.performance),
            discovery_runtime: Some(self.discovery_runtime()),
            event_bus: Arc::clone(&self.event_bus),
            preview_runtime: Arc::clone(&self.preview_runtime),
            render_loop: Arc::clone(&self.render_loop),
            scene_manager: Arc::clone(&self.scene_manager),
            input_manager: Arc::clone(&self.input_manager),
            power_state: self.power_state.subscribe(),
            device_settings: Arc::clone(&self.device_settings),
            scene_transactions: self.scene_transactions.clone(),
            screen_capture_configured: config.capture.enabled,
            canvas_dims: CanvasDims::new(config.daemon.canvas_width, config.daemon.canvas_height),
            render_acceleration_mode: render_acceleration.effective_mode,
            configured_max_fps_tier: FpsTier::from_fps(config.daemon.target_fps),
        };
        self.render_thread = Some(
            RenderThread::try_spawn(rt_state)
                .context("failed to spawn render thread with resolved compositor mode")?,
        );
        let sensor_snapshot_rx = self
            .input_manager
            .lock()
            .await
            .sensor_snapshot_receiver()
            .expect("display output requires a configured sensor snapshot receiver");
        self.display_output_thread = Some(DisplayOutputThread::spawn(DisplayOutputState {
            backend_manager: Arc::clone(&self.backend_manager),
            device_registry: self.device_registry.clone(),
            spatial_engine: Arc::clone(&self.spatial_engine),
            scene_manager: Arc::clone(&self.scene_manager),
            logical_devices: Arc::clone(&self.logical_devices),
            device_settings: Arc::clone(&self.device_settings),
            event_bus: Arc::clone(&self.event_bus),
            power_state: self.power_state.subscribe(),
            static_hold_refresh_interval: DEFAULT_STATIC_HOLD_REFRESH_INTERVAL,
            display_overlays: Arc::clone(&self.display_overlays),
            display_overlay_runtime: Arc::clone(&self.display_overlay_runtime),
            sensor_snapshot_rx,
            overlay_factory: Arc::new(DefaultOverlayRendererFactory::new()),
            display_frames: Arc::clone(&self.display_frames),
        }));

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

        // Spawn effect file watcher for hot-reload.
        if config.effect_engine.watch_effects {
            self.spawn_effect_watcher().await;
        }

        self.spawn_discovery_worker(Arc::clone(&config));

        info!("Daemon is running");
        Ok(())
    }

    /// Graceful shutdown — stops all subsystems in reverse-dependency order.
    ///
    /// Sequence:
    /// 1. Stop render loop (no more frames produced)
    /// 2. Wait for render thread to exit
    /// 3. Clear and disconnect renderable devices
    /// 4. Persist the current runtime session snapshot
    /// 5. Deactivate the effect engine (release renderer resources)
    /// 6. Scene manager cleanup
    /// 7. Log final state
    ///
    /// # Errors
    ///
    /// Returns an error if any shutdown step fails critically. Non-critical
    /// failures are logged as warnings and do not prevent the rest of the
    /// sequence from completing.
    pub async fn shutdown(&mut self) -> Result<()> {
        info!("Beginning graceful shutdown");

        if let Some(controller) = self.session_controller.take() {
            controller.shutdown().await;
        }

        // 1. Stop render loop — next tick() will return false.
        {
            let mut loop_guard = self.render_loop.write().await;
            loop_guard.stop();
        }
        info!("Render loop stopped");

        // 2. Wait for render thread to exit.
        if let Some(mut rt) = self.render_thread.take()
            && let Err(e) = rt.shutdown().await
        {
            warn!(error = %e, "render thread shutdown error");
        }
        if let Some(mut output) = self.display_output_thread.take()
            && let Err(e) = output.shutdown().await
        {
            warn!(error = %e, "display output shutdown error");
        }

        if let Some(handle) = self.effect_watcher_task.take() {
            handle.abort();
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

        let disconnected_devices =
            discovery::shutdown_renderable_devices(&self.discovery_runtime()).await;
        info!(
            disconnected_devices,
            "Render devices cleared and disconnected"
        );

        // 4. Stop input sources.
        {
            let mut input_manager = self.input_manager.lock().await;
            input_manager.stop_all();
        }
        info!("Input sources stopped");

        // 5. Persist the current runtime session before tearing down effect state.
        self.persist_runtime_session_snapshot().await;
        info!("Runtime session snapshot persisted");

        // 6. Deactivate effect engine.
        {
            let mut engine_guard = self.effect_engine.lock().await;
            engine_guard.deactivate();
        }
        info!("Effect engine deactivated");

        // 7. Scene manager — deactivate current scene.
        {
            let mut scene_guard = self.scene_manager.write().await;
            scene_guard.deactivate_current();
        }
        info!("Scene manager cleaned up");

        // 8. Log final device count.
        let device_count = self.device_registry.len().await;
        info!(devices = device_count, "Device registry final state");

        // 9. Publish shutdown event.
        self.event_bus
            .publish(hypercolor_types::event::HypercolorEvent::DaemonShutdown {
                reason: "signal".to_string(),
            });

        info!("Graceful shutdown complete");
        Ok(())
    }

    async fn persist_runtime_session_snapshot(&self) {
        let mut snapshot = {
            let engine = self.effect_engine.lock().await;
            runtime_state::snapshot_from_engine(&engine)
        };

        {
            let spatial = self.spatial_engine.read().await;
            snapshot.active_layout_id = Some(spatial.layout().id.clone());
        }
        snapshot.global_brightness = current_global_brightness(&self.power_state);
        snapshot.wled_probe_ips =
            runtime_state::collect_wled_probe_ips(&self.device_registry).await;
        snapshot.wled_probe_targets =
            runtime_state::collect_wled_probe_targets(&self.device_registry).await;

        if let Err(error) = runtime_state::save(&self.runtime_state_path, &snapshot) {
            warn!(
                path = %self.runtime_state_path.display(),
                %error,
                "Failed to persist runtime session snapshot"
            );
        }
    }

    async fn restore_runtime_session_if_configured(&self, config: &HypercolorConfig) {
        let profile_mode = config.daemon.start_profile.trim();
        if !profile_mode.eq_ignore_ascii_case("last") {
            return;
        }

        let snapshot = match runtime_state::load(&self.runtime_state_path) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                warn!(
                    path = %self.runtime_state_path.display(),
                    %error,
                    "Failed to load runtime session snapshot"
                );
                return;
            }
        };
        let Some(snapshot) = snapshot else {
            debug!(
                path = %self.runtime_state_path.display(),
                "No runtime session snapshot found to restore"
            );
            return;
        };
        set_global_brightness(&self.power_state, snapshot.global_brightness);
        {
            let mut settings = self.device_settings.write().await;
            settings.set_global_brightness(snapshot.global_brightness);
            if let Err(error) = settings.save() {
                warn!(
                    %error,
                    "Failed to sync restored global brightness into device settings store"
                );
            }
        }

        // Restore active layout if persisted.
        if let Some(layout_id) = &snapshot.active_layout_id {
            let layouts = self.layouts.read().await;
            if let Some(layout) = layouts.get(layout_id) {
                apply_layout_update(
                    &self.spatial_engine,
                    &self.scene_transactions,
                    layout.clone(),
                )
                .await;
                info!(layout_id, layout_name = %layout.name, "Restored active layout");
            } else {
                debug!(
                    layout_id,
                    "Persisted active layout not found in store; using default"
                );
            }
        }

        if let Err(error) = self.apply_runtime_session_snapshot(snapshot).await {
            warn!(%error, "Failed to restore runtime session snapshot");
        }
    }

    async fn apply_runtime_session_snapshot(
        &self,
        snapshot: RuntimeSessionSnapshot,
    ) -> anyhow::Result<()> {
        let Some(active_effect_id) = snapshot.active_effect_id.as_deref() else {
            return Ok(());
        };

        let metadata = {
            let registry = self.effect_registry.read().await;
            resolve_effect_metadata_for_restore(&registry, active_effect_id)
        };
        let Some(metadata) = metadata else {
            anyhow::bail!("saved effect is no longer available: {active_effect_id}");
        };

        let renderer = create_renderer_for_metadata_with_mode(
            &metadata,
            crate::api::effect_renderer_acceleration_mode(
                self.config_manager
                    .get()
                    .effect_engine
                    .render_acceleration_mode,
            ),
        )
        .with_context(|| format!("failed to create renderer for '{}'", metadata.name))?;

        let mut rejected_controls: Vec<String> = Vec::new();
        let mut rejected_bindings: Vec<String> = Vec::new();
        {
            let mut engine = self.effect_engine.lock().await;
            engine
                .activate(renderer, metadata.clone())
                .with_context(|| format!("failed to activate '{}'", metadata.name))?;

            for (name, value) in &snapshot.control_values {
                if let Err(error) = engine.set_control_checked(name, value) {
                    rejected_controls.push(format!("{name} ({error})"));
                }
            }

            for (name, binding) in &snapshot.control_bindings {
                if let Err(error) = engine.set_control_binding(name, binding.clone()) {
                    rejected_bindings.push(format!("{name} ({error})"));
                }
            }

            if let Some(preset_id) = snapshot.active_preset_id {
                engine.set_active_preset_id(preset_id);
            }
        }

        if !rejected_controls.is_empty() {
            warn!(
                effect_id = %metadata.id,
                effect = %metadata.name,
                rejected_controls = ?rejected_controls,
                "Some persisted control values were rejected during restore"
            );
        }

        if !rejected_bindings.is_empty() {
            warn!(
                effect_id = %metadata.id,
                effect = %metadata.name,
                rejected_bindings = ?rejected_bindings,
                "Some persisted control bindings were rejected during restore"
            );
        }

        info!(
            effect_id = %metadata.id,
            effect = %metadata.name,
            controls = snapshot.control_values.len(),
            bindings = snapshot.control_bindings.len(),
            "Restored runtime session snapshot"
        );

        Ok(())
    }

    async fn spawn_effect_watcher(&mut self) {
        let registry = Arc::clone(&self.effect_registry);
        let event_bus = Arc::clone(&self.event_bus);

        let search_paths = {
            let reg = self.effect_registry.read().await;
            reg.search_paths().to_vec()
        };

        let (watcher, mut rx) = match EffectWatcher::start(&search_paths) {
            Ok(pair) => pair,
            Err(error) => {
                warn!(%error, "Failed to start effect file watcher; hot-reload disabled");
                return;
            }
        };

        // Keep the watcher alive by moving it into the task.
        self.effect_watcher_task = Some(tokio::spawn(async move {
            let _watcher = watcher; // prevent drop until task ends

            info!("✨ Effect hot-reload watcher active");

            while let Some(event) = rx.recv().await {
                let (action, path) = match &event {
                    EffectWatchEvent::Created(p) => ("created", p.clone()),
                    EffectWatchEvent::Modified(p) => ("modified", p.clone()),
                    EffectWatchEvent::Removed(p) => ("removed", p.clone()),
                };
                info!(path = %path.display(), action, "Effect file change detected");

                let report = {
                    let mut reg = registry.write().await;
                    reg.reload_single(&path)
                };

                event_bus.publish(
                    hypercolor_types::event::HypercolorEvent::EffectRegistryUpdated {
                        added: report.added,
                        removed: report.removed,
                        updated: report.updated,
                    },
                );
            }

            debug!("Effect watcher channel closed; task exiting");
        }));
    }

    #[allow(
        clippy::too_many_lines,
        reason = "startup wires the full discovery worker context in one place for readability"
    )]
    fn spawn_discovery_worker(&mut self, config: Arc<HypercolorConfig>) {
        let worker = DiscoveryWorkerContext {
            device_registry: self.device_registry.clone(),
            backend_manager: Arc::clone(&self.backend_manager),
            lifecycle_manager: Arc::clone(&self.lifecycle_manager),
            reconnect_tasks: Arc::clone(&self.reconnect_tasks),
            event_bus: Arc::clone(&self.event_bus),
            config_manager: Arc::clone(&self.config_manager),
            driver_host: Arc::clone(&self.driver_host),
            driver_registry: Arc::clone(&self.driver_registry),
            spatial_engine: Arc::clone(&self.spatial_engine),
            layouts: Arc::clone(&self.layouts),
            layouts_path: self.layouts_path.clone(),
            layout_auto_exclusions: Arc::clone(&self.layout_auto_exclusions),
            logical_devices: Arc::clone(&self.logical_devices),
            attachment_registry: Arc::clone(&self.attachment_registry),
            attachment_profiles: Arc::clone(&self.attachment_profiles),
            device_settings: Arc::clone(&self.device_settings),
            runtime_state_path: self.runtime_state_path.clone(),
            usb_protocol_configs: self.usb_protocol_configs.clone(),
            credential_store: Arc::clone(&self.credential_store),
            in_progress: Arc::clone(&self.discovery_in_progress),
            scene_transactions: self.scene_transactions.clone(),
        };

        let initial_backends =
            match discovery::resolve_backends(None, &config, self.driver_registry.as_ref()) {
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
            worker.run_startup_wled_recovery_scans().await;

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

fn resolve_effect_metadata_for_restore(
    registry: &EffectRegistry,
    id_or_name: &str,
) -> Option<EffectMetadata> {
    if let Ok(uuid) = id_or_name.parse::<uuid::Uuid>() {
        return registry
            .get(&EffectId::new(uuid))
            .map(|entry| entry.metadata.clone());
    }

    registry
        .iter()
        .find(|(_, entry)| entry.metadata.matches_lookup(id_or_name))
        .map(|(_, entry)| entry.metadata.clone())
}
