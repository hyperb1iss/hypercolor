# Spec 82: Device Input from Driver-Owned Hardware

## Status

Phase 1 implemented (2026-09-28, shipped in 0.6.3). Phase 2 implemented
(2026-10-11, for 0.7.0): effects see touches and device buttons, with
canvas positions. Phase 3 started (2026-10-19): the Ableton Push 2 publishes
pads, touch strip, and buttons through a USB transport input seam, and
touches name the device segment they land on. Extends Spec 30 (ROLI Blocks backend), whose event
sections were never built, and Spec 71 (interactive input pipeline), which
shipped differently from its text. Read `crates/hypercolor-core/src/input/`
rather than Spec 71's design sections.

## Goal

Hardware that Hypercolor already drives can also be played. Touching a ROLI
Lightpad or pressing a LUMI key reaches the same interaction pipeline that
host keyboard and mouse input use, attributed to the `DeviceId` of the
surface that was touched, so effects can react where the hand is.

ROLI Blocks through blocksd is the first producer. The seam is driver
neutral, so a later Push 2 pad driver or any other controller with lights
publishes the same way.

## Non-goals

- MIDI note, CC, and pitch-bend input. The `InputEvent::Midi*` variants keep
  waiting for a generic MIDI source (tracked separately in Sibyl).
- Host capture changes. `[input].enabled` keeps meaning keyboard and pointer
  capture from the host.
- Canvas positions on the bus. `InputEvent::Touch` and the `input_events`
  topic keep device-surface coordinates; canvas positions exist only where a
  canvas does, in the payload an effect receives.

## Starting point

- The Blocks driver (`crates/hypercolor-core/src/device/blocks/`) only lit
  devices. It never subscribed to blocksd events, and its `uid_map` "for
  event routing" was written and never read.
- blocksd publishes `touch`, `button`, and `device` events to subscribers on
  its Unix socket. Since blocksd PR 14, touch events carry signed velocity
  and a device timestamp, button events carry the protocol button index and
  SDK function name, and a subscriber that overflows is disconnected instead
  of silently dropped.
- No driver could publish input. `DeviceBackend` has no input hook and
  `DriverHost` offered no input capability.
- `InputManager` permits one `ManagedSourceKey::Interaction` source, the
  live `[input]` swap replaces it, and `InteractionRouteSource::manager_slot`
  classifies every manager slot as host capture. A second interaction source
  in the manager would break all three.
- Browser preview input already solved the neighbouring problem: an
  always-live registry outside the manager and outside the host consent
  gate, with its own route class. Device input copies that shape.

## Design

### 1. Vocabulary (`hypercolor-types`)

`device_input.rs` holds what a driver reports, in physical terms:

```rust
pub enum DeviceInputEdge {
    TouchBegan { contact: u32, position: TouchPosition, segment: Option<Arc<str>> },
    TouchMoved { contact: u32, position: TouchPosition },
    TouchEnded { contact: u32, position: TouchPosition },
    Button { button: Arc<str>, state: InputButtonState },
}

pub struct TouchPosition { pub x: f32, pub y: f32, pub pressure: f32 }
```

Positions are normalized device-surface coordinates in `[0, 1]`, origin at
the surface's top left as the device reports it. A surface whose LEDs form a
matrix or a strip uses the frame its LED order defines: a matrix's first LED
is its top left, and a strip runs along `x` from its first LED. Pressure is
`[0, 1]`. `segment` names the device segment a contact landed on, by the
segment name layouts bind (`Output::zone_name`), for a device whose touch
surfaces span several segments; single-surface devices leave it `None`. A
contact keeps its segment until it ends.

`event.rs` gains two routed edges. `InputEvent` derives `Eq`, so positions
use the same Q16.16 fixed point as `PointerScroll`:

```rust
pub enum TouchPhase { Began, Ended, Cancelled }

InputEvent::Touch {
    source_id: String,
    device_id: DeviceId,
    contact: u32,
    phase: TouchPhase,
    x_q16_16: i64,
    y_q16_16: i64,
    pressure_q16_16: i64,
    segment: Option<String>, // omitted from JSON when None
}

InputEvent::DeviceButton {
    source_id: String,
    device_id: DeviceId,
    button: String,
    state: InputButtonState,
}
```

Touch movement is not an event. The bus carries discrete events, and
effects already read pointer motion from held state, so a moving contact
updates held state and bumps the snapshot generation while only begin and
end cross the event ring. `Cancelled` is the synthetic end the router or
registry emits when a hold is lost without a lift. Effects treat it like
`Ended` without the lift gesture.

### 2. Driver seam (`hypercolor-driver-api`)

```rust
pub trait DeviceInputSink: Send + Sync {
    fn attach(&self, device_id: DeviceId, label: &str) -> Box<dyn DeviceInputPublisher>;
}

pub trait DeviceInputPublisher: Send + Sync {
    /// `false` once this lease has been superseded or detached.
    fn publish(&self, edges: &[DeviceInputEdge]) -> bool;
    /// A fresh lease replacing this one, only while this one is current.
    fn renew(&self) -> Option<Box<dyn DeviceInputPublisher>> { None }
}

trait DriverHost {
    fn device_input(&self) -> Option<Arc<dyn DeviceInputSink>> { None }
}
```

A publisher is a lease. Dropping it retires the device's input source, and
the router cancels anything the consumer observed. Attaching the same device
again supersedes the earlier lease: the old source retires the same way, and
the old lease's `publish` returns `false` and its drop does nothing, because
close is fenced by publication incarnation. A stale lease can never retire
its successor. `renew` replaces a lease with a fresh one, cancelling what it
held, and the host checks that the lease is still current and replaces it
under one lock, so a stale lease can never take the device back from a
newer attachment either; a superseded or detached lease renews to `None`.
Hosts without device input return `None`, so every existing host and test
double keeps compiling, and publishers that cannot renew atomically keep
the default `None`.

### 3. Device input registry (`hypercolor-core::input::device`)

`DeviceInputHandle` mirrors `BrowserInputHandle`:

- One child publication per attached `DeviceId`, each with its own event
  ring, so one busy surface cannot evict another's edges.
- `registry()` returns a lock-free snapshot `{generation, children}` for the
  route catalog.
- A shared `SourceStatus` with backend `"device"`, configured and consented,
  and demanded only while interaction is demanded. Every re-demand begins a
  fresh status session.

Each child tracks what is physically held, and **announces** a press (publishes
its begin, holds it in snapshots, and publishes its end) only while
demanded:

| Edge | Rule |
| --- | --- |
| `TouchBegan` | Clamped. A non-finite position drops the edge and counts it. A begin for a contact already down ends the old contact first. More than 128 held contacts per device drops the edge (the Push 2 can hold 65). |
| `TouchMoved` | Updates a held contact. A contact whose begin was never seen, such as one down when the device attached, is ignored. |
| `TouchEnded` | Never dropped. A non-finite final position falls back to the last good one. |
| `Button` | Pressed once until released; release without a press is ignored; autorepeat is ignored. At most 16 held buttons per device. |

Losing demand cancels every announced hold with explicit `Cancelled` and
`Released` edges under the child lock. A press that began while undemanded
stays invisible until it lifts, and its lift publishes nothing. The router
therefore never sees a hold or release it did not observe being pressed,
which would otherwise leave quarantine entries that swallow a later tap.
Demand changes hold the registry writer lock, so a child attaching mid-change
still sees the new demand.

### 4. Routing

- `SourceNamespace::Device`, `SourceIncarnation::device_child`, and
  `InteractionRouteSourceClass::Device`. The catalog tracks the device
  registry generation beside the browser one and lists device children last.
- Selection: device sources join under `Host` and `Merge`, never under
  `Browser`. Physical device input behaves like physical host input: the
  daemon consumer, which defaults to `Host`, sees devices, and previews,
  which default to `Browser`, stay isolated until a user chooses `Merge`. No
  policy variant is added.
- Held controls: `InteractionControl::Touch { device, contact }` and
  `DeviceButton { device, button }`. `event_control` maps `Began` to pressed
  and `Ended` and `Cancelled` to released, with explicit arms instead of the
  former fall-through. `synthetic_release` emits `Cancelled` for touches and
  `Released` for buttons. Snapshot holds, quarantine, and held output extend
  to both, with positions always taken from the latest snapshot.
- `input_availability` counts only host sources. An attached Lightpad must not
  make a keyboard effect report input as available when host capture is off.
  The status API also excludes the `"device"` backend from host capture,
  defensively, since the device status is not registered with the manager.

### 5. Held state

```rust
pub struct InteractionData {
    // existing fields
    pub device: DeviceInteractionData,
}

pub struct DeviceInteractionData {
    pub touches: Vec<TouchContact>,
    pub buttons: Vec<DeviceButtonHold>,
}

pub struct TouchContact {
    pub device_id: DeviceId,
    pub contact: u32,
    pub x: f32,
    pub y: f32,
    pub pressure: f32,
    pub segment: Option<Arc<str>>,
}

pub struct DeviceButtonHold { pub device_id: DeviceId, pub button: Arc<str> }
```

The router rebuilds held output every frame without allocating. Touch lookups
build their key from `Copy` fields, a held touch's segment is a shared
`Arc<str>` cloned into reused slots, button lookups scan instead of building an
owned key, and
the output vectors are truncated and refilled in place. The allocation
contract test holds a device touch and button through every measured frame.
While a finger moves, every frame bumps the routed generation and defeats
frame reuse; that is the cost of live touch positions.

### 6. Demand and privacy

Device input is not keystroke capture, so `[input].enabled` does not gate
it. The input publication pump mirrors the interaction bit of its aggregate
capture demand into the device handle on every iteration, independent of the
manager reconcile that governs host capture, and clears it when the pump
stops. A drop guard owns the clear, so a worker that panics or is aborted
past the shutdown deadline withdraws demand like a clean exit. That demand
is the union of authoritative render, preview, and `input_events` WebSocket
subscribers. Once the cancels for held input have gone out, children
publish nothing while undemanded, so neither the bus nor the `input_events`
topic sees further device traffic.

### 7. ROLI Blocks producer

- The backend opens a second connection to blocksd for events and writes only
  its `subscribe` request (`device`, `touch`, `button`). Events never share
  the frame connection, which pairs each request with the next line or byte
  it reads and would misread an interleaved event.
- The factory captures `host.device_input()` when it builds the backend.
  `connect()` attaches a lease under the host's canonical `DeviceId` for the
  adopted uid, and `disconnect()` drops it. Blocks the host never adopted,
  such as a surface-less Loop Block, are ignored, and their uid is never
  re-minted into an id.
- The stream task runs while any device is connected; the last disconnect
  closes the event connection. Each stream carries a generation that every
  start and abort bumps under the input state lock. An abort lands only at
  the stream's next `.await`, so a stream aborted by a fast disconnect and
  reconnect may still hold a decoded event; it checks its generation under
  the lock and drops that event instead of publishing into the new lease.
- blocksd `start`, `move`, and `end` become `TouchBegan`, `TouchMoved`, and
  `TouchEnded` with `index` as the contact. Buttons use blocksd's SDK
  function name (`mode`, `up`, `down`), then `button{id}` from the protocol
  index.
- Event lines are capped at 64 KiB, two orders of magnitude above the
  largest subscribed event (`device_added`, a few hundred bytes). A longer
  line is skipped through its newline without being buffered, with one
  warning per connection, so a peer that never sends a newline cannot grow
  the reader without bound.
- Every event that could hide a lift supersedes leases so held input
  cancels: `device_removed` for that device, and an undecodable or oversized
  line or a lost connection for all devices. After a skipped line the stream
  keeps reading; after a lost connection it reconnects with backoff from
  250 ms doubling to 5 s, without cancelling again while blocksd stays
  unreachable. `device_added` payloads are ignored, so schema drift there
  cancels nothing. Dropping the backend aborts the stream.

### 8. Effects (phase 2)

**Payload.** The LightScript interaction payload gains a `device` block,
omitted while nothing is held, and its ordered `events` list gains two
kinds beside `key`, `button`, and `scroll`:

```json
"device": {
  "touches": [{ "device": "<DeviceId>", "contact": 2, "x": 1.0, "y": 0.5,
                "pressure": 0.75,
                "canvas": { "x": 160.5, "y": 150.5, "nx": 0.5, "ny": 0.75 } }],
  "buttons": [{ "device": "<DeviceId>", "button": "mode" }]
}
{ "kind": "touch", "device": "<DeviceId>", "contact": 2, "phase": "began",
  "x": 1.0, "y": 0.5, "pressure": 0.5, "canvas": { ... }, "source": "...",
  "atMs": 200, "seq": 7, "repeatCount": 1 }
{ "kind": "device_button", "device": "<DeviceId>", "button": "mode",
  "state": "pressed", "source": "...", "atMs": 201, "seq": 9, "repeatCount": 1 }
```

`x`, `y`, and `pressure` stay in device-surface terms, exactly as the bus
carries them. Touch phases are `began`, `ended`, and `cancelled`; device
button states reuse the key states.

**Page API.** The adapter and bootstrap mirror the host pair. `engine.touch`
follows `engine.mouse`: held `touches` with live positions, `down`, the
frame's ordered `events`, and `isDown(device?)`. `engine.deviceButtons`
follows `engine.keyboard`: a `buttons` name map, attributed `held` pairs,
the frame's `events`, and `isDown(button?, device?)`. The SDK's
`getInputData()` adds typed `touch` and `deviceButtons` snapshots, and the
build's input validation counts `engine.touch` and `engine.deviceButtons`
as input use, so an effect reading them must declare `input: true`.

**Canvas positions.** Every render zone samples its canvas through its own
spatial layout, so a touch has one canvas point per zone that places its
device and none elsewhere. `DeviceSurfaceMap`
(`hypercolor-core::spatial`) resolves a touch that names no segment through
the device's only LED output in the layout, and a touch that names a
segment through the one LED output bound to that segment: the output whose
segment name matches exactly, or else the only one matching it the way LED
routing does (`channel_name_matches_slot_alias`, ignoring case and slug
punctuation). An output that names no segment never stands in for a named
one, since its LED order follows the whole device. It maps the surface point through that output's
position, size, rotation, scale, and edge behavior, with the zone-local
transform shared with the sampler. A matrix output also applies its start
corner, because its LED order is the device's own row-major order, and a
strip output applies its direction, because its surface `x` runs from its
first LED. The result is the canvas point the LED under the finger samples. `canvas.nx`
and `canvas.ny` are normalized; `canvas.x` and `canvas.y` are Canvas 2D
drawing coordinates of the sampled pixel's center (`n * (size - 1) + 0.5`),
so an effect that draws at them lights exactly that point.

The daemon resolves layout outputs to devices through the bindings the
backend manager routes LEDs with. The output stage hands them to the zone
runtime whenever the mapping generation moves, and the zone runtime caches
one map per zone until that zone's layout or the bindings change. Effects
receive their zone's map through `FrameDataSources::device_surfaces`, and
the Servo frame queue keeps the newest one with a coalesced batch, so a
superseded touch edge is mapped with the same placement as the held
contacts beside it.

**Missing placement.** When the touched surface has no LED output in the
zone's layout, or has several, `canvas` is `null` and the surface
coordinates still arrive. The platform does not guess between outputs or
invent a position for an unplaced device; the effect decides what an
unplaced touch means. That includes a device lit only through the scene's
unassigned-output fallback: its LEDs show the fallback zone's canvas, but
no layout places it, so its touches carry no canvas point. Interactive
previews have no bindings, so their touches always carry `canvas: null`.

**Emission.** A held contact's canvas point can move while the finger
rests, when the layout or the canvas size changes, so the runtime re-sends
the interaction payload in that case as well as when the routed generation
moves. It also re-sends the batch on the frame after one that carried
edges, so the page never replays a stale edge; that applies to host input
too.

**Demand and privacy.** Unchanged. The payload carries device input only to
effects that declare `input: true`, the same declaration that gates
keyboard and pointer data, and exposing it adds no capture and no demand:
device input still flows only while interaction is demanded, and
`[input].enabled` still governs host capture alone. There is no narrower
declaration, so an effect that only wants touches also receives host keys
and pointer state whenever host capture is enabled.

**Proof.** `sdk/src/effects/touch-ripple` throws a ripple from each placed
contact's landing point, lights a pressure-scaled glow under every held
finger, and steps its palette on device button presses. It skips touches
whose `canvas` is `null` and deduplicates edges by `seq`.

### 9. USB transport producers and the Push 2 (phase 3)

**Seam.** `hypercolor-hal` cannot depend on the driver API, so it defines
its own callback, `hypercolor_hal::transport::input::TransportInputSink`,
with `publish(&[DeviceInputEdge])`, `interrupted()` (input may have been
lost, so cancel what the device held), and `closed()` (input ended for
good). A driver-owned USB binding sets `publishes_input`, and
`UsbTransportOpenRequest::input` carries the sink to its opener.

**Lease.** The builtin USB module hands `host.device_input()` to
`UsbBackend::with_device_input`. For a binding that publishes input, connect
passes the transport a `UsbDeviceInput` bridge at open, attaches the host
lease under the device's canonical `DeviceId` only once connect succeeds,
and retires it as shutdown begins, before the shutdown sequence runs.
`interrupted()` renews the lease and `closed()` retires it, so the host
cancels every hold either way, as the Blocks backend does for a lost
blocksd stream. Renewal is atomic on the host, so a bridge whose lease a
newer connection superseded gets `None` and closes itself instead of
taking the device back. Input before connect finishes is dropped, and a
device that never finishes connecting never becomes an input source.
After a close, nothing brings the lease back; a reconnect builds a new
bridge.

**Push 2.** Spec 28 §11.4 has the MIDI map. The transport's existing User
port input callback forwards each message to the sysex reply ingress, then
to a relay that decodes it and publishes the edges. Reading input sends no
MIDI and changes no device mode. Pads are contacts 0 to 63 on segment
`Pads`, in the matrix's LED frame; the touch strip is contact 64 on segment
`Touch Strip`, along the strip from its bottom LED; buttons use stable
snake_case names. The relay lives in the native callback, so dropping the
input connection reports `closed()`. A message cut short, or the Set MIDI
Mode reply announcing Live mode, reports `interrupted()`.

## Phasing

1. **Pipeline (0.6.3).** Sections 1 through 7. Touches and buttons reach
   `FrameInputs.interaction` and the bus with device attribution, held
   state, and synthetic releases.
2. **Effects (0.7.0).** Section 8. The LightScript payload, JS adapter,
   bootstrap, and SDK types expose touches and device buttons. Touches gain
   canvas coordinates by resolving the device's output through each render
   zone's layout. The Touch Ripple effect proves the path on a Lightpad.
3. **More producers.** Push 2 pads, touch strip, and buttons (section 9,
   started 2026-10-19), with segment-named touches so multi-output devices
   place per segment; a generic MIDI source.

## Testing

- Types: serde round trips for `Touch` and `DeviceButton`, phase names, and
  device attribution.
- Registry (`device_input_tests.rs`): fold rules, the contact cap, clamping
  and non-finite handling, demand gating and cancellation, presses spanning
  a demand change, supersession through both the concrete handle and the
  driver-api sink, status sessions across re-demand, and held-state merging.
- Routing: device selection under each policy, live positions between begin
  and end, detach cancellation, quarantine of holds older than the consumer,
  release on demand loss followed by a fresh tap, device buttons, and the
  allocation contract with held device input.
- Daemon: device sources route under `Host` without reporting host input
  availability, the pump drives device demand with no host capture source
  registered, and an aborted pump worker withdraws it.
- Blocks (`blocks_input_tests.rs`): a fake blocksd drives subscribe, touches,
  buttons with and without SDK names, unadopted uids, unknown event types,
  `device_removed`, an undecodable line, an oversized line, connection loss
  and resubscription, device disconnect, and the absence of an event
  connection without a sink.
  A unit test holds an aborted stream across a disconnect and reconnect and
  checks it publishes nothing into the new lease.
- Surfaces (`device_surface_tests.rs`): placement through position, size,
  rotation, scale, and edge behavior; every LED's sampled point equals the
  touch over it, for each matrix start corner and each strip direction;
  named segments resolve through their own outputs, match routing's
  aliases with exact names first, and stay unplaced when placed twice, when
  two aliases match, or when only an unnamed output exists; unbound,
  ambiguous, and display-only outputs place nothing; non-finite points;
  backend bindings and their generation.
- Segments: the registry keeps a contact's segment through begin, end, a
  re-begin, and cancel, and routes it into held output; the payload places
  segment-named touches per segment; `InputEvent::Touch` omits `segment`
  when unset.
- USB seam: the bridge drops input before connect, attaches on activation,
  renews on interruption, retires on close and stays retired, and never
  disturbs a successor, even when an interruption reaches a superseded
  bridge first or a fresh connection activates between a stale bridge
  deciding to renew and the renewal; only bindings that publish input get
  a bridge; a disconnect retires the lease before the shutdown sequence
  runs. The registry renews only a lease that is still current.
- Push 2 (`push2_input_tests.rs`, `push2_touch_surface_tests.rs`): the MIDI
  decode and relay (spec 28 §14.1), and touch-to-LED agreement through the
  real frame encoder and decoder.
- Payload (`lightscript_interaction_payload_tests.rs`): touches and buttons
  serialize with surface and canvas positions, an unplaced device carries
  `canvas: null`, a held touch re-sends when its placement or the canvas
  size changes, undeclared effects receive nothing, the bootstrap installs
  the helpers, and the batch is re-sent once after an edge frame.
- Servo queue: a superseded touch edge maps with the newest placement, and
  an undemanded queue retains none.
- Daemon zone runtime: placement resolves through observed bindings, a zone
  without the device reports none, and rebinding or moving the output moves
  the touch.
- SDK: `getInputData()` reads touches and buttons, sanitizes malformed
  values, and the production adapter carries them end to end.
- Hardware: touch a Lightpad and LUMI with an interactive effect active and
  watch `input_events` on the WebSocket; with Touch Ripple active, a ripple
  should start under the finger.

## Known limits

- A touch whose end blocksd never sends, with the stream otherwise healthy,
  stays held until its contact index is reused or the device detaches.
  blocksd has no stale-touch recovery, and whether a stationary finger keeps
  streaming updates is unverified on hardware.
- blocksd releases without button identity make every button on that block
  one control named `button`.
- Canvas positions are in the zone canvas. A scene layer with a non-default
  transform (anchor, scale, rotation, or fit) draws its effect canvas into
  the zone through that transform, and the transform is not inverted, so
  touches on such a layer land offset.
- An attachment's LED remap (`led_mapping`) is not inverted, and outputs
  other than matrices and strips take the surface frame as the output's
  local frame. Strip outputs follow their direction since phase 3; before,
  only left-to-right strips agreed with their LEDs.
- Push 2 channel pressure carries one value for the whole device, so every
  held pad takes the hardest-pressed pad's aftertouch, floored at its own
  strike velocity. The driver does not
  switch the device to poly aftertouch, which would need a sysex the driver
  avoids sending.
- A Push 2 touch strip finger that stops exactly on the strip's center
  leaves the touch one step short of it until it moves, because autoreturn
  reports the same value just before every lift.

## Open questions

- LUMI touch `x` spans the keybed. Whether it maps linearly to the 24 key
  LEDs, or needs a key-shaped lookup, waits on a hardware capture.
- Whether gesture-heavy effects need sampled `TouchMoved` events in addition
  to held state. Touch Ripple needs only held positions per frame, so moves
  stay held state until an effect needs a sub-frame path.
