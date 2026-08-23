# 72 - Windows Host Input (Raw Input)

**Status:** IMPLEMENTED (W0-W3); W4 acceptance partially run
**Author:** Nova
**Date:** 2026-07-25
**Crates:** `hypercolor-windows-input` (new), `hypercolor-core`, `hypercolor-daemon`, `hypercolor-ui`
**Related:** Spec 71 (interactive input pipeline) — this is W6's Windows half.
Spec 58 sets the Windows interop-crate precedent; `hypercolor-windows-capture`
sets the shape. [Spec 76](76-macos-screen-capture-and-host-input.md) is the macOS
authority for native host input and the final `device_query` retirement.

## Problem

Windows has no host input backend. `build_interaction_source`
(`daemon/src/startup/services.rs:630`) hands every non-Linux platform
`InteractionInput`, the device_query polling bridge — which spec 71 already
classified as interim and slated for deletion. On Windows that bridge is
strictly worse than the Linux path in four ways that matter:

1. **No events.** It samples held state at 100 Hz and diffs. Press/release
   ordering, repeat counts, and capture timestamps do not exist, so
   `InteractionBatch` arrives empty and `engine.keyboard.events` has nothing
   to deliver. Every timestamp-derived SDK helper (`pressEnvelope()`,
   `typingRate()`) degrades to nothing on Windows.
2. **No pointer position.** `mouse_data_from_state` hard-codes
   `PointerMode::None` with `norm_x`/`norm_y` at zero
   (`core/src/input/interaction/mod.rs:268`). `iMouse` stays dead.
3. **No wheel, no per-device identity.** `source_id` does not exist, so
   multi-keyboard union semantics and the phase-2 key-to-LED mapping hook have
   nothing to key on.
4. **No health surface.** `interaction_diagnostics` reports
   `devices_opened: usize::from(worker.is_some())` — a boolean wearing a
   count's clothes. It cannot distinguish "working", "no hardware", and
   "structurally unable to see input".

Meanwhile the whole downstream pipeline — folding, batching, generation
counters, demand gating, WS privacy, Servo payload v2, the SDK input module —
already shipped and is platform-independent. Windows needs one event producer.

## Verified current state

- `EvdevHostInput` (`core/src/input/evdev.rs`, 1004 lines) is the reference
  implementation: worker thread folds into `Arc<Mutex<SharedState>>`, render
  thread takes one lock in `sample_and_drain_with_delta_secs` for both the
  snapshot and the event batch, held state is tracked per `source_id` and
  unioned, vanished devices synthesize releases, and capture is demand-gated
  through `set_interaction_capture_active`.
- `hypercolor-windows-capture` establishes the interop-crate pattern the
  workspace's `unsafe_code = "forbid"` requires: `unsafe_code = "allow"` plus
  `undocumented_unsafe_blocks = "deny"` locally, Windows-only modules behind
  `#[cfg]`, a `stubs.rs` returning `UnsupportedPlatform` so every other target
  still compiles, and an `examples/` binary for hardware smoke testing.
- `windows` 0.62.2 (already a workspace dependency, pinned — see the resolver
  note at `Cargo.toml:110`) exposes the entire Raw Input surface. Verified
  present: `RegisterRawInputDevices`, `GetRegisteredRawInputDevices`,
  `GetRawInputBuffer`, `GetRawInputData`, `GetRawInputDeviceList`,
  `GetRawInputDeviceInfoW`, `RAWINPUT`/`RAWMOUSE`/`RAWKEYBOARD`/
  `RAWINPUTHEADER`, `RIDEV_INPUTSINK`, `RIDEV_DEVNOTIFY`, `RIDI_DEVICENAME`,
  `RIM_TYPEKEYBOARD`/`RIM_TYPEMOUSE` under `Win32_UI_Input`; `WM_INPUT`,
  `WM_INPUT_DEVICE_CHANGE`, `GIDC_ARRIVAL`, `RI_KEY_BREAK`, `RI_MOUSE_WHEEL`,
  `HWND_MESSAGE`, `GetCursorPos`, `MsgWaitForMultipleObjectsEx`,
  `SM_*VIRTUALSCREEN`, `SetThreadDpiAwarenessContext` under
  `Win32_UI_WindowsAndMessaging`; `GetProcessWindowStation` and
  `GetUserObjectInformationW` under `Win32_System_StationsAndDesktops`. No new
  third-party dependency is required.
- Constants this spec depends on were read from the installed SDK header
  (`WinUser.h`, 10.0.26100.0) rather than recalled: `WHEEL_DELTA` 120,
  `KEYBOARD_OVERRUN_MAKE_CODE` `0xFF`, `RI_KEY_E1` 4,
  `RI_KEY_TERMSRV_SET_LED` 8, `RI_KEY_TERMSRV_SHADOW` `0x10`,
  `RI_MOUSE_BUTTON_4_*` `0x40`/`0x80`, `RI_MOUSE_BUTTON_5_*` `0x100`/`0x200`,
  and `RAWINPUT_ALIGN` = `sizeof(QWORD)` under `_WIN64`. The same header
  confirms `RID_DEVICE_INFO_KEYBOARD`/`_MOUSE` carry no name field.
- CI builds `x86_64-pc-windows-msvc` only (`ci.yml:950`), and runs no Windows
  clippy or test job on pull requests at all — see W0.

## D1. Session model — the constraint everything else answers to

**Raw Input cannot cross session 0.** A Windows service running as LocalSystem
lives in session 0, which has its own window station and desktop and never
sees a single user keystroke or mouse movement. Critically, this fails
*silently*: `CreateWindowExW` succeeds, `RegisterRawInputDevices` succeeds, and
`WM_INPUT` simply never arrives. There is no error to report.

`scripts/install-windows-service.ps1` installs exactly that configuration, and
guards it behind `-AllowSystemDaemon` with the message "intended only as a
temporary Windows SMBus test path... keep using the foreground daemon while we
split SMBus into a narrow hardware broker" (line 297). So the supported
deployment — foreground daemon in the user's own session — is the one where
Raw Input works, and the service mode is the one where it cannot.

Consequences this spec accepts and encodes:

- **The probe is the window station, not the session id.** The intuitive test
  — `ProcessIdToSessionId(GetCurrentProcessId()) == 0` — is not sufficient: a
  service or a scheduled task configured to "run whether the user is logged on
  or not" gets a *non-interactive window station* inside a perfectly ordinary
  non-zero session, and sees exactly as little input as a session-0 service.
  So the backend calls `GetProcessWindowStation` and
  `GetUserObjectInformationW(UOI_FLAGS)` and requires `WSF_VISIBLE`. That test
  subsumes the session-0 case, catches the scheduled-task case the session id
  misses, and is the thing Raw Input actually depends on. The session id is
  still collected, but only as a diagnostic string.
- **`WTSGetActiveConsoleSessionId` is deliberately not used.** Comparing
  against the console session would wrongly condemn a legitimate RDP user, who
  has their own interactive session and their own visible `WinSta0`. D7 keeps
  RDP virtual devices for the same reason.
- Detection must be explicit, because no API call fails. Probing by "did we
  receive input" would need an unbounded wait and could not distinguish an
  idle user from an isolated session.
- The failed probe is reported as a **degraded backend state**, never a
  `start()` error (spec 71's rule: start errors roll back the entire input
  graph). No worker thread is spawned; diagnostics say why.
- **UIPI:** an unelevated daemon cannot observe input destined for an elevated
  window. Typing into an admin terminal produces nothing. Documented and
  accepted. It is not strictly unfixable — a signed binary installed to a
  secure location can request `uiAccess` — but we are choosing not to, because
  a lighting daemon has no business holding an input-integrity exemption.
- **Secure desktop:** UAC prompts, the lock screen, and Ctrl+Alt+Del are
  invisible to Raw Input, and `GetCursorPos` returns access-denied there. This
  is the correct privacy behavior, not a defect. The backend holds its last
  cursor position rather than blanking it, so effects do not lurch on unlock.

## D2. Crate split

Same seam as screen capture: unsafe COM/Win32 plumbing in an audited interop
crate, all semantics in `hypercolor-core` where they are pure and testable.

**`crates/hypercolor-windows-input`** (new) — owns the message-only window,
Raw Input registration, `RAWINPUT` decoding, device-identity resolution,
hotplug, and cursor sampling. Emits a plain Rust event vocabulary with no
Windows types in the public API. `stubs.rs` mirrors the capture crate so
Linux/macOS builds still compile.

```rust
/// Which scan-code prefix a key report carried.
pub enum RawKeyPrefix { None, E0, E1 }

pub enum RawInputEvent {
    Key {
        source_id: Arc<str>,
        make_code: u16,
        prefix: RawKeyPrefix,
        vkey: u16,          // logical; only consulted when make_code is unusable
        pressed: bool,      // raw hardware edge; repeat is derived in core
    },
    Button { source_id: Arc<str>, button: RawButton, pressed: bool },
    Scroll {
        source_id: Arc<str>,
        delta_x_q16_16: i64,
        delta_y_q16_16: i64,
    },
    /// Relative counts from a normal mouse.
    MotionRelative { source_id: Arc<str>, dx: i32, dy: i32 },
    /// Absolute position from a tablet / RDP / VM pointer, already normalized
    /// to `[0,1]²` against whichever rect `MOUSE_VIRTUAL_DESKTOP` selected.
    MotionAbsolute { source_id: Arc<str>, norm_x: f32, norm_y: f32 },
    DeviceArrived { source_id: Arc<str>, label: String, kind: RawDeviceKind },
    DeviceRemoved { source_id: Arc<str> },
    /// Ordered barrier: everything this source had held is now unknown.
    /// Core releases that source's held keys and buttons and resets its
    /// absolute baseline before applying any later event in the batch.
    /// Emitted only for a keyboard overrun report (D4).
    StateGap { source_id: Arc<str> },
}

/// One coherent drain: the events, the cursor as of the same drain, and the
/// capture stamp taken before the drain read anything.
pub struct RawInputBatch<'a> {
    pub events: &'a [RawInputEvent],
    pub cursor: Option<RawCursor>,
    pub at_ms: u64,
    /// The epoch core handed to `start()`, echoed back so core can reject a
    /// batch from a session it no longer owns.
    pub epoch: u64,
}

pub struct RawCursor {
    pub x: i32,        // signed: the virtual desktop starts at negative
    pub y: i32,        // coordinates whenever a monitor sits left of primary
    pub norm_x: f32,
    pub norm_y: f32,
}

pub struct RawInputConfig {
    pub keyboard: bool,
    pub mouse: bool,
    /// Core's monotonic clock, called on the pump immediately before each
    /// drain. Injected because `input_mono_ms` lives in core and the
    /// dependency cannot run the other way.
    pub clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    /// Allocated by core; echoed in every batch.
    pub epoch: u64,
}

pub struct RawInputSession { /* control + join handles only; no HWND */ }

impl RawInputSession {
    pub fn start(
        config: RawInputConfig,
        sink: impl FnMut(RawInputBatch<'_>) + Send + 'static,
    ) -> RawInputResult<Self>;
    pub fn device_count(&self) -> usize;
    pub fn worker_state(&self) -> WorkerState;  // Running | Failed(String)
}

pub fn interactive_session_state() -> SessionState;  // Interactive | NoInteractiveSession
```

Every decision in this shape is load-bearing, and the obvious simpler version
of each one is wrong in a way that would surface as an API break mid-build:

**Keys carry `make_code`, `prefix`, and `vkey` — not `(scancode, extended)`.**
A boolean `extended` cannot distinguish `E0` from `E1`, and without `vkey` the
`MakeCode == 0` fallback and the Pause identification that D4 specifies are
both impossible. The simpler struct would have forced an API break the moment
W2 tried to implement D4.

**`pressed` is the raw hardware edge; repeat is derived in core.** evdev has
three states (`InputButtonState::{Pressed, Released, Repeated}`), and
`evdev.rs:598-617` treats `Repeated` specially: it does not re-insert into
`pressed_keys` and, critically, does **not** append to `recent_keys` — so a
held key fires `onKeyPress` once, not sixty times a second. Windows delivers
auto-repeat as ordinary make codes with no repeat marker, so core classifies a
press whose key is already in that `source_id`'s held set as `Repeated` and
excludes it from recents. Without this, holding a key would machine-gun every
`recent_keys`-driven effect on Windows and not on Linux.

**Motion is two variants.** Relative counts and absolute positions need
different normalization and different baseline handling, and a single
`Motion { dx, dy }` cannot carry both. Absolute normalization happens in the
interop crate because that is where the `MOUSE_VIRTUAL_DESKTOP` flag and the
screen rect live; core then differences successive normalized positions
directly rather than dividing 0..65535 counts by evdev's 1200, which would be
meaningless. Per-device absolute baselines reset on arrival, removal, and any
coordinate-space flag change, and the first absolute report after a reset
emits no delta.

**The cursor rides in the batch, not in a separate atomic.** An atomic read by
the render thread while events came from the mutex would let one frame combine
cursor state from one instant with event state from another — breaking exactly
the invariant `sample_and_drain_with_delta_secs` exists to uphold ("an edge can
never be in the batch while missing from held state"). The cursor is sampled
once per drain, handed over in the same call, and folded under the same lock.

**Timestamps are core's clock, read on the pump before the drain.**
`input_mono_ms` (`core/src/input/mod.rs:45`) is a process-wide epoch living in
`hypercolor-core`, and the interop crate must not depend on core — the
dependency runs the other way. So core injects the clock as a callback in
`RawInputConfig`, and the pump calls it *immediately before* reading the
buffer, not when the sink is entered. That distinction matters: a stamp taken
at sink entry would record when folding happened rather than when input was
captured. Reading it pre-drain is exactly evdev's shape, which takes `at_ms`
once per device poll before `fetch_events` (`evdev.rs:515`) and shares that one
stamp across the whole fetched batch.

**Motion needs no wire aggregate.** Because folding is direct, core applies
each `MotionRelative` straight into `MotionAggregate` (`traits.rs:234-242`),
summing `dx`/`dy` and accumulating `distance` per event — the same arithmetic
`evdev.rs:643-649` performs. There is no coalescing step to lose path length
in, so two equal-and-opposite deltas correctly produce zero net displacement
with non-zero distance, and a fast shake still reports velocity.

**Both scroll axes stay exact.** `RI_MOUSE_WHEEL` and `RI_MOUSE_HWHEEL` map to
the vertical and horizontal axes of `InputEvent::PointerScroll`, respectively.
The event retains signed Q16.16 `Line120` units through the shared types,
LightScript payload, WebSocket protocol, and SDK.

**The sink takes a batch, not one event.** This is the whole point of the
buffered read and it is easy to get wrong. `evdev.rs:522` acquires
`shared.lock()` *once per fetched device batch* and folds every event inside
that one guard. A per-event `FnMut(RawInputEvent)` would make the core side
lock once per event — reintroducing, one layer up, exactly the per-event
overhead that justified `GetRawInputBuffer` in the first place, and at 8 kHz
that is 8000 lock round-trips a second contending with the render thread for
the same mutex. One sink call per drain, one lock per drain. Changing this
after W1 is a breaking API change, so it is decided here.

**The sink runs on the pump thread, and folds directly.** An earlier draft put
a bounded ring and a second folding thread between the pump and the sink, to
stop a blocking sink from stalling the pump. That was over-built, and it
cascaded: a queue that may drop events needs loss-barrier semantics, a
detached folding thread needs epoch guards to stop it mutating a restarted
session, and coalesced motion needs a wire representation that raw deltas
cannot carry. Three rounds of review kept finding defects in that machinery
rather than in the input handling.

The premise was wrong. The sink is not arbitrary user code — it is
`WindowsHostInput`'s fold, and the only thing it blocks on is the same
`Mutex<SharedState>` the render thread takes for the duration of
`build_snapshot`, which is bounded and short. **This is precisely what evdev
does**: `poll_devices` (`evdev.rs:514-528`) locks and folds on the poll thread
with no queue, no handoff, and no epoch, and that is the structural parity this
spec claims. Reproducing it removes the ring, the second thread, the loss
barrier for overflow, and the motion-aggregate wire format in one move.

What the removal costs, stated honestly: a render thread holding the lock
delays the pump for that interval, so a pathological stall in `build_snapshot`
becomes input latency rather than dropped input. That is the same trade Linux
already makes, and dropped input is the worse failure. A panic while folding is
caught at the pump boundary, marks `worker_state` as `Failed`, and makes
subsequent core samples report the source unavailable rather than silently
flatlining.

Because nothing is queued, nothing is dropped, so `StateGap` (below) survives
only for the case that genuinely loses information: a keyboard overrun report,
which is decode-time and therefore naturally ordered within its batch.

**Lifecycle is explicit, and a wedged sink cannot corrupt the next session.**
`start()` blocks on a readiness handshake — the same shape as `evdev.rs:267`'s
`ready_rx.recv_timeout` — and returns the worker's actual initialization error
rather than succeeding ahead of window creation and registration. `stop()` and
`Drop` are idempotent and teardown is worker-owned. The `HWND` never leaves the
worker thread; `RawInputSession` holds control, join, and snapshot handles only.

The bounded join creates a hazard that has to be closed explicitly. If the pump
is wedged — blocked in a driver call, or folding behind a render thread that
itself wedged — a bounded join must give up and detach it, and a detached
thread could later wake and mutate core state belonging to a *restarted*
session. Capture toggles with effect demand, so restart is routine, not exotic.
The epoch closes it, and *where* it is checked is the whole point:

- **Core allocates the epoch** and passes it in `RawInputConfig`; the session
  echoes it in every batch. Core does not learn it from the session, so there
  is no window where the two disagree about which session is current.
- **Core advances the epoch atomically with clearing held state**, under the
  same `SharedState` lock, whenever capture stops or a session is replaced.
- **The check happens under that lock, immediately before mutation** — not on
  entry to the sink. Checking before acquiring the lock would race exactly the
  restart it exists to guard: a zombie could pass the check, block on the
  mutex, and wake up after core had already rotated the epoch. Validating
  after the lock is held makes a stale batch inert by construction.

A detached thread also marks `worker_state` as `Failed`, so core reports the
source unavailable rather than pretending a live session.

**The epoch guards core state; a second guard is needed for the registration
itself.** Raw Input registration is process-global per `(usUsagePage,
usUsage)` (D3), and that turns a detached pump into a live hazard the epoch
cannot touch: the old pump eventually unwedges, runs its teardown, and calls
`RegisterRawInputDevices` with `RIDEV_REMOVE` — deregistering the *replacement*
session's registration. Core state stays uncorrupted and every subsequent batch
is epoch-rejected, so nothing looks broken; input simply stops arriving, from a
thread nobody is watching. Capture toggles routinely, so this is reachable.

So registration carries its own **ownership generation**, a process-wide
`Mutex<Option<u64>>` in the interop crate. A worker claims it when it
registers, and teardown deregisters **only if it still holds the claim** — a
stale worker skips `RIDEV_REMOVE` entirely and destroys just its own window,
which is thread-affine to it and harmless.

**The claim must be a lock, not a compare-and-swap.** An atomic CAS orders the
claim against itself but not against `RegisterRawInputDevices`, which is the
process-global state actually being contended, and no ordering of the two
survives:

- *Claim first, then register.* If the replacement's registration then fails,
  the claim has already been rotated, so the old worker's teardown declines to
  call `RIDEV_REMOVE` and the previous registration is orphaned — still
  pointing at a window about to be destroyed, with no owner willing to remove
  it.
- *Register first, then claim.* Between the two, a stale worker can observe
  itself as owner, pass the check, and call `RIDEV_REMOVE` — tearing down the
  replacement's registration that landed microseconds earlier.

Both windows are microseconds wide and both are reachable, because capture
toggling is exactly the workload that creates a replacement while the old pump
is still unwinding.

The fix is to make the pair atomic rather than each half atomic. One
process-wide `Mutex` covers two critical sections:

- **Startup:** acquire → `RegisterRawInputDevices(...)` → on success publish
  the new generation as owner → release. On registration failure the claim is
  left untouched, so the previous owner keeps both the registration and the
  responsibility for removing it, and `start()` returns the error with no
  process-global state mutated.
- **Teardown:** acquire → compare our generation against the owner → if we
  match, `RegisterRawInputDevices(RIDEV_REMOVE)` and clear the owner → release.
  A non-match releases immediately and touches nothing.

Because the owner check and the removal are inside the same critical section as
any competing registration, the interleavings above are unrepresentable rather
than merely unlikely. The lock is held only across two syscalls that do not
block on input, so it never couples the pump threads to each other. `DestroyWindow`
stays outside the lock — it is thread-affine and unrelated to the process-global
registration.

Generation-qualified ownership is the house pattern rather than a new
invention: the render pipeline's GPU sampling readback slots carry generations
for exactly this shape, because retired mappings can outlive ring replacement
and a stale completion must never mutate the active ring. Same hazard, same
remedy — a retired owner must prove it still owns the thing before touching it.
The difference here is that the protected resource lives in the OS rather than
in our address space, so the proof and the action have to be taken together.

**Capture toggles invalidate held state atomically.** Spec 71 requires a source
to clear all held state when capture goes inactive. Combined with repeat
derivation, that creates an edge case: a key physically held across a
disable/enable cycle produces a make code with no matching held entry, which
core would classify as a fresh `Pressed` and push into `recent_keys`. That is
the correct and unavoidable behaviour — we cannot observe what happened while
we were not listening — but it must be *chosen* rather than discovered. On
reactivation core starts from empty held state and treats the first report for
any key as a press. The epoch check above prevents pre-disable input still in
flight from repopulating state that was just cleared.

`Arc<str>` for `source_id` keeps per-event cost to a refcount bump. Raw
`HANDLE`s never escape the crate. Windows recycles handles after removal, so
cache entries are generation-tagged: a record still sitting in a buffered drain
that references a handle already removed and reissued resolves to the dead
device's generation and is dropped, rather than pouring a new device's keys
into the old device's held set.

**Devices present at registration are enumerated, not discovered lazily.**
`RIDEV_DEVNOTIFY` only fires on *change*, so an already-attached mouse produces
no `GIDC_ARRIVAL`. Resolving it lazily on first input would mean an idle
attached mouse leaves core with no pointer at all until the user moves it —
whereas `evdev.rs:501` establishes `pointer_present` from the device list at
scan time. The worker therefore walks `GetRawInputDeviceList` before signalling
readiness and emits a `DeviceArrived` for each registered-kind device.

**`crates/hypercolor-core/src/input/windows.rs`** — `WindowsHostInput`,
implementing `InputSource`. Structurally a sibling of `evdev.rs`: the same
`SharedState`, the same per-`source_id` `BTreeMap<String, BTreeSet<String>>`
held sets, the same `HeldStateKey` generation logic, the same
`synthesize_releases` on device removal, the same `push_event` overflow
accounting. Differences are confined to the event source and the pointer model.

**Key-name tables live in core, and are pure.**
`scancode_key_name(make_code: u16, prefix: RawKeyPrefix, vkey: u16) ->
KeyNameResult` takes no Windows types — `RawKeyPrefix` is a plain enum
re-exported from the interop crate's platform-independent surface — so it
compiles and unit-tests on Linux CI. The three-state prefix is carried all the
way through; collapsing it back to a boolean at the core boundary would
reintroduce exactly the E1 blindness the event contract was widened to fix.
`KeyNameResult` distinguishes a positional name, a logical `VKey`-derived
name, an unknown-key name, and "discard" (overrun), so callers cannot
accidentally treat a fallback as positional.

## D3. Raw Input mechanics

**Registration.** A message-only window (`HWND_MESSAGE` parent) on the worker
thread, then `RegisterRawInputDevices` with two entries:

| Usage page | Usage | Meaning  | Flags                            |
| ---------- | ----- | -------- | -------------------------------- |
| `0x01`     | `0x06`| Keyboard | `RIDEV_INPUTSINK \| RIDEV_DEVNOTIFY` |
| `0x01`     | `0x02`| Mouse    | `RIDEV_INPUTSINK \| RIDEV_DEVNOTIFY` |

Entries are included only for the kinds `[input].keyboard` / `[input].mouse`
enable, so declining pointer capture means the process is never registered for
mouse input at all — the honest privacy posture, not a filter applied after
the fact.

`RIDEV_INPUTSINK` is what makes background capture work and is why
`hwndTarget` must be non-null and outlive the registration. `RIDEV_DEVNOTIFY`
delivers `WM_INPUT_DEVICE_CHANGE`, replacing evdev's 2-second `RESCAN_TICKS`
poll with real event-driven hotplug.

**Device coverage boundary.** Those two top-level collections are keyboards
and mice, and nothing else. Media and OEM keys that a keyboard exposes through
a separate Consumer Control collection (`0x0C/0x01`) arrive as `RIM_TYPEHID`
with a vendor-defined report layout, not as `RAWKEYBOARD`. We do not register
for them and we never decode a `RIM_TYPEHID` record as a keyboard record. So
"play/pause lights up the keyboard" is out of scope for this spec, and any
future support is a separate consented usage entry plus a HID report parser.

**`RIDEV_NOLEGACY` is never set** because we do not need it. The correcting
detail, since the first draft of this spec got the reasoning wrong: it
suppresses the legacy `WM_KEYDOWN`/`WM_MOUSEMOVE` stream only for *this*
process, not system-wide. Omitting it is therefore not what protects the user's
other applications — nothing we could have set would have broken them. We omit
it because a pure observer has no reason to opt out of messages it already
ignores. `RIDEV_EXINPUTSINK` is also omitted: it delivers background input only
while the foreground application is *not* itself registered for raw input,
which is strictly weaker than the always-background behavior we need.

**Message loop, in this order.** The ordering is load-bearing and the obvious
arrangement is wrong:

1. `MsgWaitForMultipleObjectsEx(&[], WAKE_BUDGET_MS, QS_ALLINPUT,
   MWMO_INPUTAVAILABLE)`. An empty handle array is legal and waits purely on
   queue input. `QS_ALLINPUT` contains `QS_INPUT` which contains
   `QS_RAWINPUT`, so `WM_INPUT` wakes it. `MWMO_INPUTAVAILABLE` is what closes
   the classic lost-wakeup race where input was already observed but not yet
   removed.
2. **Drain `GetRawInputBuffer` in bounded slices.** This happens *before* any
   `PeekMessageW`, because a `PeekMessageW(PM_REMOVE)` pass would remove the
   pending `WM_INPUT` messages and the buffered read would then see nothing.
   A fully successful drain clears `QS_RAWINPUT`, so this does not spin.

   **The drain is bounded, not "until it returns 0".** An 8 kHz mouse plus a
   held key can produce input as fast as we consume it, and an unbounded drain
   would then never reach step 3 — starving the stop flag, the stop nudge, and
   `WM_QUIT` for as long as the user keeps moving the mouse. Shutdown would
   hang exactly when the machine is busiest. So each iteration reads at most a
   fixed slice of records, checks the stop flag and the filtered control
   messages between slices, and returns to the wait only once the queue is
   genuinely empty. Input is never dropped by this — unread records stay
   queued for the next slice.

   That "never dropped" is an **application-level** guarantee, and the limit is
   worth stating plainly. Since folding happens on this thread, core-mutex
   contention delays the next slice: at 8 kHz a 1 ms stall queues roughly 8
   reports and 10 ms queues roughly 80, which the OS backlog absorbs as
   latency. A *prolonged* stall can exhaust that backlog, and then Windows
   drops reports before we ever see them — no application-side queue could have
   saved those, which is why the fix for pump stalls is keeping fold work and
   lock hold times short rather than adding buffering. W1 carries an acceptance
   test asserting a bound on slice duration and on core lock-hold time.

   After each successfully decoded slice, and before the buffer is reused, the
   worker calls **`DefRawInputProc(paRawInput, nInput, cbSizeHeader)`**. The
   buffered path has no window procedure to fall through to, so this is the
   only place the system-side cleanup the raw input stack expects can happen.
3. **The control pass never removes `WM_INPUT`.** A broad
   `PeekMessageW(PM_REMOVE)` would silently eat any `WM_INPUT` that arrived in
   the window between step 2's last read and the peek — dropping real input
   with no error. So step 3 removes only explicitly targeted control messages,
   using message-range-filtered `PeekMessageW` calls for
   `WM_INPUT_DEVICE_CHANGE`, the private `WM_APP` stop nudge, and `WM_QUIT`.
   `WM_INPUT` is never in a filtered range, so a report that lands mid-cycle
   simply stays queued and is picked up by the next iteration's step 2.

The bounded 100 ms wake guarantees the worker notices a stop request even if
the nudge is lost, and unlike a peek-and-sleep loop it costs nothing while the
user is idle. `stop()` sets the flag and `PostMessageW`s the `WM_APP` nudge so
the common path tears down immediately rather than after the budget.

**Buffered reads.** An 8 kHz mouse is a real product; one message round-trip
per report is the kind of per-event overhead that becomes a throttling
argument later, and we would rather not have that argument. Four invariants,
each with a unit test on the pure arithmetic:

- **Sizing is not a one-shot query.** `GetRawInputBuffer(NULL, &size, ...)`
  returns 0 and sets `size` to the minimum for the *first* pending message,
  not for the whole batch. The implementation keeps a reusable buffer, grows
  it geometrically, and retries on `(UINT)-1` rather than treating that
  sentinel as "empty".
- `cbSizeHeader` is always the caller-ABI `sizeof(RAWINPUTHEADER)`.
- The buffer must be QWORD-aligned. We back it with a `Vec<u64>` and cast,
  rather than a `Vec<u8>` whose alignment is not guaranteed.
- **Record stride is `RAWINPUT_ALIGN`, which is target-dependent.** The SDK
  defines `NEXTRAWINPUTBLOCK` as aligning the *address* `ptr + dwSize` up to
  `sizeof(QWORD)` under `_WIN64` and `sizeof(DWORD)` otherwise. Rounding
  `dwSize` up to 8 is equivalent only because our buffer base is 8-aligned,
  and only on x64. We ship `x86_64-pc-windows-msvc` exclusively
  (`ci.yml:950`), so the crate carries a `compile_error!` on
  `target_pointer_width = "32"` rather than silently computing a 4-byte
  stride. WOW64 is excluded by the same gate; we never run as a 32-bit
  process on 64-bit Windows.

**Window class registration.** `RegisterClassW` fails with
`ERROR_CLASS_ALREADY_EXISTS` on the second call. Capture toggles with effect
demand, so the worker is created and destroyed repeatedly within one process
lifetime and this *will* happen. Registration is guarded by a `OnceLock`; that
specific error is also treated as success for belt and braces.

**Teardown order.** `RegisterRawInputDevices` with `RIDEV_REMOVE` (which
requires `hwndTarget = NULL`), then `DestroyWindow`. Removal is process-scoped
and could legally be called from any thread; `DestroyWindow` is the one that
is owner-thread restricted, and a cross-thread call fails cleanly rather than
corrupting anything. We do both on the worker thread anyway so the sequence is
deterministic. Destroying the window is **not** a documented substitute for
`RIDEV_REMOVE`, so removal is explicit and its failure is logged rather than
assumed.

The removal step is gated on the ownership generation from D2: precisely
*because* removal is process-scoped, a stale worker calling it would deregister
whichever session is currently live. Only the claim holder removes, and the
owner check and the `RIDEV_REMOVE` call happen inside the same registration
lock, so a replacement cannot register into the gap between them.

**Message-only windows do not receive broadcasts.** `WM_DISPLAYCHANGE` will
never arrive, so virtual-desktop metrics cannot be cached against it. The
worker re-reads the virtual-screen metrics on each wake instead; they resolve
from user32 shared memory and cost nothing. See D5 for which metrics, and the
DPI context they must be read under.

**Registration is process-global per `(usUsagePage, usUsage)` pair**, not per
usage page. A second `RegisterRawInputDevices` call for `0x01/0x06` from any
thread or window in this process makes *its* target window the process's sole
recipient for keyboards, silently. Only the input worker registers today; the
crate verifies its own registration with `GetRegisteredRawInputDevices` after
`start()` so a future Tauri window that steals it fails loudly instead of
producing a daemon that mysteriously stops seeing keys.

## D4. Key naming — positional, via scan code

Canonical names in this codebase are **physical-position** names:
`canonical_evdev_key_name` maps `KEY_A → "a"`, `KEY_LEFTCTRL → "ControlLeft"`
(`evdev.rs:835`), and evdev keycodes are positional. `KeyboardEvent.code`
semantics, effectively.

Windows Raw Input offers two identifiers, and only one of them is positional:

- `VKey` is **layout-dependent**. On AZERTY, the key in the QWERTY-`Q`
  position reports `VK_A`. Mapping VK→name would make a French keyboard report
  different names than Linux for the same physical key, silently breaking
  `wasdVector()` and every positional effect.
- `MakeCode` is the **set-1 scan code** — the physical position, plus a
  `RI_KEY_E0` flag in `Flags` for the extended block (arrows, right-hand
  modifiers, navigation cluster).

So the mapping is `(MakeCode, E0) → canonical name`. The W3C UI Events `code`
specification publishes the authoritative Windows scan-code column, and we
transcribe from it rather than from memory.

Four cases the decoder handles explicitly, before the table is consulted:

- **`MakeCode == KEYBOARD_OVERRUN_MAKE_CODE` (`0xFF`) is discarded, and emits
  a `StateGap`.** A keyboard past its rollover limit reports this instead of a
  key. It is not a position and must never reach the table or the `VKey`
  fallback. This is not theoretical for a lighting app — mashing keys is the
  point of half these effects. An overrun also means the keyboard's own view of
  held state is now unreliable, so the decoder emits an ordered `StateGap` for
  that `source_id` in place of the record. Core releases that source's held
  keys immediately, in stream order, before applying anything that follows.
  Deferring to some notion of "next quiet moment" would leave keys stuck for as
  long as the user keeps typing — which, during a key-mashing burst, is exactly
  the whole time.
- **`RI_KEY_TERMSRV_SET_LED` and `RI_KEY_TERMSRV_SHADOW` are masked off.** D7
  keeps RDP virtual devices, so these Terminal Services flags will be seen.
  They are LED-sync and session-shadow bookkeeping, not key edges.
- **`RI_KEY_E1`** in practice prefixes Pause/Break, which arrives as a
  two-report sequence, but Raw Input does not contract that. Pause is
  identified by `VKey` plus flags rather than by `E1` alone, and an
  unrecognized `E1` sequence takes the unknown-key path instead of being
  force-fit to `"Pause"`.
- **`MakeCode == 0` with a meaningful `VKey`** falls back to a `VKey`-derived
  name. This fallback is explicitly *logical, not positional* — it is the one
  place where a non-QWERTY layout could produce a different name than Linux
  would. Acceptable only because these keys (certain media and OEM keys) have
  no positional meaning to diverge on. Names produced this way are tagged in
  the table so the parity test does not assert on them.

Everything the table does not recognize takes an **unknown-key path** — a
stable synthetic name derived from the full `(make_code, RawKeyPrefix)` pair,
so an unrecognized `E1` sequence cannot collide with an unprefixed code of the
same value — rather than being dropped. Remapping keyboards, firmware-quirky
devices, and RDP all emit scan codes outside canonical set 1, and a
silently-dropped key is worse for an effect author to debug than an
honestly-named unknown one.

**Parity is a test, not a hope — and it is exhaustive, not curated.** A single
canonical inventory of `(evdev_code, make_code, prefix, expected_name,
positional: bool)` rows is the one source both mappers are built from and
tested against. The test asserts the mapping is *total* over that inventory in
both directions, so a key added to one platform's table and forgotten on the
other fails the build rather than silently drifting — which a hand-curated
sample of tuples would not catch. Rows marked `positional: false` (the `VKey`
fallbacks) are excluded from the cross-platform name assertion and tested
separately, as are the `E1`/Pause and unknown-key paths. The Windows column
runs on every platform; the evdev column runs under
`#[cfg(target_os = "linux")]`.

## D5. Pointer — absolute where Windows can, virtual where it must

Linux accumulates a virtual cursor from relative counts because Wayland will
not tell an unfocused client where the pointer is. Windows will. `MouseData`
already carries `PointerMode::{Absolute, Virtual, None}` for exactly this. The
browser child publication already reports `Absolute`; the Windows backend is
the first *host* source able to.

- **Position** comes from `GetCursorPos`, normalized against the virtual
  desktop rect → `PointerMode::Absolute`, sampled once per drain and delivered
  in `RawInputBatch` so it stays coherent with the events (see D2).
  Access-denied (secure desktop) holds the previous value.
- **Only when `[input].mouse` is enabled.** D3 declines mouse registration
  outright for a user who did not consent to pointer capture; sampling the
  cursor anyway would hand back pointer position through the back door on
  every keyboard wake. Keyboard-only capture reports `PointerMode::None`.
- **DPI is pinned to a named context, not assumed away.** The earlier claim
  that normalization is "DPI-awareness-agnostic" was wrong: `GetSystemMetrics`
  is explicitly *not* DPI-aware for per-monitor-aware callers, `GetCursorPos`
  can return logical coordinates, and both follow the *calling thread's* DPI
  context — which a worker inherits from the process default but which
  `SetThreadDpiAwarenessContext` elsewhere can change out from under us.
  Concretely: the worker calls
  `SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)`
  as its first act, before creating the window, and reads both the cursor and
  the virtual-screen metrics under that one context. Per-monitor-v2 is chosen
  because it makes both calls return true physical pixels, which is what
  `MouseData.x`/`y` promise. If the call fails, the worker records the
  effective context and continues — normalization stays *self-consistent*
  because both reads share whatever context is active, so the failure mode is
  a possibly-scaled pixel value, never a mismatched ratio. Mixing contexts
  between the two reads is the actual hazard, and that is what this forbids.
- **Motion** comes from the raw relative deltas, aggregated per frame into the
  existing `MotionAggregate` using the same `CURSOR_COUNTS_PER_UNIT = 1200.0`
  normalization as evdev, so velocity means the same thing on both platforms.
  This is strictly better than differencing a frame-sampled cursor: it is
  sub-frame accurate and survives the cursor hitting a screen edge.
- **Absolute-mode raw devices** (`MOUSE_MOVE_ABSOLUTE`: tablets, RDP, most
  VMs) report position rather than deltas, in a 0..65535 space over either the
  primary monitor or the virtual desktop depending on `MOUSE_VIRTUAL_DESKTOP`.
  The interop crate normalizes those to `[0,1]²` against the rect the flag
  selects, and core aggregates **differences of normalized positions** — not
  raw counts divided by 1200, which would be meaningless across two unrelated
  unit systems. Baselines reset on device arrival, removal, and any change of
  the coordinate-space flag; the first report after a reset emits no delta.

**Pixel `x`/`y` are populated on Windows, and that is a real divergence.**
`evdev.rs:165-176` never sets `MouseData.x`/`y`, so on Linux they are always 0
while `payload.rs:144-145` serializes them straight to `engine.mouse.x/y`.
Windows Absolute has genuine signed virtual-desktop pixels — signed because
the virtual desktop starts at negative coordinates whenever a monitor sits left
of the primary — and withholding them to fake parity would throw away the one
thing Absolute mode is for. So Windows fills them, Linux does not, and the
divergence is documented at the contract: `nx`/`ny` are the portable pointer
channel, `x`/`y` are pixels *where the platform has them*, and `mode` is how an
effect tells which world it is in. That is what `PointerMode` was added for.

**The generation key must include pixels.** `evdev.rs:187-190` builds
`cursor_key` from normalized coordinates bucketed at 1/10,000. On a wide
virtual desktop several distinct pixel positions collapse into one bucket, so
a cursor move that changes JS-visible `x`/`y` could leave `generation`
unchanged and be skipped by `is_dirty_against`. The Windows `HeldStateKey`
includes the pixel coordinates.

**Merge precedence is explicit.** Host capture occupies interaction slots in
the manager graph. Each browser preview publishes an exact connection-scoped
child outside that graph, and the per-consumer router selects only the requested
child. No manager-owned browser union participates in sampling or routing.

Under an explicit `merge` policy, `MouseData::injected` carries pointer
precedence through the selected-source fold. Browser child publications set it
true and host backends set it false, so `merge_from` prefers the preview pointer
without making source registration order load-bearing. The regression coverage
also preserves event order and the union of held keys and buttons.

## D6. Buttons and wheel

**Buttons** arrive as a `usButtonFlags` bitfield. Names map to the evdev
vocabulary so effect code stays portable, which is not a one-to-one rename:

| Raw Input flag         | Canonical | evdev source |
| ---------------------- | --------- | ------------ |
| `RI_MOUSE_LEFT_*`      | `left`    | `BTN_LEFT`   |
| `RI_MOUSE_RIGHT_*`     | `right`   | `BTN_RIGHT`  |
| `RI_MOUSE_MIDDLE_*`    | `middle`  | `BTN_MIDDLE` |
| `RI_MOUSE_BUTTON_4_*`  | `side`    | `BTN_SIDE`   |
| `RI_MOUSE_BUTTON_5_*`  | `extra`   | `BTN_EXTRA`  |

Buttons 4 and 5 are the logical `XBUTTON1`/`XBUTTON2`; Windows guarantees
nothing about where they physically sit. Mapping them to evdev's `side` and
`extra` is a **naming convention chosen for cross-platform portability**, not
a claim about hardware placement. It is the same convention evdev's own names
encode, so an effect written against `side` behaves the same on both.

One report can carry both the DOWN and UP flag for the same button. Both edges
are emitted, DOWN before UP — a **documented deterministic policy**, not a
claim that Windows preserved the hardware ordering, which it does not promise.
Reading only the last flag would swallow fast clicks, which is exactly the
input a shockwave effect is built for. Note the honest limit: a click that
begins and ends entirely between two hardware polls may never be reported at
all, and no decoding policy recovers it.

**Scroll** needs only the canonical Q16.16 representation. Windows'
`WHEEL_DELTA` and evdev's `REL_WHEEL_HI_RES` both use 120 units per notch, so
the signed raw value shifts directly into `Line120` without rescaling.
High-resolution Windows devices preserve sub-120 values natively.

The trap: `usButtonData` is declared `u16` but carries a **signed** value.
Scroll-down arrives as `0xFF88`. It must be reinterpreted as `i16` before
widening to `i32`; widening the `u16` yields 65416 instead of −120. Unit
tested.

## D7. Device identity and hotplug

`RAWINPUTHEADER.hDevice` is opaque and recycled. On `GIDC_ARRIVAL` (and lazily
on first sight of an unknown handle, since devices present before registration
produce no arrival notification), the worker resolves
`GetRawInputDeviceInfoW(RIDI_DEVICENAME)` to the device interface path:

```
\\?\HID#VID_046D&PID_C52B&MI_01&Col01#7&1f2a3b4c&0&0000#{884b96c3-...}
```

That string is the `source_id`, and its sizing is in **UTF-16 characters, not
bytes** — the query call returns a character count, which the implementation
allocates by and retries on.

The path is a **session-local key, not a durable identity.** It usually
survives replug into the same port, and it embeds VID/PID, but Windows does
not guarantee stability across port changes, driver re-enumeration, or a
device exposing different collections. That is fine for this spec's job
(unioning held state per device within one capture session). It is *not*
enough for spec 71's post-validation follow-up #1 (resolve input nodes to HAL
device fingerprints) or the phase-2 key-to-LED mapping, both of which need
durable identity and will have to reach SetupAPI for container ID, instance
ID, and serial. This spec preserves the raw path so that work has something to
join on; it does not claim to have solved it.

**`hDevice` can legitimately be zero.** Precision touchpads and some injected
input arrive with a null handle, so the handle→path cache cannot cover every
event, and zero is not a reliable "this was synthetic" marker. Null handles
are assigned one stable `"windows:unknown"` source bucket and skip the
device-info query entirely.

`RIDI_DEVICEINFO` confirms `RIM_TYPEKEYBOARD`/`RIM_TYPEMOUSE`, so
classification needs no heuristic — genuinely simpler than evdev's capability
sniffing. It does **not** carry a human-readable name:
`RID_DEVICE_INFO_KEYBOARD` is type, subtype, mode, and three key counts, and
`RID_DEVICE_INFO_MOUSE` is id, button count, sample rate, and a horizontal
wheel flag. The diagnostic label is therefore synthesized from the interface
path's VID/PID plus the device type, and is presented as exactly that. A
friendly product name would mean a SetupAPI dependency this wave does not
need.

On `GIDC_REMOVAL` the handle maps back through the cache to its `source_id`,
`synthesize_releases` emits release edges for everything it held, and the
entry is dropped. Same invariant as evdev: no key or button ever sticks.

Terminal Services virtual devices (`\\?\Root#RDP_MOU#`, `RDP_KBD`) are kept
rather than filtered — an RDP session is a legitimate way to use the machine,
and D1 already refuses to condemn RDP users — but are labeled so diagnostics
do not look like phantom hardware.

## D8. Degraded health, and a remedy that is not a udev command

`InteractionDiagnostics` currently expresses health as
`devices_opened`/`devices_denied`, and `input_access_remedy`
(`ui/src/input_access.rs:37`) turns "denied > 0 and opened == 0" into
`InstallRules`, whose banner renders `sudo just udev-install`
(`input_access_banner.rs:28`). On Windows there is no per-device denial and no
udev, so the existing shape can only produce wrong advice.

This spec lands spec 71's post-validation follow-up #2 as the minimum needed
to make Windows honest:

```rust
pub enum InteractionDegradation {
    /// Process has no visible window station — Windows service or a
    /// scheduled task running without an interactive desktop.
    NoInteractiveSession,
    /// Device nodes present but unreadable — Linux udev rules missing.
    AccessDenied,
    /// Backend could not initialize.
    Unavailable(String),
}
```

**`devices_opened` keeps its documented meaning: opened and streaming.**
`GetRawInputDeviceList` counts devices *present*, which is a different claim,
and reporting presence through a field the UI and MCP read as health would let
a structurally deaf backend look healthy — `daemon/api/system.rs:527` sums the
field and `mcp/tools/system.rs:279` branches on it. On Windows the counted
state is therefore "registered for this kind, identity resolved, and the worker
running": zero whenever the session probe failed or the worker is down. The
invariant is `devices_opened > 0` implies input can actually flow.

`InteractionDiagnostics` gains `degraded: Option<InteractionDegradation>`;
`InputStatus` gains a `degraded: Option<String>` snake_case code (additive and
`#[serde(default)]`-compatible, so no API break); `InputAccessRemedy` gains
`RunInUserSession`, whose banner explains that the daemon is running as a
service and points at the foreground daemon. The Linux path keeps producing
`AccessDenied` → `InstallRules` unchanged.

`devices_denied` stays 0 on Windows. There is no per-device denial to count:
capture is a **session-level capability with undetectable per-desktop
exclusions**. Either the process has a visible window station and sees input,
or it does not — and even when it does, D1's elevated windows and secure
desktop stay invisible with no signal that they were skipped. Calling that
"all-or-nothing" would overstate it. The session-level part now has its own
typed field instead of being smuggled through a counter; the per-desktop part
is documented and unobservable.

W3 also updates the MCP heuristic at `mcp/tools/system.rs:279`, which
independently reimplements the `denied > 0 && opened == 0` rule and would
otherwise keep giving udev advice on Windows after the UI stopped.

## D9. device_query retirement, complete

Spec 76 ships the native macOS `CGEventTap` backend and removes the final
`InteractionInput` consumer. The workspace dependency, macOS-only core
dependency, polling source, exports, tests, fixture labels, and lock inventory
entry are gone. Linux uses evdev, Windows uses Raw Input, and macOS uses Core
Graphics event taps. No supported build compiles or ships `device_query`.

## Testing

The pure/impure split is chosen so CI covers as much as possible, and W0 fixes
the reason that split had to be so severe.

**W0: a Windows CI job.** Today `ci.yml` runs `windows-latest` only in
`build-native-app`, gated to tags and `release_artifacts == 'full'` — there is
no Windows clippy or test job on pull requests at all. That is why Windows
clippy bitrot accumulates between manual runs, and it would make every
`#[cfg(target_os = "windows")]` test below a developer-machine courtesy rather
than a gate. W0 adds a `windows-latest` clippy + test job. It is cheap, it pays
for itself immediately across the existing Windows crates, and every Windows
assertion in this spec depends on it to mean anything.

**Pure, runs everywhere:**

- Make code + prefix → canonical name: the E0 extended block, the E1/Pause
  path, `MakeCode == 0` VKey fallback, `0xFF` overrun rejection, Terminal
  Services flag masking, and the unknown-key path.
- Exhaustive cross-platform key-name parity over the canonical inventory,
  asserting totality in both directions rather than sampling tuples.
- Key repeat classification: a press for an already-held key becomes
  `Repeated`, does not duplicate in `pressed_keys`, and never lands in
  `recent_keys`.
- Button-flag decoding: single edges, and the down+up-in-one-report case
  producing two edges DOWN-then-UP.
- Wheel sign reinterpretation (`0xFF88` → −120), sub-notch hi-res values, and
  `RI_MOUSE_HWHEEL` producing no event.
- `GetRawInputBuffer` record-walk arithmetic against synthetic headers,
  including malformed `dwSize` terminating the batch rather than walking off
  the end.
- Absolute-mode normalization including the `MOUSE_VIRTUAL_DESKTOP` variant,
  and baseline reset on arrival / removal / flag change emitting no first
  delta.
- State folding, held-state union across two `source_id`s, generation
  advancement, and release synthesis — reusing the evdev test shapes against
  `WindowsHostInput`.
- Generation behaviour under an absolute pointer: stationary cursor advances
  nothing, a moving cursor advances exactly once per sampled frame, and a
  sub-bucket pixel move still advances (the pixel-in-key rule from D5).
- Degraded classification: an injected `NoInteractiveSession` verdict spawns
  no worker, reports `devices_opened == 0`, and produces `RunInUserSession`
  rather than `InstallRules`.
- `StateGap` handling: a gap releases exactly that `source_id`'s held keys and
  buttons, leaves a second source's held state untouched, resets that source's
  absolute baseline, and is applied in stream order relative to surrounding
  events. Specifically: `down(A), StateGap, down(B)` leaves only `B` held.
- Motion accumulates `distance` per event: two equal-and-opposite
  `MotionRelative` events yield zero net `dx`/`dy` and non-zero `distance`, so
  a fast shake still reports velocity.
- Epoch rejection: a batch carrying a stale epoch mutates nothing, including
  when it arrives interleaved with a live-epoch batch.
- Capture toggle: held state clears on disable, and a key still physically
  held on re-enable reports as a fresh press exactly once.
- Browser-over-host pointer precedence via the `merge_from` rule, plus an
  assertion that `recent_keys` concatenation order is unchanged by it.

**Windows-only, `#[cfg(target_os = "windows")]`, no hardware required** — real
gates once W0 lands:

- Message-only window creates, registers, deregisters, and destroys cleanly.
- Repeated start/stop cycles do not fail on `ERROR_CLASS_ALREADY_EXISTS`.
- `interactive_session_state()` reports `Interactive` from a normal test
  process.
- Teardown under stress: a deliberately blocked sink, a dropped wake nudge, and
  a panicking sink each still reach a bounded, clean shutdown with
  `worker_state` reporting honestly.
- Drain liveness: under continuous synthetic input the stop nudge is still
  observed within the slice bound, so shutdown does not hang while input keeps
  arriving.
- Registration theft: a second `RegisterRawInputDevices` for `0x01/0x06` is
  detected by the `GetRegisteredRawInputDevices` check.
- Stale-worker teardown: a worker detached by a timed-out join, running its
  teardown *after* a replacement session has registered, does not call
  `RIDEV_REMOVE` and leaves the live registration intact — the replacement
  keeps receiving input.
- Registration-lock interleavings, driven directly against the ownership
  primitive with an injectable registration call so both orderings are
  forced rather than raced: a stale teardown attempting removal while a
  replacement holds the claim removes nothing; a failed replacement
  registration leaves the previous owner installed and still responsible for
  its own removal; and a start/stop cycle leaves the owner slot empty rather
  than pinned to a dead generation.
- Slice and lock-hold bounds: under continuous synthetic input, one drain
  slice and one core lock acquisition each stay under their stated bound.

**Manual, hardware, honestly labeled as such:** an `examples/dump_input.rs` in
the interop crate (mirroring the capture crate's `dump_frame.rs`) prints
decoded events live. The acceptance pass is: type across a real keyboard,
confirm names match the Linux daemon's for the same physical keys, hold a key
and confirm recents fire once, scroll a hi-res wheel, click faster than the
report interval, and unplug a keyboard mid-hold and confirm the held set
empties. This cannot run in CI and will not be claimed as if it had — and note
what it *cannot* establish either: handle reuse, stale buffered identity,
pointer shadowing, and generation behaviour are all covered by the core tests
above precisely because eyeballing a dump would never catch them.

## Waves

Waves W0 through W3 are built. W4 is the only one outstanding, and it cannot
be done from a CI runner or an agent session: it needs a human at a real
keyboard. See the status notes on each wave.

- **W0** — `windows-latest` clippy + test job in CI. Everything Windows-only
  below is only a gate because of this. **Done** (`ci.yml`, `rust-windows`).
- **W1** — `hypercolor-windows-input` crate: window, registration ownership,
  bounded buffered drain and direct batch sink, device enumeration and
  identity, hotplug, cursor sampling, stubs, example binary. **Done.** The
  decoding arithmetic ended up in a module that compiles on every target, so
  the wheel-sign, button-edge, record-walk, and normalization tests run on
  Linux CI rather than only on Windows.
- **W2** — `WindowsHostInput` in core: key tables, repeat derivation, state
  folding, pointer model, diagnostics. Full pure-test suite. **Done.** Two
  refinements against the plan. The canonical key inventory became a single
  table both backends derive from, which extended Linux's coverage: keys that
  previously fell through to a debug name (`KEY_F1`, `KEY_HOME`, `KEY_KP0`)
  now carry proper names on both platforms. And `WindowsHostInput` compiles
  everywhere rather than behind a `cfg`, so the fold — repeat classification,
  held-state union, release synthesis, absolute baselines, epoch rejection —
  is covered by Linux CI too.
- **W3** — Wiring: `build_interaction_source` on Windows, `MouseData::injected`
  plus the `merge_from` pointer-precedence rule and its test (source
  registration order in `services.rs:577-585` is left **unchanged**),
  degraded-health types through `InteractionDiagnostics` → `InputStatus` →
  MCP diagnose (`system.rs:279`) → UI remedy. The native macOS backend from
  spec 76 completes the planned `device_query` retirement. **Done.**
  `InputStatus.degraded` is additive and optional, so the vendored
  Python client regenerated without an API break.
- **W4** — Hardware acceptance pass, parity check against the Linux daemon,
  docs (permissions/session-model page), cross-model review. **Docs and
  review done**; acceptance partially run. `cargo run -p
  hypercolor-windows-input --example dump_input` is the harness.

  Covered so far: relative motion, button edges, cursor tracking, and single
  delivery. Still to run: key names against a live Linux daemon, a held key
  firing recents once, hi-res wheel notches, and unplugging a keyboard
  mid-hold to see the held keys release.

  The first run earned the wave outright. It found that **every event was
  being delivered to core twice** — `drain_slice` cleared its buffer at the
  start of a slice rather than after delivering, so whatever the last slice
  produced was still queued when the worker flushed. Three rounds of
  cross-model review and roughly 1,100 passing tests had all missed it,
  because both halves were individually correct and the composition only runs
  on Windows against real input. The tell was two batches with an identical
  delta and timestamp but a different cursor.

  Two things follow from that, and they are the reason this wave is not
  optional. The buffer discipline now lives on a `PendingEvents` type that
  compiles and tests on every platform, so the defect class is reachable from
  CI rather than only from hardware. And the acceptance harness now prints a
  batch number and per-event device id, because the output that hid this could
  not distinguish a delivery bug from one physical keyboard exposing several
  HID collections.

## Resolved decisions

These were open when the spec was first drafted. Both reviewers landed on the
same answers, and the reasoning is recorded so W1 does not reopen them.

1. **Absolute pointer is the Windows default.** Windows should not imitate a
   Wayland limitation it does not have. The platform difference is not hidden —
   it is exactly what `PointerMode` exists to express, and effects that want
   portable behaviour already read `nx`/`ny`.
2. **Session degradation never rewrites consent.** `[input].enabled` stays on
   and the backend reports degraded. A runtime capability failure is not a
   reason to edit the user's configuration, and a user who switches from the
   service to the foreground daemon should just start working.
3. **Keyboard and pointer get separate consent toggles in the UI.** They are
   materially different privacy surfaces, `[input].keyboard` and
   `[input].mouse` already gate registration independently, and D5 now makes
   the mouse toggle govern cursor sampling too. One switch would under-describe
   what is being granted.

## Review history

- **Round 1** — Nova self-review plus two independent Codex passes (Win32
  factual, architecture/parity). Both returned `NEEDS_CHANGES`. Confirmed
  defects fixed: message-loop ordering, window-station session probe, batch
  sink, event-contract expressiveness (E1/VKey, relative vs absolute motion,
  cursor coherence), key-repeat parity, DPI pinning, pointer merge precedence,
  `RIDI_DEVICEINFO` label, null `hDevice`, overrun scan codes, registration
  scope, `RIDEV_NOLEGACY` rationale, `devices_opened` semantics, and the
  missing Windows CI job. Two claims were corrected as over-stated rather than
  wrong: device-path stability and "all-or-nothing" access.
- **Round 2** — Codex verified round 1's fixes (22 confirmed landed) and
  attacked the new text, which is where the remaining structural defects were
  hiding. Fixed: `WM_INPUT` lost to the control pass between drains (filtered
  peek), missing `DefRawInputProc` cleanup on the buffered path, a bounded ring
  that could drop a key *release* and strand held state forever (`StateGap`
  barrier), timestamps assigned in a crate that cannot reach core's epoch
  (core stamps), a detached folding thread able to mutate a restarted session
  (epoch-tagged delivery), motion coalescing destroying `MotionAggregate`'s
  `distance`, repeat derivation across a capture toggle, "release-on-next-quiet"
  being undefined under continuous traffic, and — the sharpest one — the
  proposed pointer-precedence fix. Reordering source registration would have
  fixed the pointer by permuting the order `recent_keys` concatenation depends
  on, so precedence moved into `merge_from` instead.
- **Round 3** — Codex confirmed round 2's fixes and found that five of the
  remaining blockers all traced to the same thing: the bounded ring and
  separate folding thread introduced in round 2. A queue that may drop events
  needs loss-barrier semantics, a detached folding thread needs epoch guards,
  and coalesced motion needs a wire format raw deltas cannot carry — each
  patch spawning the next. The response was to delete the structure rather than
  keep patching it: the sink now folds on the pump thread under the mutex,
  exactly as `poll_devices` does on Linux, which is the parity the spec claimed
  all along. That dissolved the loss barrier, the motion wire aggregate, and
  most of the epoch surface at once. Also fixed: an unbounded drain that could
  starve shutdown under sustained input, timestamps taken at fold time rather
  than capture time, the epoch check racing the restart it guarded, unknown-key
  identity still collapsing `E1`, and a leftover W3 instruction contradicting
  the merge-precedence decision.
- **Round 4** — Codex confirmed the structural change against `evdev.rs`
  ("the Windows shape genuinely matches evdev") and confirmed the epoch is
  still load-bearing, since the *pump* thread can now be the one that detaches.
  Two defects remained. The sharp one: the epoch guards core state but not the
  process-global Raw Input registration, so a detached stale pump running its
  teardown could `RIDEV_REMOVE` the *replacement* session's registration —
  input would simply stop, with nothing looking broken. Registration now
  carries an ownership generation and only the claim holder removes. Also
  qualified the "nothing is dropped" claim as application-level: a prolonged
  pump stall can exhaust the OS backlog, which no application-side buffering
  would have prevented.
- **Round 5** — Codex confirmed two of the three round-4 fixes and rejected the
  third on a real defect: the ownership claim was specified as a
  compare-and-swap, which orders the claim against itself but not against
  `RegisterRawInputDevices`, the process-global state actually contended.
  Neither ordering of CAS and registration is safe — claiming first orphans the
  old registration when the replacement's registration fails, registering first
  lets a stale teardown remove a replacement that landed microseconds earlier.
  The claim became a process-wide lock covering registration-plus-publication
  and owner-check-plus-removal as whole critical sections, with the
  interleavings driven directly in tests through an injectable registration
  call rather than raced.

### Implementation review

The spec review above settled the design. The code then went through its own
three rounds, and the findings were a different class entirely — every one was
a Win32 lifetime or lifecycle defect that no amount of spec review would have
surfaced, which is the argument for reviewing both.

- **Round 1** — three blockers, three majors, all accepted. Two were the same
  memory-safety mistake in two places: a bounds check that admitted a
  `RAWINPUTHEADER` and then materialized a `&RAWINPUT`, whose tail can lie
  past the buffer because `dwSize` covers only the union arm the device
  filled. The sharpest was a lifecycle race the claim was supposed to prevent
  and did not: a worker that missed its readiness deadline is abandoned with
  the stop flag set, but it is still alive and still heading for
  `RegisterRawInputDevices`, so it could register and publish its claim
  *after* a replacement already had — taking ownership and then deregistering
  the live session on teardown. The flag is now checked inside the claim's
  critical section, which makes a cancelled worker unable to register at all.
  Also fixed: `stop()` posting to a destroyed window, a device-path query that
  dropped a device rather than retrying a size change, and `MotionAbsolute`
  discarding its coordinate space so core differenced across a
  `MOUSE_VIRTUAL_DESKTOP` flip and invented a pointer jump.
- **Round 2** — all six confirmed fixed, one new major: the payload bounds
  check used the buffer's capacity rather than the record's own declared end,
  so a short record could read into the record after it. That stays inside the
  allocation, which is what makes it worse than a crash — it silently decodes
  one device's bytes as another's key state.
- **Round 3** — `PASS`, including on the skeptical question the brief asked
  directly: whether serializing the session tests masked a real defect rather
  than fixing a flaky one. It does not. Registration is process-global, so
  unrelated tests were contending for the exact resource under test;
  deliberate concurrent-session behaviour is still exercised inside the two
  tests that assert on it.
