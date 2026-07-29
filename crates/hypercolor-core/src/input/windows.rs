//! Windows host input capture, folded from Raw Input batches.
//!
//! Structurally a sibling of [`crate::input::evdev`]: the same `SharedState`,
//! the same per-`source_id` held sets unioned across devices, the same
//! release synthesis when a device vanishes, the same overflow accounting.
//! Differences are confined to the event source and the pointer model.
//!
//! Two of those differences are worth stating up front.
//!
//! **Repeat is derived here, not reported.** evdev has three button states and
//! excludes `Repeated` from `recent_keys`, so a held key fires `onKeyPress`
//! once rather than sixty times a second. Windows delivers auto-repeat as
//! ordinary make codes with no repeat marker, so a press whose key is already
//! in that source's held set is classified as `Repeated` here. Without this,
//! holding a key would machine-gun every recents-driven effect on Windows and
//! not on Linux.
//!
//! **The pointer is absolute.** Linux accumulates a virtual cursor from
//! relative counts because Wayland will not tell an unfocused client where the
//! pointer is. Windows will, so this reports `PointerMode::Absolute` with real
//! signed virtual-desktop pixels — and motion still comes from the raw relative
//! deltas rather than from differencing the sampled cursor, because that is
//! sub-frame accurate and survives the cursor hitting a screen edge.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use hypercolor_windows_input::{
    RawButton, RawCursor, RawDeviceDescriptor, RawDeviceKind, RawInputBatch, RawInputConfig,
    RawInputError, RawInputEvent, RawInputSession, SessionState, WorkerState,
    interactive_session_state,
};
use tracing::{debug, info, warn};

use crate::input::keymap::{KeyNameResult, scancode_key_name};
use crate::input::traits::{
    InputData, InputSource, InteractionData, InteractionDegradation, MotionAggregate, PointerMode,
};
use crate::input::{
    SourceIssue, SourceKind, SourceSessionSlot, SourceStatusHandle, SourceStatusReporter,
    TerminalFailureLatch,
};
use crate::types::event::{InputButtonState, InputEvent, TimedInputEvent};

const DEFAULT_EVENT_LIMIT: usize = 256;

/// Virtual cursor travel per relative count, identical to the Linux backend so
/// velocity means the same thing on both platforms.
const CURSOR_COUNTS_PER_UNIT: f32 = 1200.0;

/// Fingerprint of the held state visible to effects, used to decide when the
/// snapshot generation must advance.
///
/// Includes the pixel coordinates, unlike the Linux key. On a wide virtual
/// desktop several distinct pixel positions collapse into one 1/10,000
/// normalized bucket, so a cursor move that changes JS-visible `x`/`y` could
/// otherwise leave the generation unchanged and be skipped by
/// `is_dirty_against`.
type HeldStateKey = (Vec<String>, Vec<String>, i32, i32, i32, i32, bool);

/// Per-device absolute pointer baseline.
///
/// Absolute devices report position rather than deltas, so motion is the
/// difference between successive normalized positions. The baseline resets on
/// arrival, removal, and any coordinate-space change, and the first report
/// after a reset emits no delta — otherwise a device appearing at the far
/// corner would report a full-screen sweep it never made.
#[derive(Debug, Clone, Copy)]
struct AbsoluteBaseline {
    norm_x: f32,
    norm_y: f32,
    /// Which rect the device's raw range covered when this baseline was taken.
    virtual_desktop: bool,
}

#[derive(Default)]
struct SharedState {
    events: VecDeque<TimedInputEvent>,
    dropped: u32,
    pressed_keys: BTreeMap<String, BTreeSet<String>>,
    recent_keys: VecDeque<String>,
    held_buttons: BTreeMap<String, BTreeSet<String>>,
    motion: MotionAggregate,
    cursor: Option<RawCursor>,
    pointer_present: bool,
    devices: BTreeMap<String, DeviceEntry>,
    absolute_baselines: BTreeMap<String, AbsoluteBaseline>,
    /// Batches stamped with any other epoch are inert. See [`WindowsHostInput`].
    epoch: u64,
}

#[derive(Debug, Clone)]
struct DeviceEntry {
    descriptor: Arc<RawDeviceDescriptor>,
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
        self.cursor = None;
        self.pointer_present = false;
        self.devices.clear();
        self.absolute_baselines.clear();
    }

    fn pointer_devices(&self) -> bool {
        self.devices
            .values()
            .any(|entry| entry.descriptor.kind == RawDeviceKind::Mouse)
    }
}

/// Allocates the epoch each capture session is tagged with.
static NEXT_EPOCH: AtomicU64 = AtomicU64::new(1);

/// Windows host input source: keyboards and pointers, via Raw Input.
///
/// Capture is demand-driven: no window is created and no registration is taken
/// while [`set_interaction_capture_active`](InputSource::set_interaction_capture_active)
/// is off, and all held state clears when capture stops so no key or button
/// ever sticks.
pub struct WindowsHostInput {
    name: String,
    running: bool,
    capture_active: bool,
    capture_keyboard: bool,
    capture_pointer: bool,
    event_limit: usize,
    generation: u64,
    last_state_key: Option<HeldStateKey>,
    shared: Arc<Mutex<SharedState>>,
    session: Option<RawInputSession>,
    worker_failure: TerminalFailureLatch,
    degraded: Option<InteractionDegradation>,
    status: SourceStatusReporter,
    status_session: SourceSessionSlot,
    #[cfg(all(target_os = "windows", feature = "windows-capture-fixtures"))]
    fixture: Option<Arc<WindowsHostInputFixtureState>>,
}

#[cfg(all(target_os = "windows", feature = "windows-capture-fixtures"))]
struct WindowsHostInputFixtureState {
    active_epoch: Mutex<Option<u64>>,
}

/// Deterministic publisher at the Windows Raw Input adapter boundary.
#[cfg(all(target_os = "windows", feature = "windows-capture-fixtures"))]
#[doc(hidden)]
pub struct WindowsHostInputFixture {
    state: Arc<WindowsHostInputFixtureState>,
    shared: Arc<Mutex<SharedState>>,
    status_session: SourceSessionSlot,
    event_limit: usize,
}

#[cfg(all(target_os = "windows", feature = "windows-capture-fixtures"))]
impl WindowsHostInputFixture {
    /// Whether daemon demand has activated the deterministic source.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.state
            .active_epoch
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some()
    }

    /// Fold one post-adapter Raw Input batch through the production source.
    ///
    /// # Errors
    ///
    /// Returns an error while daemon demand leaves the source inactive.
    pub fn publish(
        &self,
        events: &[RawInputEvent],
        cursor: Option<RawCursor>,
        at_ms: u64,
    ) -> anyhow::Result<bool> {
        let epoch = self
            .state
            .active_epoch
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .ok_or_else(|| anyhow::anyhow!("deterministic Windows Raw Input source is inactive"))?;
        Ok(publish_raw_input_batch(
            &self.shared,
            &self.status_session,
            RawInputBatch {
                events,
                cursor,
                at_ms,
                epoch,
            },
            self.event_limit,
        ))
    }
}

impl WindowsHostInput {
    /// Create a host input source capturing the enabled device kinds.
    #[must_use]
    pub fn new(capture_keyboard: bool, capture_pointer: bool) -> Self {
        Self {
            name: "WindowsHostInput".to_owned(),
            running: false,
            capture_active: false,
            capture_keyboard,
            capture_pointer,
            event_limit: DEFAULT_EVENT_LIMIT,
            generation: 0,
            last_state_key: None,
            shared: Arc::new(Mutex::new(SharedState::default())),
            session: None,
            worker_failure: TerminalFailureLatch::default(),
            degraded: None,
            status: SourceStatusReporter::new(
                "windows_host_input",
                SourceKind::Interaction,
                "raw_input",
                true,
                true,
                false,
            ),
            status_session: SourceSessionSlot::new(),
            #[cfg(all(target_os = "windows", feature = "windows-capture-fixtures"))]
            fixture: None,
        }
    }

    /// Create a source whose post-adapter batches are supplied deterministically.
    ///
    /// The source retains production epoch rotation, folding, sampling, demand,
    /// status, and event publication. It never creates a Raw Input window or
    /// claims physical devices that were not present in a published batch.
    #[cfg(all(target_os = "windows", feature = "windows-capture-fixtures"))]
    #[doc(hidden)]
    #[must_use]
    pub fn new_deterministic_fixture(
        capture_keyboard: bool,
        capture_pointer: bool,
    ) -> (Self, WindowsHostInputFixture) {
        let mut source = Self::new(capture_keyboard, capture_pointer);
        let state = Arc::new(WindowsHostInputFixtureState {
            active_epoch: Mutex::new(None),
        });
        source.fixture = Some(Arc::clone(&state));
        let fixture = WindowsHostInputFixture {
            state,
            shared: Arc::clone(&source.shared),
            status_session: source.status_session.clone(),
            event_limit: source.event_limit,
        };
        (source, fixture)
    }

    /// Devices registered, identified, and streaming.
    ///
    /// Reports opened-and-streaming rather than merely present: the UI and MCP
    /// read this field as health, so a structurally deaf backend must not look
    /// healthy. Zero whenever the session probe failed or the worker is down.
    #[must_use]
    pub fn device_count(&self) -> usize {
        if self.degraded.is_some() {
            return 0;
        }
        #[cfg(all(target_os = "windows", feature = "windows-capture-fixtures"))]
        if self.fixture.as_ref().is_some_and(|fixture| {
            fixture
                .active_epoch
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_some()
        }) {
            return shared_device_count(&self.shared);
        }
        self.session
            .as_ref()
            .filter(|session| session.worker_state() == WorkerState::Running)
            .map_or(0, RawInputSession::device_count)
    }

    /// Why input is not flowing, when it is not.
    #[must_use]
    pub fn degradation(&self) -> Option<InteractionDegradation> {
        self.degraded.clone()
    }

    /// The epoch batches must carry to be accepted.
    #[must_use]
    pub fn epoch(&self) -> u64 {
        self.shared.lock().map_or(0, |guard| guard.epoch)
    }

    /// Fold one batch and take the resulting frame, the same two steps the live
    /// pipeline performs — the sink calls the first, the render thread the
    /// second.
    ///
    /// Exposed because everything interesting here is a pure state machine
    /// that a live Raw Input session makes *harder* to test, not easier:
    /// repeat classification across a capture toggle, held-state union across
    /// two keyboards, release synthesis on removal, absolute baselines, and
    /// epoch rejection are all invisible to someone eyeballing a dump of real
    /// input. Driving them directly is also what lets Linux CI cover them.
    pub fn fold_and_snapshot(
        &mut self,
        batch: RawInputBatch<'_>,
    ) -> (InteractionData, Vec<TimedInputEvent>) {
        publish_raw_input_batch(&self.shared, &self.status_session, batch, self.event_limit);
        let shared = Arc::clone(&self.shared);
        let Ok(mut guard) = shared.lock() else {
            return (InteractionData::default(), Vec::new());
        };
        let events = drain_events(&mut guard.events);
        let snapshot = self.build_snapshot(&mut guard);
        (snapshot, events)
    }

    fn build_snapshot(&mut self, guard: &mut SharedState) -> InteractionData {
        let mut data = InteractionData::default();
        data.keyboard.pressed_keys = guard.union_pressed();
        data.keyboard.recent_keys = guard.recent_keys.drain(..).collect();
        data.mouse.buttons = guard.union_buttons();
        data.mouse.down = !data.mouse.buttons.is_empty();

        let pointer_present = guard.pointer_present;
        if pointer_present && let Some(cursor) = guard.cursor {
            data.mouse.mode = PointerMode::Absolute;
            data.mouse.x = cursor.x;
            data.mouse.y = cursor.y;
            data.mouse.norm_x = cursor.norm_x;
            data.mouse.norm_y = cursor.norm_y;
        }
        data.batch.motion = std::mem::take(&mut guard.motion);
        data.batch.dropped_events = std::mem::take(&mut guard.dropped);

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
            data.mouse.x,
            data.mouse.y,
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

    fn rotate_epoch_and_clear(&mut self) -> u64 {
        let epoch = NEXT_EPOCH.fetch_add(1, Ordering::Relaxed);
        let mut guard = self
            .shared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.clear_live_state();
        guard.epoch = epoch;
        drop(guard);
        self.last_state_key = None;
        epoch
    }

    /// Infallible by design.
    ///
    /// Every way host capture can fail on Windows — no visible window station,
    /// a stolen registration, a window that would not create — is a *degraded
    /// backend state*, never a start error. Start errors roll back the entire
    /// input graph, and a daemon that cannot see keystrokes should still light
    /// up; it just says why it cannot in diagnostics.
    fn start_session(&mut self) {
        if self.session.is_some() {
            return;
        }
        self.worker_failure.reset();

        // A failed probe is a degraded backend state, never a start error:
        // start errors roll back the entire input graph, and a daemon running
        // as a service should still light up, just without host input.
        if interactive_session_state() == SessionState::NoInteractiveSession {
            warn!(
                source = %self.name,
                "no visible window station; Raw Input cannot observe input. \
                 Run the foreground daemon in your own desktop session."
            );
            self.degraded = Some(InteractionDegradation::NoInteractiveSession);
            if let Some(status) = self.status.session() {
                status.unavailable(
                    SourceIssue::new(
                        InteractionDegradation::NoInteractiveSession.code(),
                        "Raw Input requires an interactive desktop session",
                        true,
                    )
                    .with_remediation(
                        "run the foreground daemon inside the signed-in desktop session",
                    ),
                );
            }
            return;
        }

        // Advanced atomically with clearing held state, so a batch still in
        // flight from the previous session cannot repopulate what was cleared.
        let epoch = self.rotate_epoch_and_clear();

        #[cfg(all(target_os = "windows", feature = "windows-capture-fixtures"))]
        if let Some(fixture) = self.fixture.as_ref() {
            *fixture
                .active_epoch
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(epoch);
            self.degraded = None;
            if let Some(status) = self.status_session.load() {
                status.mark_event_driven_live_without_deadline(shared_device_count(&self.shared));
            }
            return;
        }

        let shared = Arc::clone(&self.shared);
        let status_session = self.status_session.clone();
        let event_limit = self.event_limit;
        let config = RawInputConfig {
            keyboard: self.capture_keyboard,
            mouse: self.capture_pointer,
            clock: Arc::new(crate::input::input_mono_ms),
            epoch,
        };

        match RawInputSession::start(config, move |batch| {
            publish_raw_input_batch(&shared, &status_session, batch, event_limit);
        }) {
            Ok(session) => {
                info!(
                    source = %self.name,
                    devices = session.device_count(),
                    keyboard = self.capture_keyboard,
                    pointer = self.capture_pointer,
                    "Started Windows Raw Input capture"
                );
                self.degraded = None;
                self.session = Some(session);
                if let Some(status) = self.status.session() {
                    status.mark_event_driven_live_without_deadline(self.device_count());
                }
            }
            Err(error) => {
                self.rotate_epoch_and_clear();
                if matches!(&error, RawInputError::NoInteractiveSession) {
                    self.degraded = Some(InteractionDegradation::NoInteractiveSession);
                    if let Some(status) = self.status.session() {
                        status.unavailable(SourceIssue::new(
                            InteractionDegradation::NoInteractiveSession.code(),
                            "Raw Input requires an interactive desktop session",
                            true,
                        ));
                    }
                } else {
                    warn!(source = %self.name, %error, "Windows Raw Input capture unavailable");
                    self.degraded = Some(InteractionDegradation::Unavailable(error.to_string()));
                    if let Some(status) = self.status.session() {
                        status.unavailable(SourceIssue::new(
                            "windows_raw_input_unavailable",
                            error.to_string(),
                            true,
                        ));
                    }
                }
            }
        }
    }

    fn stop_session(&mut self) {
        #[cfg(all(target_os = "windows", feature = "windows-capture-fixtures"))]
        if let Some(fixture) = self.fixture.as_ref() {
            *fixture
                .active_epoch
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        }
        if let Some(mut session) = self.session.take() {
            session.stop();
        }
        // Advancing the epoch here is what makes a detached pump inert: it may
        // still hold a batch and sees the rotated epoch at the shared lock.
        self.rotate_epoch_and_clear();
        self.worker_failure.reset();
    }

    fn publish_worker_failure_once(&mut self) -> bool {
        if self.worker_failure.is_latched() {
            return true;
        }
        let failure = self.worker_failure.take(|| {
            self.session
                .as_ref()
                .and_then(|session| match session.worker_state() {
                    WorkerState::Running => None,
                    WorkerState::Failed(reason) => Some(reason),
                })
        });
        let Some(reason) = failure else {
            return false;
        };
        self.degraded = Some(InteractionDegradation::Unavailable(reason.clone()));
        if let Some(status) = self.status.session() {
            status.failed(SourceIssue::new(
                "windows_raw_input_worker_failed",
                reason,
                true,
            ));
        }
        true
    }

    fn capture_session_active(&self) -> bool {
        if self.session.is_some() {
            return true;
        }
        #[cfg(all(target_os = "windows", feature = "windows-capture-fixtures"))]
        {
            self.fixture.as_ref().is_some_and(|fixture| {
                fixture
                    .active_epoch
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .is_some()
            })
        }
        #[cfg(not(all(target_os = "windows", feature = "windows-capture-fixtures")))]
        false
    }
}

impl InputSource for WindowsHostInput {
    fn name(&self) -> &str {
        &self.name
    }

    fn start(&mut self) -> anyhow::Result<()> {
        if self.running {
            return Ok(());
        }
        if self.capture_active {
            if let Some(session) = self.status.begin_session()? {
                self.status_session.store(session);
            }
            self.start_session();
        }
        self.running = true;
        Ok(())
    }

    fn stop(&mut self) {
        self.status_session.clear();
        self.status.stop();
        self.stop_session();
        self.running = false;
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        if self.publish_worker_failure_once() {
            return Ok(InputData::None);
        }
        if !self.running || !self.capture_session_active() {
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
        if self.publish_worker_failure_once() {
            return (Ok(InputData::None), Vec::new());
        }
        if !self.running || !self.capture_session_active() {
            return (Ok(InputData::None), Vec::new());
        }
        let shared = Arc::clone(&self.shared);
        let Ok(mut guard) = shared.lock() else {
            return (Ok(InputData::None), Vec::new());
        };
        // One lock for both: an edge can never land in the frame's event batch
        // while missing from the same frame's held-state snapshot.
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
        let worker_failure =
            self.session
                .as_ref()
                .and_then(|session| match session.worker_state() {
                    WorkerState::Running => None,
                    WorkerState::Failed(reason) => {
                        Some(InteractionDegradation::Unavailable(reason))
                    }
                });
        Some(crate::input::InteractionDiagnostics {
            backend: "raw_input",
            host_capture: true,
            capturing: self.capture_active
                && self.capture_session_active()
                && worker_failure.is_none(),
            devices_opened: self.device_count(),
            // There is no per-device denial on Windows to count: capture is a
            // session-level capability, and D1's elevated windows and secure
            // desktop stay invisible with no signal that they were skipped.
            devices_denied: 0,
            degraded: worker_failure.or_else(|| self.degraded.clone()),
        })
    }

    fn set_interaction_capture_active(&mut self, active: bool) -> anyhow::Result<()> {
        self.status.set_policy(true, true, active)?;
        if self.capture_active == active {
            return Ok(());
        }
        self.capture_active = active;
        if !self.running {
            return Ok(());
        }
        if active {
            if let Some(session) = self.status.begin_session()? {
                self.status_session.store(session);
            }
            self.start_session();
        } else {
            self.status_session.clear();
            self.stop_session();
        }
        Ok(())
    }

    fn drain_events(&mut self) -> Vec<TimedInputEvent> {
        if self.publish_worker_failure_once() {
            return Vec::new();
        }
        if let Ok(mut guard) = self.shared.lock() {
            return drain_events(&mut guard.events);
        }
        Vec::new()
    }
}

// ── Folding ────────────────────────────────────────────────────────────────

/// Fold one drain into shared state, under one lock.
///
/// Runs on the pump thread, exactly as `poll_devices` folds on the Linux poll
/// thread. One lock per drain rather than per event: at 8 kHz the per-event
/// shape would mean 8000 lock round-trips a second contending with the render
/// thread for this same mutex.
fn publish_raw_input_batch(
    shared: &Arc<Mutex<SharedState>>,
    status_session: &SourceSessionSlot,
    batch: RawInputBatch<'_>,
    event_limit: usize,
) -> bool {
    let Some(result) = fold_batch(shared, batch, event_limit) else {
        return false;
    };
    if let Some(resource_count) = result.resource_count
        && let Some(status) = status_session.load()
    {
        status.mark_event_driven_live_without_deadline(resource_count);
    }
    true
}

#[cfg(all(target_os = "windows", feature = "windows-capture-fixtures"))]
fn shared_device_count(shared: &Arc<Mutex<SharedState>>) -> usize {
    shared.lock().map_or(0, |state| state.devices.len())
}

struct FoldBatchResult {
    resource_count: Option<usize>,
}

fn fold_batch(
    shared: &Arc<Mutex<SharedState>>,
    batch: RawInputBatch<'_>,
    event_limit: usize,
) -> Option<FoldBatchResult> {
    let Ok(mut guard) = shared.lock() else {
        return None;
    };

    // Checked here, under the lock and immediately before mutation — not on
    // entry. Checking before acquiring the lock would race exactly the restart
    // it guards: a detached pump could pass the check, block on the mutex, and
    // wake after the epoch had rotated.
    if guard.epoch != batch.epoch {
        return None;
    }

    if let Some(cursor) = batch.cursor {
        guard.cursor = Some(cursor);
    }

    let mut resources_changed = false;
    for event in batch.events {
        resources_changed |= matches!(
            event,
            RawInputEvent::DeviceArrived { .. } | RawInputEvent::DeviceRemoved { .. }
        );
        fold_event(&mut guard, event, batch.at_ms, event_limit);
    }

    guard.pointer_present = guard.pointer_devices();
    Some(FoldBatchResult {
        resource_count: resources_changed.then_some(guard.devices.len()),
    })
}

fn fold_event(state: &mut SharedState, event: &RawInputEvent, at_ms: u64, event_limit: usize) {
    match event {
        RawInputEvent::Key {
            device,
            make_code,
            prefix,
            vkey,
            pressed,
        } => fold_key(
            state,
            &device.source_id,
            *make_code,
            *prefix,
            *vkey,
            *pressed,
            at_ms,
            event_limit,
        ),
        RawInputEvent::Button {
            device,
            button,
            pressed,
        } => fold_button(
            state,
            &device.source_id,
            *button,
            *pressed,
            at_ms,
            event_limit,
        ),
        RawInputEvent::Wheel {
            device,
            delta_hi_res,
        } => push_event(
            state,
            TimedInputEvent {
                event: InputEvent::MouseWheel {
                    source_id: device.source_id.to_string(),
                    delta_hi_res: *delta_hi_res,
                },
                at_ms,
                seq: 0,
                physical_code: Some("windows:wheel:vertical".to_owned()),
                repeat_count: 1,
            },
            event_limit,
        ),
        RawInputEvent::MotionRelative { dx, dy, .. } => {
            #[expect(
                clippy::cast_precision_loss,
                clippy::as_conversions,
                reason = "relative counts are small integers"
            )]
            let (delta_x, delta_y) = (
                *dx as f32 / CURSOR_COUNTS_PER_UNIT,
                *dy as f32 / CURSOR_COUNTS_PER_UNIT,
            );
            state.motion.dx += delta_x;
            state.motion.dy += delta_y;
            state.motion.distance += delta_x.hypot(delta_y);
        }
        RawInputEvent::MotionAbsolute {
            device,
            norm_x,
            norm_y,
            virtual_desktop,
        } => fold_absolute(state, &device.source_id, *norm_x, *norm_y, *virtual_desktop),
        RawInputEvent::DeviceArrived { device } => {
            debug!(device = %device.label, kind = ?device.kind, "Raw Input device arrived");
            let source_id = device.source_id.to_string();
            let first_arrival = state
                .devices
                .insert(
                    source_id.clone(),
                    DeviceEntry {
                        descriptor: Arc::clone(device),
                    },
                )
                .is_none();
            if first_arrival {
                // A new device's first absolute report must not be a delta
                // from a retired generation. Duplicate metadata refreshes do
                // not destroy a baseline established by earlier data.
                state.absolute_baselines.remove(source_id.as_str());
            }
        }
        RawInputEvent::DeviceRemoved { device } => {
            let source_id = &device.source_id;
            if let Some(entry) = state.devices.remove(source_id.as_ref()) {
                debug!(device = %entry.descriptor.label, "Raw Input device removed");
            }
            state.absolute_baselines.remove(source_id.as_ref());
            synthesize_releases(state, source_id, at_ms, event_limit);
        }
        RawInputEvent::StateGap { device } => {
            let source_id = &device.source_id;
            // A rollover overrun means the keyboard's own view of held state is
            // unreliable. Releasing immediately, in stream order, is the only
            // honest answer: deferring to a quiet moment would leave keys stuck
            // for as long as the user keeps mashing, which is the whole time.
            state.absolute_baselines.remove(source_id.as_ref());
            synthesize_releases(state, source_id, at_ms, event_limit);
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "the key report's fields plus the fold context; bundling them would \
              only move the arity into a single-use struct"
)]
fn fold_key(
    state: &mut SharedState,
    source_id: &str,
    make_code: u16,
    prefix: hypercolor_windows_input::RawKeyPrefix,
    vkey: u16,
    pressed: bool,
    at_ms: u64,
    event_limit: usize,
) {
    let resolved = scancode_key_name(make_code, prefix, vkey);
    if resolved == KeyNameResult::Discard {
        return;
    }
    let Some(key) = resolved.name() else {
        return;
    };

    let button_state = if pressed {
        // Windows delivers auto-repeat as an ordinary make code with no repeat
        // marker, so a press for a key this source already holds is a repeat.
        // Repeats stay out of `recent_keys`, matching evdev, so a held key
        // fires `onKeyPress` once instead of sixty times a second.
        let already_held = state
            .pressed_keys
            .get(source_id)
            .is_some_and(|held| held.contains(&key));
        if already_held {
            InputButtonState::Repeated
        } else {
            state
                .pressed_keys
                .entry(source_id.to_owned())
                .or_default()
                .insert(key.clone());
            state.recent_keys.push_back(key.clone());
            cap_recent(&mut state.recent_keys, event_limit);
            InputButtonState::Pressed
        }
    } else {
        if let Some(held) = state.pressed_keys.get_mut(source_id) {
            held.remove(&key);
        }
        InputButtonState::Released
    };

    push_event(
        state,
        TimedInputEvent {
            event: InputEvent::Key {
                source_id: source_id.to_owned(),
                key,
                state: button_state,
            },
            at_ms,
            seq: 0,
            physical_code: windows_key_physical_code(make_code, prefix),
            repeat_count: 1,
        },
        event_limit,
    );
}

fn fold_button(
    state: &mut SharedState,
    source_id: &str,
    button: RawButton,
    pressed: bool,
    at_ms: u64,
    event_limit: usize,
) {
    let name = button.canonical_name();
    let button_state = if pressed {
        state
            .held_buttons
            .entry(source_id.to_owned())
            .or_default()
            .insert(name.to_owned());
        InputButtonState::Pressed
    } else {
        if let Some(held) = state.held_buttons.get_mut(source_id) {
            held.remove(name);
        }
        InputButtonState::Released
    };

    push_event(
        state,
        TimedInputEvent {
            event: InputEvent::MouseButton {
                source_id: source_id.to_owned(),
                button: name.to_owned(),
                state: button_state,
            },
            at_ms,
            seq: 0,
            physical_code: Some(format!("windows:button:{name}")),
            repeat_count: 1,
        },
        event_limit,
    );
}

/// Accumulate motion from an absolute device as a difference of normalized
/// positions.
///
/// Dividing raw 0..65535 counts by the relative backend's 1200 would be
/// meaningless: the two unit systems are unrelated.
fn fold_absolute(
    state: &mut SharedState,
    source_id: &str,
    norm_x: f32,
    norm_y: f32,
    virtual_desktop: bool,
) {
    let next = AbsoluteBaseline {
        norm_x,
        norm_y,
        virtual_desktop,
    };
    let previous = state.absolute_baselines.insert(source_id.to_owned(), next);
    // A device that switched coordinate spaces mid-session has a baseline
    // measured against a different rect. Differencing across that boundary
    // would invent a jump the pointer never made, so the change resets the
    // baseline exactly as an arrival does.
    if let Some(previous) = previous
        && previous.virtual_desktop == virtual_desktop
    {
        let delta_x = norm_x - previous.norm_x;
        let delta_y = norm_y - previous.norm_y;
        state.motion.dx += delta_x;
        state.motion.dy += delta_y;
        state.motion.distance += delta_x.hypot(delta_y);
    }
    // No previous baseline, or a changed space: emit no delta.
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

/// Emit release edges for everything a source held, so nothing ever sticks.
fn synthesize_releases(state: &mut SharedState, source_id: &str, at_ms: u64, event_limit: usize) {
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

fn windows_key_physical_code(
    make_code: u16,
    prefix: hypercolor_windows_input::RawKeyPrefix,
) -> Option<String> {
    if make_code == 0 {
        return None;
    }
    let prefix = match prefix {
        hypercolor_windows_input::RawKeyPrefix::None => "none",
        hypercolor_windows_input::RawKeyPrefix::E0 => "e0",
        hypercolor_windows_input::RawKeyPrefix::E1 => "e1",
    };
    Some(format!("windows:set1:{prefix}:{make_code:02x}"))
}

#[cfg(test)]
mod tests;
