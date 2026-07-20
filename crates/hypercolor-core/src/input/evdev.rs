//! Linux host input capture via `/dev/input/event*`.
//!
//! One evdev-backed source owns every readable keyboard and pointer node:
//! it emits ordered, capture-timestamped edges for the frame batch and the
//! event bus, and folds held state (pressed keys, buttons, a virtual
//! pointer) into the per-frame [`InteractionData`] snapshot. The worker
//! rescans for hotplug and retries permission-denied nodes, so installing
//! the udev rules heals a running daemon without a restart.

use std::collections::{BTreeMap, BTreeSet};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::Context;
use evdev::{Device, EventSummary, InputEvent as EvdevInputEvent, KeyCode, RelativeAxisCode};
use tracing::{debug, info, trace, warn};

use crate::input::input_mono_ms;
use crate::input::traits::{InputData, InputSource, InteractionData, MotionAggregate, PointerMode};
use crate::types::event::{InputButtonState, InputEvent, TimedInputEvent};

const POLL_INTERVAL: Duration = Duration::from_millis(8);
const READY_TIMEOUT: Duration = Duration::from_secs(1);
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
    hi_res_wheel: bool,
}

struct OpenDevice {
    source_id: String,
    label: String,
    caps: DeviceCaps,
    device: Device,
}

#[derive(Default)]
struct SharedState {
    events: Vec<TimedInputEvent>,
    dropped: u32,
    pressed_keys: BTreeMap<String, BTreeSet<String>>,
    recent_keys: Vec<String>,
    held_buttons: BTreeMap<String, BTreeSet<String>>,
    cursor_x: f32,
    cursor_y: f32,
    motion: MotionAggregate,
    pointer_present: bool,
    device_status: Vec<DeviceOpenStatus>,
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
    stop_flag: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
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
            stop_flag: Arc::new(AtomicBool::new(false)),
            worker: None,
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
        data.keyboard.recent_keys = std::mem::take(&mut guard.recent_keys);
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
        self.stop_flag.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        if let Ok(mut guard) = self.shared.lock() {
            guard.clear_live_state();
        }
        self.last_state_key = None;
    }

    fn spawn_worker(&mut self) -> anyhow::Result<()> {
        if self.worker.is_some() {
            return Ok(());
        }

        self.stop_flag.store(false, Ordering::Release);
        let shared = Arc::clone(&self.shared);
        let stop_flag = Arc::clone(&self.stop_flag);
        let event_limit = self.event_limit;
        let source_name = self.name.clone();
        let capture_keyboard = self.capture_keyboard;
        let capture_pointer = self.capture_pointer;
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);

        let worker = thread::Builder::new()
            .name("hypercolor-evdev-input".to_owned())
            .spawn(move || {
                let mut devices: BTreeMap<PathBuf, OpenDevice> = BTreeMap::new();
                rescan_devices(
                    &mut devices,
                    &shared,
                    capture_keyboard,
                    capture_pointer,
                    event_limit,
                    &source_name,
                );
                let _ = ready_tx.send(());

                let mut ticks_since_rescan: u32 = 0;
                while !stop_flag.load(Ordering::Acquire) {
                    ticks_since_rescan += 1;
                    if ticks_since_rescan >= RESCAN_TICKS {
                        ticks_since_rescan = 0;
                        rescan_devices(
                            &mut devices,
                            &shared,
                            capture_keyboard,
                            capture_pointer,
                            event_limit,
                            &source_name,
                        );
                    }

                    poll_devices(&mut devices, &shared, event_limit, &source_name);
                    thread::sleep(POLL_INTERVAL);
                }
            })
            .context("failed to spawn evdev host input worker")?;

        ready_rx
            .recv_timeout(READY_TIMEOUT)
            .context("timed out waiting for evdev host input worker readiness")?;

        self.worker = Some(worker);
        Ok(())
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

        self.running = true;
        if self.capture_active {
            self.spawn_worker()?;
        }
        Ok(())
    }

    fn stop(&mut self) {
        self.stop_worker();
        self.running = false;
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
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
        if !self.running || self.worker.is_none() {
            return (Ok(InputData::None), Vec::new());
        }

        let shared = Arc::clone(&self.shared);
        let Ok(mut guard) = shared.lock() else {
            return (Ok(InputData::None), Vec::new());
        };
        // One lock for both: an edge can never land in the frame's event
        // batch while missing from the same frame's held-state snapshot.
        let events = std::mem::take(&mut guard.events);
        let snapshot = self.build_snapshot(&mut guard);
        (Ok(InputData::Interaction(snapshot)), events)
    }

    fn is_running(&self) -> bool {
        self.running
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
        })
    }

    fn set_interaction_capture_active(&mut self, active: bool) -> anyhow::Result<()> {
        if self.capture_active == active {
            return Ok(());
        }

        self.capture_active = active;
        if !self.running {
            return Ok(());
        }

        if active {
            self.spawn_worker()?;
        } else {
            self.stop_worker();
        }
        Ok(())
    }

    fn drain_events(&mut self) -> Vec<TimedInputEvent> {
        if let Ok(mut guard) = self.shared.lock() {
            return std::mem::take(&mut guard.events);
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
) {
    let mut status = Vec::new();
    let mut present = BTreeSet::new();

    for path in enumerate_event_nodes() {
        present.insert(path.clone());
        if let Some(open) = devices.get(&path) {
            status.push(DeviceOpenStatus {
                path,
                label: open.label.clone(),
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
                        source_id: path.display().to_string(),
                        label,
                        caps,
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
                debug!(source = %source_name, device = %open.label, "Evdev input device removed");
                synthesize_releases(&mut guard, &open.source_id, event_limit);
            }
        }
    }

    if let Ok(mut guard) = shared.lock() {
        let denied = status
            .iter()
            .filter(|entry| entry.state == DeviceOpenState::PermissionDenied)
            .count();
        if denied > 0 && guard.device_status.is_empty() {
            warn!(
                source = %source_name,
                denied,
                "Some input nodes are unreadable; run `just udev-install` and replug (or re-login)"
            );
        }
        guard.pointer_present = devices.values().any(|device| device.caps.pointer);
        guard.device_status = status;
    }
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
                    fold_event(&mut guard, open, event, at_ms, event_limit);
                }
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {}
            Err(error) => {
                warn!(
                    source = %source_name,
                    device = %open.label,
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
                synthesize_releases(&mut guard, &open.source_id, event_limit);
            }
        }
        guard.pointer_present = devices.values().any(|device| device.caps.pointer);
    }
}

fn fold_event(
    state: &mut SharedState,
    open: &OpenDevice,
    event: EvdevInputEvent,
    at_ms: u64,
    event_limit: usize,
) {
    match event.destructure() {
        EventSummary::Key(_, code, value) => {
            let Some(button_state) = button_state_from_value(value) else {
                trace!(device = %open.label, code = ?code, value, "Ignoring evdev key value");
                return;
            };

            if let Some(button) = pointer_button_name(code) {
                if !open.caps.pointer {
                    return;
                }
                update_held(
                    &mut state.held_buttons,
                    &open.source_id,
                    button,
                    button_state,
                );
                push_event(
                    state,
                    TimedInputEvent {
                        event: InputEvent::MouseButton {
                            source_id: open.source_id.clone(),
                            button: button.to_owned(),
                            state: button_state,
                        },
                        at_ms,
                        seq: 0,
                    },
                    event_limit,
                );
                return;
            }

            if !open.caps.keyboard || is_non_keyboard_key(code) {
                return;
            }
            let key = canonical_evdev_key_name(code);
            match button_state {
                InputButtonState::Pressed => {
                    state
                        .pressed_keys
                        .entry(open.source_id.clone())
                        .or_default()
                        .insert(key.clone());
                    state.recent_keys.push(key.clone());
                    if state.recent_keys.len() > event_limit {
                        let overflow = state.recent_keys.len() - event_limit;
                        state.recent_keys.drain(..overflow);
                    }
                }
                InputButtonState::Released => {
                    if let Some(held) = state.pressed_keys.get_mut(&open.source_id) {
                        held.remove(&key);
                    }
                }
                InputButtonState::Repeated => {}
            }
            push_event(
                state,
                TimedInputEvent {
                    event: InputEvent::Key {
                        source_id: open.source_id.clone(),
                        key,
                        state: button_state,
                    },
                    at_ms,
                    seq: 0,
                },
                event_limit,
            );
        }
        EventSummary::RelativeAxis(_, axis, value) => {
            if !open.caps.pointer {
                return;
            }
            match axis {
                RelativeAxisCode::REL_X | RelativeAxisCode::REL_Y => {
                    #[expect(
                        clippy::cast_precision_loss,
                        clippy::as_conversions,
                        reason = "relative counts are small integers"
                    )]
                    let delta = value as f32 / CURSOR_COUNTS_PER_UNIT;
                    if axis == RelativeAxisCode::REL_X {
                        state.cursor_x = (state.cursor_x + delta).clamp(0.0, 1.0);
                        state.motion.dx += delta;
                    } else {
                        state.cursor_y = (state.cursor_y + delta).clamp(0.0, 1.0);
                        state.motion.dy += delta;
                    }
                    state.motion.distance += delta.abs();
                }
                RelativeAxisCode::REL_WHEEL_HI_RES => {
                    push_event(
                        state,
                        TimedInputEvent {
                            event: InputEvent::MouseWheel {
                                source_id: open.source_id.clone(),
                                delta_hi_res: value,
                            },
                            at_ms,
                            seq: 0,
                        },
                        event_limit,
                    );
                }
                RelativeAxisCode::REL_WHEEL => {
                    // Devices with hi-res wheels report both; keep only the
                    // hi-res stream to avoid double counting.
                    if !open.caps.hi_res_wheel {
                        push_event(
                            state,
                            TimedInputEvent {
                                event: InputEvent::MouseWheel {
                                    source_id: open.source_id.clone(),
                                    delta_hi_res: value.saturating_mul(120),
                                },
                                at_ms,
                                seq: 0,
                            },
                            event_limit,
                        );
                    }
                }
                _ => {}
            }
        }
        _ => {}
    }
}

fn push_event(state: &mut SharedState, event: TimedInputEvent, limit: usize) {
    state.events.push(event);
    if state.events.len() > limit {
        let overflow = state.events.len() - limit;
        state.events.drain(..overflow);
        state.dropped = state
            .dropped
            .saturating_add(u32::try_from(overflow).unwrap_or(u32::MAX));
    }
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
    let at_ms = input_mono_ms();
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
    let looks_like_keyboard = keys.is_some_and(|keys| {
        keys.contains(KeyCode::KEY_A)
            || keys.contains(KeyCode::KEY_Z)
            || keys.contains(KeyCode::KEY_ENTER)
            || keys.contains(KeyCode::KEY_SPACE)
    });
    let axes = device.supported_relative_axes();
    let looks_like_pointer = axes.is_some_and(|axes| {
        axes.contains(RelativeAxisCode::REL_X) && axes.contains(RelativeAxisCode::REL_Y)
    }) && keys.is_some_and(|keys| keys.contains(KeyCode::BTN_LEFT));
    let hi_res_wheel = axes.is_some_and(|axes| axes.contains(RelativeAxisCode::REL_WHEEL_HI_RES));

    DeviceCaps {
        keyboard: capture_keyboard && looks_like_keyboard,
        pointer: capture_pointer && looks_like_pointer,
        hi_res_wheel,
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

fn canonical_evdev_key_name(code: KeyCode) -> String {
    match code {
        KeyCode::KEY_0 => "0".to_owned(),
        KeyCode::KEY_1 => "1".to_owned(),
        KeyCode::KEY_2 => "2".to_owned(),
        KeyCode::KEY_3 => "3".to_owned(),
        KeyCode::KEY_4 => "4".to_owned(),
        KeyCode::KEY_5 => "5".to_owned(),
        KeyCode::KEY_6 => "6".to_owned(),
        KeyCode::KEY_7 => "7".to_owned(),
        KeyCode::KEY_8 => "8".to_owned(),
        KeyCode::KEY_9 => "9".to_owned(),
        KeyCode::KEY_A => "a".to_owned(),
        KeyCode::KEY_B => "b".to_owned(),
        KeyCode::KEY_C => "c".to_owned(),
        KeyCode::KEY_D => "d".to_owned(),
        KeyCode::KEY_E => "e".to_owned(),
        KeyCode::KEY_F => "f".to_owned(),
        KeyCode::KEY_G => "g".to_owned(),
        KeyCode::KEY_H => "h".to_owned(),
        KeyCode::KEY_I => "i".to_owned(),
        KeyCode::KEY_J => "j".to_owned(),
        KeyCode::KEY_K => "k".to_owned(),
        KeyCode::KEY_L => "l".to_owned(),
        KeyCode::KEY_M => "m".to_owned(),
        KeyCode::KEY_N => "n".to_owned(),
        KeyCode::KEY_O => "o".to_owned(),
        KeyCode::KEY_P => "p".to_owned(),
        KeyCode::KEY_Q => "q".to_owned(),
        KeyCode::KEY_R => "r".to_owned(),
        KeyCode::KEY_S => "s".to_owned(),
        KeyCode::KEY_T => "t".to_owned(),
        KeyCode::KEY_U => "u".to_owned(),
        KeyCode::KEY_V => "v".to_owned(),
        KeyCode::KEY_W => "w".to_owned(),
        KeyCode::KEY_X => "x".to_owned(),
        KeyCode::KEY_Y => "y".to_owned(),
        KeyCode::KEY_Z => "z".to_owned(),
        KeyCode::KEY_UP => "ArrowUp".to_owned(),
        KeyCode::KEY_DOWN => "ArrowDown".to_owned(),
        KeyCode::KEY_LEFT => "ArrowLeft".to_owned(),
        KeyCode::KEY_RIGHT => "ArrowRight".to_owned(),
        KeyCode::KEY_LEFTCTRL => "ControlLeft".to_owned(),
        KeyCode::KEY_RIGHTCTRL => "ControlRight".to_owned(),
        KeyCode::KEY_LEFTSHIFT => "ShiftLeft".to_owned(),
        KeyCode::KEY_RIGHTSHIFT => "ShiftRight".to_owned(),
        KeyCode::KEY_LEFTALT => "AltLeft".to_owned(),
        KeyCode::KEY_RIGHTALT => "AltRight".to_owned(),
        KeyCode::KEY_LEFTMETA => "MetaLeft".to_owned(),
        KeyCode::KEY_RIGHTMETA => "MetaRight".to_owned(),
        KeyCode::KEY_SPACE => "Space".to_owned(),
        KeyCode::KEY_ENTER => "Enter".to_owned(),
        KeyCode::KEY_ESC => "Escape".to_owned(),
        KeyCode::KEY_TAB => "Tab".to_owned(),
        KeyCode::KEY_BACKSPACE => "Backspace".to_owned(),
        KeyCode::KEY_MINUS => "-".to_owned(),
        KeyCode::KEY_EQUAL => "=".to_owned(),
        KeyCode::KEY_LEFTBRACE => "[".to_owned(),
        KeyCode::KEY_RIGHTBRACE => "]".to_owned(),
        KeyCode::KEY_BACKSLASH => "\\".to_owned(),
        KeyCode::KEY_SEMICOLON => ";".to_owned(),
        KeyCode::KEY_APOSTROPHE => "'".to_owned(),
        KeyCode::KEY_GRAVE => "`".to_owned(),
        KeyCode::KEY_COMMA => ",".to_owned(),
        KeyCode::KEY_DOT => ".".to_owned(),
        KeyCode::KEY_SLASH => "/".to_owned(),
        other => format!("{other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
                },
                4,
            );
        }
        assert_eq!(state.events.len(), 4);
        assert_eq!(state.dropped, 6);
        assert_eq!(state.events.first().map(|event| event.at_ms), Some(6));
    }
}
