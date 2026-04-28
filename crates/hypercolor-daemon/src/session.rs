//! Daemon-side session power orchestration.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use tokio::sync::watch;
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

const FADE_STEP_MS: u64 = 16;

/// Session-driven output scaling consumed by the render thread.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OutputPowerState {
    pub global_brightness: f32,
    pub session_brightness: f32,
    pub sleeping: bool,
    pub off_output_behavior: OffOutputBehavior,
    pub off_output_color: [u8; 3],
}

impl Default for OutputPowerState {
    fn default() -> Self {
        Self {
            global_brightness: 1.0,
            session_brightness: 1.0,
            sleeping: false,
            off_output_behavior: OffOutputBehavior::Static,
            off_output_color: [0, 0, 0],
        }
    }
}

impl OutputPowerState {
    #[must_use]
    pub fn effective_brightness(self) -> f32 {
        (self.global_brightness * self.session_brightness).clamp(0.0, 1.0)
    }
}

/// Owns the core session watcher and the daemon-side power policy task.
pub struct SessionController {
    watcher: SessionWatcher,
    task: JoinHandle<()>,
}

#[derive(Clone)]
struct SessionRuntime {
    config_manager: Arc<ConfigManager>,
    event_bus: Arc<HypercolorBus>,
    power_tx: watch::Sender<OutputPowerState>,
    discovery_runtime: DiscoveryRuntime,
    driver_host: Arc<DaemonDriverHost>,
    driver_registry: Arc<DriverModuleRegistry>,
}

impl SessionController {
    /// Start watching session events and applying power policy.
    pub fn start(
        config_manager: Arc<ConfigManager>,
        event_bus: Arc<HypercolorBus>,
        power_tx: watch::Sender<OutputPowerState>,
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
            power_tx,
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
        } => Some(tokio::spawn(async move {
            ensure_awake(&runtime).await;
            fade_session_to(&runtime.power_tx, brightness, fade_ms).await;
        })),
        SleepAction::Off {
            fade_ms,
            output_behavior,
            static_color,
        } => Some(tokio::spawn(async move {
            ensure_awake(&runtime).await;
            fade_session_to(&runtime.power_tx, 0.0, fade_ms).await;
            pause_output(&runtime, output_behavior, static_color).await;
        })),
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
        WakeAction::Restore { fade_ms } => Some(tokio::spawn(async move {
            let current = current_power_state(&runtime.power_tx);
            if current.sleeping && current.off_output_behavior == OffOutputBehavior::Release {
                run_full_reconnect_scan(&runtime).await;
            } else if matches!(event, SessionEvent::Resumed) {
                run_usb_resume_scan(&runtime).await;
            }

            if current.sleeping {
                set_power_state(
                    &runtime.power_tx,
                    OutputPowerState {
                        global_brightness: current.global_brightness,
                        session_brightness: current.session_brightness,
                        sleeping: false,
                        off_output_behavior: current.off_output_behavior,
                        off_output_color: current.off_output_color,
                    },
                );
            }
            fade_session_to(&runtime.power_tx, 1.0, fade_ms).await;
        })),
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

async fn ensure_awake(runtime: &SessionRuntime) {
    let current = current_power_state(&runtime.power_tx);
    if !current.sleeping {
        return;
    }

    if current.off_output_behavior == OffOutputBehavior::Release {
        run_full_reconnect_scan(runtime).await;
    }
    set_power_state(
        &runtime.power_tx,
        OutputPowerState {
            global_brightness: current.global_brightness,
            session_brightness: current.session_brightness,
            sleeping: false,
            off_output_behavior: current.off_output_behavior,
            off_output_color: current.off_output_color,
        },
    );
}

async fn run_usb_resume_scan(runtime: &SessionRuntime) {
    let config_guard = runtime.config_manager.get();
    let config = Arc::clone(&*config_guard);
    let Some(result) = discovery::execute_discovery_scan_if_idle(
        runtime.discovery_runtime.clone(),
        Arc::clone(&runtime.driver_registry),
        Arc::clone(&runtime.driver_host),
        config,
        vec![DiscoveryTarget::usb(), DiscoveryTarget::smbus()],
        discovery::default_timeout(),
    )
    .await
    else {
        debug!(
            in_progress = runtime
                .discovery_runtime
                .in_progress
                .load(Ordering::Acquire),
            "Skipping USB resume recovery scan because discovery is already running"
        );
        return;
    };

    debug!(
        found = result.new_devices.len() + result.reappeared_devices.len(),
        vanished = result.vanished_devices.len(),
        duration_ms = result.duration_ms,
        "USB resume recovery scan finished"
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

    let Some(result) = discovery::execute_discovery_scan_if_idle(
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
            "Skipping output reconnect scan because discovery is already running"
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
) {
    let current = current_power_state(&runtime.power_tx);
    set_power_state(
        &runtime.power_tx,
        OutputPowerState {
            global_brightness: current.global_brightness,
            session_brightness: 0.0,
            sleeping: true,
            off_output_behavior: output_behavior,
            off_output_color: static_color,
        },
    );

    if output_behavior == OffOutputBehavior::Release {
        let released = discovery::release_renderable_devices(&runtime.discovery_runtime).await;
        debug!(
            released,
            "Temporarily released renderable devices for session sleep"
        );
    }
}

async fn fade_session_to(power_tx: &watch::Sender<OutputPowerState>, target: f32, fade_ms: u64) {
    let target = target.clamp(0.0, 1.0);
    let current = current_power_state(power_tx);
    let start = current.session_brightness;

    if fade_ms == 0 || (start - target).abs() <= f32::EPSILON {
        set_power_state(
            power_tx,
            OutputPowerState {
                global_brightness: current.global_brightness,
                session_brightness: target,
                sleeping: current.sleeping,
                off_output_behavior: current.off_output_behavior,
                off_output_color: current.off_output_color,
            },
        );
        return;
    }

    let steps = u16::try_from((fade_ms / FADE_STEP_MS).max(1)).unwrap_or(u16::MAX);
    let step_delay = Duration::from_millis((fade_ms / u64::from(steps)).max(1));

    for step in 1..=steps {
        let progress = f32::from(step) / f32::from(steps);
        let brightness = start + (target - start) * progress;
        set_power_state(
            power_tx,
            OutputPowerState {
                global_brightness: current.global_brightness,
                session_brightness: brightness,
                sleeping: false,
                off_output_behavior: current.off_output_behavior,
                off_output_color: current.off_output_color,
            },
        );
        tokio::time::sleep(step_delay).await;
    }

    set_power_state(
        power_tx,
        OutputPowerState {
            global_brightness: current.global_brightness,
            session_brightness: target,
            sleeping: false,
            off_output_behavior: current.off_output_behavior,
            off_output_color: current.off_output_color,
        },
    );
}

pub fn set_global_brightness(power_tx: &watch::Sender<OutputPowerState>, brightness: f32) {
    let current = current_power_state(power_tx);
    set_power_state(
        power_tx,
        OutputPowerState {
            global_brightness: brightness.clamp(0.0, 1.0),
            session_brightness: current.session_brightness,
            sleeping: current.sleeping,
            off_output_behavior: current.off_output_behavior,
            off_output_color: current.off_output_color,
        },
    );
}

#[must_use]
pub fn current_global_brightness(power_tx: &watch::Sender<OutputPowerState>) -> f32 {
    current_power_state(power_tx).global_brightness
}

fn current_power_state(power_tx: &watch::Sender<OutputPowerState>) -> OutputPowerState {
    *power_tx.borrow()
}

fn set_power_state(power_tx: &watch::Sender<OutputPowerState>, state: OutputPowerState) {
    power_tx.send_replace(state);
}
