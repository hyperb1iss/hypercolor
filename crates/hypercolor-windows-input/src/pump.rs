//! The message-only window, the registration, and the drain loop.
//!
//! The loop ordering here is load-bearing and the obvious arrangement is
//! wrong, so it is worth stating before the code says it less clearly:
//!
//! 1. Wait on `MsgWaitForMultipleObjectsEx` with `MWMO_INPUTAVAILABLE`, which
//!    closes the classic lost-wakeup race where input was already observed but
//!    not yet removed from the queue.
//! 2. Drain `GetRawInputBuffer` in bounded slices — *before* any
//!    `PeekMessageW`, because a `PM_REMOVE` pass would remove the pending
//!    `WM_INPUT` messages and the buffered read would then find nothing.
//! 3. Pass over control messages using message-range filters only. A broad
//!    `PeekMessageW(PM_REMOVE)` would silently eat any `WM_INPUT` that landed
//!    between the last read and the peek, dropping real input with no error.
//!
//! The drain is bounded rather than "until it returns 0" because an 8 kHz
//! mouse plus a held key can produce input as fast as we consume it. An
//! unbounded drain would starve the stop flag and `WM_QUIT` for as long as the
//! user keeps moving the mouse, so shutdown would hang exactly when the
//! machine is busiest.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::{
    DefRawInputProc, GetRawInputBuffer, GetRegisteredRawInputDevices, RAWINPUT, RAWINPUTDEVICE,
    RAWINPUTDEVICE_FLAGS, RAWINPUTHEADER, RID_DEVICE_INFO_TYPE, RIDEV_DEVNOTIFY, RIDEV_INPUTSINK,
    RIDEV_REMOVE, RIM_TYPEKEYBOARD, RIM_TYPEMOUSE, RegisterRawInputDevices,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GIDC_ARRIVAL, GIDC_REMOVAL, HWND_MESSAGE, MSG,
    MWMO_INPUTAVAILABLE, MsgWaitForMultipleObjectsEx, PM_REMOVE, PeekMessageW, QS_ALLINPUT,
    RegisterClassW, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_INPUT_DEVICE_CHANGE, WM_QUIT,
    WNDCLASSW,
};
use windows::core::{PCWSTR, w};

use crate::claim::PROCESS_CLAIM;
use crate::decode::{
    KeyReport, MotionKind, RecordStep, ScreenRect, button_edges, classify_key, is_horizontal_wheel,
    motion_kind, next_record, normalize_absolute, wheel_delta,
};
use crate::devices::{DeviceCache, enumerate_devices, seed_cache};
use crate::metrics::{pin_dpi_context, primary_screen_rect, sample_cursor, virtual_screen_rect};
use crate::shared::{
    RawCursor, RawInputBatch, RawInputConfig, RawInputError, RawInputEvent, RawInputResult,
};

/// Window class shared by every session in this process.
const WINDOW_CLASS: PCWSTR = w!("HypercolorRawInputSink");

/// Private message the stop path posts so teardown does not wait out the wake
/// budget.
pub const WM_HYPERCOLOR_STOP: u32 = WM_APP + 1;

/// Wake budget. Bounded so the worker notices a stop request even if the nudge
/// is lost, and unlike a peek-and-sleep loop it costs nothing while idle.
const WAKE_BUDGET_MS: u32 = 100;

/// Initial buffered-read capacity, in `u64` units. `Vec<u64>` rather than
/// `Vec<u8>` because the buffer must be QWORD-aligned and `Vec<u8>` guarantees
/// no such thing — the record walk's offset arithmetic depends on it.
const INITIAL_BUFFER_QWORDS: usize = 2048;

/// Ceiling on buffer growth, so a pathological sizing loop cannot exhaust
/// memory. 1 MiB holds thousands of reports.
const MAX_BUFFER_QWORDS: usize = 128 * 1024;

/// Slices per wake before returning to the wait, so the control pass always
/// gets a look-in under sustained input.
const MAX_SLICES_PER_WAKE: usize = 32;

/// The HID usage page and usages we register for. Keyboards and mice only:
/// media keys arrive through a separate Consumer Control collection as
/// `RIM_TYPEHID` with a vendor-defined layout, and we neither register for
/// that nor ever decode one as a keyboard record.
const USAGE_PAGE_GENERIC: u16 = 0x01;
const USAGE_MOUSE: u16 = 0x02;
const USAGE_KEYBOARD: u16 = 0x06;

/// Everything the pump owns for one session's lifetime.
pub struct Pump {
    window: HWND,
    generation: u64,
    config: RawInputConfig,
    cache: DeviceCache,
    buffer: Vec<u64>,
    events: Vec<RawInputEvent>,
    last_cursor: Option<RawCursor>,
    virtual_screen: ScreenRect,
    primary_screen: ScreenRect,
    registered: bool,
}

impl Pump {
    /// Create the window and take the registration.
    ///
    /// Runs entirely on the worker thread: the `HWND` is thread-affine and
    /// must never leave it.
    pub fn create(config: RawInputConfig, generation: u64) -> RawInputResult<Self> {
        if !config.keyboard && !config.mouse {
            return Err(RawInputError::NothingToCapture);
        }

        // First act, before the window exists: both the cursor and the screen
        // metrics follow this thread's DPI context, and mixing contexts
        // between those two reads is the actual hazard.
        if !pin_dpi_context() {
            tracing::debug!(
                "could not pin per-monitor-v2 DPI context; normalization stays self-consistent"
            );
        }

        let window = create_message_window()?;
        let mut pump = Self {
            window,
            generation,
            config,
            cache: DeviceCache::new(),
            buffer: vec![0u64; INITIAL_BUFFER_QWORDS],
            events: Vec::new(),
            last_cursor: None,
            virtual_screen: virtual_screen_rect(),
            primary_screen: primary_screen_rect(),
            registered: false,
        };

        pump.register()?;
        pump.registered = true;
        pump.seed_devices();
        Ok(pump)
    }

    /// The window control messages are posted to.
    pub const fn window(&self) -> HWND {
        self.window
    }

    /// Devices resolved and streaming.
    pub fn device_count(&self) -> usize {
        self.cache.len()
    }

    /// Register for the enabled usages, publishing the ownership claim in the
    /// same critical section so a stale teardown cannot remove what we just
    /// installed.
    fn register(&self) -> RawInputResult<()> {
        let entries =
            self.registration_entries(RIDEV_INPUTSINK | RIDEV_DEVNOTIFY, Some(self.window));
        PROCESS_CLAIM.acquire(self.generation, || {
            // SAFETY: `entries` is a live slice of correctly initialized
            // `RAWINPUTDEVICE` values owned by this frame, and the size
            // argument is the caller-ABI element size the API requires.
            unsafe {
                RegisterRawInputDevices(
                    &entries,
                    u32::try_from(size_of::<RAWINPUTDEVICE>()).unwrap_or(u32::MAX),
                )
            }
            .map_err(|error| RawInputError::Registration(error.to_string()))
        })?;

        self.verify_registration()
    }

    /// Confirm our window is still the process's raw input target.
    ///
    /// Registration is process-global per `(usUsagePage, usUsage)` pair, not
    /// per usage page, so a second `RegisterRawInputDevices` from any thread or
    /// window in this process makes *its* window the sole recipient, silently.
    /// Only this worker registers today; checking means a future Tauri window
    /// that steals it fails loudly instead of producing a daemon that
    /// mysteriously stops seeing keys.
    fn verify_registration(&self) -> RawInputResult<()> {
        let mut count: u32 = 0;
        let entry_size = u32::try_from(size_of::<RAWINPUTDEVICE>()).unwrap_or(u32::MAX);
        // SAFETY: the query form takes a null list pointer and only writes the
        // registration count.
        let probe = unsafe { GetRegisteredRawInputDevices(None, &raw mut count, entry_size) };
        if probe == u32::MAX || count == 0 {
            return Err(RawInputError::RegistrationStolen);
        }

        let mut list = vec![RAWINPUTDEVICE::default(); count as usize];
        // SAFETY: `list` holds `count` entries and outlives the call; `count`
        // describes exactly that allocation.
        let written = unsafe {
            GetRegisteredRawInputDevices(Some(list.as_mut_ptr()), &raw mut count, entry_size)
        };
        if written == u32::MAX {
            return Err(RawInputError::RegistrationStolen);
        }
        list.truncate((written as usize).min(list.len()));

        let ours_registered = |usage: u16| {
            list.iter().any(|entry| {
                entry.usUsagePage == USAGE_PAGE_GENERIC
                    && entry.usUsage == usage
                    && entry.hwndTarget == self.window
            })
        };
        let keyboard_ok = !self.config.keyboard || ours_registered(USAGE_KEYBOARD);
        let mouse_ok = !self.config.mouse || ours_registered(USAGE_MOUSE);
        if keyboard_ok && mouse_ok {
            Ok(())
        } else {
            Err(RawInputError::RegistrationStolen)
        }
    }

    fn registration_entries(
        &self,
        flags: RAWINPUTDEVICE_FLAGS,
        target: Option<HWND>,
    ) -> Vec<RAWINPUTDEVICE> {
        let mut entries = Vec::with_capacity(2);
        let hwnd = target.unwrap_or_default();
        if self.config.keyboard {
            entries.push(RAWINPUTDEVICE {
                usUsagePage: USAGE_PAGE_GENERIC,
                usUsage: USAGE_KEYBOARD,
                dwFlags: flags,
                hwndTarget: hwnd,
            });
        }
        if self.config.mouse {
            entries.push(RAWINPUTDEVICE {
                usUsagePage: USAGE_PAGE_GENERIC,
                usUsage: USAGE_MOUSE,
                dwFlags: flags,
                hwndTarget: hwnd,
            });
        }
        entries
    }

    /// Record devices already attached at registration time.
    fn seed_devices(&mut self) {
        let devices = enumerate_devices(self.config.keyboard, self.config.mouse);
        seed_cache(&mut self.cache, devices);
    }

    /// Queue arrivals for everything present at startup.
    ///
    /// Delivered through the ordinary pending-flush path rather than a
    /// hand-rolled batch, so these carry the same clock and the same epoch as
    /// every other batch — an arrivals batch stamped with anything else would
    /// be rejected by the very check that exists to catch stale sessions.
    pub fn queue_initial_arrivals(&mut self) {
        let arrivals: Vec<RawInputEvent> = self
            .cache
            .identities()
            .map(|identity| RawInputEvent::DeviceArrived {
                source_id: Arc::clone(&identity.source_id),
                label: identity.label.clone(),
                kind: identity.kind,
            })
            .collect();
        self.events.extend(arrivals);
    }

    /// One turn of the loop: wait, drain, deliver, then handle control
    /// messages. Returns `false` when the pump should stop.
    pub fn run_once(
        &mut self,
        stop: &AtomicBool,
        sink: &mut impl FnMut(RawInputBatch<'_>),
    ) -> bool {
        // SAFETY: an empty handle array is legal and waits purely on queue
        // input. `QS_ALLINPUT` contains `QS_RAWINPUT`, so `WM_INPUT` wakes it.
        unsafe {
            MsgWaitForMultipleObjectsEx(None, WAKE_BUDGET_MS, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
        }

        self.virtual_screen = virtual_screen_rect();
        self.primary_screen = primary_screen_rect();

        for _ in 0..MAX_SLICES_PER_WAKE {
            if stop.load(Ordering::Acquire) {
                return false;
            }
            if !self.drain_slice(sink) {
                break;
            }
            if !self.control_pass() {
                return false;
            }
        }

        if stop.load(Ordering::Acquire) {
            return false;
        }
        self.control_pass()
    }

    /// Read one buffered slice and deliver it. Returns whether anything was
    /// read, so the caller knows when the queue is genuinely empty.
    fn drain_slice(&mut self, sink: &mut impl FnMut(RawInputBatch<'_>)) -> bool {
        // Stamped before the read, not at sink entry: a stamp taken while
        // folding would record when folding happened rather than when input
        // was captured. One stamp per drain, shared by the whole batch, which
        // is exactly what the Linux backend does per device poll.
        let at_ms = (self.config.clock)();

        let Some(count) = self.read_buffer() else {
            return false;
        };
        if count == 0 {
            return false;
        }

        self.events.clear();
        self.decode_records(count);
        self.notify_raw_input_handled(count);

        let cursor = self.sample_pointer();
        if self.events.is_empty() {
            return true;
        }

        sink(RawInputBatch {
            events: &self.events,
            cursor,
            at_ms,
            epoch: self.config.epoch,
        });
        true
    }

    /// Fill `self.buffer`, growing it when the API says it is too small.
    ///
    /// Sizing is not a one-shot query: `GetRawInputBuffer(NULL, &size, ...)`
    /// returns the minimum for the *first* pending message, not for the whole
    /// batch, and `(UINT)-1` is an error rather than "empty". So the buffer is
    /// reused across drains and grown geometrically on failure.
    fn read_buffer(&mut self) -> Option<u32> {
        loop {
            let mut size = u32::try_from(self.buffer.len() * size_of::<u64>()).unwrap_or(u32::MAX);
            // SAFETY: `self.buffer` is a live `Vec<u64>` — QWORD-aligned by
            // construction, which the record walk depends on — and `size`
            // describes exactly its byte capacity. `cbsizeheader` is the
            // caller-ABI header size the API requires.
            let count = unsafe {
                GetRawInputBuffer(
                    Some(self.buffer.as_mut_ptr().cast::<RAWINPUT>()),
                    &raw mut size,
                    u32::try_from(size_of::<RAWINPUTHEADER>()).unwrap_or(u32::MAX),
                )
            };
            if count != u32::MAX {
                return Some(count);
            }
            if self.buffer.len() >= MAX_BUFFER_QWORDS {
                tracing::warn!(
                    qwords = self.buffer.len(),
                    "raw input buffer hit its ceiling; dropping this slice"
                );
                return None;
            }
            self.buffer.resize(self.buffer.len() * 2, 0);
        }
    }

    /// Walk the records the last read produced.
    fn decode_records(&mut self, count: u32) {
        let capacity = self.buffer.len() * size_of::<u64>();
        let min_size = size_of::<RAWINPUTHEADER>();
        let base = self.buffer.as_ptr().cast::<u8>();
        let mut offset = 0usize;

        for _ in 0..count {
            if offset + min_size > capacity {
                break;
            }
            // SAFETY: `offset` stays within the buffer's byte capacity (checked
            // above and by `next_record` below), the base is QWORD-aligned by
            // the `Vec<u64>` backing, and the API wrote `count` well-formed
            // records starting at the base.
            let record = unsafe { &*base.add(offset).cast::<RAWINPUT>() };
            let dw_size = record.header.dwSize;
            self.decode_one(record);

            match next_record(offset, dw_size, capacity, min_size) {
                RecordStep::Next(next) => offset = next,
                RecordStep::End => break,
                RecordStep::Malformed => {
                    tracing::warn!(dw_size, offset, "malformed raw input record; ending batch");
                    break;
                }
            }
        }
    }

    /// Decode one record into zero or more events.
    fn decode_one(&mut self, record: &RAWINPUT) {
        let handle = record.header.hDevice;
        let source_id = match self.cache.resolve(handle) {
            Some(identity) => identity.source_id,
            // A null handle is legitimate — precision touchpads and some
            // injected input arrive that way — and is not a reliable marker
            // that the input was synthetic, so it gets a stable bucket rather
            // than being dropped.
            None if handle.is_invalid() => self.cache.unknown_source(),
            None => return,
        };

        match RID_DEVICE_INFO_TYPE(record.header.dwType) {
            RIM_TYPEKEYBOARD if self.config.keyboard => {
                // SAFETY: the union discriminant is `header.dwType`, which the
                // arm above matched as `RIM_TYPEKEYBOARD`.
                let keyboard = unsafe { &record.data.keyboard };
                self.decode_keyboard(&source_id, keyboard.MakeCode, keyboard.Flags, keyboard.VKey);
            }
            RIM_TYPEMOUSE if self.config.mouse => {
                // SAFETY: the union discriminant is `header.dwType`, which the
                // arm above matched as `RIM_TYPEMOUSE`.
                let mouse = unsafe { &record.data.mouse };
                let button_flags = {
                    // SAFETY: `Anonymous`/`Anonymous` is the documented layout
                    // for the button fields; both union arms alias the same
                    // `DWORD` and this reads the split form.
                    unsafe { mouse.Anonymous.Anonymous.usButtonFlags }
                };
                let button_data = {
                    // SAFETY: as above.
                    unsafe { mouse.Anonymous.Anonymous.usButtonData }
                };
                self.decode_mouse(
                    &source_id,
                    mouse.usFlags.0,
                    u32::from(button_flags),
                    button_data,
                    mouse.lLastX,
                    mouse.lLastY,
                );
            }
            _ => {}
        }
    }

    fn decode_keyboard(&mut self, source_id: &Arc<str>, make_code: u16, flags: u16, vkey: u16) {
        match classify_key(make_code, flags) {
            KeyReport::Ignored => {}
            KeyReport::Overrun => {
                // The keyboard's own view of held state is now unreliable, so
                // core releases everything this source held, in stream order,
                // before applying anything that follows. Deferring to a "next
                // quiet moment" would leave keys stuck for as long as the user
                // keeps typing — which during a key-mashing burst is the whole
                // time, and mashing keys is the point of half these effects.
                self.events.push(RawInputEvent::StateGap {
                    source_id: Arc::clone(source_id),
                });
            }
            KeyReport::Edge { prefix, pressed } => {
                self.events.push(RawInputEvent::Key {
                    source_id: Arc::clone(source_id),
                    make_code,
                    prefix,
                    vkey,
                    pressed,
                });
            }
        }
    }

    fn decode_mouse(
        &mut self,
        source_id: &Arc<str>,
        flags: u16,
        button_flags: u32,
        button_data: u16,
        last_x: i32,
        last_y: i32,
    ) {
        for (button, pressed) in button_edges(button_flags) {
            self.events.push(RawInputEvent::Button {
                source_id: Arc::clone(source_id),
                button,
                pressed,
            });
        }

        if let Some(delta) = wheel_delta(button_flags, button_data) {
            self.events.push(RawInputEvent::Wheel {
                source_id: Arc::clone(source_id),
                delta_hi_res: delta,
            });
        } else if is_horizontal_wheel(button_flags) {
            tracing::trace!("dropping horizontal wheel: the shared event contract has no axis");
        }

        match motion_kind(flags) {
            MotionKind::Relative => {
                if last_x != 0 || last_y != 0 {
                    self.events.push(RawInputEvent::MotionRelative {
                        source_id: Arc::clone(source_id),
                        dx: last_x,
                        dy: last_y,
                    });
                }
            }
            MotionKind::Absolute(space) => {
                let (norm_x, norm_y) = normalize_absolute(
                    last_x,
                    last_y,
                    space,
                    self.primary_screen,
                    self.virtual_screen,
                );
                self.events.push(RawInputEvent::MotionAbsolute {
                    source_id: Arc::clone(source_id),
                    norm_x,
                    norm_y,
                });
            }
        }
    }

    /// Let the raw input stack do its system-side cleanup for this slice.
    ///
    /// The buffered path has no window procedure to fall through to, so this
    /// is the only place that cleanup can happen.
    fn notify_raw_input_handled(&mut self, count: u32) {
        let base = self.buffer.as_mut_ptr().cast::<RAWINPUT>();
        let pointers: Vec<*const RAWINPUT> = (0..count as usize)
            .scan(0usize, |offset, _| {
                let capacity = self.buffer.len() * size_of::<u64>();
                if *offset + size_of::<RAWINPUTHEADER>() > capacity {
                    return None;
                }
                // SAFETY: `*offset` stays within the buffer's byte capacity,
                // checked immediately above and advanced only by
                // `next_record`, which validates each step.
                let record = unsafe { &*base.cast::<u8>().add(*offset).cast::<RAWINPUT>() };
                let current = record as *const RAWINPUT;
                match next_record(
                    *offset,
                    record.header.dwSize,
                    capacity,
                    size_of::<RAWINPUTHEADER>(),
                ) {
                    RecordStep::Next(next) => {
                        *offset = next;
                        Some(Some(current))
                    }
                    RecordStep::End => {
                        *offset = capacity;
                        Some(Some(current))
                    }
                    RecordStep::Malformed => None,
                }
            })
            .flatten()
            .collect();

        if pointers.is_empty() {
            return;
        }
        // SAFETY: every pointer addresses a record inside the buffer we still
        // own and have not resized since decoding, and `cbsizeheader` is the
        // caller-ABI header size the API requires.
        unsafe {
            DefRawInputProc(
                &pointers,
                u32::try_from(size_of::<RAWINPUTHEADER>()).unwrap_or(u32::MAX),
            );
        }
    }

    /// Sample the cursor, holding the previous value when it is unreadable.
    ///
    /// Gated on mouse consent: registration already declines the mouse usage
    /// for a user who did not consent to pointer capture, and sampling the
    /// cursor anyway would hand pointer position back through the back door on
    /// every keyboard wake.
    fn sample_pointer(&mut self) -> Option<RawCursor> {
        if !self.config.mouse {
            return None;
        }
        if let Some(cursor) = sample_cursor(self.virtual_screen) {
            self.last_cursor = Some(cursor);
        }
        self.last_cursor
    }

    /// Handle control messages without touching `WM_INPUT`.
    ///
    /// Every peek below is message-range filtered. `WM_INPUT` is never inside
    /// one of those ranges, so a report that lands mid-cycle stays queued and
    /// the next drain picks it up. Returns `false` when the pump should stop.
    fn control_pass(&mut self) -> bool {
        let mut message = MSG::default();

        // SAFETY: `message` is a live `MSG` owned by this frame. The filter
        // range restricts removal to `WM_QUIT` alone.
        if unsafe { PeekMessageW(&raw mut message, None, WM_QUIT, WM_QUIT, PM_REMOVE) }.as_bool() {
            return false;
        }

        // SAFETY: as above, restricted to our private stop nudge.
        if unsafe {
            PeekMessageW(
                &raw mut message,
                None,
                WM_HYPERCOLOR_STOP,
                WM_HYPERCOLOR_STOP,
                PM_REMOVE,
            )
        }
        .as_bool()
        {
            return false;
        }

        // SAFETY: as above, restricted to device-change notifications.
        while unsafe {
            PeekMessageW(
                &raw mut message,
                None,
                WM_INPUT_DEVICE_CHANGE,
                WM_INPUT_DEVICE_CHANGE,
                PM_REMOVE,
            )
        }
        .as_bool()
        {
            self.handle_device_change(message.wParam, message.lParam);
        }

        true
    }

    /// Fold a hotplug notification into the cache and the pending events.
    fn handle_device_change(&mut self, w_param: WPARAM, l_param: LPARAM) {
        let handle = HANDLE(l_param.0 as *mut std::ffi::c_void);
        match u32::try_from(w_param.0).unwrap_or_default() {
            GIDC_ARRIVAL => {
                if let Some(identity) = self.cache.resolve(handle) {
                    self.events.push(RawInputEvent::DeviceArrived {
                        source_id: Arc::clone(&identity.source_id),
                        label: identity.label.clone(),
                        kind: identity.kind,
                    });
                }
            }
            GIDC_REMOVAL => {
                if let Some(identity) = self.cache.remove(handle) {
                    self.events.push(RawInputEvent::DeviceRemoved {
                        source_id: identity.source_id,
                    });
                }
            }
            _ => {}
        }
    }

    /// Deliver any events the control pass produced outside a drain slice.
    pub fn flush_pending(&mut self, sink: &mut impl FnMut(RawInputBatch<'_>)) {
        if self.events.is_empty() {
            return;
        }
        let at_ms = (self.config.clock)();
        let cursor = self.sample_pointer();
        sink(RawInputBatch {
            events: &self.events,
            cursor,
            at_ms,
            epoch: self.config.epoch,
        });
        self.events.clear();
    }
}

impl Drop for Pump {
    /// Deregister, then destroy the window — in that order, and only if we
    /// still own the registration.
    ///
    /// Removal is process-scoped, which is precisely why a stale worker must
    /// not call it: it would deregister whichever session is currently live.
    /// The ownership check and the removal happen inside the same claim lock,
    /// so a replacement cannot register into the gap between them.
    /// `DestroyWindow` needs no such guard — the window is thread-affine to
    /// this worker and belongs to nobody else.
    fn drop(&mut self) {
        if self.registered {
            let entries = self.registration_entries(RIDEV_REMOVE, None);
            let removed = PROCESS_CLAIM.release(self.generation, || {
                // SAFETY: `entries` is a live slice of `RAWINPUTDEVICE` values
                // with `hwndTarget` null, which `RIDEV_REMOVE` requires.
                unsafe {
                    RegisterRawInputDevices(
                        &entries,
                        u32::try_from(size_of::<RAWINPUTDEVICE>()).unwrap_or(u32::MAX),
                    )
                }
            });
            match removed {
                Ok(true) => tracing::debug!("deregistered raw input"),
                Ok(false) => tracing::debug!(
                    generation = self.generation,
                    "stale raw input worker skipped deregistration; a replacement owns it"
                ),
                Err(error) => tracing::warn!(%error, "raw input deregistration failed"),
            }
        }

        // SAFETY: the window was created on this thread and is destroyed on it;
        // a cross-thread call would fail cleanly rather than corrupt anything.
        if let Err(error) = unsafe { DestroyWindow(self.window) } {
            tracing::debug!(%error, "destroying the raw input window failed");
        }
    }
}

/// Create the message-only window the registration targets.
///
/// `RIDEV_INPUTSINK` is what makes background capture work, and it requires a
/// non-null `hwndTarget` that outlives the registration — hence a real window,
/// even though nothing is ever drawn into it.
fn create_message_window() -> RawInputResult<HWND> {
    register_window_class()?;

    // SAFETY: the class was registered above, `HWND_MESSAGE` is the documented
    // parent for a message-only window, and every other argument is a null or
    // zero the API accepts.
    let window = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            WINDOW_CLASS,
            w!("hypercolor raw input"),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            None,
            None,
        )
    }
    .map_err(|error| RawInputError::WindowCreation(error.to_string()))?;

    Ok(window)
}

/// Register the window class once per process.
///
/// `RegisterClassW` fails with `ERROR_CLASS_ALREADY_EXISTS` on the second
/// call, and capture toggles with effect demand so the worker really is
/// created and destroyed repeatedly within one process lifetime. The
/// `OnceLock` is the mechanism; treating that specific error as success is
/// belt and braces.
fn register_window_class() -> RawInputResult<()> {
    use std::sync::OnceLock;
    static REGISTERED: OnceLock<Result<(), String>> = OnceLock::new();

    const ERROR_CLASS_ALREADY_EXISTS: i32 = 1410;

    REGISTERED
        .get_or_init(|| {
            // SAFETY: takes the module handle of the current process; the
            // `None` argument is documented to mean exactly that.
            let instance = unsafe { GetModuleHandleW(None) }.map_err(|error| error.to_string())?;

            let class = WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                hInstance: instance.into(),
                lpszClassName: WINDOW_CLASS,
                ..Default::default()
            };
            // SAFETY: `class` is fully initialized and lives for the duration
            // of the call; `lpszClassName` points at a `'static` wide literal.
            let atom = unsafe { RegisterClassW(&raw const class) };
            if atom != 0 {
                return Ok(());
            }
            let error = windows::core::Error::from_thread();
            if error.code().0 & 0xFFFF == ERROR_CLASS_ALREADY_EXISTS {
                Ok(())
            } else {
                Err(error.to_string())
            }
        })
        .clone()
        .map_err(RawInputError::WindowCreation)
}

/// Nothing is handled here: the drain reads `WM_INPUT` from the queue with
/// `GetRawInputBuffer` rather than through a window procedure, and control
/// messages are peeked explicitly.
extern "system" fn window_proc(
    window: HWND,
    message: u32,
    w_param: WPARAM,
    l_param: LPARAM,
) -> LRESULT {
    // SAFETY: the default handler is always safe to call with the arguments
    // the system just handed us.
    unsafe { DefWindowProcW(window, message, w_param, l_param) }
}
