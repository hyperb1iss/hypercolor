//! Daemon-side session power orchestration.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use tokio::task::JoinHandle;
use tracing::{debug, warn};

use hypercolor_core::bus::HypercolorBus;
use hypercolor_core::config::ConfigManager;
use hypercolor_core::session::{SessionWatcher, SleepPolicy};
use hypercolor_network::DriverModuleRegistry;
use hypercolor_types::event::HypercolorEvent;
use hypercolor_types::session::{OffOutputBehavior, SessionEvent, SleepAction, WakeAction};

use crate::discovery::{self, DiscoveryRuntime, DiscoveryTarget};
use crate::network::DaemonDriverHost;
use crate::output_power::OutputPower;

/// Owns the core session watcher and the daemon-side power policy task.
pub struct SessionController {
    watcher: SessionWatcher,
    task: JoinHandle<()>,
}

#[derive(Clone)]
struct SessionRuntime {
    config_manager: Arc<ConfigManager>,
    event_bus: Arc<HypercolorBus>,
    output_power: OutputPower,
    discovery_runtime: DiscoveryRuntime,
    driver_host: Arc<DaemonDriverHost>,
    driver_registry: Arc<DriverModuleRegistry>,
}

impl SessionController {
    /// Start watching session events and applying power policy.
    pub fn start(
        config_manager: Arc<ConfigManager>,
        event_bus: Arc<HypercolorBus>,
        output_power: OutputPower,
        discovery_runtime: DiscoveryRuntime,
        driver_host: Arc<DaemonDriverHost>,
        driver_registry: Arc<DriverModuleRegistry>,
    ) -> Self {
        let session_config = config_manager.get().session.clone();
        let watcher = SessionWatcher::start(&session_config);
        let event_rx = watcher.subscribe();
        let runtime = SessionRuntime {
            config_manager,
            event_bus,
            output_power,
            discovery_runtime,
            driver_host,
            driver_registry,
        };
        let task = tokio::spawn(run_session_loop(event_rx, runtime));

        Self { watcher, task }
    }

    /// Stop the policy loop and shut down the underlying watcher.
    pub async fn shutdown(self) {
        self.task.abort();
        let _ = self.task.await;
        self.watcher.shutdown().await;
    }
}

async fn run_session_loop(
    mut rx: tokio::sync::broadcast::Receiver<SessionEvent>,
    runtime: SessionRuntime,
) {
    let mut transition_task: Option<JoinHandle<()>> = None;

    loop {
        match rx.recv().await {
            Ok(event) => {
                runtime
                    .event_bus
                    .publish(HypercolorEvent::SessionChanged(event.clone()));

                let config = runtime.config_manager.get().session.clone();
                if !config.enabled {
                    continue;
                }

                if let Some(handle) = transition_task.take() {
                    handle.abort();
                    let _ = handle.await;
                }

                let policy = SleepPolicy::new(config);
                if let Some(action) = policy.sleep_action(&event) {
                    transition_task = spawn_sleep_transition(runtime.clone(), action);
                } else if let Some(action) = policy.wake_action(&event) {
                    transition_task = spawn_wake_transition(runtime.clone(), action, event);
                }
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                warn!(skipped, "Session controller lagged behind session events");
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        }
    }

    if let Some(handle) = transition_task {
        handle.abort();
        let _ = handle.await;
    }
}

fn spawn_sleep_transition(runtime: SessionRuntime, action: SleepAction) -> Option<JoinHandle<()>> {
    match action {
        SleepAction::Ignore => None,
        SleepAction::Dim {
            brightness,
            fade_ms,
        } => {
            let generation = begin_session_transition(&runtime);
            Some(tokio::spawn(async move {
                if ensure_awake(&runtime, generation).await {
                    runtime
                        .output_power
                        .fade_session_to(brightness, fade_ms, generation)
                        .await;
                }
            }))
        }
        SleepAction::Off {
            fade_ms,
            output_behavior,
            static_color,
        } => {
            let generation = begin_session_transition(&runtime);
            Some(tokio::spawn(async move {
                if ensure_awake(&runtime, generation).await
                    && runtime
                        .output_power
                        .fade_session_to(0.0, fade_ms, generation)
                        .await
                {
                    pause_output(&runtime, output_behavior, static_color, generation).await;
                }
            }))
        }
        SleepAction::Scene {
            scene_name,
            fade_ms,
        } => {
            warn!(
                scene = %scene_name,
                fade_ms,
                "Session scene actions are not implemented yet; ignoring"
            );
            None
        }
    }
}

fn spawn_wake_transition(
    runtime: SessionRuntime,
    action: WakeAction,
    event: SessionEvent,
) -> Option<JoinHandle<()>> {
    match action {
        WakeAction::Restore { fade_ms } => {
            let generation = begin_session_transition(&runtime);
            Some(tokio::spawn(async move {
                let current = runtime.output_power.snapshot();
                if current.session_release_active() {
                    run_full_reconnect_scan(&runtime).await;
                } else if matches!(event, SessionEvent::Resumed) {
                    run_host_resume_scan(&runtime).await;
                }

                if clear_session_sleep(&runtime, generation).await {
                    runtime
                        .output_power
                        .fade_session_to(1.0, fade_ms, generation)
                        .await;
                }
            }))
        }
        WakeAction::Scene {
            scene_name,
            fade_ms,
        } => {
            warn!(
                scene = %scene_name,
                fade_ms,
                "Session wake scene actions are not implemented yet; ignoring"
            );
            None
        }
    }
}

fn begin_session_transition(runtime: &SessionRuntime) -> u64 {
    runtime.output_power.begin_session_transition()
}

async fn ensure_awake(runtime: &SessionRuntime, generation: u64) -> bool {
    let current = runtime.output_power.snapshot();
    if !current.session_sleeping {
        return current.transition_generation == generation;
    }

    if current.session_release_active() {
        run_full_reconnect_scan(runtime).await;
    }
    clear_session_sleep(runtime, generation).await
}

async fn clear_session_sleep(runtime: &SessionRuntime, generation: u64) -> bool {
    runtime
        .output_power
        .clear_session_sleep(&runtime.event_bus, generation)
        .await
}

async fn run_host_resume_scan(runtime: &SessionRuntime) {
    let config_guard = runtime.config_manager.get();
    let config = Arc::clone(&*config_guard);
    let Some(result) = discovery::execute_discovery_scan_or_enqueue(
        runtime.discovery_runtime.clone(),
        Arc::clone(&runtime.driver_registry),
        Arc::clone(&runtime.driver_host),
        config,
        DiscoveryTarget::session_resume_targets(),
        discovery::default_timeout(),
    )
    .await
    else {
        debug!(
            in_progress = runtime
                .discovery_runtime
                .in_progress
                .load(Ordering::Acquire),
            "Queued host resume recovery scan behind active discovery"
        );
        return;
    };

    debug!(
        found = result.new_devices.len() + result.reappeared_devices.len(),
        vanished = result.vanished_devices.len(),
        duration_ms = result.duration_ms,
        "Host resume recovery scan finished"
    );
}

async fn run_full_reconnect_scan(runtime: &SessionRuntime) {
    let config_guard = runtime.config_manager.get();
    let config = Arc::clone(&*config_guard);
    let targets = match discovery::resolve_targets(None, &config, &runtime.driver_registry) {
        Ok(targets) => targets,
        Err(error) => {
            warn!(%error, "Failed to resolve discovery targets for output reconnect scan");
            return;
        }
    };

    let Some(result) = discovery::execute_discovery_scan_or_enqueue(
        runtime.discovery_runtime.clone(),
        Arc::clone(&runtime.driver_registry),
        Arc::clone(&runtime.driver_host),
        config,
        targets,
        discovery::default_timeout(),
    )
    .await
    else {
        debug!(
            in_progress = runtime
                .discovery_runtime
                .in_progress
                .load(Ordering::Acquire),
            "Queued output reconnect scan behind active discovery"
        );
        return;
    };

    debug!(
        found = result.new_devices.len() + result.reappeared_devices.len(),
        vanished = result.vanished_devices.len(),
        duration_ms = result.duration_ms,
        "Output reconnect scan finished"
    );
}

async fn pause_output(
    runtime: &SessionRuntime,
    output_behavior: OffOutputBehavior,
    static_color: [u8; 3],
    generation: u64,
) -> bool {
    let applied = runtime
        .output_power
        .pause_for_session(
            &runtime.event_bus,
            generation,
            output_behavior,
            static_color,
        )
        .await;

    if applied && runtime.output_power.snapshot().session_release_active() {
        let released = discovery::release_renderable_devices(&runtime.discovery_runtime).await;
        debug!(
            released,
            "Temporarily released renderable devices for session sleep"
        );
    }
    applied
}
