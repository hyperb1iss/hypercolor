use std::collections::{BTreeMap, BTreeSet};
use std::io::ErrorKind;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use evdev::{
    AttributeSetRef, Device, EventSummary, InputEvent, KeyCode, RelativeAxisCode,
    SynchronizationCode,
};
use hypercolor_types::event::{PointerScrollPhase, PointerScrollUnit};
use hypercolor_types::host_input::{
    HostInputBatch, HostInputCapabilities, HostInputDevice, HostInputEvent, HostInputGapReason,
    HostPointerButton, HostPointerMotion, HostRepeatEvidence,
};
use hypercolor_worker_retention::{retain_worker, spawn_worker};
use tracing::{debug, info, trace, warn};

use crate::shared::normalize_evdev_key;
use crate::{
    DeviceOpenState, DeviceOpenStatus, EvdevInputConfig, EvdevInputError, EvdevInputResult,
    EvdevWorkerState,
};

const POLL_INTERVAL: Duration = Duration::from_millis(8);
const READY_TIMEOUT: Duration = Duration::from_secs(1);
const STOP_TIMEOUT: Duration = Duration::from_secs(1);
const RESCAN_TICKS: u32 = 250;
const POINTER_COUNTS_PER_UNIT: f64 = 1200.0;

#[derive(Debug, Default)]
struct PendingEvents {
    events: Vec<HostInputEvent>,
}

impl PendingEvents {
    const fn new() -> Self {
        Self { events: Vec::new() }
    }

    fn push(&mut self, event: HostInputEvent) {
        self.events.push(event);
    }

    fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    fn deliver(
        &mut self,
        at_ms: u64,
        device_catalog_generation: u64,
        sink: &mut impl FnMut(HostInputBatch<'_>),
    ) -> bool {
        if self.events.is_empty() {
            return false;
        }
        sink(HostInputBatch {
            events: &self.events,
            pointer: None,
            at_ms,
            device_catalog_generation,
        });
        self.events.clear();
        true
    }
}

struct OpenDevice {
    event_state: DeviceEventState,
    device: Device,
}

struct DeviceEventState {
    descriptor: Arc<HostInputDevice>,
    capabilities: NativeCapabilities,
    relative_motion: RelativeMotionFrame,
    discard_until_report: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NativeCapabilities {
    keyboard: bool,
    pointer: bool,
    hi_res_vertical_scroll: bool,
    hi_res_horizontal_scroll: bool,
}

impl NativeCapabilities {
    const fn public(self) -> HostInputCapabilities {
        HostInputCapabilities {
            keyboard: self.keyboard,
            pointer: self.pointer,
        }
    }
}

#[derive(Debug, Default)]
struct RelativeMotionFrame {
    dx: i32,
    dy: i32,
}

impl RelativeMotionFrame {
    fn accumulate(&mut self, axis: RelativeAxisCode, value: i32) {
        match axis {
            RelativeAxisCode::REL_X => self.dx = self.dx.saturating_add(value),
            RelativeAxisCode::REL_Y => self.dy = self.dy.saturating_add(value),
            _ => {}
        }
    }

    fn take_event(&mut self, device: &Arc<HostInputDevice>) -> Option<HostInputEvent> {
        let dx = std::mem::take(&mut self.dx);
        let dy = std::mem::take(&mut self.dy);
        (dx != 0 || dy != 0).then(|| HostInputEvent::Motion {
            device: Some(Arc::clone(device)),
            motion: HostPointerMotion::Relative {
                delta_x: f64::from(dx),
                delta_y: f64::from(dy),
                units_per_x: POINTER_COUNTS_PER_UNIT,
                units_per_y: POINTER_COUNTS_PER_UNIT,
            },
        })
    }

    fn clear(&mut self) {
        self.dx = 0;
        self.dy = 0;
    }
}

struct WorkerContext {
    config: EvdevInputConfig,
    devices: BTreeMap<PathBuf, OpenDevice>,
    next_device_generation: u64,
    device_catalog_generation: u64,
    pending: PendingEvents,
    status: Arc<Mutex<Vec<DeviceOpenStatus>>>,
    device_count: Arc<AtomicUsize>,
    published_topology: Arc<AtomicU64>,
}

impl WorkerContext {
    fn new(
        config: EvdevInputConfig,
        status: Arc<Mutex<Vec<DeviceOpenStatus>>>,
        device_count: Arc<AtomicUsize>,
        published_topology: Arc<AtomicU64>,
    ) -> Self {
        Self {
            config,
            devices: BTreeMap::new(),
            next_device_generation: 1,
            device_catalog_generation: 0,
            pending: PendingEvents::new(),
            status,
            device_count,
            published_topology,
        }
    }

    fn allocate_device_generation(&mut self) -> u64 {
        let generation = self.next_device_generation;
        self.next_device_generation = self.next_device_generation.wrapping_add(1).max(1);
        generation
    }

    fn device_catalog_changed(&mut self) {
        self.device_catalog_generation = self.device_catalog_generation.wrapping_add(1);
        self.published_topology
            .store(self.device_catalog_generation, Ordering::Release);
        self.device_count
            .store(self.devices.len(), Ordering::Release);
    }

    fn deliver(&mut self, sink: &mut impl FnMut(HostInputBatch<'_>)) -> Result<(), ()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        std::panic::catch_unwind(AssertUnwindSafe(|| {
            let at_ms = (self.config.clock)();
            self.pending
                .deliver(at_ms, self.device_catalog_generation, sink);
        }))
        .map_err(|_| ())
    }
}

/// A live evdev acquisition session.
pub struct EvdevInputSession {
    stop: mpsc::Sender<()>,
    finished: mpsc::Receiver<()>,
    worker: Option<JoinHandle<()>>,
    status: Arc<Mutex<Vec<DeviceOpenStatus>>>,
    device_count: Arc<AtomicUsize>,
    device_catalog_generation: Arc<AtomicU64>,
    state: Arc<Mutex<EvdevWorkerState>>,
}

impl EvdevInputSession {
    /// Start capture and wait until the initial device scan is complete.
    pub fn start(
        config: EvdevInputConfig,
        sink: impl FnMut(HostInputBatch<'_>) + Send + 'static,
    ) -> EvdevInputResult<Self> {
        if !config.keyboard && !config.pointer {
            return Err(EvdevInputError::NothingToCapture);
        }

        let status = Arc::new(Mutex::new(Vec::new()));
        let device_count = Arc::new(AtomicUsize::new(0));
        let device_catalog_generation = Arc::new(AtomicU64::new(0));
        let state = Arc::new(Mutex::new(EvdevWorkerState::Running));
        let (stop_tx, stop_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::sync_channel(1);
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);

        let worker = spawn_worker(
            thread::Builder::new().name("hypercolor-evdev-input".to_owned()),
            {
                let status = Arc::clone(&status);
                let device_count = Arc::clone(&device_count);
                let device_catalog_generation = Arc::clone(&device_catalog_generation);
                let state = Arc::clone(&state);
                move || {
                    run_worker(
                        config,
                        sink,
                        stop_rx,
                        &ready_tx,
                        &state,
                        status,
                        device_count,
                        device_catalog_generation,
                    );
                    let _ = finished_tx.send(());
                }
            },
        )
        .map_err(|error| EvdevInputError::WorkerSpawn(error.to_string()))?;

        match ready_rx.recv_timeout(READY_TIMEOUT) {
            Ok(Ok(())) => Ok(Self {
                stop: stop_tx,
                finished: finished_rx,
                worker: Some(worker),
                status,
                device_count,
                device_catalog_generation,
                state,
            }),
            Ok(Err(error)) => {
                let _ = stop_tx.send(());
                let _ = worker.join();
                Err(error)
            }
            Err(_) => {
                let _ = stop_tx.send(());
                retain_worker(worker, "evdev input readiness timeout");
                Err(EvdevInputError::WorkerReadyTimeout)
            }
        }
    }

    /// Devices opened and currently streaming.
    #[must_use]
    pub fn device_count(&self) -> usize {
        self.device_count.load(Ordering::Acquire)
    }

    /// Current open-device topology generation.
    #[must_use]
    pub fn device_catalog_generation(&self) -> u64 {
        self.device_catalog_generation.load(Ordering::Acquire)
    }

    /// Results from the latest `/dev/input` discovery pass.
    #[must_use]
    pub fn device_status(&self) -> Vec<DeviceOpenStatus> {
        self.status
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_else(|poisoned| poisoned.into_inner().clone())
    }

    /// Liveness of the acquisition worker.
    #[must_use]
    pub fn worker_state(&self) -> EvdevWorkerState {
        if self
            .worker
            .as_ref()
            .is_some_and(std::thread::JoinHandle::is_finished)
            && let Ok(mut state) = self.state.lock()
            && matches!(*state, EvdevWorkerState::Running)
        {
            *state = EvdevWorkerState::Failed("evdev input worker exited unexpectedly".to_owned());
        }
        self.state
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_else(|poisoned| poisoned.into_inner().clone())
    }

    /// Stop capture. The call is idempotent and bounded.
    pub fn stop(&mut self) {
        let Some(worker) = self.worker.as_ref() else {
            return;
        };
        let _ = self.stop.send(());
        match self.finished.recv_timeout(STOP_TIMEOUT) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                let worker = self.worker.take().expect("live worker remains owned");
                if let Err(panic) = worker.join() {
                    set_worker_failure(
                        &self.state,
                        format!("evdev input worker panicked: {panic:?}"),
                    );
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if worker.is_finished() {
                    let worker = self.worker.take().expect("finished worker remains owned");
                    if let Err(panic) = worker.join() {
                        set_worker_failure(
                            &self.state,
                            format!("evdev input worker panicked: {panic:?}"),
                        );
                    }
                } else {
                    set_worker_failure(
                        &self.state,
                        "evdev input worker did not stop within the deadline".to_owned(),
                    );
                }
            }
        }
    }
}

impl Drop for EvdevInputSession {
    fn drop(&mut self) {
        self.stop();
        if let Some(worker) = self.worker.take() {
            let _ = self.stop.send(());
            retain_worker(worker, "evdev input session drop after stop timeout");
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "worker control and observable state have distinct owners"
)]
fn run_worker(
    config: EvdevInputConfig,
    mut sink: impl FnMut(HostInputBatch<'_>),
    stop_rx: mpsc::Receiver<()>,
    ready_tx: &mpsc::SyncSender<EvdevInputResult<()>>,
    worker_state: &Mutex<EvdevWorkerState>,
    status: Arc<Mutex<Vec<DeviceOpenStatus>>>,
    device_count: Arc<AtomicUsize>,
    device_catalog_generation: Arc<AtomicU64>,
) {
    let mut context = WorkerContext::new(config, status, device_count, device_catalog_generation);
    rescan_devices(&mut context);
    if context.deliver(&mut sink).is_err() {
        set_worker_failure(worker_state, "evdev input publication panicked".to_owned());
        let _ = ready_tx.send(Err(EvdevInputError::InitialPublicationPanicked));
        return;
    }
    let _ = ready_tx.send(Ok(()));

    let mut ticks_since_rescan = 0_u32;
    loop {
        poll_devices(&mut context);
        if context.deliver(&mut sink).is_err() {
            set_worker_failure(worker_state, "evdev input publication panicked".to_owned());
            return;
        }

        match stop_rx.recv_timeout(POLL_INTERVAL) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => return,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        ticks_since_rescan = ticks_since_rescan.saturating_add(1);
        if ticks_since_rescan >= RESCAN_TICKS {
            ticks_since_rescan = 0;
            rescan_devices(&mut context);
            if context.deliver(&mut sink).is_err() {
                set_worker_failure(worker_state, "evdev input publication panicked".to_owned());
                return;
            }
        }
    }
}

fn rescan_devices(context: &mut WorkerContext) {
    let mut statuses = Vec::new();
    let mut present = BTreeSet::new();
    let mut catalog_changed = false;
    let paths = match enumerate_event_nodes() {
        Ok(paths) => paths,
        Err(error) => {
            warn!(%error, "failed to enumerate evdev input nodes");
            mark_status_failed(&context.status, Path::new("/dev/input"), error.to_string());
            return;
        }
    };

    for path in paths {
        present.insert(path.clone());
        if let Some(open) = context.devices.get(&path) {
            statuses.push(DeviceOpenStatus {
                path,
                label: open.event_state.descriptor.label.to_string(),
                state: DeviceOpenState::Opened,
            });
            continue;
        }

        match Device::open(&path) {
            Ok(device) => {
                let capabilities =
                    classify_device(&device, context.config.keyboard, context.config.pointer);
                let label = device_label(&path, &device);
                if !capabilities.keyboard && !capabilities.pointer {
                    statuses.push(DeviceOpenStatus {
                        path,
                        label,
                        state: DeviceOpenState::Ignored,
                    });
                    continue;
                }
                if let Err(error) = device.set_nonblocking(true) {
                    statuses.push(DeviceOpenStatus {
                        path,
                        label,
                        state: DeviceOpenState::Failed(error.to_string()),
                    });
                    continue;
                }

                let device_generation = context.allocate_device_generation();
                let path_text: Arc<str> = Arc::from(path.display().to_string());
                let descriptor = Arc::new(HostInputDevice {
                    source_id: Arc::from(format!(
                        "linux:evdev:s{}:d{device_generation}:{path_text}",
                        context.config.session_generation
                    )),
                    label: Arc::from(label.clone()),
                    capabilities: capabilities.public(),
                    session_generation: context.config.session_generation,
                    device_generation,
                });
                info!(
                    device = %descriptor.label,
                    keyboard = capabilities.keyboard,
                    pointer = capabilities.pointer,
                    "opened evdev input device"
                );
                statuses.push(DeviceOpenStatus {
                    path: path.clone(),
                    label,
                    state: DeviceOpenState::Opened,
                });
                context.pending.push(HostInputEvent::DeviceArrived {
                    device: Arc::clone(&descriptor),
                });
                context.devices.insert(
                    path,
                    OpenDevice {
                        event_state: DeviceEventState {
                            descriptor,
                            capabilities,
                            relative_motion: RelativeMotionFrame::default(),
                            discard_until_report: false,
                        },
                        device,
                    },
                );
                catalog_changed = true;
            }
            Err(error) if error.kind() == ErrorKind::PermissionDenied => {
                statuses.push(DeviceOpenStatus {
                    path: path.clone(),
                    label: path.display().to_string(),
                    state: DeviceOpenState::PermissionDenied,
                });
            }
            Err(error) => {
                statuses.push(DeviceOpenStatus {
                    path: path.clone(),
                    label: path.display().to_string(),
                    state: DeviceOpenState::Failed(error.to_string()),
                });
            }
        }
    }

    let removed = context
        .devices
        .keys()
        .filter(|path| !present.contains(*path))
        .cloned()
        .collect::<Vec<_>>();
    for path in removed {
        if let Some(open) = context.devices.remove(&path) {
            debug!(device = %open.event_state.descriptor.label, "evdev input device removed");
            queue_removal(context, open.event_state.descriptor, None);
            catalog_changed = true;
        }
    }

    let denied = statuses
        .iter()
        .filter(|entry| entry.state == DeviceOpenState::PermissionDenied)
        .count();
    let opened = statuses
        .iter()
        .filter(|entry| entry.state == DeviceOpenState::Opened)
        .count();
    if denied > 0 && opened == 0 {
        warn!(
            denied,
            "evdev input nodes are unreadable; install the Hypercolor udev rules"
        );
    }
    replace_status(&context.status, statuses);
    if catalog_changed {
        context.device_catalog_changed();
    } else {
        context
            .device_count
            .store(context.devices.len(), Ordering::Release);
    }
}

fn poll_devices(context: &mut WorkerContext) {
    let mut failed = Vec::new();
    for (path, open) in &mut context.devices {
        let fetched: Result<Vec<InputEvent>, std::io::Error> =
            open.device.fetch_events().map(Iterator::collect);
        match fetched {
            Ok(events) => {
                for event in events {
                    translate_event(&mut open.event_state, event, &mut context.pending);
                }
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {}
            Err(error) => {
                warn!(device = %open.event_state.descriptor.label, %error, "evdev input device read failed");
                failed.push((path.clone(), error.to_string()));
            }
        }
    }

    if failed.is_empty() {
        return;
    }
    for (path, error) in failed {
        if let Some(open) = context.devices.remove(&path) {
            queue_removal(
                context,
                open.event_state.descriptor,
                Some(HostInputGapReason::ReadFailed),
            );
        }
        mark_status_failed(&context.status, &path, error);
    }
    context.device_catalog_changed();
}

fn translate_event(open: &mut DeviceEventState, event: InputEvent, pending: &mut PendingEvents) {
    let summary = event.destructure();
    if open.discard_until_report {
        if matches!(
            &summary,
            EventSummary::Synchronization(_, SynchronizationCode::SYN_REPORT, _)
        ) {
            open.discard_until_report = false;
        }
        return;
    }
    if matches!(
        &summary,
        EventSummary::Synchronization(_, SynchronizationCode::SYN_DROPPED, _)
    ) {
        open.relative_motion.clear();
        open.discard_until_report = true;
        pending.push(HostInputEvent::StateGap {
            device: Some(Arc::clone(&open.descriptor)),
            reason: HostInputGapReason::SynchronizationLost,
        });
        return;
    }

    match summary {
        EventSummary::Key(_, code, value) => {
            let Some((pressed, _)) = key_signal(value) else {
                trace!(device = %open.descriptor.label, ?code, value, "ignoring evdev key value");
                return;
            };
            if let Some(button) = pointer_button(code) {
                if open.capabilities.pointer {
                    pending.push(HostInputEvent::Button {
                        device: Some(Arc::clone(&open.descriptor)),
                        button,
                        pressed,
                        physical_code: Arc::from(format!("evdev:{code:?}")),
                    });
                }
            } else if open.capabilities.keyboard && !is_non_keyboard_key(code) {
                let native_name = format!("{code:?}");
                if let Some(event) =
                    normalize_evdev_key(&open.descriptor, code.0, &native_name, value)
                {
                    pending.push(event);
                }
            }
        }
        EventSummary::RelativeAxis(_, axis, value) => {
            if !open.capabilities.pointer {
                return;
            }
            match axis {
                RelativeAxisCode::REL_X | RelativeAxisCode::REL_Y => {
                    open.relative_motion.accumulate(axis, value);
                }
                RelativeAxisCode::REL_WHEEL_HI_RES => {
                    queue_scroll(
                        pending,
                        &open.descriptor,
                        0,
                        i64::from(value) << 16,
                        "evdev:REL_WHEEL_HI_RES",
                    );
                }
                RelativeAxisCode::REL_HWHEEL_HI_RES => {
                    queue_scroll(
                        pending,
                        &open.descriptor,
                        i64::from(value) << 16,
                        0,
                        "evdev:REL_HWHEEL_HI_RES",
                    );
                }
                RelativeAxisCode::REL_WHEEL if !open.capabilities.hi_res_vertical_scroll => {
                    queue_scroll(
                        pending,
                        &open.descriptor,
                        0,
                        (i64::from(value) * 120) << 16,
                        "evdev:REL_WHEEL",
                    );
                }
                RelativeAxisCode::REL_HWHEEL if !open.capabilities.hi_res_horizontal_scroll => {
                    queue_scroll(
                        pending,
                        &open.descriptor,
                        (i64::from(value) * 120) << 16,
                        0,
                        "evdev:REL_HWHEEL",
                    );
                }
                _ => {}
            }
        }
        EventSummary::Synchronization(_, SynchronizationCode::SYN_REPORT, _) => {
            if let Some(event) = open.relative_motion.take_event(&open.descriptor) {
                pending.push(event);
            }
        }
        _ => {}
    }
}

fn queue_scroll(
    pending: &mut PendingEvents,
    device: &Arc<HostInputDevice>,
    delta_x_q16_16: i64,
    delta_y_q16_16: i64,
    physical_code: &'static str,
) {
    pending.push(HostInputEvent::Scroll {
        device: Some(Arc::clone(device)),
        delta_x_q16_16,
        delta_y_q16_16,
        unit: PointerScrollUnit::Line120,
        phase: PointerScrollPhase::None,
        momentum_phase: PointerScrollPhase::None,
        physical_code: Arc::from(physical_code),
    });
}

fn queue_removal(
    context: &mut WorkerContext,
    device: Arc<HostInputDevice>,
    reason: Option<HostInputGapReason>,
) {
    if let Some(reason) = reason {
        context.pending.push(HostInputEvent::StateGap {
            device: Some(Arc::clone(&device)),
            reason,
        });
    }
    context
        .pending
        .push(HostInputEvent::DeviceRemoved { device });
}

fn enumerate_event_nodes() -> std::io::Result<Vec<PathBuf>> {
    let entries = std::fs::read_dir("/dev/input")?.map(|entry| entry.map(|entry| entry.path()));
    collect_event_nodes(entries)
}

fn collect_event_nodes(
    entries: impl IntoIterator<Item = std::io::Result<PathBuf>>,
) -> std::io::Result<Vec<PathBuf>> {
    let mut nodes = Vec::new();
    for path in entries {
        let path = path?;
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("event"))
        {
            nodes.push(path);
        }
    }
    nodes.sort();
    Ok(nodes)
}

fn classify_device(device: &Device, keyboard: bool, pointer: bool) -> NativeCapabilities {
    classify_capabilities(
        device.supported_keys(),
        device.supported_relative_axes(),
        keyboard,
        pointer,
    )
}

fn classify_capabilities(
    keys: Option<&AttributeSetRef<KeyCode>>,
    axes: Option<&AttributeSetRef<RelativeAxisCode>>,
    keyboard: bool,
    pointer: bool,
) -> NativeCapabilities {
    let looks_like_keyboard = keys.is_some_and(|keys| {
        keys.iter()
            .any(|code| pointer_button(code).is_none() && !is_non_keyboard_key(code))
    });
    let looks_like_pointer = axes.is_some_and(|axes| {
        axes.contains(RelativeAxisCode::REL_X) && axes.contains(RelativeAxisCode::REL_Y)
    }) && keys.is_some_and(|keys| keys.contains(KeyCode::BTN_LEFT));
    NativeCapabilities {
        keyboard: keyboard && looks_like_keyboard,
        pointer: pointer && looks_like_pointer,
        hi_res_vertical_scroll: axes
            .is_some_and(|axes| axes.contains(RelativeAxisCode::REL_WHEEL_HI_RES)),
        hi_res_horizontal_scroll: axes
            .is_some_and(|axes| axes.contains(RelativeAxisCode::REL_HWHEEL_HI_RES)),
    }
}

fn device_label(path: &Path, device: &Device) -> String {
    device.name().map_or_else(
        || path.display().to_string(),
        |name| format!("{name} ({})", path.display()),
    )
}

const fn key_signal(value: i32) -> Option<(bool, HostRepeatEvidence)> {
    match value {
        0 => Some((false, HostRepeatEvidence::NotRepeat)),
        1 => Some((true, HostRepeatEvidence::NotRepeat)),
        2 => Some((true, HostRepeatEvidence::Repeat)),
        _ => None,
    }
}

fn pointer_button(code: KeyCode) -> Option<HostPointerButton> {
    match code {
        KeyCode::BTN_LEFT => Some(HostPointerButton::left()),
        KeyCode::BTN_RIGHT => Some(HostPointerButton::right()),
        KeyCode::BTN_MIDDLE => Some(HostPointerButton::middle()),
        KeyCode::BTN_SIDE => Some(HostPointerButton::side()),
        KeyCode::BTN_EXTRA => Some(HostPointerButton::extra()),
        _ => None,
    }
}

const fn is_non_keyboard_key(code: KeyCode) -> bool {
    let code = code.0;
    (code >= 0x100 && code < 0x160)
        || (code >= 0x220 && code < 0x228)
        || (code >= 0x2c0 && code < 0x2e0)
}

fn replace_status(status: &Mutex<Vec<DeviceOpenStatus>>, replacement: Vec<DeviceOpenStatus>) {
    match status.lock() {
        Ok(mut guard) => *guard = replacement,
        Err(poisoned) => *poisoned.into_inner() = replacement,
    }
}

fn mark_status_failed(status: &Mutex<Vec<DeviceOpenStatus>>, path: &Path, error: String) {
    let mut guard = status
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(entry) = guard.iter_mut().find(|entry| entry.path == path) {
        entry.state = DeviceOpenState::Failed(error);
    } else {
        guard.push(DeviceOpenStatus {
            path: path.to_path_buf(),
            label: path.display().to_string(),
            state: DeviceOpenState::Failed(error),
        });
    }
}

fn set_worker_failure(state: &Mutex<EvdevWorkerState>, failure: String) {
    match state.lock() {
        Ok(mut guard) => *guard = EvdevWorkerState::Failed(failure),
        Err(poisoned) => *poisoned.into_inner() = EvdevWorkerState::Failed(failure),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use evdev::{AttributeSet, EventType};
    use hypercolor_types::host_input::{HostKeyIdentity, HostKeySignal};

    fn descriptor() -> Arc<HostInputDevice> {
        Arc::new(HostInputDevice {
            source_id: Arc::from("linux:evdev:s7:d1:/dev/input/event0"),
            label: Arc::from("fixture"),
            capabilities: HostInputCapabilities {
                keyboard: true,
                pointer: true,
            },
            session_generation: 7,
            device_generation: 1,
        })
    }

    fn event_state(capabilities: NativeCapabilities) -> DeviceEventState {
        DeviceEventState {
            descriptor: descriptor(),
            capabilities,
            relative_motion: RelativeMotionFrame::default(),
            discard_until_report: false,
        }
    }

    fn relative_event(axis: RelativeAxisCode, value: i32) -> InputEvent {
        InputEvent::new(EventType::RELATIVE.0, axis.0, value)
    }

    fn key_event(code: KeyCode, value: i32) -> InputEvent {
        InputEvent::new(EventType::KEY.0, code.0, value)
    }

    fn sync_event(code: SynchronizationCode) -> InputEvent {
        InputEvent::new(EventType::SYNCHRONIZATION.0, code.0, 0)
    }

    #[test]
    fn event_node_enumeration_propagates_entry_failures() {
        let entries = [
            Ok(PathBuf::from("/dev/input/event2")),
            Err(std::io::Error::new(
                ErrorKind::PermissionDenied,
                "fixture entry denied",
            )),
            Ok(PathBuf::from("/dev/input/event1")),
        ];

        let error = collect_event_nodes(entries).expect_err("entry failure must remain visible");
        assert_eq!(error.kind(), ErrorKind::PermissionDenied);
    }

    #[test]
    fn delivery_contains_clock_panics() {
        let config = EvdevInputConfig {
            keyboard: true,
            pointer: true,
            session_generation: 7,
            clock: Arc::new(|| panic!("fixture clock panic")),
        };
        let mut context = WorkerContext::new(
            config,
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicU64::new(0)),
        );
        context.pending.push(HostInputEvent::DeviceArrived {
            device: descriptor(),
        });

        assert!(context.deliver(&mut |_| {}).is_err());
    }

    #[test]
    fn idle_delivery_does_not_sample_the_clock() {
        let config = EvdevInputConfig {
            keyboard: true,
            pointer: true,
            session_generation: 7,
            clock: Arc::new(|| panic!("idle delivery must not sample the clock")),
        };
        let mut context = WorkerContext::new(
            config,
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicU64::new(0)),
        );

        assert!(context.deliver(&mut |_| {}).is_ok());
    }

    #[test]
    fn media_only_nodes_are_keyboard_capable() {
        let keys = [KeyCode::KEY_VOLUMEUP]
            .into_iter()
            .collect::<AttributeSet<_>>();
        let capabilities = classify_capabilities(Some(&keys), None, true, true);
        assert!(capabilities.keyboard);
        assert!(!capabilities.pointer);
    }

    #[test]
    fn motion_flushes_only_for_its_own_syn_report() {
        let capabilities = NativeCapabilities {
            keyboard: false,
            pointer: true,
            hi_res_vertical_scroll: false,
            hi_res_horizontal_scroll: false,
        };
        let mut first = event_state(capabilities);
        let mut second = event_state(capabilities);
        second.descriptor = Arc::new(HostInputDevice {
            source_id: Arc::from("linux:evdev:s7:d2:/dev/input/event1"),
            label: Arc::from("fixture-2"),
            capabilities: capabilities.public(),
            session_generation: 7,
            device_generation: 2,
        });
        let mut pending = PendingEvents::new();

        translate_event(
            &mut first,
            relative_event(RelativeAxisCode::REL_X, 3),
            &mut pending,
        );
        translate_event(
            &mut second,
            relative_event(RelativeAxisCode::REL_X, 12),
            &mut pending,
        );
        translate_event(
            &mut first,
            relative_event(RelativeAxisCode::REL_Y, 4),
            &mut pending,
        );
        assert!(pending.is_empty());

        translate_event(
            &mut first,
            sync_event(SynchronizationCode::SYN_REPORT),
            &mut pending,
        );
        let mut captured = Vec::new();
        pending.deliver(1, 1, &mut |batch| {
            captured.extend_from_slice(batch.events);
        });
        let [
            HostInputEvent::Motion {
                motion:
                    HostPointerMotion::Relative {
                        delta_x: 3.0,
                        delta_y: 4.0,
                        units_per_x,
                        units_per_y,
                    },
                ..
            },
        ] = captured.as_slice()
        else {
            panic!("SYN_REPORT must emit one relative-motion event");
        };
        assert_eq!((*units_per_x, *units_per_y), (1200.0, 1200.0));
    }

    #[test]
    fn dropped_synchronization_emits_gap_and_discards_partial_motion() {
        let capabilities = NativeCapabilities {
            keyboard: false,
            pointer: true,
            hi_res_vertical_scroll: false,
            hi_res_horizontal_scroll: false,
        };
        let mut open = event_state(capabilities);
        let mut pending = PendingEvents::new();
        translate_event(
            &mut open,
            relative_event(RelativeAxisCode::REL_X, 9),
            &mut pending,
        );
        translate_event(
            &mut open,
            sync_event(SynchronizationCode::SYN_DROPPED),
            &mut pending,
        );
        translate_event(
            &mut open,
            relative_event(RelativeAxisCode::REL_Y, 5),
            &mut pending,
        );
        translate_event(
            &mut open,
            sync_event(SynchronizationCode::SYN_REPORT),
            &mut pending,
        );

        let mut captured = Vec::new();
        pending.deliver(1, 1, &mut |batch| {
            captured.extend_from_slice(batch.events);
        });
        assert!(matches!(
            captured.as_slice(),
            [HostInputEvent::StateGap {
                reason: HostInputGapReason::SynchronizationLost,
                ..
            }]
        ));
    }

    #[test]
    fn high_resolution_scroll_suppresses_legacy_axis_shadow() {
        let capabilities = NativeCapabilities {
            keyboard: false,
            pointer: true,
            hi_res_vertical_scroll: true,
            hi_res_horizontal_scroll: false,
        };
        let mut open = event_state(capabilities);
        let mut pending = PendingEvents::new();
        translate_event(
            &mut open,
            relative_event(RelativeAxisCode::REL_WHEEL, 1),
            &mut pending,
        );
        translate_event(
            &mut open,
            relative_event(RelativeAxisCode::REL_WHEEL_HI_RES, 30),
            &mut pending,
        );
        let mut captured = Vec::new();
        pending.deliver(1, 1, &mut |batch| {
            captured.extend_from_slice(batch.events);
        });
        assert!(matches!(
            captured.as_slice(),
            [HostInputEvent::Scroll {
                delta_x_q16_16: 0,
                delta_y_q16_16,
                unit: PointerScrollUnit::Line120,
                physical_code,
                ..
            }] if *delta_y_q16_16 == 30_i64 << 16
                && &**physical_code == "evdev:REL_WHEEL_HI_RES"
        ));
    }

    #[test]
    fn evdev_key_fixture_normalizes_name_identity_and_repeat_evidence() {
        let capabilities = NativeCapabilities {
            keyboard: true,
            pointer: false,
            hi_res_vertical_scroll: false,
            hi_res_horizontal_scroll: false,
        };
        let mut open = event_state(capabilities);
        let mut pending = PendingEvents::new();

        translate_event(&mut open, key_event(KeyCode::KEY_A, 1), &mut pending);
        translate_event(&mut open, key_event(KeyCode::KEY_A, 2), &mut pending);
        translate_event(&mut open, key_event(KeyCode::KEY_A, 0), &mut pending);

        let mut captured = Vec::new();
        pending.deliver(9, 3, &mut |batch| {
            assert_eq!(batch.device_catalog_generation, 3);
            captured.extend_from_slice(batch.events);
        });
        assert_eq!(captured.len(), 3);
        for event in &captured {
            assert!(matches!(
                event,
                HostInputEvent::Key {
                    identity: HostKeyIdentity { key, physical_code },
                    ..
                } if &**key == "a" && &**physical_code == "evdev:KEY_A"
            ));
        }
        assert!(matches!(
            captured[0],
            HostInputEvent::Key {
                signal: HostKeySignal::Edge {
                    pressed: true,
                    repeat: HostRepeatEvidence::NotRepeat,
                },
                ..
            }
        ));
        assert!(matches!(
            captured[1],
            HostInputEvent::Key {
                signal: HostKeySignal::Edge {
                    pressed: true,
                    repeat: HostRepeatEvidence::Repeat,
                },
                ..
            }
        ));
        assert!(matches!(
            captured[2],
            HostInputEvent::Key {
                signal: HostKeySignal::Edge {
                    pressed: false,
                    repeat: HostRepeatEvidence::NotRepeat,
                },
                ..
            }
        ));
    }
}
