//! Canonical global output power and brightness authority.

use std::sync::Arc;
use std::time::Duration;

use hypercolor_core::bus::HypercolorBus;
use hypercolor_types::event::HypercolorEvent;
use hypercolor_types::session::OffOutputBehavior;
use tokio::sync::{Mutex, MutexGuard, RwLock, watch};

use crate::device_settings::DeviceSettingsStore;

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
    /// A latched pause, a destructive stop, and a session sleep all leave
    /// output dark. A stop still publishes no `Paused` event because
    /// `EffectStopped` already announces that gesture.
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

#[derive(Clone)]
pub struct OutputPower {
    inner: Arc<OutputPowerInner>,
}

struct OutputPowerInner {
    state: watch::Sender<OutputPowerState>,
    transition: Mutex<()>,
    settings: Arc<RwLock<DeviceSettingsStore>>,
}

pub(crate) struct OutputPowerGuard<'a> {
    power: &'a OutputPower,
    _transition: MutexGuard<'a, ()>,
}

impl OutputPower {
    #[must_use]
    pub fn new(settings: DeviceSettingsStore) -> Self {
        let global_brightness = settings.global_brightness();
        let (state, _) = watch::channel(OutputPowerState {
            global_brightness,
            ..OutputPowerState::default()
        });
        Self {
            inner: Arc::new(OutputPowerInner {
                state,
                transition: Mutex::new(()),
                settings: Arc::new(RwLock::new(settings)),
            }),
        }
    }

    #[must_use]
    pub fn snapshot(&self) -> OutputPowerState {
        *self.inner.state.borrow()
    }

    #[must_use]
    pub fn subscribe(&self) -> watch::Receiver<OutputPowerState> {
        self.inner.state.subscribe()
    }

    #[must_use]
    pub fn global_brightness(&self) -> f32 {
        self.snapshot().global_brightness
    }

    #[must_use]
    pub fn device_settings(&self) -> Arc<RwLock<DeviceSettingsStore>> {
        Arc::clone(&self.inner.settings)
    }

    /// Persist global brightness before publishing it to live consumers.
    ///
    /// A failed save restores the in-memory store and leaves the watch lane
    /// unchanged, so the persisted store remains the sole authority.
    pub async fn set_global_brightness(&self, brightness: f32) -> anyhow::Result<f32> {
        let brightness = brightness.clamp(0.0, 1.0);
        let _transition = self.inner.transition.lock().await;
        let previous_live = self.global_brightness();
        let mut settings = self.inner.settings.write().await;
        let previous_persisted = settings.global_brightness();
        settings.set_global_brightness(brightness);
        if let Err(error) = settings.save() {
            settings.set_global_brightness(previous_persisted);
            return Err(error);
        }
        drop(settings);
        self.update_state(|state| state.global_brightness = brightness);
        Ok(previous_live)
    }

    pub(crate) async fn transition(&self) -> OutputPowerGuard<'_> {
        OutputPowerGuard {
            power: self,
            _transition: self.inner.transition.lock().await,
        }
    }

    /// Set or clear the persistent user pause latch.
    pub async fn set_manual_pause(
        &self,
        event_bus: &HypercolorBus,
        paused: bool,
        static_color: [u8; 3],
    ) -> OutputPowerTransition {
        self.transition()
            .await
            .set_manual_pause(event_bus, paused, static_color)
    }

    /// Restore the persistent pause latch without publishing a live event.
    pub async fn restore_manual_pause(&self, static_color: [u8; 3]) {
        self.transition().await.restore_manual_pause(static_color);
    }

    /// Mark output as explicitly stopped without conflating it with OS sleep.
    pub async fn set_output_stopped(&self, event_bus: &HypercolorBus) -> OutputPowerTransition {
        self.transition().await.set_output_stopped(event_bus)
    }

    /// Clear explicit output state and transient session sleep.
    pub async fn clear_output_override(&self, event_bus: &HypercolorBus) -> OutputPowerTransition {
        self.transition().await.clear_output_override(event_bus)
    }

    #[must_use]
    pub fn begin_session_transition(&self) -> u64 {
        let mut generation = 0;
        self.inner.state.send_modify(|state| {
            state.transition_generation = state.transition_generation.wrapping_add(1);
            generation = state.transition_generation;
        });
        generation
    }

    pub async fn clear_session_sleep(&self, event_bus: &HypercolorBus, generation: u64) -> bool {
        self.transition()
            .await
            .update_with_events_for_generation(event_bus, generation, |state| {
                state.session_sleeping = false;
            })
    }

    pub async fn pause_for_session(
        &self,
        event_bus: &HypercolorBus,
        generation: u64,
        output_behavior: OffOutputBehavior,
        static_color: [u8; 3],
    ) -> bool {
        self.transition()
            .await
            .update_with_events_for_generation(event_bus, generation, |state| {
                state.session_brightness = 0.0;
                state.session_sleeping = true;
                state.off_output_behavior = output_behavior;
                state.off_output_color = static_color;
            })
    }

    pub async fn fade_session_to(&self, target: f32, fade_ms: u64, generation: u64) -> bool {
        let target = target.clamp(0.0, 1.0);
        let start = self.snapshot().session_brightness;

        if fade_ms == 0 || (start - target).abs() <= f32::EPSILON {
            return self.update_for_generation(generation, |state| {
                state.session_brightness = target;
            });
        }

        let steps = u16::try_from((fade_ms / FADE_STEP_MS).max(1)).unwrap_or(u16::MAX);
        let step_delay = Duration::from_millis((fade_ms / u64::from(steps)).max(1));

        for step in 1..=steps {
            let progress = f32::from(step) / f32::from(steps);
            let brightness = start + (target - start) * progress;
            if !self.update_for_generation(generation, |state| {
                state.session_brightness = brightness;
            }) {
                return false;
            }
            tokio::time::sleep(step_delay).await;
        }

        self.update_for_generation(generation, |state| {
            state.session_brightness = target;
        })
    }

    pub(crate) fn update_for_generation(
        &self,
        generation: u64,
        update: impl FnOnce(&mut OutputPowerState),
    ) -> bool {
        let mut applied = false;
        self.inner.state.send_modify(|state| {
            if state.transition_generation == generation {
                update(state);
                applied = true;
            }
        });
        applied
    }

    fn update_state(&self, update: impl FnOnce(&mut OutputPowerState)) -> OutputPowerTransition {
        self.update_state_with_observer(update, |_| {})
    }

    fn update_state_with_observer(
        &self,
        update: impl FnOnce(&mut OutputPowerState),
        observe: impl FnOnce(OutputPowerTransition),
    ) -> OutputPowerTransition {
        let mut transition = OutputPowerTransition::Unchanged;
        self.inner.state.send_modify(|state| {
            let previous = *state;
            update(state);
            transition = power_transition(previous, *state);
            observe(transition);
        });
        transition
    }
}

impl OutputPowerGuard<'_> {
    #[must_use]
    pub(crate) fn snapshot(&self) -> OutputPowerState {
        self.power.snapshot()
    }

    pub(crate) fn set_manual_pause(
        &self,
        event_bus: &HypercolorBus,
        paused: bool,
        static_color: [u8; 3],
    ) -> OutputPowerTransition {
        self.power.update_state_with_events(event_bus, |state| {
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

    pub(crate) fn restore_manual_pause(&self, static_color: [u8; 3]) {
        self.power.update_state(|state| {
            state.output_override = OutputOverride::Paused;
            state.manual_off_output_color = static_color;
        });
    }

    pub(crate) fn set_output_stopped(&self, event_bus: &HypercolorBus) -> OutputPowerTransition {
        self.power.update_state_with_events(event_bus, |state| {
            state.transition_generation = state.transition_generation.wrapping_add(1);
            state.output_override = OutputOverride::Stopped;
        })
    }

    pub(crate) fn clear_output_override(&self, event_bus: &HypercolorBus) -> OutputPowerTransition {
        self.power.update_state_with_events(event_bus, |state| {
            state.transition_generation = state.transition_generation.wrapping_add(1);
            state.output_override = OutputOverride::None;
            state.session_sleeping = false;
            state.session_brightness = 1.0;
        })
    }

    pub(crate) fn update_with_events_for_generation(
        &self,
        event_bus: &HypercolorBus,
        generation: u64,
        update: impl FnOnce(&mut OutputPowerState),
    ) -> bool {
        let mut applied = false;
        self.power.inner.state.send_modify(|state| {
            if state.transition_generation == generation {
                let previous = *state;
                update(state);
                publish_power_transition(event_bus, power_transition(previous, *state));
                applied = true;
            }
        });
        applied
    }
}

impl OutputPower {
    fn update_state_with_events(
        &self,
        event_bus: &HypercolorBus,
        update: impl FnOnce(&mut OutputPowerState),
    ) -> OutputPowerTransition {
        self.update_state_with_observer(update, |transition| {
            publish_power_transition(event_bus, transition);
        })
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

    use super::{OutputOverride, OutputPower, OutputPowerTransition};
    use crate::device_settings::DeviceSettingsStore;

    fn output_power() -> OutputPower {
        OutputPower::new(DeviceSettingsStore::new(
            std::env::temp_dir().join("hypercolor-output-power-tests.json"),
        ))
    }

    #[tokio::test]
    async fn explicit_running_clears_session_sleep_and_manual_pause() {
        let power = output_power();
        let event_bus = HypercolorBus::new();
        assert_eq!(
            power.set_manual_pause(&event_bus, true, [0, 0, 0]).await,
            OutputPowerTransition::Paused
        );
        let generation = power.begin_session_transition();
        power.update_for_generation(generation, |state| state.session_sleeping = true);

        let guard = power.transition().await;
        let transition = guard.set_manual_pause(&event_bus, false, [0, 0, 0]);

        assert_eq!(transition, OutputPowerTransition::Resumed);
        assert!(!power.snapshot().sleeping());
        assert_eq!(power.snapshot().session_brightness, 1.0);
    }

    #[tokio::test]
    async fn destructive_stop_reads_as_paused_without_publishing_a_pause_event() {
        let power = output_power();
        let event_bus = HypercolorBus::new();
        let mut events = event_bus.subscribe_all();

        assert_eq!(
            power.set_output_stopped(&event_bus).await,
            OutputPowerTransition::Unchanged
        );
        assert_eq!(power.snapshot().output_override, OutputOverride::Stopped);
        assert!(power.snapshot().reported_paused());
        assert!(matches!(
            events.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
    }

    #[tokio::test]
    async fn concurrent_power_writers_publish_events_in_state_order() {
        let power = Arc::new(output_power());
        let event_bus = Arc::new(HypercolorBus::new());
        let mut events = event_bus.subscribe_all();
        let (pause_boundary_tx, pause_boundary_rx) = mpsc::channel();
        let (release_pause_tx, release_pause_rx) = mpsc::channel();

        let pause_power = Arc::clone(&power);
        let pause_event_bus = Arc::clone(&event_bus);
        let pause = std::thread::spawn(move || {
            pause_power.update_state_with_observer(
                |state| state.output_override = OutputOverride::Paused,
                |transition| {
                    pause_boundary_tx
                        .send(())
                        .expect("pause boundary receiver should remain open");
                    release_pause_rx
                        .recv()
                        .expect("pause release sender should remain open");
                    publish_power_transition(&pause_event_bus, transition);
                },
            );
        });
        pause_boundary_rx
            .recv()
            .expect("pause writer should reach the publication boundary");

        let resume_power = Arc::clone(&power);
        let resume_event_bus = Arc::clone(&event_bus);
        let (resume_started_tx, resume_started_rx) = mpsc::channel();
        let (resume_boundary_tx, resume_boundary_rx) = mpsc::channel();
        let resume = std::thread::spawn(move || {
            resume_started_tx
                .send(())
                .expect("resume start receiver should remain open");
            resume_power.update_state_with_observer(
                |state| {
                    resume_boundary_tx
                        .send(())
                        .expect("resume boundary receiver should remain open");
                    state.output_override = OutputOverride::None;
                },
                |transition| publish_power_transition(&resume_event_bus, transition),
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
        assert!(!power.snapshot().sleeping());
    }

    use super::publish_power_transition;
}
