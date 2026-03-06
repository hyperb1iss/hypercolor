//! Daemon-side session power orchestration.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, watch};
use tokio::task::JoinHandle;
use tracing::{debug, warn};

use hypercolor_core::bus::HypercolorBus;
use hypercolor_core::config::ConfigManager;
use hypercolor_core::effect::EffectEngine;
use hypercolor_core::session::{SessionWatcher, SleepPolicy};
use hypercolor_types::event::HypercolorEvent;
use hypercolor_types::session::{SessionEvent, SleepAction, WakeAction};

use crate::discovery::{self, DiscoveryBackend, DiscoveryRuntime};

const FADE_STEP_MS: u64 = 16;

/// Session-driven output scaling consumed by the render thread.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OutputPowerState {
    pub brightness: f32,
    pub sleeping: bool,
}

impl Default for OutputPowerState {
    fn default() -> Self {
        Self {
            brightness: 1.0,
            sleeping: false,
        }
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
    effect_engine: Arc<Mutex<EffectEngine>>,
    power_tx: watch::Sender<OutputPowerState>,
    discovery_runtime: DiscoveryRuntime,
}

impl SessionController {
    /// Start watching session events and applying power policy.
    pub fn start(
        config_manager: Arc<ConfigManager>,
        event_bus: Arc<HypercolorBus>,
        effect_engine: Arc<Mutex<EffectEngine>>,
        power_tx: watch::Sender<OutputPowerState>,
        discovery_runtime: DiscoveryRuntime,
    ) -> Self {
        let session_config = config_manager.get().session.clone();
        let watcher = SessionWatcher::start(&session_config);
        let event_rx = watcher.subscribe();
        let runtime = SessionRuntime {
            config_manager,
            event_bus,
            effect_engine,
            power_tx,
            discovery_runtime,
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
            fade_to(&runtime.power_tx, brightness, fade_ms).await;
        })),
        SleepAction::Off { fade_ms } => Some(tokio::spawn(async move {
            ensure_awake(&runtime).await;
            fade_to(&runtime.power_tx, 0.0, fade_ms).await;
            {
                let mut engine = runtime.effect_engine.lock().await;
                engine.pause();
            }
            set_power_state(
                &runtime.power_tx,
                OutputPowerState {
                    brightness: 0.0,
                    sleeping: true,
                },
            );
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
            if current.sleeping {
                set_power_state(
                    &runtime.power_tx,
                    OutputPowerState {
                        brightness: current.brightness,
                        sleeping: false,
                    },
                );
            }

            if matches!(event, SessionEvent::Resumed) {
                run_usb_resume_scan(&runtime).await;
            }

            {
                let mut engine = runtime.effect_engine.lock().await;
                engine.resume();
            }
            fade_to(&runtime.power_tx, 1.0, fade_ms).await;
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

    {
        let mut engine = runtime.effect_engine.lock().await;
        engine.resume();
    }
    set_power_state(
        &runtime.power_tx,
        OutputPowerState {
            brightness: current.brightness,
            sleeping: false,
        },
    );
}

async fn run_usb_resume_scan(runtime: &SessionRuntime) {
    let config_guard = runtime.config_manager.get();
    let config = Arc::clone(&*config_guard);
    let result = discovery::execute_discovery_scan(
        runtime.discovery_runtime.clone(),
        config,
        vec![DiscoveryBackend::Usb],
        discovery::default_timeout(),
    )
    .await;

    debug!(
        found = result.new_devices.len() + result.reappeared_devices.len(),
        vanished = result.vanished_devices.len(),
        duration_ms = result.duration_ms,
        "USB resume recovery scan finished"
    );
}

async fn fade_to(power_tx: &watch::Sender<OutputPowerState>, target: f32, fade_ms: u64) {
    let target = target.clamp(0.0, 1.0);
    let current = current_power_state(power_tx);
    let start = current.brightness;

    if fade_ms == 0 || (start - target).abs() <= f32::EPSILON {
        set_power_state(
            power_tx,
            OutputPowerState {
                brightness: target,
                sleeping: current.sleeping,
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
                brightness,
                sleeping: false,
            },
        );
        tokio::time::sleep(step_delay).await;
    }

    set_power_state(
        power_tx,
        OutputPowerState {
            brightness: target,
            sleeping: false,
        },
    );
}

fn current_power_state(power_tx: &watch::Sender<OutputPowerState>) -> OutputPowerState {
    *power_tx.borrow()
}

fn set_power_state(power_tx: &watch::Sender<OutputPowerState>, state: OutputPowerState) {
    let _ = power_tx.send(state);
}
