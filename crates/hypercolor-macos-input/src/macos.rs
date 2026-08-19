use std::ffi::c_void;
use std::mem::MaybeUninit;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use hypercolor_worker_retention::{retain_worker, spawn_worker};
use objc2_app_kit::NSEvent;
use objc2_core_foundation::{
    CFMachPort, CFRetained, CFRunLoop, CFRunLoopSource, CFRunLoopSourceContext,
    kCFRunLoopCommonModes,
};
use objc2_core_graphics::{
    CGDirectDisplayID, CGDisplayBounds, CGError, CGEvent, CGEventField, CGEventTapInformation,
    CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventType,
    CGGetActiveDisplayList, CGGetEventTapList, CGGetOnlineDisplayList,
    CGPreflightListenEventAccess, CGRequestListenEventAccess,
};

use crate::queue::{DEFAULT_QUEUE_CAPACITY, EventQueue};
use crate::{
    EffectiveEventMasks, MacosInputBatch, MacosInputConfig, MacosInputDiagnostics, MacosInputError,
    MacosInputEvent, MacosInputGapReason, MacosInputPublicationOutcome, MacosInputResult,
    MacosModifierFlags, MacosScrollPhase, MacosScrollUnit, MacosVirtualDesktop,
    MacosWorkerDegradation, MacosWorkerState, decode_button_event, decode_media_key,
    decode_momentum_phase, decode_scroll_phase, event_masks,
};

const READY_TIMEOUT: Duration = Duration::from_secs(2);
const WORKER_STOP_TIMEOUT: Duration = Duration::from_secs(2);
const HEALTH_INTERVAL: Duration = Duration::from_millis(250);
const TOPOLOGY_INTERVAL: Duration = Duration::from_secs(1);
const TAP_DISABLE_HEALTH_WINDOW: Duration = Duration::from_secs(10);
const SYSTEM_DEFINED_EVENT: CGEventType = CGEventType(14);

#[derive(Debug, Clone, Copy)]
enum TapKind {
    Keyboard,
    Pointer,
}

impl TapKind {
    const fn label(self) -> &'static str {
        match self {
            Self::Keyboard => "keyboard",
            Self::Pointer => "pointer",
        }
    }
}

#[derive(Clone, Copy, Default)]
struct RunLoopHandles {
    run_loop: usize,
    stop_source: usize,
}

struct RunLoopControl {
    stopping: AtomicBool,
    handles: Mutex<RunLoopHandles>,
}

impl RunLoopControl {
    fn new() -> Self {
        Self {
            stopping: AtomicBool::new(false),
            handles: Mutex::new(RunLoopHandles::default()),
        }
    }

    fn install(&self, run_loop: &CFRunLoop, stop_source: &CFRunLoopSource) {
        *self
            .handles
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = RunLoopHandles {
            run_loop: std::ptr::from_ref(run_loop).expose_provenance(),
            stop_source: std::ptr::from_ref(stop_source).expose_provenance(),
        };
    }

    fn clear(&self) {
        *self
            .handles
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = RunLoopHandles::default();
    }

    fn request_stop(&self) {
        self.stopping.store(true, Ordering::Release);
        let handles = self
            .handles
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if handles.run_loop == 0 {
            return;
        }
        // SAFETY: the run-loop worker owns the retained objects and clears
        // these addresses under the same mutex before releasing them. Core
        // Foundation permits stopping and waking a run loop from another
        // thread.
        let run_loop =
            unsafe { &*std::ptr::with_exposed_provenance::<CFRunLoop>(handles.run_loop) };
        if handles.stop_source != 0 {
            // Signaling the dedicated stop source closes the startup race:
            // a `CFRunLoopStop` that lands after the worker's stopping
            // check but before `CFRunLoopRun` begins is a no-op, while a
            // signaled source is serviced as soon as the loop enters and
            // its perform callback stops the loop from inside.
            // SAFETY: same ownership discipline as the run-loop address.
            let stop_source = unsafe {
                &*std::ptr::with_exposed_provenance::<CFRunLoopSource>(handles.stop_source)
            };
            stop_source.signal();
        }
        run_loop.stop();
        run_loop.wake_up();
    }
}

unsafe extern "C-unwind" fn stop_run_loop_perform(_info: *mut c_void) {
    if let Some(run_loop) = CFRunLoop::current() {
        run_loop.stop();
    }
}

struct TapContext {
    queue: Arc<EventQueue>,
    tap: AtomicPtr<CFMachPort>,
    last_disable_ms: AtomicU64,
}

struct TapBundle {
    source: CFRetained<CFRunLoopSource>,
    tap: CFRetained<CFMachPort>,
    context: Box<TapContext>,
}

struct ManagedWorker {
    handle: JoinHandle<()>,
    finished: mpsc::Receiver<()>,
}

impl ManagedWorker {
    fn retire(self, timeout: Duration, context: &'static str) -> bool {
        let finished = matches!(
            self.finished.recv_timeout(timeout),
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected)
        );
        retain_worker(self.handle, context);
        finished
    }
}

impl TapBundle {
    fn teardown(&self, run_loop: &CFRunLoop) {
        // SAFETY: Core Foundation exports this process-lifetime static mode.
        let mode = unsafe { kCFRunLoopCommonModes };
        run_loop.remove_source(Some(&self.source), mode);
        self.tap.invalidate();
        self.context
            .tap
            .store(std::ptr::null_mut(), Ordering::Release);
    }
}

/// A live Core Graphics event-tap session.
pub struct MacosInputSession {
    masks: EffectiveEventMasks,
    state: Arc<Mutex<MacosWorkerState>>,
    queue: Arc<EventQueue>,
    control: Arc<RunLoopControl>,
    event_worker: Option<ManagedWorker>,
    sink_worker: Option<ManagedWorker>,
    stopped: bool,
}

impl MacosInputSession {
    /// Start the requested event taps and block until their run loop is ready.
    pub fn start(
        config: MacosInputConfig,
        sink: impl FnMut(MacosInputBatch<'_>) -> MacosInputPublicationOutcome + Send + 'static,
    ) -> MacosInputResult<Self> {
        if !config.keyboard && !config.pointer {
            return Err(MacosInputError::NothingToCapture);
        }
        if config.keyboard && !input_monitoring_granted() {
            return Err(MacosInputError::PermissionDenied);
        }

        let masks = event_masks(config.keyboard, config.pointer);
        let desktop = current_virtual_desktop()?;
        let queue = Arc::new(EventQueue::new(DEFAULT_QUEUE_CAPACITY));
        let state = Arc::new(Mutex::new(MacosWorkerState::Running));
        let control = Arc::new(RunLoopControl::new());

        let (sink_finished_tx, sink_finished_rx) = mpsc::sync_channel(1);
        let sink_handle = spawn_worker(
            thread::Builder::new().name("hypercolor-macos-input-fold".to_owned()),
            {
                let queue = Arc::clone(&queue);
                let state = Arc::clone(&state);
                let control = Arc::clone(&control);
                let config = config.clone();
                move || {
                    drain_batches(config, desktop, sink, &queue, &state, &control);
                    let _ = sink_finished_tx.send(());
                }
            },
        )
        .map_err(|error| MacosInputError::WorkerSpawn(error.to_string()))?;
        let sink_worker = ManagedWorker {
            handle: sink_handle,
            finished: sink_finished_rx,
        };

        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let (event_finished_tx, event_finished_rx) = mpsc::sync_channel(1);
        let event_handle = match spawn_worker(
            thread::Builder::new().name("hypercolor-macos-event-tap".to_owned()),
            {
                let queue = Arc::clone(&queue);
                let state = Arc::clone(&state);
                let control = Arc::clone(&control);
                move || {
                    run_event_taps(masks, &queue, &state, &control, &ready_tx);
                    let _ = event_finished_tx.send(());
                }
            },
        ) {
            Ok(worker) => worker,
            Err(error) => {
                queue.close();
                sink_worker.retire(WORKER_STOP_TIMEOUT, "macOS input sink startup failure");
                return Err(MacosInputError::WorkerSpawn(error.to_string()));
            }
        };
        let event_worker = ManagedWorker {
            handle: event_handle,
            finished: event_finished_rx,
        };

        match ready_rx.recv_timeout(READY_TIMEOUT) {
            Ok(Ok(())) => Ok(Self {
                masks,
                state,
                queue,
                control,
                event_worker: Some(event_worker),
                sink_worker: Some(sink_worker),
                stopped: false,
            }),
            Ok(Err(error)) => {
                control.request_stop();
                event_worker.retire(WORKER_STOP_TIMEOUT, "macOS event tap startup failure");
                queue.close();
                sink_worker.retire(WORKER_STOP_TIMEOUT, "macOS input sink startup failure");
                Err(error)
            }
            Err(_) => {
                control.request_stop();
                event_worker.retire(WORKER_STOP_TIMEOUT, "macOS event tap readiness timeout");
                queue.close();
                sink_worker.retire(WORKER_STOP_TIMEOUT, "macOS input sink readiness timeout");
                Err(MacosInputError::WorkerReadyTimeout)
            }
        }
    }

    #[must_use]
    pub const fn effective_masks(&self) -> EffectiveEventMasks {
        self.masks
    }

    /// Return the event masks Core Graphics reports for this process's
    /// installed listen-only session taps.
    pub fn installed_masks(&self) -> MacosInputResult<EffectiveEventMasks> {
        let mut count = 0;
        // SAFETY: the count-only form accepts a null list and writes one u32.
        let result = unsafe { CGGetEventTapList(0, std::ptr::null_mut(), &mut count) };
        if result != CGError::Success {
            return Err(MacosInputError::TapInspection(result.0));
        }
        let capacity = usize::try_from(count).map_err(|_| MacosInputError::TapInspection(-1))?;
        let mut taps = vec![MaybeUninit::<CGEventTapInformation>::uninit(); capacity];
        let mut written = count;
        // SAFETY: `taps` has room for `count` records and Core Graphics writes
        // at most that many initialized records, reported through `written`.
        let result = unsafe {
            CGGetEventTapList(
                count,
                taps.as_mut_ptr().cast::<CGEventTapInformation>(),
                &mut written,
            )
        };
        if result != CGError::Success || written > count {
            return Err(MacosInputError::TapInspection(result.0));
        }
        let pid =
            i32::try_from(std::process::id()).map_err(|_| MacosInputError::TapInspection(-1))?;
        let taps = taps
            .into_iter()
            .take(usize::try_from(written).unwrap_or(capacity))
            .map(|tap| {
                // SAFETY: Core Graphics initialized the first `written`
                // records on the successful call above.
                unsafe { tap.assume_init() }
            })
            .collect::<Vec<_>>();
        Ok(installed_masks_for_process(self.masks, pid, &taps))
    }

    #[must_use]
    pub fn worker_state(&self) -> MacosWorkerState {
        self.state
            .lock()
            .map(|state| state.clone())
            .unwrap_or_else(|poisoned| poisoned.into_inner().clone())
    }

    #[must_use]
    pub fn diagnostics(&self) -> MacosInputDiagnostics {
        self.queue.diagnostics_snapshot()
    }

    /// Stop the run loop, tear down both taps, join their worker, then flush
    /// the ordered source-stop barrier through the sink.
    pub fn stop(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        self.control.request_stop();
        if let Some(worker) = self.event_worker.take() {
            worker.retire(WORKER_STOP_TIMEOUT, "macOS event tap shutdown");
        }
        self.queue.request_gap(MacosInputGapReason::SourceStopped);
        self.queue.close();
        if let Some(worker) = self.sink_worker.take() {
            worker.retire(WORKER_STOP_TIMEOUT, "macOS input sink shutdown");
        }
    }
}

fn installed_masks_for_process(
    requested: EffectiveEventMasks,
    pid: i32,
    taps: &[CGEventTapInformation],
) -> EffectiveEventMasks {
    let installed = taps
        .iter()
        .filter(|tap| {
            tap.tappingProcess == pid
                && tap.tapPoint == CGEventTapLocation::SessionEventTap
                && tap.options == CGEventTapOptions::ListenOnly
                && tap.enabled
        })
        .fold(0, |mask, tap| mask | tap.eventsOfInterest);
    EffectiveEventMasks {
        keyboard: installed & requested.keyboard,
        pointer: installed & requested.pointer,
    }
}

impl Drop for MacosInputSession {
    fn drop(&mut self) {
        self.stop();
    }
}

#[must_use]
pub fn input_monitoring_granted() -> bool {
    CGPreflightListenEventAccess()
}

/// Whether any process currently holds the macOS secure-input assertion
/// (Secure Keyboard Entry). While held, an event tap receives no keyboard
/// events even though pointer events keep flowing, so held-key state must
/// be cleared through an ordered gap.
#[must_use]
pub fn secure_event_input_enabled() -> bool {
    #[link(name = "Carbon", kind = "framework")]
    unsafe extern "C" {
        /// `Boolean IsSecureEventInputEnabled(void)` from HIToolbox.
        fn IsSecureEventInputEnabled() -> u8;
    }
    // SAFETY: the function takes no arguments, has no side effects, and
    // reads a session-global flag maintained by the window server.
    (unsafe { IsSecureEventInputEnabled() }) != 0
}

/// Ask macOS to grant Input Monitoring to the current signed process.
#[must_use]
pub fn request_input_monitoring() -> bool {
    CGRequestListenEventAccess()
}

/// Snapshot the union of active display bounds.
pub fn current_virtual_desktop() -> MacosInputResult<MacosVirtualDesktop> {
    query_virtual_desktop(1)
}

fn query_virtual_desktop(generation: u64) -> MacosInputResult<MacosVirtualDesktop> {
    let mut displays = query_display_ids(false)?;
    if displays.is_empty() {
        displays = query_display_ids(true)?;
    }
    let mut bounds = displays.into_iter().map(|display| CGDisplayBounds(display));
    let first = bounds.next().ok_or(MacosInputError::NoActiveDisplays)?;
    let mut min_x = first.origin.x;
    let mut min_y = first.origin.y;
    let mut max_x = first.origin.x + first.size.width;
    let mut max_y = first.origin.y + first.size.height;
    for rect in bounds {
        min_x = min_x.min(rect.origin.x);
        min_y = min_y.min(rect.origin.y);
        max_x = max_x.max(rect.origin.x + rect.size.width);
        max_y = max_y.max(rect.origin.y + rect.size.height);
    }
    MacosVirtualDesktop::new(min_x, min_y, max_x - min_x, max_y - min_y, generation)
}

fn query_display_ids(online: bool) -> MacosInputResult<Vec<CGDirectDisplayID>> {
    let mut count = 0;
    // SAFETY: both Core Graphics count-only forms write only the display count
    // because the capacity and display pointer are zero.
    let error = unsafe {
        if online {
            CGGetOnlineDisplayList(0, std::ptr::null_mut(), &raw mut count)
        } else {
            CGGetActiveDisplayList(0, std::ptr::null_mut(), &raw mut count)
        }
    };
    if error != CGError::Success {
        return Err(MacosInputError::DisplayTopology(error.0));
    }
    if count == 0 {
        return Ok(Vec::new());
    }

    let mut displays = vec![0; usize::try_from(count).unwrap_or(usize::MAX)];
    let mut written = count;
    // SAFETY: both Core Graphics list forms write at most `count` identifiers
    // into `displays` and report the initialized length through `written`.
    let error = unsafe {
        if online {
            CGGetOnlineDisplayList(count, displays.as_mut_ptr(), &raw mut written)
        } else {
            CGGetActiveDisplayList(count, displays.as_mut_ptr(), &raw mut written)
        }
    };
    if error != CGError::Success {
        return Err(MacosInputError::DisplayTopology(error.0));
    }
    displays.truncate(usize::try_from(written).unwrap_or(displays.len()));
    Ok(displays)
}

fn run_event_taps(
    masks: EffectiveEventMasks,
    queue: &Arc<EventQueue>,
    state: &Arc<Mutex<MacosWorkerState>>,
    control: &Arc<RunLoopControl>,
    ready: &mpsc::SyncSender<MacosInputResult<()>>,
) {
    let Some(run_loop) = CFRunLoop::current() else {
        let _ = ready.send(Err(MacosInputError::WorkerSpawn(
            "Core Foundation returned no current run loop".to_owned(),
        )));
        return;
    };
    let mut stop_context = CFRunLoopSourceContext {
        version: 0,
        info: std::ptr::null_mut(),
        retain: None,
        release: None,
        copyDescription: None,
        equal: None,
        hash: None,
        schedule: None,
        cancel: None,
        perform: Some(stop_run_loop_perform),
    };
    // SAFETY: the context pointer is valid for the duration of the call and
    // Core Foundation copies the structure; the perform callback uses no
    // context state.
    let Some(stop_source) = (unsafe { CFRunLoopSource::new(None, 0, &raw mut stop_context) })
    else {
        let _ = ready.send(Err(MacosInputError::WorkerSpawn(
            "Core Foundation refused the run-loop stop source".to_owned(),
        )));
        return;
    };
    // SAFETY: Core Foundation exports this process-lifetime static mode.
    let common_modes = unsafe { kCFRunLoopCommonModes };
    run_loop.add_source(Some(&stop_source), common_modes);
    control.install(&run_loop, &stop_source);

    let mut taps = Vec::with_capacity(2);
    let result = (|| {
        if masks.keyboard != 0 {
            taps.push(create_tap(
                TapKind::Keyboard,
                masks.keyboard,
                &run_loop,
                queue,
            )?);
        }
        if masks.pointer != 0 {
            taps.push(create_tap(
                TapKind::Pointer,
                masks.pointer,
                &run_loop,
                queue,
            )?);
        }
        Ok(())
    })();

    if let Err(error) = result {
        for tap in &taps {
            tap.teardown(&run_loop);
        }
        control.clear();
        run_loop.remove_source(Some(&stop_source), common_modes);
        let _ = ready.send(Err(error));
        return;
    }
    if ready.send(Ok(())).is_err() {
        control.request_stop();
    }
    if !control.stopping.load(Ordering::Acquire) {
        CFRunLoop::run();
    }
    for tap in &taps {
        tap.teardown(&run_loop);
    }
    control.clear();
    run_loop.remove_source(Some(&stop_source), common_modes);

    if !control.stopping.load(Ordering::Acquire) {
        set_worker_state(
            state,
            MacosWorkerState::Failed("event-tap run loop exited unexpectedly".to_owned()),
        );
        queue.request_gap(MacosInputGapReason::WorkerExited);
        queue.close();
    }
}

fn create_tap(
    kind: TapKind,
    mask: u64,
    run_loop: &CFRunLoop,
    queue: &Arc<EventQueue>,
) -> MacosInputResult<TapBundle> {
    let mut context = Box::new(TapContext {
        queue: Arc::clone(queue),
        tap: AtomicPtr::new(std::ptr::null_mut()),
        last_disable_ms: AtomicU64::new(0),
    });
    // SAFETY: `context` remains at a stable Box address until its tap is
    // removed and invalidated. The callback returns the borrowed event and
    // never retains the proxy or event pointers.
    let tap = unsafe {
        CGEvent::tap_create(
            CGEventTapLocation::SessionEventTap,
            CGEventTapPlacement::HeadInsertEventTap,
            CGEventTapOptions::ListenOnly,
            mask,
            Some(event_tap_callback),
            std::ptr::from_mut(context.as_mut()).cast::<c_void>(),
        )
    }
    .ok_or(MacosInputError::TapCreation(kind.label()))?;
    context.tap.store(
        std::ptr::from_ref::<CFMachPort>(&tap).cast_mut(),
        Ordering::Release,
    );
    let source = CFMachPort::new_run_loop_source(None, Some(&tap), 0)
        .ok_or(MacosInputError::RunLoopSource(kind.label()))?;
    // SAFETY: Core Foundation exports this process-lifetime static mode.
    let mode = unsafe { kCFRunLoopCommonModes };
    run_loop.add_source(Some(&source), mode);
    CGEvent::tap_enable(&tap, true);
    if !CGEvent::tap_is_enabled(&tap) {
        run_loop.remove_source(Some(&source), mode);
        return Err(MacosInputError::TapCreation(kind.label()));
    }
    Ok(TapBundle {
        source,
        tap,
        context,
    })
}

unsafe extern "C-unwind" fn event_tap_callback(
    _proxy: objc2_core_graphics::CGEventTapProxy,
    event_type: CGEventType,
    event: NonNull<CGEvent>,
    user_info: *mut c_void,
) -> *mut CGEvent {
    let callback_entry = Instant::now();
    // SAFETY: Core Graphics supplies both pointers for the lifetime of this
    // callback. `create_tap` keeps the boxed context alive through teardown.
    let context = unsafe { &*(user_info.cast::<TapContext>()) };
    // SAFETY: Core Graphics guarantees this non-null event for the callback.
    let event_ref = unsafe { event.as_ref() };

    if event_type == CGEventType::TapDisabledByTimeout {
        handle_tap_disable(
            context,
            MacosInputGapReason::TapDisabledTimeout,
            callback_entry,
        );
    } else if event_type == CGEventType::TapDisabledByUserInput {
        handle_tap_disable(
            context,
            MacosInputGapReason::TapDisabledUserInput,
            callback_entry,
        );
    } else if let Some(decoded) = decode_native_event(event_type, event_ref, context) {
        context.queue.enqueue_at(decoded, callback_entry);
    }
    event.as_ptr()
}

fn handle_tap_disable(context: &TapContext, reason: MacosInputGapReason, callback_entry: Instant) {
    static DISABLE_CLOCK: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    let elapsed_ms = DISABLE_CLOCK
        .get_or_init(Instant::now)
        .elapsed()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
        + 1;
    let previous = context.last_disable_ms.swap(elapsed_ms, Ordering::AcqRel);
    let health_window_ms = u64::try_from(TAP_DISABLE_HEALTH_WINDOW.as_millis()).unwrap_or(u64::MAX);
    let repeated = previous != 0 && elapsed_ms.saturating_sub(previous) < health_window_ms;
    context
        .queue
        .diagnostics()
        .record_tap_disable(repeated, reason);
    context
        .queue
        .enqueue_at(MacosInputEvent::StateGap { reason }, callback_entry);
    // Always re-enable, including on repeated disables. A disabled tap
    // fires no callbacks, so refusing here would be permanent capture
    // death with no recovery trigger anywhere. The retry cadence is
    // bounded by macOS itself: each re-enable requires the window server
    // to deliver a fresh disable event before another cycle can happen,
    // and repeated disables stay observable through the diagnostics
    // counters and the Degraded worker state.
    let tap = context.tap.load(Ordering::Acquire);
    if tap.is_null() {
        return;
    }
    // SAFETY: the callback runs on the owning run-loop thread while the tap is
    // retained. Teardown clears this pointer only after removing the source.
    CGEvent::tap_enable(unsafe { &*tap }, true);
    context.queue.diagnostics().record_tap_reenabled();
}

fn decode_native_event(
    event_type: CGEventType,
    event: &CGEvent,
    context: &TapContext,
) -> Option<MacosInputEvent> {
    if event_type == CGEventType::KeyDown || event_type == CGEventType::KeyUp {
        return Some(MacosInputEvent::Key {
            virtual_keycode: u16::try_from(CGEvent::integer_value_field(
                Some(event),
                CGEventField::KeyboardEventKeycode,
            ))
            .ok()?,
            pressed: event_type == CGEventType::KeyDown,
            autorepeat: CGEvent::integer_value_field(
                Some(event),
                CGEventField::KeyboardEventAutorepeat,
            ) != 0,
        });
    }
    if event_type == CGEventType::FlagsChanged {
        return Some(MacosInputEvent::ModifierFlags {
            virtual_keycode: u16::try_from(CGEvent::integer_value_field(
                Some(event),
                CGEventField::KeyboardEventKeycode,
            ))
            .ok()?,
            flags: MacosModifierFlags::from_bits(CGEvent::flags(Some(event)).bits()),
        });
    }
    if event_type == SYSTEM_DEFINED_EVENT {
        // The tap thread never spins an autorelease pool of its own, and
        // the NSEvent bridge autoreleases; without this scope every media
        // key leaks the bridged event and spams the console.
        return objc2::rc::autoreleasepool(|_| {
            let Some(native) = NSEvent::eventWithCGEvent(event) else {
                context
                    .queue
                    .diagnostics()
                    .record_unsupported_system_event();
                return None;
            };
            let data1 = i64::try_from(native.data1()).ok()?;
            if let Some(media) = decode_media_key(native.subtype().0, data1) {
                return Some(MacosInputEvent::MediaKey {
                    nx_key_type: media.nx_key_type,
                    pressed: media.pressed,
                    repeat: media.repeat,
                });
            }
            context
                .queue
                .diagnostics()
                .record_unsupported_system_event();
            None
        });
    }
    if let Some((button, pressed)) = decode_button_event(
        event_type.0,
        u16::try_from(CGEvent::integer_value_field(
            Some(event),
            CGEventField::MouseEventButtonNumber,
        ))
        .ok()?,
    ) {
        return Some(MacosInputEvent::Button { button, pressed });
    }
    if matches!(
        event_type,
        CGEventType::MouseMoved
            | CGEventType::LeftMouseDragged
            | CGEventType::RightMouseDragged
            | CGEventType::OtherMouseDragged
    ) {
        let location = CGEvent::location(Some(event));
        return Some(MacosInputEvent::Motion {
            x: location.x,
            y: location.y,
            delta_x: CGEvent::integer_value_field(Some(event), CGEventField::MouseEventDeltaX)
                as f64,
            delta_y: CGEvent::integer_value_field(Some(event), CGEventField::MouseEventDeltaY)
                as f64,
        });
    }
    if event_type == CGEventType::ScrollWheel {
        let point_y = CGEvent::integer_value_field(
            Some(event),
            CGEventField::ScrollWheelEventPointDeltaAxis1,
        );
        let point_x = CGEvent::integer_value_field(
            Some(event),
            CGEventField::ScrollWheelEventPointDeltaAxis2,
        );
        context
            .queue
            .diagnostics()
            .record_point_delta(point_x, point_y);
        let phase = decode_phase(
            CGEvent::integer_value_field(Some(event), CGEventField::ScrollWheelEventScrollPhase),
            context,
            decode_scroll_phase,
        );
        let momentum_phase = decode_phase(
            CGEvent::integer_value_field(Some(event), CGEventField::ScrollWheelEventMomentumPhase),
            context,
            decode_momentum_phase,
        );
        let unit = if CGEvent::integer_value_field(
            Some(event),
            CGEventField::ScrollWheelEventIsContinuous,
        ) != 0
        {
            MacosScrollUnit::Pixels
        } else {
            MacosScrollUnit::Notches
        };
        return Some(MacosInputEvent::Wheel {
            fixed_delta_x: q16_16_field(event, CGEventField::ScrollWheelEventFixedPtDeltaAxis2),
            fixed_delta_y: q16_16_field(event, CGEventField::ScrollWheelEventFixedPtDeltaAxis1),
            unit,
            phase,
            momentum_phase,
        });
    }
    None
}

/// Read a 16.16 fixed-point scroll field as raw Q16.16 bits.
///
/// The integer accessor rounds fixed-point fields to the nearest whole
/// unit, discarding the fractional 16 bits (one notch reads as 1, not
/// 65536). The double accessor applies the documented 1/65536 scaling, so
/// multiplying back yields the raw representation the fold pipeline
/// expects.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    reason = "the scaled value is clamped to the i64 range before conversion"
)]
fn q16_16_field(event: &CGEvent, field: CGEventField) -> i64 {
    const Q16_16_SCALE: f64 = 65536.0;
    let scaled = CGEvent::double_value_field(Some(event), field) * Q16_16_SCALE;
    if scaled.is_finite() {
        scaled.round().clamp(i64::MIN as f64, i64::MAX as f64) as i64
    } else {
        0
    }
}

fn decode_phase(
    raw: i64,
    context: &TapContext,
    decode: impl FnOnce(i64) -> Option<MacosScrollPhase>,
) -> MacosScrollPhase {
    decode(raw).unwrap_or_else(|| {
        context.queue.diagnostics().record_invalid_scroll_phase();
        MacosScrollPhase::None
    })
}

fn drain_batches(
    config: MacosInputConfig,
    mut desktop: MacosVirtualDesktop,
    mut sink: impl FnMut(MacosInputBatch<'_>) -> MacosInputPublicationOutcome,
    queue: &EventQueue,
    state: &Mutex<MacosWorkerState>,
    control: &RunLoopControl,
) {
    let mut events = Vec::with_capacity(DEFAULT_QUEUE_CAPACITY + 2);
    let mut callback_entries = Vec::with_capacity(DEFAULT_QUEUE_CAPACITY);
    let mut next_topology_check = Instant::now() + TOPOLOGY_INTERVAL;
    let mut secure_input_active = false;
    loop {
        queue.wait(HEALTH_INTERVAL);
        let now = Instant::now();
        if let Some(reason) = queue.diagnostics().take_repeated_tap_disable() {
            set_worker_state(
                state,
                MacosWorkerState::Degraded(MacosWorkerDegradation::TapDisabled(reason)),
            );
        }
        if config.keyboard && !input_monitoring_granted() {
            set_worker_state(state, MacosWorkerState::PermissionRevoked);
            queue.request_gap(MacosInputGapReason::PermissionRevoked);
            control.request_stop();
            queue.close();
        }
        // Secure Keyboard Entry starves the tap of keyboard events while
        // pointer events keep flowing; without an ordered gap on the
        // rising edge, keys held at that moment stay pressed forever.
        if config.keyboard {
            let secure_now = secure_event_input_enabled();
            if secure_now != secure_input_active {
                secure_input_active = secure_now;
                queue.diagnostics().set_secure_input_active(secure_now);
                if secure_now {
                    queue.request_gap(MacosInputGapReason::SessionInterrupted);
                }
            }
        }
        if config.pointer && now >= next_topology_check {
            match query_virtual_desktop(desktop.topology_generation) {
                Ok(current) if desktop_geometry_changed(desktop, current) => {
                    desktop = MacosVirtualDesktop {
                        topology_generation: desktop.topology_generation.saturating_add(1),
                        ..current
                    };
                }
                Ok(_) => {}
                Err(error) => set_worker_state(
                    state,
                    MacosWorkerState::Degraded(MacosWorkerDegradation::DisplayTopology(
                        error.to_string(),
                    )),
                ),
            }
            next_topology_check = now + TOPOLOGY_INTERVAL;
        }

        events.clear();
        callback_entries.clear();
        let at_ms = (config.clock)();
        queue.drain_into(&mut events, &mut callback_entries);
        if !events.is_empty() {
            let outcome = sink(MacosInputBatch {
                epoch: config.epoch,
                at_ms,
                events: &events,
                virtual_desktop: desktop,
            });
            if outcome == MacosInputPublicationOutcome::Published {
                queue
                    .diagnostics()
                    .record_published(events.len(), &callback_entries);
            }
        }
        if queue.is_closed() && queue.is_empty() {
            break;
        }
    }
}

fn desktop_geometry_changed(left: MacosVirtualDesktop, right: MacosVirtualDesktop) -> bool {
    left.origin_x != right.origin_x
        || left.origin_y != right.origin_y
        || left.width != right.width
        || left.height != right.height
}

fn set_worker_state(state: &Mutex<MacosWorkerState>, value: MacosWorkerState) {
    *state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = value;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tap(pid: i32, mask: u64, enabled: bool) -> CGEventTapInformation {
        CGEventTapInformation {
            eventTapID: 1,
            tapPoint: CGEventTapLocation::SessionEventTap,
            options: CGEventTapOptions::ListenOnly,
            eventsOfInterest: mask,
            tappingProcess: pid,
            processBeingTapped: 0,
            enabled,
            minUsecLatency: 0.0,
            avgUsecLatency: 0.0,
            maxUsecLatency: 0.0,
        }
    }

    #[test]
    fn installed_masks_use_only_enabled_current_process_session_taps() {
        let requested = EffectiveEventMasks {
            keyboard: 0b0011,
            pointer: 0b1100,
        };
        let taps = [
            tap(42, 0b0001, true),
            tap(42, 0b0100, true),
            tap(42, 0b0010, false),
            tap(7, 0b1000, true),
        ];

        assert_eq!(
            installed_masks_for_process(requested, 42, &taps),
            EffectiveEventMasks {
                keyboard: 0b0001,
                pointer: 0b0100,
            }
        );
    }

    #[test]
    fn managed_worker_observes_cooperative_exit() {
        let (finished_tx, finished_rx) = mpsc::sync_channel(1);
        let handle = spawn_worker(
            thread::Builder::new().name("macos-managed-worker-exit-test".to_owned()),
            move || {
                let _ = finished_tx.send(());
            },
        )
        .expect("worker spawns");
        let worker = ManagedWorker {
            handle,
            finished: finished_rx,
        };

        assert!(worker.retire(Duration::from_secs(1), "cooperative test worker"));
    }

    #[test]
    fn managed_worker_retirement_is_bounded_when_exit_stalls() {
        let (release_tx, release_rx) = mpsc::channel();
        let (_finished_tx, finished_rx) = mpsc::sync_channel(1);
        let handle = spawn_worker(
            thread::Builder::new().name("macos-managed-worker-timeout-test".to_owned()),
            move || {
                let _ = release_rx.recv();
            },
        )
        .expect("worker spawns");
        let worker = ManagedWorker {
            handle,
            finished: finished_rx,
        };
        let started = Instant::now();

        assert!(!worker.retire(Duration::from_millis(20), "stalled test worker"));
        assert!(started.elapsed() < Duration::from_millis(500));
        release_tx.send(()).expect("retained worker is released");
    }
}
