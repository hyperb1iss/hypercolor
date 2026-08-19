//! Daemon-side session power orchestration.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::{debug, warn};

use hypercolor_core::bus::HypercolorBus;
use hypercolor_core::config::ConfigManager;
use hypercolor_core::session::{SessionMonitor, SessionWatcher, SleepPolicy};
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
    pub output_override: OutputOverride,
    pub session_sleeping: bool,
    pub transition_generation: u64,
    pub off_output_behavior: OffOutputBehavior,
    pub off_output_color: [u8; 3],
    pub manual_off_output_color: [u8; 3],
}

/// Explicit output ownership state, independent from transient OS sleep.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OutputOverride {
    /// No explicit user output override.
    #[default]
    None,
    /// Preserve state and hold static output.
    Paused,
    /// Destructively stopped output may release device ownership.
    Stopped,
}

/// Effective output transition produced by a state update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputPowerTransition {
    /// Effective output power did not change.
    Unchanged,
    /// Effective output changed from running to paused.
    Paused,
    /// Effective output changed from paused to running.
    Resumed,
}

impl Default for OutputPowerState {
    fn default() -> Self {
        Self {
            global_brightness: 1.0,
            session_brightness: 1.0,
            output_override: OutputOverride::None,
            session_sleeping: false,
            transition_generation: 0,
            off_output_behavior: OffOutputBehavior::Static,
            off_output_color: [0, 0, 0],
            manual_off_output_color: [0, 0, 0],
        }
    }
}

impl OutputPowerState {
    #[must_use]
    pub fn sleeping(self) -> bool {
        self.output_override != OutputOverride::None || self.session_sleeping
    }

    #[must_use]
    pub fn manually_paused(self) -> bool {
        self.output_override == OutputOverride::Paused
    }

    /// Whether output reads as paused on every observing surface.
    ///
    /// The exact complement of running: a latched pause, a destructive
    /// stop, and a session sleep all leave outputs dark, so all three
    /// answer `true` (Spec 78 §7.1). A stop still publishes no `Paused`
    /// event — `EffectStopped` already announces that gesture, and
    /// minting a pause for it would make the two indistinguishable on
    /// the event stream — so clients converge on the next snapshot read
    /// rather than on a delta.
    #[must_use]
    pub fn reported_paused(self) -> bool {
        self.sleeping()
    }

    #[must_use]
    pub fn effective_off_output_behavior(self) -> OffOutputBehavior {
        match self.output_override {
            OutputOverride::Paused => OffOutputBehavior::Static,
            OutputOverride::Stopped => OffOutputBehavior::Release,
            OutputOverride::None => self.off_output_behavior,
        }
    }

    #[must_use]
    pub(crate) fn session_release_active(self) -> bool {
        self.session_sleeping
            && self.output_override == OutputOverride::None
            && self.off_output_behavior == OffOutputBehavior::Release
    }

    #[must_use]
    pub fn effective_off_output_color(self) -> [u8; 3] {
        match self.output_override {
            OutputOverride::Paused => self.manual_off_output_color,
            OutputOverride::Stopped => [0, 0, 0],
            OutputOverride::None => self.off_output_color,
        }
    }

    #[must_use]
    pub fn effective_brightness(self) -> f32 {
        if self.sleeping() {
            0.0
        } else {
            (self.global_brightness * self.session_brightness).clamp(0.0, 1.0)
        }
    }
}

fn power_transition(
    previous: OutputPowerState,
    current: OutputPowerState,
) -> OutputPowerTransition {
    if current.output_override == OutputOverride::Stopped {
        return OutputPowerTransition::Unchanged;
    }
    match (previous.sleeping(), current.sleeping()) {
        (false, true) => OutputPowerTransition::Paused,
        (true, false) => OutputPowerTransition::Resumed,
        _ => OutputPowerTransition::Unchanged,
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
    output_power_transition: Arc<tokio::sync::Mutex<()>>,
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
        output_power_transition: Arc<tokio::sync::Mutex<()>>,
        discovery_runtime: DiscoveryRuntime,
        driver_host: Arc<DaemonDriverHost>,
        driver_registry: Arc<DriverModuleRegistry>,
        monitors: Vec<Box<dyn SessionMonitor>>,
    ) -> Self {
        let session_config = config_manager.get().session.clone();
        let watcher = SessionWatcher::start(&session_config, monitors);
        let event_rx = watcher.subscribe();
        let runtime = SessionRuntime {
            config_manager,
            event_bus,
            power_tx,
            output_power_transition,
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

pub(crate) fn platform_session_monitors(
    config: &hypercolor_types::session::SessionConfig,
) -> Vec<Box<dyn SessionMonitor>> {
    #[cfg(target_os = "linux")]
    {
        return hypercolor_linux_session::monitors(config);
    }

    #[cfg(target_os = "windows")]
    {
        let _ = config;
        return hypercolor_windows_session::standalone_monitors();
    }

    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        let _ = config;
        Vec::new()
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
                    fade_session_to(&runtime.power_tx, brightness, fade_ms, generation).await;
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
                    && fade_session_to(&runtime.power_tx, 0.0, fade_ms, generation).await
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
                let current = current_power_state(&runtime.power_tx);
                if current.session_release_active() {
                    run_full_reconnect_scan(&runtime).await;
                } else if matches!(event, SessionEvent::Resumed) {
                    run_host_resume_scan(&runtime).await;
                }

                if clear_session_sleep(&runtime, generation).await {
                    fade_session_to(&runtime.power_tx, 1.0, fade_ms, generation).await;
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
    advance_transition_generation(&runtime.power_tx)
}

async fn ensure_awake(runtime: &SessionRuntime, generation: u64) -> bool {
    let current = current_power_state(&runtime.power_tx);
    if !current.session_sleeping {
        return current.transition_generation == generation;
    }

    if current.session_release_active() {
        run_full_reconnect_scan(runtime).await;
    }
    clear_session_sleep(runtime, generation).await
}

async fn clear_session_sleep(runtime: &SessionRuntime, generation: u64) -> bool {
    let _transition_guard = runtime.output_power_transition.lock().await;
    update_power_state_with_events_for_generation(
        &runtime.power_tx,
        &runtime.event_bus,
        generation,
        |state| {
            state.session_sleeping = false;
        },
    )
}

async fn run_host_resume_scan(runtime: &SessionRuntime) {
    let config_guard = runtime.config_manager.get();
    let config = Arc::clone(&*config_guard);
    let targets = match DiscoveryTarget::session_resume_targets(
        config.as_ref(),
        runtime.driver_registry.as_ref(),
    ) {
        Ok(targets) => targets,
        Err(error) => {
            warn!(%error, "Failed to resolve host resume discovery targets");
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
    let _transition_guard = runtime.output_power_transition.lock().await;
    let applied = update_power_state_with_events_for_generation(
        &runtime.power_tx,
        &runtime.event_bus,
        generation,
        |state| {
            state.session_brightness = 0.0;
            state.session_sleeping = true;
            state.off_output_behavior = output_behavior;
            state.off_output_color = static_color;
        },
    );

    if applied && current_power_state(&runtime.power_tx).session_release_active() {
        let released = discovery::release_renderable_devices(&runtime.discovery_runtime).await;
        debug!(
            released,
            "Temporarily released renderable devices for session sleep"
        );
    }
    applied
}

async fn fade_session_to(
    power_tx: &watch::Sender<OutputPowerState>,
    target: f32,
    fade_ms: u64,
    generation: u64,
) -> bool {
    let target = target.clamp(0.0, 1.0);
    let current = current_power_state(power_tx);
    let start = current.session_brightness;

    if fade_ms == 0 || (start - target).abs() <= f32::EPSILON {
        return update_power_state_for_generation(power_tx, generation, |state| {
            state.session_brightness = target;
        });
    }

    let steps = u16::try_from((fade_ms / FADE_STEP_MS).max(1)).unwrap_or(u16::MAX);
    let step_delay = Duration::from_millis((fade_ms / u64::from(steps)).max(1));

    for step in 1..=steps {
        let progress = f32::from(step) / f32::from(steps);
        let brightness = start + (target - start) * progress;
        if !update_power_state_for_generation(power_tx, generation, |state| {
            state.session_brightness = brightness;
        }) {
            return false;
        }
        tokio::time::sleep(step_delay).await;
    }

    update_power_state_for_generation(power_tx, generation, |state| {
        state.session_brightness = target;
    })
}

pub fn set_global_brightness(power_tx: &watch::Sender<OutputPowerState>, brightness: f32) {
    update_power_state(power_tx, |state| {
        state.global_brightness = brightness.clamp(0.0, 1.0);
    });
}

/// Set or clear the persistent user pause latch.
pub(crate) fn set_manual_pause(
    power_tx: &watch::Sender<OutputPowerState>,
    event_bus: &HypercolorBus,
    paused: bool,
    static_color: [u8; 3],
) -> OutputPowerTransition {
    update_power_state_with_events(power_tx, event_bus, |state| {
        state.transition_generation = state.transition_generation.wrapping_add(1);
        state.output_override = if paused {
            OutputOverride::Paused
        } else {
            OutputOverride::None
        };
        state.manual_off_output_color = static_color;
        if !paused {
            state.session_sleeping = false;
            state.session_brightness = 1.0;
        }
    })
}

/// Restore the persistent pause latch without publishing a live transition.
pub(crate) fn restore_manual_pause(
    power_tx: &watch::Sender<OutputPowerState>,
    static_color: [u8; 3],
) {
    update_power_state(power_tx, |state| {
        state.output_override = OutputOverride::Paused;
        state.manual_off_output_color = static_color;
    });
}

/// Mark output as explicitly stopped without conflating it with OS sleep.
pub(crate) fn set_output_stopped(
    power_tx: &watch::Sender<OutputPowerState>,
    event_bus: &HypercolorBus,
) -> OutputPowerTransition {
    update_power_state_with_events(power_tx, event_bus, |state| {
        state.transition_generation = state.transition_generation.wrapping_add(1);
        state.output_override = OutputOverride::Stopped;
    })
}

/// Clear explicit output state and transient session sleep.
pub(crate) fn clear_output_override(
    power_tx: &watch::Sender<OutputPowerState>,
    event_bus: &HypercolorBus,
) -> OutputPowerTransition {
    update_power_state_with_events(power_tx, event_bus, |state| {
        state.transition_generation = state.transition_generation.wrapping_add(1);
        state.output_override = OutputOverride::None;
        state.session_sleeping = false;
        state.session_brightness = 1.0;
    })
}

#[must_use]
pub fn current_global_brightness(power_tx: &watch::Sender<OutputPowerState>) -> f32 {
    current_power_state(power_tx).global_brightness
}

fn current_power_state(power_tx: &watch::Sender<OutputPowerState>) -> OutputPowerState {
    *power_tx.borrow()
}

fn advance_transition_generation(power_tx: &watch::Sender<OutputPowerState>) -> u64 {
    let mut generation = 0;
    power_tx.send_modify(|state| {
        state.transition_generation = state.transition_generation.wrapping_add(1);
        generation = state.transition_generation;
    });
    generation
}

fn update_power_state_for_generation(
    power_tx: &watch::Sender<OutputPowerState>,
    generation: u64,
    update: impl FnOnce(&mut OutputPowerState),
) -> bool {
    let mut applied = false;
    power_tx.send_modify(|state| {
        if state.transition_generation == generation {
            update(state);
            applied = true;
        }
    });
    applied
}

fn update_power_state_with_events_for_generation(
    power_tx: &watch::Sender<OutputPowerState>,
    event_bus: &HypercolorBus,
    generation: u64,
    update: impl FnOnce(&mut OutputPowerState),
) -> bool {
    let mut applied = false;
    power_tx.send_modify(|state| {
        if state.transition_generation == generation {
            let previous = *state;
            update(state);
            publish_power_transition(event_bus, power_transition(previous, *state));
            applied = true;
        }
    });
    applied
}

fn update_power_state(
    power_tx: &watch::Sender<OutputPowerState>,
    update: impl FnOnce(&mut OutputPowerState),
) -> OutputPowerTransition {
    update_power_state_with_observer(power_tx, update, |_| {})
}

fn update_power_state_with_events(
    power_tx: &watch::Sender<OutputPowerState>,
    event_bus: &HypercolorBus,
    update: impl FnOnce(&mut OutputPowerState),
) -> OutputPowerTransition {
    update_power_state_with_observer(power_tx, update, |transition| {
        publish_power_transition(event_bus, transition);
    })
}

fn update_power_state_with_observer(
    power_tx: &watch::Sender<OutputPowerState>,
    update: impl FnOnce(&mut OutputPowerState),
    observe: impl FnOnce(OutputPowerTransition),
) -> OutputPowerTransition {
    let mut transition = OutputPowerTransition::Unchanged;
    power_tx.send_modify(|state| {
        let previous = *state;
        update(state);
        transition = power_transition(previous, *state);
        observe(transition);
    });
    transition
}

fn publish_power_transition(event_bus: &HypercolorBus, transition: OutputPowerTransition) {
    match transition {
        OutputPowerTransition::Paused => event_bus.publish(HypercolorEvent::Paused),
        OutputPowerTransition::Resumed => event_bus.publish(HypercolorEvent::Resumed),
        OutputPowerTransition::Unchanged => {}
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::mpsc;

    use hypercolor_core::bus::HypercolorBus;
    use hypercolor_types::event::HypercolorEvent;
    use tokio::sync::watch;

    use super::{
        OutputPowerState, OutputPowerTransition, advance_transition_generation,
        clear_output_override, fade_session_to, set_manual_pause, set_output_stopped,
        update_power_state, update_power_state_with_observer,
    };

    #[test]
    fn session_wake_does_not_clear_manual_pause() {
        let (power_tx, _) = watch::channel(OutputPowerState::default());
        let event_bus = HypercolorBus::new();
        assert_eq!(
            set_manual_pause(&power_tx, &event_bus, true, [0, 0, 0]),
            OutputPowerTransition::Paused
        );
        update_power_state(&power_tx, |state| state.session_sleeping = true);

        let transition = update_power_state(&power_tx, |state| {
            state.session_sleeping = false;
            state.session_brightness = 1.0;
        });

        assert_eq!(transition, OutputPowerTransition::Unchanged);
        assert!(power_tx.borrow().manually_paused());
        assert!(power_tx.borrow().sleeping());
    }

    #[test]
    fn manual_pause_prevents_session_release_policy_from_releasing_devices() {
        let (power_tx, _) = watch::channel(OutputPowerState::default());
        let event_bus = HypercolorBus::new();
        update_power_state(&power_tx, |state| {
            state.session_sleeping = true;
            state.off_output_behavior = hypercolor_types::session::OffOutputBehavior::Release;
        });
        assert!(power_tx.borrow().session_release_active());

        set_manual_pause(&power_tx, &event_bus, true, [0, 0, 0]);

        assert!(!power_tx.borrow().session_release_active());
        assert_eq!(
            power_tx.borrow().effective_off_output_behavior(),
            hypercolor_types::session::OffOutputBehavior::Static
        );
    }

    #[test]
    fn explicit_running_clears_session_sleep_and_manual_pause() {
        let (power_tx, _) = watch::channel(OutputPowerState::default());
        let event_bus = HypercolorBus::new();
        set_manual_pause(&power_tx, &event_bus, true, [0, 0, 0]);
        update_power_state(&power_tx, |state| state.session_sleeping = true);

        let transition = set_manual_pause(&power_tx, &event_bus, false, [0, 0, 0]);

        assert_eq!(transition, OutputPowerTransition::Resumed);
        assert!(!power_tx.borrow().sleeping());
        assert_eq!(power_tx.borrow().session_brightness, 1.0);
    }

    /// A stop leaves output dark, so every surface reads it as paused —
    /// but it publishes no `Paused` event, because `EffectStopped`
    /// already announces that gesture and minting a pause for it would
    /// make the two indistinguishable on the stream (Spec 78 §7.1).
    #[test]
    fn destructive_stop_reads_as_paused_without_publishing_a_pause_event() {
        let (power_tx, _) = watch::channel(OutputPowerState::default());
        let event_bus = HypercolorBus::new();
        let mut events = event_bus.subscribe_all();

        assert_eq!(
            set_output_stopped(&power_tx, &event_bus),
            OutputPowerTransition::Unchanged
        );
        assert_eq!(
            power_tx.borrow().output_override,
            super::OutputOverride::Stopped
        );
        assert!(power_tx.borrow().reported_paused());
        assert!(matches!(
            events.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
    }

    #[tokio::test]
    async fn explicit_resume_preempts_an_in_flight_session_fade() {
        let (power_tx, _) = watch::channel(OutputPowerState::default());
        let event_bus = HypercolorBus::new();
        let generation = advance_transition_generation(&power_tx);
        let mut updates = power_tx.subscribe();
        let fade_power_tx = power_tx.clone();
        let fade =
            tokio::spawn(async move { fade_session_to(&fade_power_tx, 0.0, 96, generation).await });

        loop {
            updates
                .changed()
                .await
                .expect("fade sender should remain available");
            if updates.borrow().session_brightness < 1.0 {
                break;
            }
        }

        clear_output_override(&power_tx, &event_bus);

        assert!(!fade.await.expect("fade task should join"));
        assert_eq!(power_tx.borrow().session_brightness, 1.0);
        assert!(!power_tx.borrow().sleeping());
    }

    #[test]
    fn concurrent_power_writers_publish_events_in_state_order() {
        let (power_tx, _) = watch::channel(OutputPowerState::default());
        let power_tx = Arc::new(power_tx);
        let event_bus = Arc::new(HypercolorBus::new());
        let mut events = event_bus.subscribe_all();
        let (pause_boundary_tx, pause_boundary_rx) = mpsc::channel();
        let (release_pause_tx, release_pause_rx) = mpsc::channel();

        let pause_power_tx = Arc::clone(&power_tx);
        let pause_event_bus = Arc::clone(&event_bus);
        let pause = std::thread::spawn(move || {
            update_power_state_with_observer(
                pause_power_tx.as_ref(),
                |state| state.output_override = super::OutputOverride::Paused,
                |transition| {
                    pause_boundary_tx
                        .send(())
                        .expect("pause boundary receiver should remain open");
                    release_pause_rx
                        .recv()
                        .expect("pause release sender should remain open");
                    super::publish_power_transition(&pause_event_bus, transition);
                },
            );
        });
        pause_boundary_rx
            .recv()
            .expect("pause writer should reach the publication boundary");

        let resume_power_tx = Arc::clone(&power_tx);
        let resume_event_bus = Arc::clone(&event_bus);
        let (resume_started_tx, resume_started_rx) = mpsc::channel();
        let (resume_boundary_tx, resume_boundary_rx) = mpsc::channel();
        let resume = std::thread::spawn(move || {
            resume_started_tx
                .send(())
                .expect("resume start receiver should remain open");
            update_power_state_with_observer(
                resume_power_tx.as_ref(),
                |state| {
                    resume_boundary_tx
                        .send(())
                        .expect("resume boundary receiver should remain open");
                    state.output_override = super::OutputOverride::None;
                },
                |transition| {
                    super::publish_power_transition(&resume_event_bus, transition);
                },
            );
        });
        resume_started_rx
            .recv()
            .expect("resume writer should start before pause is released");
        assert!(matches!(
            events.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
        assert!(matches!(
            resume_boundary_rx.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));

        release_pause_tx
            .send(())
            .expect("pause writer should remain blocked at its boundary");
        pause.join().expect("pause writer should not panic");
        resume.join().expect("resume writer should not panic");
        resume_boundary_rx
            .recv()
            .expect("resume writer should cross its boundary after pause publishes");

        assert!(matches!(
            events
                .try_recv()
                .expect("pause event should be published first")
                .event,
            HypercolorEvent::Paused
        ));
        assert!(matches!(
            events
                .try_recv()
                .expect("resume event should be published second")
                .event,
            HypercolorEvent::Resumed
        ));
        assert!(!power_tx.borrow().sleeping());
    }
}
