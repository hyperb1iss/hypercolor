//! Daemon lifecycle: start, shutdown, runtime session persistence, and background workers.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tracing::{debug, info, warn};

use hypercolor_core::device::{UsbHotplugEvent, UsbHotplugMonitor};
use hypercolor_core::effect::{EffectWatchEvent, EffectWatcher};
use hypercolor_types::config::{EffectErrorFallbackPolicy, HypercolorConfig};
use hypercolor_types::event::{HypercolorEvent, SceneChangeReason};
use hypercolor_types::scene::SceneId;

use crate::device_metrics::spawn_device_metrics_collector;
use crate::discovery::{self, DiscoveryTarget};
use crate::display_output::{
    DEFAULT_STATIC_HOLD_REFRESH_INTERVAL, DisplayOutputState, DisplayOutputThread,
};
use crate::interactive_preview::{
    InteractivePreviewAcceleration, InteractivePreviewContext, InteractivePreviewExecutor,
};
use crate::persistence::{self, AtomicWriteOutcome};
use crate::render_thread::{CanvasDims, RenderThread, RenderThreadState};
use crate::runtime_state::{self, RuntimeSessionSnapshot};
use crate::session::SessionController;
use crate::simulators::activate_simulated_displays;

use super::DaemonState;
use super::discovery_worker::DiscoveryWorkerContext;
use super::input_status_events::InputStatusEventPublisher;

const USB_HOTPLUG_REMOVAL_RECOVERY_SCAN_DELAY: Duration = Duration::from_secs(2);
const SHUTDOWN_PERSISTENCE_FLUSH_TIMEOUT: Duration = Duration::from_secs(5);

impl DaemonState {
    /// Start all subsystems — render loop, render thread, backend discovery.
    ///
    /// After this call the daemon is fully operational and processing frames.
    ///
    /// # Errors
    ///
    /// Returns an error if any subsystem fails to start.
    pub async fn start(&mut self) -> Result<()> {
        if let Err(start_error) = Box::pin(self.start_inner()).await {
            if let Err(rollback_error) = self.shutdown().await {
                return Err(start_error.context(format!(
                    "daemon startup rollback also failed: {rollback_error:#}"
                )));
            }
            return Err(start_error);
        }
        Ok(())
    }

    async fn start_inner(&mut self) -> Result<()> {
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
        if self.input_status_event_publisher.is_none() {
            self.input_status_event_publisher = Some(InputStatusEventPublisher::start(
                self.input_status.clone(),
                Arc::clone(&self.event_bus),
            ));
        }

        // Seed portable identity pins before the first scan, so a claimed
        // device's first attach of the session resolves to the identity
        // its layouts reference.
        if let Some(aliases) = self.startup_device_aliases.take() {
            crate::device_aliases::seed_registry_document(
                &self.device_aliases_path,
                aliases,
                &self.device_registry,
            )
            .await;
        } else {
            crate::device_aliases::seed_registry(&self.device_aliases_path, &self.device_registry)
                .await;
        }

        // Restore persisted scene state before the render loop begins producing frames.
        self.restore_runtime_session(&config).await;

        self.session_controller = Some(SessionController::start(
            Arc::clone(&self.config_manager),
            Arc::clone(&self.event_bus),
            self.output_power.clone(),
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
            if self.output_power.snapshot().manually_paused() {
                loop_guard.pause();
            }
        }

        // Spawn the render thread.
        let initial_canvas_dims = {
            let spatial = self.spatial_engine.snapshot();
            let layout = spatial.layout();
            CanvasDims::new(layout.canvas_width, layout.canvas_height)
        };
        let rt_state = RenderThreadState {
            effect_registry: Arc::clone(&self.effect_registry),
            asset_library: Arc::clone(&self.asset_library),
            spatial_engine: self.spatial_engine.clone(),
            backend_manager: Arc::clone(&self.backend_manager),
            device_registry: self.device_registry.clone(),
            performance: Arc::clone(&self.performance),
            discovery_runtime: Some(self.discovery_runtime()),
            event_bus: Arc::clone(&self.event_bus),
            preview_runtime: Arc::clone(&self.preview_runtime),
            zone_layout_previews: Arc::clone(&self.zone_layout_previews),
            render_loop: Arc::clone(&self.render_loop),
            scene_manager: self.scene_manager.clone(),
            scene_plan: self.scene_manager.plan_reader(),
            input_manager: Arc::clone(&self.input_manager),
            interaction_routing: self.interaction_routing.clone(),
            power_state: self.output_power.subscribe(),
            scene_transactions: self.scene_transactions.clone(),
            screen_capture_configured: config.capture.enabled,
            canvas_dims: initial_canvas_dims,
            render_acceleration_mode: self.render_acceleration.effective_mode,
            #[cfg(feature = "wgpu")]
            render_gpu_device: self.render_acceleration.gpu_render_device.clone(),
            configured_max_fps_tier: self.configured_max_fps_tier.clone(),
            face_fps_cap: config.display.effective_face_fps_cap(),
        };
        self.render_thread = Some(
            RenderThread::try_spawn(rt_state)
                .context("failed to spawn render thread with resolved compositor mode")?,
        );
        let (input_graph, sensor_snapshots) = {
            let input_manager = self.input_manager.lock().await;
            (
                input_manager.input_graph_handle(),
                input_manager.sensor_snapshot_receiver(),
            )
        };
        let input_demands = self
            .render_thread
            .as_ref()
            .expect("render thread was installed after successful spawn")
            .input_publication_demands();
        let interactive_preview = InteractivePreviewExecutor::start(InteractivePreviewContext {
            scene_manager: self.scene_manager.clone(),
            effect_registry: Arc::clone(&self.effect_registry),
            asset_library: Some(Arc::clone(&self.asset_library)),
            event_bus: Arc::clone(&self.event_bus),
            input_graph,
            sensor_snapshots,
            interaction_routing: self.interaction_routing.clone(),
            input_demands,
            canvas_width: config.daemon.canvas_width,
            canvas_height: config.daemon.canvas_height,
            acceleration: InteractivePreviewAcceleration::from_authoritative(
                self.render_acceleration.effective_mode,
                #[cfg(feature = "wgpu")]
                self.render_acceleration.gpu_render_device.clone(),
            ),
            resource_capacity_bytes: config.web.interactive_preview_resource_bytes,
        })
        .await
        .context("failed to start interactive preview executor")?;
        self.preview_runtime
            .install_interactive_executor(Arc::new(interactive_preview));
        self.display_output_thread = Some(DisplayOutputThread::spawn(DisplayOutputState {
            backend_manager: Arc::clone(&self.backend_manager),
            device_registry: self.device_registry.clone(),
            spatial_engine: self.spatial_engine.clone(),
            logical_devices: Arc::clone(&self.logical_devices),
            event_bus: Arc::clone(&self.event_bus),
            preview_runtime: Arc::clone(&self.preview_runtime),
            power_state: self.output_power.subscribe(),
            static_hold_refresh_interval: DEFAULT_STATIC_HOLD_REFRESH_INTERVAL,
            display_frames: Arc::clone(&self.display_frames),
            face_fps_cap: config.display.effective_face_fps_cap(),
        }));
        self.domains.output.reconcile_static_hold().await;
        self.device_metrics_collector_task = Some(spawn_device_metrics_collector(
            Arc::clone(&self.device_metrics),
            Arc::clone(&self.backend_manager),
        ));

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

        self.spawn_effect_error_fallback_worker();
        self.spawn_output_static_hold_worker();
        self.spawn_display_preference_sync_worker();
        if config.discovery.background_enabled {
            self.spawn_discovery_worker(Arc::clone(&config));
        }

        for extension in self.lifecycle_extensions.clone() {
            info!(extension = extension.name(), "Starting daemon extension");
            extension.start(self).await.with_context(|| {
                format!("failed to start daemon extension {}", extension.name())
            })?;
        }

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
    /// 5. Scene manager cleanup
    /// 6. Log final state
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

        for extension in self.lifecycle_extensions.clone().iter().rev() {
            info!(
                extension = extension.name(),
                "Shutting down daemon extension"
            );
            if let Err(error) = extension.shutdown(self).await {
                warn!(
                    extension = extension.name(),
                    %error,
                    "daemon extension shutdown error"
                );
            }
        }

        self.preview_runtime.clear_interactive_executor();

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

        #[cfg(feature = "servo")]
        match hypercolor_core::effect::shutdown_servo_runtime() {
            Ok(()) => info!("Servo runtime shutdown complete"),
            Err(e) => warn!(error = %e, "Servo runtime shutdown error"),
        }

        if let Some(handle) = self.effect_watcher_task.take() {
            handle.abort();
            if let Err(error) = handle.await
                && !error.is_cancelled()
            {
                warn!(%error, "Effect watcher did not stop cleanly");
            }
        }
        if let Some(handle) = self.display_preference_sync_task.take() {
            handle.abort();
        }
        if let Some(handle) = self.output_static_hold_task.take() {
            handle.abort();
        }
        if let Some(handle) = self.effect_error_fallback_task.take() {
            handle.abort();
        }
        if let Some(handle) = self.discovery_task.take() {
            handle.abort();
        }
        if let Some(handle) = self.device_metrics_collector_task.take() {
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
        drop(self.input_status_event_publisher.take());

        // 5. Persist the current runtime session before scene cleanup.
        let runtime_snapshot = self.persist_runtime_session_snapshot().await;
        let scene_snapshot = persist_scene_store_snapshot(&self.scene_manager).await;
        let flush_report = persistence::flush_all(SHUTDOWN_PERSISTENCE_FLUSH_TIMEOUT);
        for error in flush_report.errors() {
            warn!(%error, "Persistence retry did not converge during shutdown");
        }
        let persistence_complete =
            runtime_snapshot.is_ok() && scene_snapshot.is_ok() && flush_report.is_complete();
        if let (Ok(runtime), Ok(scenes)) = (&runtime_snapshot, &scene_snapshot)
            && flush_report.is_complete()
        {
            info!(
                ?runtime,
                ?scenes,
                clean = flush_report.clean(),
                committed = flush_report.written(),
                superseded = flush_report.superseded(),
                "Shutdown persistence complete"
            );
        } else {
            if let Err(error) = &runtime_snapshot {
                warn!(
                    path = %self.runtime_state_path.display(),
                    %error,
                    "Runtime session snapshot failed during shutdown"
                );
            }
            if let Err(error) = &scene_snapshot {
                warn!(%error, "Scene store snapshot failed during shutdown");
            }
            warn!(
                clean = flush_report.clean(),
                committed = flush_report.written(),
                superseded = flush_report.superseded(),
                failed = flush_report.errors().len(),
                "Shutdown completed with persistence failures"
            );
        }

        // 6. Scene manager — deactivate current scene.
        //
        // POST-TEARDOWN WRITER (Spec 76 §2.3), and the only one of this
        // file's five that is not pre-init. Step 2 above already
        // awaited the render thread's exit, so by here the one thread
        // that reads scene state on a cadence is gone and every durable
        // store has flushed — there is nothing left to race and nothing
        // left to observe the result, which is dropped with the manager
        // a few lines later.
        let mut mutation = self.scene_manager.begin_mutation().await;
        mutation.deactivate_current(SceneChangeReason::UserDeactivate);
        self.scene_manager
            .commit_mutation(mutation)
            .await
            .context("failed to deactivate scene during shutdown")?;
        info!("Scene manager cleaned up");

        // 7. Log final device count.
        let device_count = self.device_registry.len().await;
        info!(devices = device_count, "Device registry final state");

        // 8. Publish shutdown event.
        self.event_bus
            .publish(hypercolor_types::event::HypercolorEvent::DaemonShutdown {
                reason: "signal".to_string(),
            });

        if persistence_complete {
            info!("Graceful shutdown complete");
        } else {
            warn!("Graceful shutdown completed with persistence failures");
        }
        Ok(())
    }

    async fn persist_runtime_session_snapshot(&self) -> Result<AtomicWriteOutcome> {
        self.domains
            .runtime_session
            .persist_snapshot()
            .await
            .map_err(Into::into)
    }

    async fn restore_runtime_session(&mut self, config: &HypercolorConfig) {
        let scene_mode = config.daemon.start_scene.trim();
        let snapshot = self.startup_runtime_snapshot.take().or_else(|| {
            match runtime_state::load(&self.runtime_state_path) {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    warn!(
                        path = %self.runtime_state_path.display(),
                        %error,
                        "Failed to load runtime session snapshot"
                    );
                    None
                }
            }
        });

        if let Some(snapshot) = snapshot.as_ref()
            && snapshot.manual_paused
        {
            self.output_power.restore_manual_pause([0, 0, 0]).await;
        }

        if !scene_mode.eq_ignore_ascii_case("last") {
            self.activate_configured_start_scene(scene_mode).await;
            return;
        }

        let Some(snapshot) = snapshot else {
            debug!(
                path = %self.runtime_state_path.display(),
                "No runtime session snapshot found to restore"
            );
            return;
        };
        // Restore active layout if persisted.
        if let Some(layout_id) = &snapshot.active_layout_id {
            match self.domains.layout.restore_startup_layout(layout_id).await {
                Ok(Some(layout)) => {
                    info!(layout_id, layout_name = %layout.name, "Restored active layout");
                }
                Ok(None) => {
                    debug!(
                        layout_id,
                        "Persisted active layout not found in store; using default"
                    );
                }
                Err(error) => {
                    warn!(layout_id, %error, "Rejected persisted active layout");
                }
            }
        }

        if let Err(error) = self.apply_runtime_session_snapshot(snapshot).await {
            warn!(%error, "Failed to restore runtime session snapshot");
        }
    }

    async fn activate_configured_start_scene(&self, selector: &str) {
        if selector.is_empty() {
            return;
        }

        let target = {
            let scenes = self.scene_manager.snapshot().await;
            let scene_id = if selector.eq_ignore_ascii_case("default") {
                Some(SceneId::DEFAULT)
            } else if let Ok(uuid) = selector.parse::<uuid::Uuid>() {
                Some(SceneId(uuid))
            } else {
                let matches = scenes
                    .list()
                    .into_iter()
                    .filter(|scene| scene.name.eq_ignore_ascii_case(selector))
                    .map(|scene| scene.id)
                    .collect::<Vec<_>>();
                match matches.as_slice() {
                    [scene_id] => Some(*scene_id),
                    [] => None,
                    _ => {
                        warn!(selector, candidates = ?matches, "Configured startup scene is ambiguous");
                        return;
                    }
                }
            };
            let Some(scene_id) = scene_id else {
                warn!(selector, "Configured startup scene was not found");
                return;
            };
            let Some(scene) = scenes.get(&scene_id) else {
                warn!(selector, scene_id = %scene_id, "Configured startup scene was not found");
                return;
            };
            (
                scene.id,
                scene.name.clone(),
                scene.layout_id.clone(),
                scene.activation_brightness,
                scenes.active_scene_id().copied(),
            )
        };

        let (scene_id, scene_name, layout_id, activation_brightness, previous_scene_id) = target;
        if previous_scene_id == Some(scene_id) {
            return;
        }

        {
            let mut mutation = self.scene_manager.begin_mutation().await;
            if let Err(error) = mutation.activate(scene_id, None, SceneChangeReason::DaemonStart) {
                warn!(selector, scene_id = %scene_id, %error, "Failed to activate configured startup scene");
                return;
            }
            if let Err(error) = self.scene_manager.commit_mutation(mutation).await {
                warn!(selector, scene_id = %scene_id, %error, "Failed to commit configured startup scene");
                return;
            }
        }

        if let Some(layout_id) = layout_id {
            match self
                .domains
                .layout
                .restore_startup_layout(layout_id.as_str())
                .await
            {
                Ok(Some(_)) => {}
                Ok(None) => {
                    warn!(%layout_id, "Configured startup scene layout was not found");
                }
                Err(error) => {
                    warn!(%layout_id, %error, "Rejected configured startup scene layout");
                }
            }
        }

        if let Some(brightness) = activation_brightness
            && let Err(error) = self
                .output_power
                .set_global_brightness(&self.event_bus, brightness)
                .await
        {
            warn!(%error, "Failed to persist configured startup scene brightness");
        }

        info!(scene_id = %scene_id, scene_name, "Activated configured startup scene");
    }

    async fn apply_runtime_session_snapshot(
        &self,
        snapshot: RuntimeSessionSnapshot,
    ) -> anyhow::Result<()> {
        let requested_active_scene_id = snapshot
            .active_scene_id
            .as_deref()
            .map(str::parse::<uuid::Uuid>)
            .transpose()
            .map(|scene_id| scene_id.map(SceneId))?;

        {
            let mut mutation = self.scene_manager.begin_mutation().await;
            if !snapshot.default_scene_zones.is_empty() {
                let Some(mut default_scene) = mutation.scenes().get(&SceneId::DEFAULT).cloned()
                else {
                    anyhow::bail!("default scene is missing during runtime restore");
                };
                default_scene
                    .zones
                    .clone_from(&snapshot.default_scene_zones);
                mutation.restore_scene(default_scene)?;
            }

            if let Some(scene_id) =
                requested_active_scene_id.filter(|scene_id| !scene_id.is_default())
            {
                if mutation.scenes().get(&scene_id).is_some() {
                    mutation.activate(scene_id, None, SceneChangeReason::DaemonStart)?;
                } else {
                    warn!(
                        scene_id = %scene_id,
                        "Persisted active scene was not found in the scene store"
                    );
                }
            }

            // Persisted zones carry a frozen layout snapshot that may pre-date
            // the active layout restored just above. Re-align the primary zone
            // so the render pipeline sees the current layout's zones.
            let active_layout = self.spatial_engine.snapshot().layout().as_ref().clone();
            mutation.sync_primary_layout(&active_layout);
            self.scene_manager.commit_mutation(mutation).await?;
        }
        if !snapshot.default_scene_zones.is_empty() || requested_active_scene_id.is_some() {
            info!(
                zones = snapshot.default_scene_zones.len(),
                active_scene_id = ?requested_active_scene_id.unwrap_or(SceneId::DEFAULT),
                "Restored runtime scene snapshot"
            );
        }

        Ok(())
    }

    async fn spawn_effect_watcher(&mut self) {
        let effects = self.domains.effects.clone();
        let search_paths = effects.search_paths().await;

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

                if let Err(error) = effects.reload_registry_file(&path).await {
                    warn!(path = %path.display(), %error, "Effect hot reload rejected");
                }
            }

            debug!("Effect watcher channel closed; task exiting");
        }));
    }

    /// Reconcile native display geometry and default-face overlays whenever a
    /// display connects.
    fn spawn_display_preference_sync_worker(&mut self) {
        let display = self.domains.display.clone();
        let mut event_rx = self.event_bus.subscribe_all();

        self.display_preference_sync_task = Some(tokio::spawn(async move {
            loop {
                let event = match event_rx.recv().await {
                    Ok(event) => event,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        warn!(skipped, "Display reconciliation worker lagged");
                        display.sync_connected_surfaces().await;
                        display.sync_preference_overlays().await;
                        continue;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                };
                if matches!(event.event, HypercolorEvent::DeviceConnected { .. }) {
                    display.sync_connected_surfaces().await;
                    display.sync_preference_overlays().await;
                }
            }
        }));
    }

    fn spawn_output_static_hold_worker(&mut self) {
        let output = self.domains.output.clone();
        let mut event_rx = self.event_bus.subscribe_all();

        self.output_static_hold_task = Some(tokio::spawn(async move {
            loop {
                match event_rx.recv().await {
                    Ok(event) if matches!(event.event, HypercolorEvent::DeviceConnected { .. }) => {
                        output.reconcile_static_hold().await;
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        warn!(skipped, "Static output hold worker lagged");
                        output.reconcile_static_hold().await;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        }));
    }

    fn spawn_effect_error_fallback_worker(&mut self) {
        let effects = self.domains.effects.clone();
        let config_manager = Arc::clone(&self.config_manager);
        let event_bus = Arc::clone(&self.event_bus);
        let performance = Arc::clone(&self.performance);
        let mut event_rx = self.event_bus.subscribe_all();

        self.effect_error_fallback_task = Some(tokio::spawn(async move {
            info!("Effect-error fallback worker active");

            loop {
                let event = match event_rx.recv().await {
                    Ok(event) => event,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        warn!(skipped, "Effect-error fallback worker lagged");
                        continue;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        debug!("Effect-error fallback worker channel closed; task exiting");
                        break;
                    }
                };

                let HypercolorEvent::EffectError {
                    effect_id,
                    error,
                    fallback,
                } = event.event
                else {
                    continue;
                };
                if fallback.is_some() {
                    continue;
                }

                {
                    let mut tracker = performance.write().await;
                    tracker.record_effect_error();
                }

                let policy = config_manager.get().effect_engine.effect_error_fallback;
                if matches!(policy, EffectErrorFallbackPolicy::None) {
                    continue;
                }

                match crate::domain::effect::apply_error_fallback(&effects, &effect_id, policy)
                    .await
                {
                    Ok(Some(applied)) => {
                        {
                            let mut tracker = performance.write().await;
                            tracker.record_effect_fallback_applied();
                        }
                        if let Some(fallback_label) = policy.event_label() {
                            event_bus.publish(HypercolorEvent::EffectError {
                                effect_id: effect_id.clone(),
                                error: error.clone(),
                                fallback: Some(fallback_label.to_owned()),
                            });
                        }
                        info!(
                            effect_id,
                            effect = %applied.effect.name,
                            cleared_zones = applied.cleared_zone_count,
                            fallback_policy = ?policy,
                            "Applied effect-error fallback"
                        );
                    }
                    Ok(None) => {
                        debug!(
                            effect_id,
                            fallback_policy = ?policy,
                            "Effect-error fallback found no active assignments to clear"
                        );
                    }
                    Err(fallback_error) => {
                        warn!(
                            effect_id,
                            fallback_policy = ?policy,
                            reason = %fallback_error,
                            "Failed to apply effect-error fallback"
                        );
                    }
                }
            }
        }));
    }

    #[allow(
        clippy::too_many_lines,
        reason = "startup wires the full discovery worker context in one place for readability"
    )]
    fn spawn_discovery_worker(&mut self, config: Arc<HypercolorConfig>) {
        let worker = DiscoveryWorkerContext {
            discovery: self.driver_host.discovery_runtime(),
            config_manager: Arc::clone(&self.config_manager),
            driver_host: Arc::clone(&self.driver_host),
            driver_registry: Arc::clone(&self.driver_registry),
        };

        let initial_targets =
            match discovery::resolve_targets(None, &config, self.driver_registry.as_ref()) {
                Ok(targets) => targets,
                Err(error) => {
                    warn!(error = %error, "Initial discovery target resolution failed");
                    Vec::<DiscoveryTarget>::new()
                }
            };
        let scan_interval =
            std::time::Duration::from_secs(config.discovery.scan_interval_secs.max(1));
        let driver_registry = Arc::clone(&self.driver_registry);

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
                    initial_targets,
                    "Skipping initial discovery scan; scan already in progress",
                )
                .await;
            worker.run_startup_driver_recovery_scans().await;

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
                                    let driver_id = descriptor.driver_id();
                                    if crate::network::module_enabled_by_id(
                                        driver_registry.as_ref(),
                                        &config,
                                        driver_id.as_ref(),
                                    ) {
                                        info!(
                                            vendor_id,
                                            product_id,
                                            device = descriptor.name,
                                            "USB hotplug arrival detected"
                                        );
                                        true
                                    } else {
                                        debug!(
                                            vendor_id,
                                            product_id,
                                            driver_id = %driver_id,
                                            device = descriptor.name,
                                            "Ignoring USB hotplug arrival for disabled HAL driver"
                                        );
                                        false
                                    }
                                }
                                Ok(UsbHotplugEvent::Removed { vendor_id, product_id }) => {
                                    info!(vendor_id, product_id, "USB hotplug removal detected");
                                    spawn_delayed_usb_hotplug_scan(worker.clone());
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

pub(crate) async fn persist_scene_store_snapshot(
    scene_manager: &crate::domain::scene::SceneService,
) -> Result<Option<AtomicWriteOutcome>> {
    scene_manager.persist_snapshot().await
}

fn spawn_delayed_usb_hotplug_scan(worker: DiscoveryWorkerContext) {
    std::mem::drop(tokio::spawn(async move {
        tokio::time::sleep(USB_HOTPLUG_REMOVAL_RECOVERY_SCAN_DELAY).await;
        debug!(
            delay_ms = USB_HOTPLUG_REMOVAL_RECOVERY_SCAN_DELAY.as_millis(),
            "running delayed USB hotplug recovery scan"
        );
        worker.run_usb_hotplug_scan().await;
    }));
}
