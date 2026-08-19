//! Linux host input capture via `/dev/input/event*`.
//!
//! One evdev-backed source owns every readable keyboard and pointer node:
//! it emits ordered, capture-timestamped edges for the frame batch and the
//! event bus, and folds held state (pressed keys, buttons, a virtual
//! pointer) into the per-frame [`InteractionData`] snapshot. The worker
//! rescans for hotplug and retries permission-denied nodes, so installing
//! the udev rules heals a running daemon without a restart.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::Context;
use evdev::{
    AttributeSetRef, Device, EventSummary, InputEvent as EvdevInputEvent, KeyCode,
    RelativeAxisCode, SynchronizationCode,
};
use tracing::{debug, info, trace, warn};

use crate::input::input_mono_ms;
use crate::input::traits::{InputData, InputSource, InteractionData, MotionAggregate, PointerMode};
use crate::input::{
    LegacyWheelProjector, SourceIssue, SourceKind, SourceResourceScanHealth, SourceStatusHandle,
    SourceStatusReporter, classify_source_resource_scan,
};
use crate::types::event::{
    InputButtonState, InputEvent, PointerScrollPhase, PointerScrollUnit, TimedInputEvent,
};
use hypercolor_worker_retention::{retain_worker, spawn_worker};

const POLL_INTERVAL: Duration = Duration::from_millis(8);
const READY_TIMEOUT: Duration = Duration::from_secs(1);
const STOP_TIMEOUT: Duration = Duration::from_secs(1);
const DEFAULT_EVENT_LIMIT: usize = 256;
/// Rescan cadence in poll ticks (≈2 s at the 8 ms poll interval).
const RESCAN_TICKS: u32 = 250;
/// Virtual cursor travel per relative count: a full sweep of a typical
/// desk surface (~1200 counts) crosses the whole canvas once.
const CURSOR_COUNTS_PER_UNIT: f32 = 1200.0;

/// Why an event node is not currently contributing input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceOpenState {
    /// Node is open and streaming events.
    Opened,
    /// Node exists but the daemon lacks read access (udev rules missing).
    PermissionDenied,
    /// Node is not a keyboard or pointer, or its kind is disabled by config.
    Ignored,
    /// Open failed for another reason (transient IO, stale node).
    Failed(String),
}

/// Fingerprint of the held-state visible to effects, used to decide when
/// the snapshot generation must advance.
type HeldStateKey = (Vec<String>, Vec<String>, i32, i32, bool);

/// Per-node open result for diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceOpenStatus {
    pub path: PathBuf,
    pub label: String,
    pub state: DeviceOpenState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DeviceCaps {
    keyboard: bool,
    pointer: bool,
    hi_res_vertical_scroll: bool,
    hi_res_horizontal_scroll: bool,
}

struct OpenDevice {
    event_state: DeviceEventState,
    device: Device,
}

struct DeviceEventState {
    source_id: String,
    label: String,
    caps: DeviceCaps,
    relative_motion: RelativeMotionFrame,
    discard_until_report: bool,
}

#[derive(Debug, Default)]
struct RelativeMotionFrame {
    dx_counts: i32,
    dy_counts: i32,
}

impl RelativeMotionFrame {
    fn accumulate(&mut self, axis: RelativeAxisCode, value: i32) {
        match axis {
            RelativeAxisCode::REL_X => {
                self.dx_counts = self.dx_counts.saturating_add(value);
            }
            RelativeAxisCode::REL_Y => {
                self.dy_counts = self.dy_counts.saturating_add(value);
            }
            _ => {}
        }
    }

    fn flush_into(&mut self, state: &mut SharedState) {
        let dx_counts = std::mem::take(&mut self.dx_counts);
        let dy_counts = std::mem::take(&mut self.dy_counts);
        if dx_counts == 0 && dy_counts == 0 {
            return;
        }

        #[expect(
            clippy::cast_precision_loss,
            clippy::as_conversions,
            reason = "relative counts are small integers"
        )]
        let dx = dx_counts as f32 / CURSOR_COUNTS_PER_UNIT;
        #[expect(
            clippy::cast_precision_loss,
            clippy::as_conversions,
            reason = "relative counts are small integers"
        )]
        let dy = dy_counts as f32 / CURSOR_COUNTS_PER_UNIT;
        state.cursor_x = (state.cursor_x + dx).clamp(0.0, 1.0);
        state.cursor_y = (state.cursor_y + dy).clamp(0.0, 1.0);
        state.motion.dx += dx;
        state.motion.dy += dy;
        state.motion.distance += dx.hypot(dy);
    }

    fn finish_sync(&mut self, code: SynchronizationCode, state: &mut SharedState) {
        match code {
            SynchronizationCode::SYN_REPORT => self.flush_into(state),
            SynchronizationCode::SYN_DROPPED => {
                self.dx_counts = 0;
                self.dy_counts = 0;
            }
            _ => {}
        }
    }
}

#[derive(Default)]
struct SharedState {
    events: VecDeque<TimedInputEvent>,
    dropped: u32,
    pressed_keys: BTreeMap<String, BTreeSet<String>>,
    recent_keys: VecDeque<String>,
    held_buttons: BTreeMap<String, BTreeSet<String>>,
    cursor_x: f32,
    cursor_y: f32,
    motion: MotionAggregate,
    pointer_present: bool,
    device_status: Vec<DeviceOpenStatus>,
    legacy_wheel_projectors: BTreeMap<String, LegacyWheelProjector>,
}

impl SharedState {
    fn union_pressed(&self) -> Vec<String> {
        let mut keys: BTreeSet<&String> = BTreeSet::new();
        for held in self.pressed_keys.values() {
            keys.extend(held.iter());
        }
        keys.into_iter().cloned().collect()
    }

    fn union_buttons(&self) -> Vec<String> {
        let mut buttons: BTreeSet<&String> = BTreeSet::new();
        for held in self.held_buttons.values() {
            buttons.extend(held.iter());
        }
        buttons.into_iter().cloned().collect()
    }

    fn clear_live_state(&mut self) {
        self.events.clear();
        self.dropped = 0;
        self.pressed_keys.clear();
        self.recent_keys.clear();
        self.held_buttons.clear();
        self.motion = MotionAggregate::default();
        self.pointer_present = false;
        self.device_status.clear();
        self.legacy_wheel_projectors.clear();
    }
}

/// Linux evdev host input source: keyboards and relative pointers.
///
/// Capture is demand-driven: devices are only opened while
/// [`set_interaction_capture_active`](InputSource::set_interaction_capture_active)
/// is on, and all held state and queued events clear when capture stops so
/// no key or button ever sticks.
pub struct EvdevHostInput {
    name: String,
    running: bool,
    capture_active: bool,
    capture_keyboard: bool,
    capture_pointer: bool,
    event_limit: usize,
    generation: u64,
    last_state_key: Option<HeldStateKey>,
    shared: Arc<Mutex<SharedState>>,
    worker: Option<EvdevWorker>,
    status: SourceStatusReporter,
}

struct EvdevWorker {
    stop_tx: mpsc::Sender<()>,
    exit_rx: mpsc::Receiver<()>,
    join_handle: JoinHandle<()>,
}

impl EvdevHostInput {
    /// Create a new host input source capturing the enabled device kinds.
    #[must_use]
    pub fn new(capture_keyboard: bool, capture_pointer: bool) -> Self {
        Self {
            name: "EvdevHostInput".to_owned(),
            running: false,
            capture_active: false,
            capture_keyboard,
            capture_pointer,
            event_limit: DEFAULT_EVENT_LIMIT,
            generation: 0,
            last_state_key: None,
            shared: Arc::new(Mutex::new(SharedState::default())),
            worker: None,
            status: SourceStatusReporter::new(
                "evdev_host_input",
                SourceKind::Interaction,
                "evdev",
                true,
                true,
                false,
            ),
        }
    }

    /// Snapshot of per-node open results for diagnostics.
    #[must_use]
    pub fn device_status(&self) -> Vec<DeviceOpenStatus> {
        self.shared
            .lock()
            .map(|guard| guard.device_status.clone())
            .unwrap_or_default()
    }

    fn build_snapshot(&mut self, guard: &mut SharedState) -> InteractionData {
        let mut data = InteractionData::default();
        data.keyboard.pressed_keys = guard.union_pressed();
        data.keyboard.recent_keys = guard.recent_keys.drain(..).collect();
        data.mouse.buttons = guard.union_buttons();
        data.mouse.down = !data.mouse.buttons.is_empty();
        let pointer_present = guard.pointer_present;
        if pointer_present {
            data.mouse.mode = PointerMode::Virtual;
            data.mouse.norm_x = guard.cursor_x;
            data.mouse.norm_y = guard.cursor_y;
        }
        data.batch.motion = std::mem::take(&mut guard.motion);
        data.batch.dropped_events = std::mem::take(&mut guard.dropped);

        // Coarse fixed-point cursor key so idle jitter below one part in
        // 10⁴ never bumps the generation.
        #[expect(
            clippy::cast_possible_truncation,
            clippy::as_conversions,
            reason = "values are clamped to [0,1] before scaling"
        )]
        let cursor_key = (
            (data.mouse.norm_x * 10_000.0) as i32,
            (data.mouse.norm_y * 10_000.0) as i32,
        );
        let state_key: HeldStateKey = (
            data.keyboard.pressed_keys.clone(),
            data.mouse.buttons.clone(),
            cursor_key.0,
            cursor_key.1,
            pointer_present,
        );
        if self.last_state_key.as_ref() != Some(&state_key) || !data.keyboard.recent_keys.is_empty()
        {
            self.generation = self.generation.wrapping_add(1);
            self.last_state_key = Some(state_key);
        }
        data.generation = self.generation;
        data
    }

    fn stop_worker(&mut self) {
        if let Some(worker) = &self.worker {
            let _ = worker.stop_tx.send(());
        }
        let (cursor_x, cursor_y) = self
            .shared
            .lock()
            .map_or((0.0, 0.0), |guard| (guard.cursor_x, guard.cursor_y));
        self.shared = Arc::new(Mutex::new(SharedState {
            cursor_x,
            cursor_y,
            ..SharedState::default()
        }));
        self.last_state_key = None;
        if let Some(worker) = &self.worker {
            let _ = worker.exit_rx.recv_timeout(STOP_TIMEOUT);
        }
        if self
            .worker
            .as_ref()
            .is_some_and(|worker| worker.join_handle.is_finished())
        {
            let worker = self.worker.take().expect("finished worker remains owned");
            if let Err(panic) = worker.join_handle.join() {
                warn!(source = %self.name, message = ?panic, "Evdev input worker panicked");
            }
        } else if self.worker.is_some() {
            warn!(source = %self.name, "Evdev input worker did not stop before the deadline");
        }
    }

    fn spawn_worker(&mut self) -> anyhow::Result<()> {
        self.observe_worker_exit(false);
        if self.worker.is_some() {
            anyhow::bail!("previous evdev host input worker is still stopping");
        }

        let shared = Arc::clone(&self.shared);
        let event_limit = self.event_limit;
        let source_name = self.name.clone();
        let capture_keyboard = self.capture_keyboard;
        let capture_pointer = self.capture_pointer;
        let status = self.status.session();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let (stop_tx, stop_rx) = mpsc::channel();
        let (exit_tx, exit_rx) = mpsc::sync_channel(1);

        let join_handle = spawn_worker(
            thread::Builder::new().name("hypercolor-evdev-input".to_owned()),
            move || {
                let mut devices: BTreeMap<PathBuf, OpenDevice> = BTreeMap::new();
                let health = rescan_devices(
                    &mut devices,
                    &shared,
                    capture_keyboard,
                    capture_pointer,
                    event_limit,
                    &source_name,
                );
                if let Some(status) = &status {
                    status.publish_resource_scan_health(health);
                }
                let _ = ready_tx.send(());

                let mut ticks_since_rescan: u32 = 0;
                loop {
                    ticks_since_rescan += 1;
                    if ticks_since_rescan >= RESCAN_TICKS {
                        ticks_since_rescan = 0;
                        let health = rescan_devices(
                            &mut devices,
                            &shared,
                            capture_keyboard,
                            capture_pointer,
                            event_limit,
                            &source_name,
                        );
                        if let Some(status) = &status {
                            status.publish_resource_scan_health(health);
                        }
                    }

                    poll_devices(&mut devices, &shared, event_limit, &source_name);
                    match stop_rx.recv_timeout(POLL_INTERVAL) {
                        Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                }
                let _ = exit_tx.send(());
            },
        )
        .context("failed to spawn evdev host input worker")?;

        self.worker = Some(EvdevWorker {
            stop_tx,
            exit_rx,
            join_handle,
        });
        if let Err(error) = ready_rx.recv_timeout(READY_TIMEOUT) {
            self.stop_worker();
            anyhow::bail!("timed out waiting for evdev host input worker readiness: {error}");
        }
        if self.observe_worker_exit(true) {
            anyhow::bail!("evdev host input worker exited during startup");
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
                || "evdev host input worker exited unexpectedly".to_owned(),
                |panic| format!("evdev host input worker panicked: {panic:?}"),
            );
            status.failed(SourceIssue::new("evdev_worker_exited", detail, true));
        }
        if let Ok(mut guard) = self.shared.lock() {
            guard.clear_live_state();
        }
        self.last_state_key = None;
        true
    }
}

impl Drop for EvdevHostInput {
    fn drop(&mut self) {
        let Some(worker) = self.worker.take() else {
            return;
        };
        let _ = worker.stop_tx.send(());
        if worker.join_handle.is_finished() {
            let _ = worker.join_handle.join();
            return;
        }
        retain_worker(
            worker.join_handle,
            Arc::<str>::from(format!("evdev input source {}", self.name)),
        );
    }
}

impl InputSource for EvdevHostInput {
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
        let Ok(mut guard) = shared.lock() else {
            return Ok(InputData::None);
        };
        let snapshot = self.build_snapshot(&mut guard);
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

        let shared = Arc::clone(&self.shared);
        let Ok(mut guard) = shared.lock() else {
            return (Ok(InputData::None), Vec::new());
        };
        // One lock for both: an edge can never land in the frame's event
        // batch while missing from the same frame's held-state snapshot.
        let events = drain_events(&mut guard.events);
        let snapshot = self.build_snapshot(&mut guard);
        (Ok(InputData::Interaction(snapshot)), events)
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
        let status = self.device_status();
        let devices_opened = status
            .iter()
            .filter(|entry| entry.state == DeviceOpenState::Opened)
            .count();
        let devices_denied = status
            .iter()
            .filter(|entry| entry.state == DeviceOpenState::PermissionDenied)
            .count();
        Some(crate::input::InteractionDiagnostics {
            backend: "evdev",
            host_capture: true,
            capturing: self.capture_active && self.worker.is_some(),
            devices_opened,
            devices_denied,
            // The counters already express Linux's only failure mode; the
            // typed field carries it so consumers can stop reimplementing the
            // `denied > 0 && opened == 0` heuristic for themselves.
            degraded: (devices_denied > 0 && devices_opened == 0)
                .then_some(crate::input::InteractionDegradation::AccessDenied),
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

    fn drain_events(&mut self) -> Vec<TimedInputEvent> {
        if !self.running || self.worker.is_none() {
            return Vec::new();
        }
        if let Ok(mut guard) = self.shared.lock() {
            return drain_events(&mut guard.events);
        }

        Vec::new()
    }
}

// ── Worker internals ───────────────────────────────────────────────────────

fn rescan_devices(
    devices: &mut BTreeMap<PathBuf, OpenDevice>,
    shared: &Arc<Mutex<SharedState>>,
    capture_keyboard: bool,
    capture_pointer: bool,
    event_limit: usize,
    source_name: &str,
) -> SourceResourceScanHealth {
    let mut status = Vec::new();
    let mut present = BTreeSet::new();

    for path in enumerate_event_nodes() {
        present.insert(path.clone());
        if let Some(open) = devices.get(&path) {
            status.push(DeviceOpenStatus {
                path,
                label: open.event_state.label.clone(),
                state: DeviceOpenState::Opened,
            });
            continue;
        }

        match Device::open(&path) {
            Ok(device) => {
                let caps = classify_device(&device, capture_keyboard, capture_pointer);
                let label = device_label(&path, &device);
                if !caps.keyboard && !caps.pointer {
                    status.push(DeviceOpenStatus {
                        path,
                        label,
                        state: DeviceOpenState::Ignored,
                    });
                    continue;
                }
                if let Err(error) = device.set_nonblocking(true) {
                    status.push(DeviceOpenStatus {
                        path,
                        label,
                        state: DeviceOpenState::Failed(error.to_string()),
                    });
                    continue;
                }
                info!(
                    source = %source_name,
                    device = %label,
                    keyboard = caps.keyboard,
                    pointer = caps.pointer,
                    "Opened evdev input device"
                );
                status.push(DeviceOpenStatus {
                    path: path.clone(),
                    label: label.clone(),
                    state: DeviceOpenState::Opened,
                });
                devices.insert(
                    path.clone(),
                    OpenDevice {
                        event_state: DeviceEventState {
                            source_id: path.display().to_string(),
                            label,
                            caps,
                            relative_motion: RelativeMotionFrame::default(),
                            discard_until_report: false,
                        },
                        device,
                    },
                );
            }
            Err(error) if error.kind() == ErrorKind::PermissionDenied => {
                status.push(DeviceOpenStatus {
                    path: path.clone(),
                    label: path.display().to_string(),
                    state: DeviceOpenState::PermissionDenied,
                });
            }
            Err(error) => {
                status.push(DeviceOpenStatus {
                    path: path.clone(),
                    label: path.display().to_string(),
                    state: DeviceOpenState::Failed(error.to_string()),
                });
            }
        }
    }

    // Nodes that vanished: drop them and synthesize releases for anything
    // they still held so effects see clean edges and nothing sticks.
    let removed: Vec<PathBuf> = devices
        .keys()
        .filter(|path| !present.contains(*path))
        .cloned()
        .collect();
    if !removed.is_empty()
        && let Ok(mut guard) = shared.lock()
    {
        for path in &removed {
            if let Some(open) = devices.remove(path) {
                debug!(source = %source_name, device = %open.event_state.label, "Evdev input device removed");
                synthesize_releases(&mut guard, &open.event_state.source_id, event_limit);
            }
        }
    }

    let opened = status
        .iter()
        .filter(|entry| entry.state == DeviceOpenState::Opened)
        .count();
    let denied = status
        .iter()
        .filter(|entry| entry.state == DeviceOpenState::PermissionDenied)
        .count();
    let failed = status
        .iter()
        .filter(|entry| matches!(&entry.state, DeviceOpenState::Failed(_)))
        .count();
    if let Ok(mut guard) = shared.lock() {
        if denied > 0 && guard.device_status.is_empty() {
            warn!(
                source = %source_name,
                denied,
                "Some input nodes are unreadable; run `just udev-install` and replug (or re-login)"
            );
        }
        guard.pointer_present = devices
            .values()
            .any(|device| device.event_state.caps.pointer);
        guard.device_status = status;
    }
    classify_source_resource_scan(opened, denied, failed)
}

fn poll_devices(
    devices: &mut BTreeMap<PathBuf, OpenDevice>,
    shared: &Arc<Mutex<SharedState>>,
    event_limit: usize,
    source_name: &str,
) {
    let mut stale = Vec::new();

    for (path, open) in devices.iter_mut() {
        let at_ms = input_mono_ms();
        // Collect first: the fetch iterator holds a mutable borrow of the
        // device, and folding needs the rest of the OpenDevice immutably.
        let fetched: Result<Vec<EvdevInputEvent>, std::io::Error> =
            open.device.fetch_events().map(Iterator::collect);
        match fetched {
            Ok(events) => {
                let Ok(mut guard) = shared.lock() else {
                    continue;
                };
                for event in events {
                    fold_event(&mut guard, &mut open.event_state, event, at_ms, event_limit);
                }
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {}
            Err(error) => {
                warn!(
                    source = %source_name,
                    device = %open.event_state.label,
                    %error,
                    "Evdev input device stopped producing events"
                );
                stale.push(path.clone());
            }
        }
    }

    if !stale.is_empty()
        && let Ok(mut guard) = shared.lock()
    {
        for path in stale {
            if let Some(open) = devices.remove(&path) {
                synthesize_releases(&mut guard, &open.event_state.source_id, event_limit);
            }
        }
        guard.pointer_present = devices
            .values()
            .any(|device| device.event_state.caps.pointer);
    }
}

fn fold_event(
    state: &mut SharedState,
    device: &mut DeviceEventState,
    event: EvdevInputEvent,
    at_ms: u64,
    event_limit: usize,
) {
    let summary = event.destructure();
    if device.discard_until_report {
        if matches!(
            &summary,
            EventSummary::Synchronization(_, SynchronizationCode::SYN_REPORT, _)
        ) {
            device.discard_until_report = false;
        }
        return;
    }
    if matches!(
        &summary,
        EventSummary::Synchronization(_, SynchronizationCode::SYN_DROPPED, _)
    ) {
        device.relative_motion = RelativeMotionFrame::default();
        device.discard_until_report = true;
        synthesize_releases_at(state, &device.source_id, event_limit, at_ms);
        return;
    }

    match summary {
        EventSummary::Key(_, code, value) => {
            let Some(button_state) = button_state_from_value(value) else {
                trace!(device = %device.label, code = ?code, value, "Ignoring evdev key value");
                return;
            };

            if let Some(button) = pointer_button_name(code) {
                if !device.caps.pointer {
                    return;
                }
                update_held(
                    &mut state.held_buttons,
                    &device.source_id,
                    button,
                    button_state,
                );
                push_event(
                    state,
                    TimedInputEvent {
                        event: InputEvent::MouseButton {
                            source_id: device.source_id.clone(),
                            button: button.to_owned(),
                            state: button_state,
                        },
                        at_ms,
                        seq: 0,
                        physical_code: Some(format!("evdev:{code:?}")),
                        repeat_count: 1,
                    },
                    event_limit,
                );
                return;
            }

            if !device.caps.keyboard || is_non_keyboard_key(code) {
                return;
            }
            let key = canonical_evdev_key_name(code);
            match button_state {
                InputButtonState::Pressed | InputButtonState::Repeated => {
                    state
                        .pressed_keys
                        .entry(device.source_id.clone())
                        .or_default()
                        .insert(key.clone());
                    if button_state == InputButtonState::Pressed {
                        state.recent_keys.push_back(key.clone());
                        cap_recent(&mut state.recent_keys, event_limit);
                    }
                }
                InputButtonState::Released => {
                    if let Some(held) = state.pressed_keys.get_mut(&device.source_id) {
                        held.remove(&key);
                    }
                }
            }
            push_event(
                state,
                TimedInputEvent {
                    event: InputEvent::Key {
                        source_id: device.source_id.clone(),
                        key,
                        state: button_state,
                    },
                    at_ms,
                    seq: 0,
                    physical_code: Some(format!("evdev:{code:?}")),
                    repeat_count: 1,
                },
                event_limit,
            );
        }
        EventSummary::RelativeAxis(_, axis, value) => {
            if !device.caps.pointer {
                return;
            }
            match axis {
                RelativeAxisCode::REL_X | RelativeAxisCode::REL_Y => {
                    device.relative_motion.accumulate(axis, value);
                }
                RelativeAxisCode::REL_WHEEL_HI_RES => {
                    fold_scroll(
                        state,
                        &device.source_id,
                        0,
                        i64::from(value) << 16,
                        "evdev:REL_WHEEL_HI_RES",
                        at_ms,
                        event_limit,
                    );
                }
                RelativeAxisCode::REL_HWHEEL_HI_RES => {
                    fold_scroll(
                        state,
                        &device.source_id,
                        i64::from(value) << 16,
                        0,
                        "evdev:REL_HWHEEL_HI_RES",
                        at_ms,
                        event_limit,
                    );
                }
                // Devices with hi-res axes report both forms. Suppress each
                // low-resolution axis independently to avoid double counting.
                RelativeAxisCode::REL_WHEEL if !device.caps.hi_res_vertical_scroll => {
                    fold_scroll(
                        state,
                        &device.source_id,
                        0,
                        (i64::from(value) * 120) << 16,
                        "evdev:REL_WHEEL",
                        at_ms,
                        event_limit,
                    );
                }
                RelativeAxisCode::REL_HWHEEL if !device.caps.hi_res_horizontal_scroll => {
                    fold_scroll(
                        state,
                        &device.source_id,
                        (i64::from(value) * 120) << 16,
                        0,
                        "evdev:REL_HWHEEL",
                        at_ms,
                        event_limit,
                    );
                }
                _ => {}
            }
        }
        EventSummary::Synchronization(_, code, _) => {
            device.relative_motion.finish_sync(code, state);
        }
        _ => {}
    }
}

fn fold_scroll(
    state: &mut SharedState,
    source_id: &str,
    delta_x_q16_16: i64,
    delta_y_q16_16: i64,
    physical_code: &str,
    at_ms: u64,
    event_limit: usize,
) {
    push_event(
        state,
        TimedInputEvent {
            event: InputEvent::PointerScroll {
                source_id: source_id.to_owned(),
                delta_x_q16_16,
                delta_y_q16_16,
                unit: PointerScrollUnit::Line120,
                phase: PointerScrollPhase::None,
                momentum_phase: PointerScrollPhase::None,
            },
            at_ms,
            seq: 0,
            physical_code: Some(physical_code.to_owned()),
            repeat_count: 1,
        },
        event_limit,
    );

    let legacy_delta = state
        .legacy_wheel_projectors
        .entry(source_id.to_owned())
        .or_default()
        .project(delta_y_q16_16);
    if legacy_delta == 0 {
        return;
    }
    push_event(
        state,
        TimedInputEvent {
            event: InputEvent::MouseWheel {
                source_id: source_id.to_owned(),
                delta_hi_res: legacy_delta,
            },
            at_ms,
            seq: 0,
            physical_code: Some("evdev:legacy-wheel-shadow".to_owned()),
            repeat_count: 1,
        },
        event_limit,
    );
}

fn push_event(state: &mut SharedState, event: TimedInputEvent, limit: usize) {
    if limit == 0 {
        state.dropped = state
            .dropped
            .saturating_add(u32::try_from(state.events.len()).unwrap_or(u32::MAX))
            .saturating_add(1);
        state.events.clear();
        return;
    }
    while state.events.len() >= limit {
        state.events.pop_front();
        state.dropped = state.dropped.saturating_add(1);
    }
    state.events.push_back(event);
}

fn cap_recent(recent: &mut VecDeque<String>, limit: usize) {
    while recent.len() > limit {
        recent.pop_front();
    }
}

fn drain_events(events: &mut VecDeque<TimedInputEvent>) -> Vec<TimedInputEvent> {
    events.drain(..).collect()
}

fn update_held(
    held: &mut BTreeMap<String, BTreeSet<String>>,
    source_id: &str,
    name: &str,
    state: InputButtonState,
) {
    match state {
        InputButtonState::Pressed => {
            held.entry(source_id.to_owned())
                .or_default()
                .insert(name.to_owned());
        }
        InputButtonState::Released => {
            if let Some(entries) = held.get_mut(source_id) {
                entries.remove(name);
            }
        }
        InputButtonState::Repeated => {}
    }
}

/// Emit synthetic release edges for everything a vanished device held.
fn synthesize_releases(state: &mut SharedState, source_id: &str, event_limit: usize) {
    synthesize_releases_at(state, source_id, event_limit, input_mono_ms());
}

fn synthesize_releases_at(
    state: &mut SharedState,
    source_id: &str,
    event_limit: usize,
    at_ms: u64,
) {
    state.legacy_wheel_projectors.remove(source_id);
    if let Some(keys) = state.pressed_keys.remove(source_id) {
        for key in keys {
            push_event(
                state,
                TimedInputEvent {
                    event: InputEvent::Key {
                        source_id: source_id.to_owned(),
                        key,
                        state: InputButtonState::Released,
                    },
                    at_ms,
                    seq: 0,
                    physical_code: None,
                    repeat_count: 1,
                },
                event_limit,
            );
        }
    }
    if let Some(buttons) = state.held_buttons.remove(source_id) {
        for button in buttons {
            push_event(
                state,
                TimedInputEvent {
                    event: InputEvent::MouseButton {
                        source_id: source_id.to_owned(),
                        button,
                        state: InputButtonState::Released,
                    },
                    at_ms,
                    seq: 0,
                    physical_code: None,
                    repeat_count: 1,
                },
                event_limit,
            );
        }
    }
}

fn enumerate_event_nodes() -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir("/dev/input") else {
        return Vec::new();
    };
    let mut nodes: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("event"))
        })
        .collect();
    nodes.sort();
    nodes
}

fn classify_device(device: &Device, capture_keyboard: bool, capture_pointer: bool) -> DeviceCaps {
    let keys = device.supported_keys();
    let axes = device.supported_relative_axes();
    classify_capabilities(keys, axes, capture_keyboard, capture_pointer)
}

fn classify_capabilities(
    keys: Option<&AttributeSetRef<KeyCode>>,
    axes: Option<&AttributeSetRef<RelativeAxisCode>>,
    capture_keyboard: bool,
    capture_pointer: bool,
) -> DeviceCaps {
    let looks_like_keyboard = keys.is_some_and(|keys| {
        keys.contains(KeyCode::KEY_A)
            || keys.contains(KeyCode::KEY_Z)
            || keys.contains(KeyCode::KEY_ENTER)
            || keys.contains(KeyCode::KEY_SPACE)
            || crate::input::keymap::MEDIA_KEYS
                .iter()
                .any(|media_key| keys.contains(KeyCode(media_key.evdev_code)))
    });
    let looks_like_pointer = axes.is_some_and(|axes| {
        axes.contains(RelativeAxisCode::REL_X) && axes.contains(RelativeAxisCode::REL_Y)
    }) && keys.is_some_and(|keys| keys.contains(KeyCode::BTN_LEFT));
    let hi_res_vertical_scroll =
        axes.is_some_and(|axes| axes.contains(RelativeAxisCode::REL_WHEEL_HI_RES));
    let hi_res_horizontal_scroll =
        axes.is_some_and(|axes| axes.contains(RelativeAxisCode::REL_HWHEEL_HI_RES));

    DeviceCaps {
        keyboard: capture_keyboard && looks_like_keyboard,
        pointer: capture_pointer && looks_like_pointer,
        hi_res_vertical_scroll,
        hi_res_horizontal_scroll,
    }
}

fn device_label(path: &Path, device: &Device) -> String {
    device.name().map_or_else(
        || path.display().to_string(),
        |name| format!("{name} ({})", path.display()),
    )
}

fn button_state_from_value(value: i32) -> Option<InputButtonState> {
    match value {
        0 => Some(InputButtonState::Released),
        1 => Some(InputButtonState::Pressed),
        2 => Some(InputButtonState::Repeated),
        _ => None,
    }
}

fn pointer_button_name(code: KeyCode) -> Option<&'static str> {
    match code {
        KeyCode::BTN_LEFT => Some("left"),
        KeyCode::BTN_RIGHT => Some("right"),
        KeyCode::BTN_MIDDLE => Some("middle"),
        KeyCode::BTN_SIDE => Some("side"),
        KeyCode::BTN_EXTRA => Some("extra"),
        _ => None,
    }
}

/// Non-keyboard key codes that must not enter the pressed-keys set: the
/// `BTN_*` ranges Linux uses for pointers, joysticks, gamepads (`0x100..0x160`),
/// D-pads (`BTN_DPAD_*`, `0x220..0x228`), and the trigger-happy programmable
/// button block (`0x2c0..0x2e0`).
fn is_non_keyboard_key(code: KeyCode) -> bool {
    let c = code.0;
    (0x100..0x160).contains(&c) || (0x220..0x228).contains(&c) || (0x2c0..0x2e0).contains(&c)
}

/// Canonical name for an evdev keycode, from the shared inventory.
///
/// The table lives in [`crate::input::keymap`] because Windows has to produce
/// the same names from set-1 scan codes, and two hand-maintained tables drift
/// the moment someone adds a key to one of them. Codes outside the inventory
/// keep their debug name so an unmapped key is visible rather than dropped.
fn canonical_evdev_key_name(code: KeyCode) -> String {
    crate::input::keymap::evdev_key_name(code.0)
        .map_or_else(|| format!("{code:?}"), ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use evdev::{AttributeSet, EventType};

    fn event_state(source_id: &str, keyboard: bool, pointer: bool) -> DeviceEventState {
        DeviceEventState {
            source_id: source_id.to_owned(),
            label: source_id.to_owned(),
            caps: DeviceCaps {
                keyboard,
                pointer,
                hi_res_vertical_scroll: false,
                hi_res_horizontal_scroll: false,
            },
            relative_motion: RelativeMotionFrame::default(),
            discard_until_report: false,
        }
    }

    fn relative_event(axis: RelativeAxisCode, value: i32) -> EvdevInputEvent {
        EvdevInputEvent::new(EventType::RELATIVE.0, axis.0, value)
    }

    fn key_event(key: KeyCode, value: i32) -> EvdevInputEvent {
        EvdevInputEvent::new(EventType::KEY.0, key.0, value)
    }

    fn sync_event(code: SynchronizationCode) -> EvdevInputEvent {
        EvdevInputEvent::new(EventType::SYNCHRONIZATION.0, code.0, 0)
    }

    #[test]
    fn canonical_key_names_follow_browser_style() {
        assert_eq!(canonical_evdev_key_name(KeyCode::KEY_A), "a");
        assert_eq!(canonical_evdev_key_name(KeyCode::KEY_LEFT), "ArrowLeft");
        assert_eq!(canonical_evdev_key_name(KeyCode::KEY_SPACE), "Space");
        assert_eq!(
            canonical_evdev_key_name(KeyCode::KEY_LEFTCTRL),
            "ControlLeft"
        );
    }

    #[test]
    fn pointer_buttons_map_to_stable_names() {
        assert_eq!(pointer_button_name(KeyCode::BTN_LEFT), Some("left"));
        assert_eq!(pointer_button_name(KeyCode::BTN_EXTRA), Some("extra"));
        assert_eq!(pointer_button_name(KeyCode::KEY_A), None);
    }

    #[test]
    fn button_range_keys_stay_out_of_the_keyboard_set() {
        assert!(is_non_keyboard_key(KeyCode::BTN_LEFT));
        assert!(is_non_keyboard_key(KeyCode::BTN_SIDE));
        assert!(!is_non_keyboard_key(KeyCode::KEY_A));
        assert!(!is_non_keyboard_key(KeyCode::KEY_SPACE));
    }

    #[test]
    fn every_shared_media_key_classifies_a_media_only_node_as_a_keyboard() {
        for media_key in crate::input::keymap::MEDIA_KEYS {
            let keys = [KeyCode(media_key.evdev_code)]
                .into_iter()
                .collect::<AttributeSet<_>>();
            let enabled = classify_capabilities(Some(&keys), None, true, true);
            let disabled = classify_capabilities(Some(&keys), None, false, true);

            assert_eq!(
                enabled,
                DeviceCaps {
                    keyboard: true,
                    pointer: false,
                    hi_res_vertical_scroll: false,
                    hi_res_horizontal_scroll: false,
                },
                "{} should make a media-only node eligible",
                media_key.name
            );
            assert!(!disabled.keyboard);
        }
    }

    #[test]
    fn media_keys_fold_with_the_shared_canonical_vocabulary() {
        let mut state = SharedState::default();
        let mut device = event_state("consumer-control", true, false);

        for media_key in crate::input::keymap::MEDIA_KEYS {
            fold_event(
                &mut state,
                &mut device,
                key_event(KeyCode(media_key.evdev_code), 1),
                1,
                DEFAULT_EVENT_LIMIT,
            );
        }

        let expected = crate::input::keymap::MEDIA_KEYS
            .iter()
            .map(|media_key| media_key.name.to_owned())
            .collect::<Vec<_>>();
        let mut expected_held = expected.clone();
        expected_held.sort();
        assert_eq!(state.union_pressed(), expected_held);
        assert_eq!(
            state.recent_keys.iter().cloned().collect::<Vec<_>>(),
            expected
        );
        assert_eq!(
            state
                .events
                .iter()
                .map(|timed| match &timed.event {
                    InputEvent::Key { key, .. } => key.clone(),
                    event => panic!("expected key event, got {event:?}"),
                })
                .collect::<Vec<_>>(),
            expected
        );
    }

    #[test]
    fn scroll_capabilities_are_detected_per_axis() {
        let keys = [KeyCode::BTN_LEFT].into_iter().collect::<AttributeSet<_>>();
        let axes = [
            RelativeAxisCode::REL_X,
            RelativeAxisCode::REL_Y,
            RelativeAxisCode::REL_WHEEL_HI_RES,
        ]
        .into_iter()
        .collect::<AttributeSet<_>>();

        let caps = classify_capabilities(Some(&keys), Some(&axes), false, true);
        assert!(caps.pointer);
        assert!(caps.hi_res_vertical_scroll);
        assert!(!caps.hi_res_horizontal_scroll);
    }

    #[test]
    fn high_resolution_vertical_scroll_emits_exact_event_then_shadow() {
        let mut state = SharedState::default();
        let mut device = event_state("mouse", false, true);

        fold_event(
            &mut state,
            &mut device,
            relative_event(RelativeAxisCode::REL_WHEEL_HI_RES, -30),
            7,
            DEFAULT_EVENT_LIMIT,
        );

        assert_eq!(state.events.len(), 2);
        assert!(matches!(
            &state.events[0].event,
            InputEvent::PointerScroll {
                delta_x_q16_16: 0,
                delta_y_q16_16,
                unit: PointerScrollUnit::Line120,
                ..
            } if *delta_y_q16_16 == -30 * crate::input::Q16_16_SCALE
        ));
        assert!(matches!(
            &state.events[1].event,
            InputEvent::MouseWheel {
                delta_hi_res: -30,
                ..
            }
        ));
    }

    #[test]
    fn high_resolution_horizontal_scroll_never_projects_to_legacy_wheel() {
        let mut state = SharedState::default();
        let mut device = event_state("mouse", false, true);

        fold_event(
            &mut state,
            &mut device,
            relative_event(RelativeAxisCode::REL_HWHEEL_HI_RES, 45),
            7,
            DEFAULT_EVENT_LIMIT,
        );

        assert_eq!(state.events.len(), 1);
        assert!(matches!(
            &state.events[0].event,
            InputEvent::PointerScroll {
                delta_x_q16_16,
                delta_y_q16_16: 0,
                ..
            } if *delta_x_q16_16 == 45 * crate::input::Q16_16_SCALE
        ));
    }

    #[test]
    fn low_resolution_scroll_converts_notches_and_defers_to_each_high_res_axis() {
        let mut state = SharedState::default();
        let mut device = event_state("mouse", false, true);
        device.caps.hi_res_vertical_scroll = true;

        fold_event(
            &mut state,
            &mut device,
            relative_event(RelativeAxisCode::REL_WHEEL, 1),
            7,
            DEFAULT_EVENT_LIMIT,
        );
        fold_event(
            &mut state,
            &mut device,
            relative_event(RelativeAxisCode::REL_HWHEEL, -1),
            8,
            DEFAULT_EVENT_LIMIT,
        );

        assert_eq!(
            state.events.len(),
            1,
            "vertical low-res duplicate is suppressed"
        );
        assert!(matches!(
            &state.events[0].event,
            InputEvent::PointerScroll {
                delta_x_q16_16,
                delta_y_q16_16: 0,
                ..
            } if *delta_x_q16_16 == -120 * crate::input::Q16_16_SCALE
        ));
    }

    #[test]
    fn fold_event_tracks_pressed_and_released_keys_per_source() {
        let mut state = SharedState::default();
        let mut held = BTreeMap::new();
        update_held(&mut held, "kbd-a", "left", InputButtonState::Pressed);
        update_held(&mut held, "kbd-b", "left", InputButtonState::Pressed);
        update_held(&mut held, "kbd-a", "left", InputButtonState::Released);
        state.held_buttons = held;

        assert_eq!(
            state.union_buttons(),
            vec!["left".to_owned()],
            "release on one source must not clear another source's hold"
        );
    }

    #[test]
    fn synthesized_releases_clear_held_state_and_emit_edges() {
        let mut state = SharedState::default();
        state
            .pressed_keys
            .entry("kbd".to_owned())
            .or_default()
            .insert("a".to_owned());
        state
            .held_buttons
            .entry("kbd".to_owned())
            .or_default()
            .insert("left".to_owned());

        synthesize_releases(&mut state, "kbd", DEFAULT_EVENT_LIMIT);

        assert!(state.union_pressed().is_empty());
        assert!(state.union_buttons().is_empty());
        assert_eq!(state.events.len(), 2);
        assert!(state.events.iter().all(|timed| matches!(
            &timed.event,
            InputEvent::Key {
                state: InputButtonState::Released,
                ..
            } | InputEvent::MouseButton {
                state: InputButtonState::Released,
                ..
            }
        )));
    }

    #[test]
    fn capture_beginning_on_repeat_establishes_held_state_and_synthesizes_release() {
        let mut state = SharedState::default();
        let mut device = event_state("kbd", true, false);

        fold_event(
            &mut state,
            &mut device,
            key_event(KeyCode::KEY_A, 2),
            1,
            DEFAULT_EVENT_LIMIT,
        );

        assert_eq!(state.union_pressed(), vec!["a".to_owned()]);
        assert!(state.recent_keys.is_empty());
        assert!(matches!(
            &state.events[0].event,
            InputEvent::Key {
                key,
                state: InputButtonState::Repeated,
                ..
            } if key == "a"
        ));

        synthesize_releases_at(&mut state, "kbd", DEFAULT_EVENT_LIMIT, 2);
        assert!(state.union_pressed().is_empty());
        assert!(matches!(
            &state.events[1].event,
            InputEvent::Key {
                key,
                state: InputButtonState::Released,
                ..
            } if key == "a"
        ));
    }

    #[test]
    fn event_queue_overflow_drops_oldest_and_counts() {
        let mut state = SharedState::default();
        for seq in 0..10 {
            push_event(
                &mut state,
                TimedInputEvent {
                    event: InputEvent::Key {
                        source_id: "kbd".to_owned(),
                        key: "a".to_owned(),
                        state: InputButtonState::Pressed,
                    },
                    at_ms: seq,
                    seq,
                    physical_code: None,
                    repeat_count: 1,
                },
                4,
            );
        }
        assert_eq!(state.events.len(), 4);
        assert_eq!(state.dropped, 6);
        assert_eq!(state.events.front().map(|event| event.at_ms), Some(6));
    }

    #[test]
    fn event_queue_enforces_a_lowered_or_zero_limit() {
        let mut state = SharedState::default();
        for seq in 0..4 {
            push_event(
                &mut state,
                TimedInputEvent {
                    event: InputEvent::Key {
                        source_id: "kbd".to_owned(),
                        key: "a".to_owned(),
                        state: InputButtonState::Pressed,
                    },
                    at_ms: seq,
                    seq,
                    physical_code: None,
                    repeat_count: 1,
                },
                4,
            );
        }

        let next = state.events.back().cloned().expect("queued event");
        push_event(&mut state, next.clone(), 2);
        assert_eq!(state.events.len(), 2);
        assert_eq!(state.dropped, 3);

        push_event(&mut state, next, 0);
        assert!(state.events.is_empty());
        assert_eq!(state.dropped, 6);
    }

    #[test]
    fn late_worker_publication_is_isolated_after_stop() {
        let mut source = EvdevHostInput::new(true, true);
        let retired_state = Arc::clone(&source.shared);
        let late_worker_state = Arc::clone(&retired_state);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let late_worker = thread::spawn(move || {
            release_rx.recv().expect("late worker should be released");
            let event = TimedInputEvent {
                event: InputEvent::Key {
                    source_id: "late-worker".to_owned(),
                    key: "a".to_owned(),
                    state: InputButtonState::Pressed,
                },
                at_ms: 1,
                seq: 1,
                physical_code: None,
                repeat_count: 1,
            };
            push_event(
                &mut late_worker_state.lock().expect("retired state lock"),
                event,
                DEFAULT_EVENT_LIMIT,
            );
        });
        source.stop_worker();
        assert!(!Arc::ptr_eq(&retired_state, &source.shared));
        release_tx.send(()).expect("late worker should resume");
        late_worker.join().expect("late worker should finish");

        assert_eq!(
            retired_state
                .lock()
                .expect("retired state lock")
                .events
                .len(),
            1
        );
        assert_eq!(Arc::strong_count(&retired_state), 1);
        assert!(
            source
                .shared
                .lock()
                .expect("active state lock")
                .events
                .is_empty()
        );
    }

    #[test]
    fn stopping_preserves_relative_cursor_position() {
        let mut source = EvdevHostInput::new(false, true);
        {
            let mut state = source.shared.lock().expect("active state lock");
            state.cursor_x = 0.75;
            state.cursor_y = 0.25;
        }

        source.stop_worker();

        let state = source.shared.lock().expect("successor state lock");
        assert!((state.cursor_x - 0.75).abs() < f32::EPSILON);
        assert!((state.cursor_y - 0.25).abs() < f32::EPSILON);
    }

    #[test]
    fn fold_event_flushes_motion_per_device_on_its_syn_report() {
        let mut first = event_state("mouse-a", false, true);
        let mut second = event_state("mouse-b", false, true);
        let mut state = SharedState::default();

        fold_event(
            &mut state,
            &mut first,
            relative_event(RelativeAxisCode::REL_X, 3),
            1,
            DEFAULT_EVENT_LIMIT,
        );
        fold_event(
            &mut state,
            &mut second,
            relative_event(RelativeAxisCode::REL_X, 12),
            1,
            DEFAULT_EVENT_LIMIT,
        );
        fold_event(
            &mut state,
            &mut first,
            relative_event(RelativeAxisCode::REL_Y, 4),
            1,
            DEFAULT_EVENT_LIMIT,
        );
        assert_eq!(state.motion, MotionAggregate::default());

        fold_event(
            &mut state,
            &mut first,
            sync_event(SynchronizationCode::SYN_REPORT),
            1,
            DEFAULT_EVENT_LIMIT,
        );

        assert!((state.motion.dx - 3.0 / CURSOR_COUNTS_PER_UNIT).abs() < f32::EPSILON);
        assert!((state.motion.dy - 4.0 / CURSOR_COUNTS_PER_UNIT).abs() < f32::EPSILON);
        assert!((state.motion.distance - 5.0 / CURSOR_COUNTS_PER_UNIT).abs() < f32::EPSILON);

        fold_event(
            &mut state,
            &mut second,
            relative_event(RelativeAxisCode::REL_Y, 5),
            1,
            DEFAULT_EVENT_LIMIT,
        );
        fold_event(
            &mut state,
            &mut second,
            sync_event(SynchronizationCode::SYN_REPORT),
            1,
            DEFAULT_EVENT_LIMIT,
        );
        assert!((state.motion.distance - 18.0 / CURSOR_COUNTS_PER_UNIT).abs() < f32::EPSILON);
    }

    #[test]
    fn syn_dropped_releases_state_and_discards_until_the_next_report() {
        let mut device = event_state("combo", true, true);
        let mut state = SharedState::default();

        fold_event(
            &mut state,
            &mut device,
            key_event(KeyCode::KEY_A, 1),
            1,
            DEFAULT_EVENT_LIMIT,
        );
        fold_event(
            &mut state,
            &mut device,
            relative_event(RelativeAxisCode::REL_X, 300),
            1,
            DEFAULT_EVENT_LIMIT,
        );
        fold_event(
            &mut state,
            &mut device,
            sync_event(SynchronizationCode::SYN_DROPPED),
            2,
            DEFAULT_EVENT_LIMIT,
        );
        fold_event(
            &mut state,
            &mut device,
            relative_event(RelativeAxisCode::REL_Y, 400),
            2,
            DEFAULT_EVENT_LIMIT,
        );
        fold_event(
            &mut state,
            &mut device,
            key_event(KeyCode::KEY_B, 1),
            2,
            DEFAULT_EVENT_LIMIT,
        );
        fold_event(
            &mut state,
            &mut device,
            sync_event(SynchronizationCode::SYN_REPORT),
            2,
            DEFAULT_EVENT_LIMIT,
        );

        assert_eq!(state.motion, MotionAggregate::default());
        assert!(state.union_pressed().is_empty());
        assert!(matches!(
            state.events.back().map(|event| &event.event),
            Some(InputEvent::Key {
                key,
                state: InputButtonState::Released,
                ..
            }) if key == "a"
        ));

        fold_event(
            &mut state,
            &mut device,
            relative_event(RelativeAxisCode::REL_X, 12),
            3,
            DEFAULT_EVENT_LIMIT,
        );
        fold_event(
            &mut state,
            &mut device,
            sync_event(SynchronizationCode::SYN_REPORT),
            3,
            DEFAULT_EVENT_LIMIT,
        );
        assert!((state.motion.dx - 12.0 / CURSOR_COUNTS_PER_UNIT).abs() < f32::EPSILON);
    }
}
