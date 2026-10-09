# Spec 82: Device Input from Driver-Owned Hardware

## Status

Phase 1 implemented (2026-09-28). Extends Spec 30
(ROLI Blocks backend), whose event sections were never built, and Spec 71
(interactive input pipeline), which shipped differently from its text. Read
`crates/hypercolor-core/src/input/` rather than Spec 71's design sections.

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
- Translating touches to canvas coordinates, and exposing touches to HTML
  effects through the LightScript payload and SDK. That is phase 2 below.
- Host capture changes. `[input].enabled` keeps meaning keyboard and pointer
  capture from the host.

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
    TouchBegan { contact: u32, position: TouchPosition },
    TouchMoved { contact: u32, position: TouchPosition },
    TouchEnded { contact: u32, position: TouchPosition },
    Button { button: Arc<str>, state: InputButtonState },
}

pub struct TouchPosition { pub x: f32, pub y: f32, pub pressure: f32 }
```

Positions are normalized device-surface coordinates in `[0, 1]`, origin at
the surface's top left as the device reports it. Pressure is `[0, 1]`.

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
its successor. Hosts without device input return `None`, so every existing
host and test double keeps compiling.

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
| `TouchBegan` | Clamped. A non-finite position drops the edge and counts it. A begin for a contact already down ends the old contact first. More than 32 contacts per device drops the edge. |
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
}

pub struct DeviceButtonHold { pub device_id: DeviceId, pub button: Arc<str> }
```

The router rebuilds held output every frame without allocating. Touch lookups
build their key from `Copy` fields, button lookups scan instead of building an
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
- Every event that could hide a lift supersedes leases so held input
  cancels: `device_removed` for that device, and an undecodable line or a
  lost connection for all devices. After an undecodable line the stream keeps
  reading; after a lost connection it reconnects with backoff from 250 ms
  doubling to 5 s, without cancelling again while blocksd stays unreachable.
  `device_added` payloads are ignored, so schema drift there cancels nothing.
  Dropping the backend aborts the stream.

## Phasing

1. **Pipeline (this change).** Everything above. Touches and buttons reach
   `FrameInputs.interaction` and the bus with device attribution, held
   state, and synthetic releases.
2. **Effects.** LightScript payload, JS adapter, bootstrap, and SDK types
   expose touches and device buttons. Touches gain canvas coordinates by
   resolving the device's zone through the active layout. A touch-ripple
   effect proves the path on a Lightpad.
3. **More producers.** Push 2 pads; a generic MIDI source.

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
  `device_removed`, an undecodable line, connection loss and resubscription,
  device disconnect, and the absence of an event connection without a sink.
  A unit test holds an aborted stream across a disconnect and reconnect and
  checks it publishes nothing into the new lease.
- Hardware: touch a Lightpad and LUMI with an interactive effect active and
  watch `input_events` on the WebSocket.

## Known limits

- A touch whose end blocksd never sends, with the stream otherwise healthy,
  stays held until its contact index is reused or the device detaches.
  blocksd has no stale-touch recovery, and whether a stationary finger keeps
  streaming updates is unverified on hardware.
- blocksd releases without button identity make every button on that block
  one control named `button`.

## Open questions

- LUMI touch `x` spans the keybed. Whether it maps linearly to the 24 key
  LEDs, or needs a key-shaped lookup, waits on a hardware capture.
- Whether gesture-heavy effects need sampled `TouchMoved` events in addition
  to held state. Phase 2 decides with a real effect.
