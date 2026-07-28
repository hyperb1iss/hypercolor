//! Host keyboard and mouse capture for interactive `LightScript` effects.
//!
//! The capture backend runs on a dedicated polling thread so the public input
//! source stays `Send` even when the platform device handle is not.

use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::Context;
use device_query::{DeviceQuery, DeviceState, Keycode};
use hypercolor_types::event::{InputButtonState, InputEvent, TimedInputEvent};
use tracing::warn;

use crate::input::traits::{InputData, InputSource, InteractionData, MouseData};
use crate::input::worker_retention::{retain_input_worker, spawn_input_worker};
use crate::input::{SourceIssue, SourceKind, SourceStatusHandle, SourceStatusReporter};

const POLL_INTERVAL: Duration = Duration::from_millis(10);
const READY_TIMEOUT: Duration = Duration::from_secs(1);
const STOP_TIMEOUT: Duration = Duration::from_secs(1);
const DEFAULT_RECENT_KEY_LIMIT: usize = 32;
const DEFAULT_EVENT_LIMIT: usize = crate::input::InteractionBatch::MAX_EVENTS;
const DEVICE_QUERY_SOURCE_ID: &str = "host:device_query";

#[derive(Default)]
struct SharedInteractionState {
    interaction: InteractionData,
    events: Vec<TimedInputEvent>,
}

/// Global host input source for `LightScript` keyboard and mouse helpers.
///
/// Interim bridge backend for platforms without a native event backend
/// (Windows/macOS). Never constructed on Linux — evdev owns host input
/// there. Capture is demand-driven: the worker thread only runs while
/// [`set_interaction_capture_active`](InputSource::set_interaction_capture_active)
/// is on.
pub struct InteractionInput {
    name: String,
    running: bool,
    capture_active: bool,
    recent_key_limit: usize,
    generation: u64,
    last_held: Option<(Vec<String>, MouseData)>,
    shared: Arc<Mutex<SharedInteractionState>>,
    worker: Option<InteractionWorker>,
    status: SourceStatusReporter,
}

struct InteractionWorker {
    stop_tx: mpsc::Sender<()>,
    exit_rx: mpsc::Receiver<()>,
    join_handle: JoinHandle<()>,
}

impl InteractionInput {
    /// Create a new host input capture source.
    #[must_use]
    pub fn new() -> Self {
        Self {
            name: "HostInput".to_owned(),
            running: false,
            capture_active: false,
            recent_key_limit: DEFAULT_RECENT_KEY_LIMIT,
            generation: 0,
            last_held: None,
            shared: Arc::new(Mutex::new(SharedInteractionState::default())),
            worker: None,
            status: SourceStatusReporter::new(
                "host_input",
                SourceKind::Interaction,
                "device_query",
                true,
                true,
                false,
            ),
        }
    }

    fn stop_worker(&mut self) {
        if let Some(worker) = &self.worker {
            let _ = worker.stop_tx.send(());
            let _ = worker.exit_rx.recv_timeout(STOP_TIMEOUT);
        }
        if self
            .worker
            .as_ref()
            .is_some_and(|worker| worker.join_handle.is_finished())
        {
            let worker = self.worker.take().expect("finished worker remains owned");
            if let Err(panic) = worker.join_handle.join() {
                warn!(source = %self.name, message = ?panic, "Host input worker panicked");
            }
        } else if self.worker.is_some() {
            warn!(source = %self.name, "Host input worker did not stop before the deadline");
        }
        if let Ok(mut guard) = self.shared.lock() {
            *guard = SharedInteractionState::default();
        }
        self.last_held = None;
    }

    fn spawn_worker(&mut self) -> anyhow::Result<()> {
        self.observe_worker_exit(false);
        if self.worker.is_some() {
            anyhow::bail!("previous host input worker is still stopping");
        }

        let shared = Arc::clone(&self.shared);
        let recent_key_limit = self.recent_key_limit;
        let source_name = self.name.clone();
        let status = self.status.session();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let (stop_tx, stop_rx) = mpsc::channel();
        let (exit_tx, exit_rx) = mpsc::sync_channel(1);

        let join_handle = spawn_input_worker(
            thread::Builder::new().name("hypercolor-host-input".to_owned()),
            move || {
                let Some(device_state) = try_create_device_state() else {
                    warn!(
                        source = %source_name,
                        "Host input capture unavailable; interactive LightScript input will stay idle"
                    );
                    if let Some(status) = &status {
                        status.unavailable(
                            SourceIssue::new(
                                "host_input_backend_unavailable",
                                "host input capture backend is unavailable",
                                true,
                            )
                            .with_remediation(
                                "run Hypercolor inside an interactive desktop session",
                            ),
                        );
                    }
                    let _ = ready_tx.send(false);
                    let _ = exit_tx.send(());
                    return;
                };

                if let Some(status) = &status {
                    status.mark_event_driven_live_without_deadline(1);
                }
                let _ = ready_tx.send(true);
                let mut previous_keys: Vec<Keycode> = Vec::new();

                loop {
                    let current_keys = sorted_keys(device_state.get_keys());
                    let mouse_state = device_state.get_mouse();

                    publish_poll(
                        &shared,
                        &previous_keys,
                        &current_keys,
                        &mouse_state,
                        recent_key_limit,
                        DEFAULT_EVENT_LIMIT,
                        crate::input::input_mono_ms(),
                    );
                    previous_keys.clone_from(&current_keys);

                    match stop_rx.recv_timeout(POLL_INTERVAL) {
                        Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                }
                let _ = exit_tx.send(());
            },
        )
        .context("failed to spawn host input capture worker")?;

        self.worker = Some(InteractionWorker {
            stop_tx,
            exit_rx,
            join_handle,
        });
        let ready = match ready_rx.recv_timeout(READY_TIMEOUT) {
            Ok(ready) => ready,
            Err(error) => {
                self.stop_worker();
                anyhow::bail!("timed out waiting for host input worker readiness: {error}");
            }
        };
        if !ready {
            self.stop_worker();
            return Ok(());
        }
        if self.observe_worker_exit(true) {
            anyhow::bail!("host input worker exited during startup");
        }
        Ok(())
    }

    fn observe_worker_exit(&mut self, publish_failure: bool) -> bool {
        let Some(worker) = self.worker.as_ref() else {
            return false;
        };
        if !worker.join_handle.is_finished() {
            return false;
        }
        let worker = self.worker.take().expect("finished worker remains owned");
        let failure = worker.join_handle.join().err();
        if publish_failure && let Some(status) = self.status.session() {
            let detail = failure.map_or_else(
                || "host input worker exited unexpectedly".to_owned(),
                |panic| format!("host input worker panicked: {panic:?}"),
            );
            status.failed(SourceIssue::new("host_input_worker_exited", detail, true));
        }
        if let Ok(mut guard) = self.shared.lock() {
            *guard = SharedInteractionState::default();
        }
        self.last_held = None;
        true
    }

    fn build_snapshot(&mut self, guard: &mut SharedInteractionState) -> InteractionData {
        let mut snapshot = guard.interaction.clone();
        snapshot.keyboard.recent_keys = std::mem::take(&mut guard.interaction.keyboard.recent_keys);
        snapshot.batch.dropped_events = std::mem::take(&mut guard.interaction.batch.dropped_events);

        let held = (
            snapshot.keyboard.pressed_keys.clone(),
            snapshot.mouse.clone(),
        );
        if self.last_held.as_ref() != Some(&held) || !snapshot.keyboard.recent_keys.is_empty() {
            self.generation = self.generation.wrapping_add(1);
            self.last_held = Some(held);
        }
        snapshot.generation = self.generation;
        snapshot
    }

    fn take_snapshot_and_events(&mut self) -> Option<(InteractionData, Vec<TimedInputEvent>)> {
        let shared = Arc::clone(&self.shared);
        let mut guard = shared.lock().ok()?;
        let events = std::mem::take(&mut guard.events);
        let mut snapshot = self.build_snapshot(&mut guard);
        project_recent_keys(&mut snapshot.keyboard.recent_keys, &events);
        Some((snapshot, events))
    }

    /// Fold deterministic `device_query` snapshots through the production
    /// publication path without starting an operating-system capture worker.
    #[doc(hidden)]
    pub fn fold_polled_key_sequence_for_testing(
        &mut self,
        polls: &[(Vec<Keycode>, u64)],
        event_limit: usize,
    ) -> (InteractionData, Vec<TimedInputEvent>) {
        let mut previous = Vec::new();
        let mouse = device_query::MouseState {
            coords: (0, 0),
            button_pressed: Vec::new(),
        };
        for (keys, at_ms) in polls {
            let current = sorted_keys(keys.clone());
            publish_poll(
                &self.shared,
                &previous,
                &current,
                &mouse,
                DEFAULT_RECENT_KEY_LIMIT,
                event_limit,
                *at_ms,
            );
            previous = current;
        }
        self.take_snapshot_and_events().unwrap_or_default()
    }
}

impl Drop for InteractionInput {
    fn drop(&mut self) {
        let Some(worker) = self.worker.take() else {
            return;
        };
        let _ = worker.stop_tx.send(());
        if worker.join_handle.is_finished() {
            let _ = worker.join_handle.join();
            return;
        }
        retain_input_worker(
            worker.join_handle,
            Arc::<str>::from(format!("host input source {}", self.name)),
        );
    }
}

impl InputSource for InteractionInput {
    fn name(&self) -> &str {
        &self.name
    }

    fn start(&mut self) -> anyhow::Result<()> {
        if self.running {
            return Ok(());
        }

        if self.capture_active {
            self.status.begin_session()?;
            if let Err(error) = self.spawn_worker() {
                self.status.stop();
                self.stop_worker();
                return Err(error);
            }
        }
        self.running = true;
        Ok(())
    }

    fn stop(&mut self) {
        self.status.stop();
        self.stop_worker();
        self.running = false;
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        self.observe_worker_exit(self.running && self.capture_active);
        if !self.running || self.worker.is_none() {
            return Ok(InputData::None);
        }

        let shared = Arc::clone(&self.shared);
        let snapshot = shared.lock().map_or_else(
            |_| InteractionData::default(),
            |mut guard| self.build_snapshot(&mut guard),
        );

        Ok(InputData::Interaction(snapshot))
    }

    fn sample_and_drain_with_delta_secs(
        &mut self,
        _delta_secs: f32,
    ) -> (anyhow::Result<InputData>, Vec<TimedInputEvent>) {
        self.observe_worker_exit(self.running && self.capture_active);
        if !self.running || self.worker.is_none() {
            return (Ok(InputData::None), Vec::new());
        }

        self.take_snapshot_and_events().map_or_else(
            || (Ok(InputData::None), Vec::new()),
            |(snapshot, events)| (Ok(InputData::Interaction(snapshot)), events),
        )
    }

    fn drain_events(&mut self) -> Vec<TimedInputEvent> {
        if !self.running || self.worker.is_none() {
            return Vec::new();
        }
        self.shared.lock().map_or_else(
            |_| Vec::new(),
            |mut guard| std::mem::take(&mut guard.events),
        )
    }

    fn is_running(&self) -> bool {
        self.running
    }

    fn source_status_handle(&self) -> Option<SourceStatusHandle> {
        Some(self.status.handle())
    }

    fn source_status_reporter(&mut self) -> Option<&mut SourceStatusReporter> {
        Some(&mut self.status)
    }

    fn is_interaction_source(&self) -> bool {
        true
    }

    fn is_host_capture_source(&self) -> bool {
        true
    }

    fn interaction_diagnostics(&self) -> Option<crate::input::InteractionDiagnostics> {
        Some(crate::input::InteractionDiagnostics {
            backend: "device_query",
            host_capture: true,
            capturing: self.capture_active && self.worker.is_some(),
            devices_opened: usize::from(self.worker.is_some()),
            devices_denied: 0,
            degraded: None,
        })
    }

    fn set_interaction_capture_active(&mut self, active: bool) -> anyhow::Result<()> {
        let previous = self.capture_active;
        self.status.set_policy(true, true, active)?;
        if previous == active {
            return Ok(());
        }
        if !self.running {
            self.capture_active = active;
            return Ok(());
        }

        if active {
            self.status.begin_session()?;
            if let Err(error) = self.spawn_worker() {
                self.status.stop();
                self.status.set_policy(true, true, previous)?;
                self.stop_worker();
                return Err(error);
            }
        } else {
            self.status.stop();
            self.stop_worker();
        }
        self.capture_active = active;
        Ok(())
    }
}

impl Default for InteractionInput {
    fn default() -> Self {
        Self::new()
    }
}

fn sorted_keys(mut keys: Vec<Keycode>) -> Vec<Keycode> {
    keys.sort_by_key(Keycode::to_string);
    keys
}

fn publish_poll(
    shared: &Arc<Mutex<SharedInteractionState>>,
    previous: &[Keycode],
    current: &[Keycode],
    mouse_state: &device_query::MouseState,
    recent_key_limit: usize,
    event_limit: usize,
    at_ms: u64,
) {
    let (recent_keys, events) = key_transitions(previous, current, at_ms);
    let pressed_keys = current.iter().copied().map(canonical_key_name).collect();
    let mouse = mouse_data_from_state(mouse_state);

    if let Ok(mut guard) = shared.lock() {
        guard.interaction.keyboard.pressed_keys = pressed_keys;
        extend_recent_keys(
            &mut guard.interaction.keyboard.recent_keys,
            recent_keys,
            recent_key_limit,
        );
        let dropped = extend_events(&mut guard.events, events, event_limit);
        guard.interaction.batch.dropped_events = guard
            .interaction
            .batch
            .dropped_events
            .saturating_add(dropped);
        guard.interaction.mouse = mouse;
    }
}

fn key_transitions(
    previous: &[Keycode],
    current: &[Keycode],
    at_ms: u64,
) -> (Vec<String>, Vec<TimedInputEvent>) {
    let released = previous.iter().filter(|key| !current.contains(key));
    let pressed = current.iter().filter(|key| !previous.contains(key));
    let mut recent_keys = Vec::new();
    let mut events = Vec::new();

    // Snapshot polling has no within-poll ordering. Release-before-press keeps
    // replacement chords from briefly exposing both old and new held state.
    for key in released {
        events.push(key_event(
            canonical_key_name(*key),
            InputButtonState::Released,
            at_ms,
        ));
    }
    for key in pressed {
        let key = canonical_key_name(*key);
        recent_keys.push(key.clone());
        events.push(key_event(key, InputButtonState::Pressed, at_ms));
    }

    (recent_keys, events)
}

fn key_event(key: String, state: InputButtonState, at_ms: u64) -> TimedInputEvent {
    TimedInputEvent {
        event: InputEvent::Key {
            source_id: DEVICE_QUERY_SOURCE_ID.to_owned(),
            key,
            state,
        },
        at_ms,
        seq: 0,
        physical_code: None,
        repeat_count: 1,
    }
}

fn extend_recent_keys(target: &mut Vec<String>, mut recent: Vec<String>, limit: usize) {
    target.append(&mut recent);
    if target.len() > limit {
        let overflow = target.len() - limit;
        target.drain(..overflow);
    }
}

fn extend_events(
    target: &mut Vec<TimedInputEvent>,
    mut events: Vec<TimedInputEvent>,
    limit: usize,
) -> u32 {
    target.append(&mut events);
    let overflow = target.len().saturating_sub(limit);
    if overflow > 0 {
        target.drain(..overflow);
    }
    u32::try_from(overflow).unwrap_or(u32::MAX)
}

fn project_recent_keys(target: &mut Vec<String>, events: &[TimedInputEvent]) {
    target.clear();
    target.extend(events.iter().filter_map(|event| match &event.event {
        InputEvent::Key {
            key,
            state: InputButtonState::Pressed,
            ..
        } => Some(key.clone()),
        InputEvent::Key { .. }
        | InputEvent::MouseButton { .. }
        | InputEvent::MouseWheel { .. }
        | InputEvent::MidiNote { .. }
        | InputEvent::MidiControlChange { .. }
        | InputEvent::MidiPitchBend { .. }
        | InputEvent::MidiRealtime { .. } => None,
    }));
}

fn mouse_data_from_state(mouse_state: &device_query::MouseState) -> MouseData {
    let buttons = mouse_state
        .button_pressed
        .iter()
        .enumerate()
        .filter(|(_, pressed)| **pressed)
        .map(|(idx, _)| mouse_button_name(idx))
        .collect::<Vec<String>>();
    let (x, y) = mouse_state.coords;
    // device_query reports desktop pixels without screen geometry, so the
    // normalized fields stay unavailable until the native backends land.
    MouseData {
        x,
        y,
        down: !buttons.is_empty(),
        buttons,
        norm_x: 0.0,
        norm_y: 0.0,
        mode: crate::input::traits::PointerMode::None,
        injected: false,
    }
}

fn mouse_button_name(index: usize) -> String {
    match index {
        1 => "left",
        2 => "middle",
        3 => "right",
        4 => "button4",
        5 => "button5",
        _ => "button",
    }
    .to_owned()
}

fn canonical_key_name(key: Keycode) -> String {
    match key {
        Keycode::Key0 => "0",
        Keycode::Key1 => "1",
        Keycode::Key2 => "2",
        Keycode::Key3 => "3",
        Keycode::Key4 => "4",
        Keycode::Key5 => "5",
        Keycode::Key6 => "6",
        Keycode::Key7 => "7",
        Keycode::Key8 => "8",
        Keycode::Key9 => "9",
        Keycode::A => "a",
        Keycode::B => "b",
        Keycode::C => "c",
        Keycode::D => "d",
        Keycode::E => "e",
        Keycode::F => "f",
        Keycode::G => "g",
        Keycode::H => "h",
        Keycode::I => "i",
        Keycode::J => "j",
        Keycode::K => "k",
        Keycode::L => "l",
        Keycode::M => "m",
        Keycode::N => "n",
        Keycode::O => "o",
        Keycode::P => "p",
        Keycode::Q => "q",
        Keycode::R => "r",
        Keycode::S => "s",
        Keycode::T => "t",
        Keycode::U => "u",
        Keycode::V => "v",
        Keycode::W => "w",
        Keycode::X => "x",
        Keycode::Y => "y",
        Keycode::Z => "z",
        Keycode::Up => "ArrowUp",
        Keycode::Down => "ArrowDown",
        Keycode::Left => "ArrowLeft",
        Keycode::Right => "ArrowRight",
        Keycode::LControl => "ControlLeft",
        Keycode::RControl => "ControlRight",
        Keycode::LShift => "ShiftLeft",
        Keycode::RShift => "ShiftRight",
        Keycode::LAlt | Keycode::LOption => "AltLeft",
        Keycode::RAlt | Keycode::ROption => "AltRight",
        Keycode::LMeta | Keycode::Command => "MetaLeft",
        Keycode::RMeta | Keycode::RCommand => "MetaRight",
        other => match other {
            Keycode::Grave => "`",
            Keycode::Minus => "-",
            Keycode::Equal => "=",
            Keycode::LeftBracket => "[",
            Keycode::RightBracket => "]",
            Keycode::BackSlash => "\\",
            Keycode::Semicolon => ";",
            Keycode::Apostrophe => "'",
            Keycode::Comma => ",",
            Keycode::Dot => ".",
            Keycode::Slash => "/",
            _ => return other.to_string(),
        },
    }
    .to_owned()
}

fn try_create_device_state() -> Option<DeviceState> {
    #[cfg(target_os = "linux")]
    {
        if !host_input_session_available() {
            return None;
        }
        DeviceState::checked_new()
    }

    #[cfg(not(target_os = "linux"))]
    {
        std::panic::catch_unwind(DeviceState::new).ok()
    }
}

#[cfg(target_os = "linux")]
fn host_input_session_available() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("DISPLAY").is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use device_query::Keycode;

    #[test]
    fn canonical_key_names_follow_browser_style_for_common_keys() {
        assert_eq!(canonical_key_name(Keycode::A), "a");
        assert_eq!(canonical_key_name(Keycode::Escape), "Escape");
        assert_eq!(canonical_key_name(Keycode::Left), "ArrowLeft");
        assert_eq!(canonical_key_name(Keycode::Space), "Space");
        assert_eq!(canonical_key_name(Keycode::LControl), "ControlLeft");
    }

    #[test]
    fn extend_recent_keys_caps_queue_size() {
        let mut recent = vec!["a".to_owned(), "b".to_owned()];
        extend_recent_keys(&mut recent, vec!["c".to_owned(), "d".to_owned()], 3);
        assert_eq!(recent, vec!["b", "c", "d"]);
    }

    #[test]
    fn mouse_state_maps_common_buttons() {
        let mouse_state = device_query::MouseState {
            coords: (12, 34),
            button_pressed: vec![false, true, false, true],
        };
        let mouse = mouse_data_from_state(&mouse_state);
        assert_eq!(mouse.x, 12);
        assert_eq!(mouse.y, 34);
        assert!(mouse.down);
        assert_eq!(mouse.buttons, vec!["left", "right"]);
    }
}
