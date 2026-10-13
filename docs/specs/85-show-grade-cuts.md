# 85 -- Show-Grade Cuts: Truthful Health and Hold-Then-Swap

> Every effect change today replaces a zone's layer stack and lets the new
> producer warm up in public: HTML effects flash a placeholder, the layer
> reports Active before it shows a real pixel, a failed load stays Active
> forever, and nothing reports when a cut actually reached the devices.
> This spec makes runtime health tell the truth, keeps the presenting
> graph on screen until the incoming one has real pixels, and gives every
> cut a terminal delivery outcome per target with monotonic latency.
> Preparing effects ahead of time (zone cues) builds on this and is
> specified separately once this lands and its costs are measured (§10).

**Status:** DRAFT rev 5
**Crates:** `hypercolor-core` (effect pool, renderers, readiness, device output), `hypercolor-daemon` (render zones, sampling, display lane, scene domain, API, WebSocket), `hypercolor-types` (health, readiness, receipt contracts), `hypercolor-driver-api` (delivery outcomes), `sdk` (preparation handshake)
**Amends:** LayerHealth semantics; the effect-error fallback policy; Spec 78 Appendix A (one route)
**Author:** Nova
**Date:** 2026-10-12

---

## Table of Contents

0. [Goals, non-goals, doctrine](#0-goals-non-goals-doctrine)
1. [Evidence](#1-evidence)
2. [Incarnations, readiness, and truthful health](#2-incarnations-readiness-and-truthful-health)
3. [Hold-then-swap](#3-hold-then-swap)
4. [The preparation handshake and source classes](#4-the-preparation-handshake-and-source-classes)
5. [Cut receipts](#5-cut-receipts)
6. [Failure fallback](#6-failure-fallback)
7. [Runtime snapshot and client migration](#7-runtime-snapshot-and-client-migration)
8. [Resources](#8-resources)
9. [Phases and acceptance](#9-phases-and-acceptance)
10. [Follow-up: zone cues](#10-follow-up-zone-cues)
11. [Open questions](#11-open-questions)
12. [Review history](#12-review-history)

---

## 0. Goals, non-goals, doctrine

**Goals**

1. No layer-stack change presents a placeholder or an empty zone while
   new producers warm up. The presenting graph keeps rendering live until
   every new producer has produced real output or failed. Across a
   geometry change, and when a scene transition is interrupted, an
   immutable still of the last presented frame holds instead (§3.5,
   §3.6).
2. Health means what it says, per producer incarnation, and a client can
   read the current truth after a reconnect.
3. Every cut ends with a terminal outcome for every target it was routed
   to at the swap, with latency measured on a monotonic clock from the
   request's arrival.

**Non-goals:** preparing effects ahead of a request (zone cues, §10);
capacity admission (§10); crossfade between stacks in one zone; Servo
worker sharding (Design 70); physical latency past transport completion,
which the film measures on camera.

**Doctrine:** Spec 78 lockstep and one name per concept. Performance
baselines are product contracts: nothing here lowers render cadence,
resolution, or tier. A user action on the live scene is never refused for
capacity.

## 1. Evidence

On branch `nova/demo-video` at `ddcbde65d`, `demo/evidence/` holds the
raw runner reports for the full rehearsal (runner `c50a2498e`) and the
warm cut take (runner `6478d05a1`), and the daemon's API audit log for
both windows, with revisions, rig, and caveats in its README. Runner
`ackMs` is a client round trip; the audit log's `latency_ms` is the
daemon's own handling time.

| Observation | Measured | Cause |
| --- | --- | --- |
| Scene commit cost | server-side control PATCH p50 43 ms, p99 68 ms (n=254); applies 57 to 68 ms; output PATCH p50 3 ms (n=425), all from the audit log | synchronous durable writes per commit (tracked separately) |
| Request to layer Active, warm effects | 63 to 108 ms client-side (`cues[].activeMs`, warm report) | the commit above, then admission on the next frame |
| What Active meant | admitted, not visible | any effect frame marks the layer Active (`render_zones/layer_rendering.rs:178-188`), including the Servo placeholder (`effect/servo/renderer.rs:137-151`, `:183-201`, `:233-235`) |
| A failed page load | Active forever, placeholder on the LEDs | `load_failed` is recorded (`renderer/load.rs:219-229`) but never checked (`renderer.rs:304-315`) |
| `LayerHealth::Stalled` | never emitted | no production emitter exists |
| Five faces assigned together | a running face hard-stalled 516 ms (`events[]`, full report) | one Servo worker blocks on every page load (`servo/worker.rs:519-594`, `:694-739`); queue wait counted as stall |

## 2. Incarnations, readiness, and truthful health

Phase 1.

### 2.1 Incarnations

Every runtime producer instance gets a `ProducerIncarnation` (a
daemon-monotonic `u64`) distinct from its authored layer id. The pool
maps `(zone_id, layer_id)` to the current incarnation. Building a slot
and rebuilding it for a registry, canvas, or descriptor change
(`effect/pool.rs:516-531`) each mint a new incarnation. A control change
to a reused producer (`effect/pool.rs:596`) keeps the incarnation and
bumps its **preparation revision**. Health, failure notifications,
receipts, and deadlines are keyed by incarnation.

### 2.2 Readiness is declared per source

`EffectRenderer` gains a required method with no default:

```rust
fn readiness(&self) -> Readiness;

pub enum Readiness {
    /// No real output for the current preparation revision yet.
    Warming,
    /// Real output exists for the current preparation revision (§4).
    Ready,
    /// This incarnation will never produce real output.
    Failed { reason: String },
}
```

"Real output" means pixels the source authored, not a cleared or
placeholder canvas.

| Source | Class (§4) | Ready when | Failed when |
| --- | --- | --- | --- |
| Servo HTML effect, SDK page | prepared | the handshake's origin frame for the current preparation revision completes | load fails, the session dies, or the first-output deadline passes |
| Servo HTML effect, page without the handshake | live page | the first completed render after load | as above |
| `screen_cast` builtin | live capture | it draws from a renderable capture publication for its source (`effect/builtin/screen_cast.rs:46-52` clears and succeeds without one today) | capture is denied or unavailable past the deadline |
| `media_player` builtin or Media layer backed by a live stream (`media/producer.rs:451`) | live stream | the first decoded sample is drawn (`effect/builtin/media_player.rs:103`) | the source cannot be resolved or opened, or no sample arrives by the deadline |
| `media_player` builtin or Media layer, finite file or image | restartable | the frame at logical playback zero is drawn (§4) | the asset is missing or undecodable |
| `web_viewport` builtin and the authored WebViewport layer (`effect/pool.rs:842`) | live page | the first completed navigation draws (`effect/builtin/web_viewport.rs:207`) | navigation fails |
| Other CPU builtins | stateful synchronous | their first `tick` returns | init returns an error |
| ScreenRegion layer | live capture | the first capture sample for its region arrives (`render_zones/frame_helpers.rs:157` returns nothing today) | capture is denied or unavailable past the deadline |
| ColorFill layer | stateless | immediately; it has no Warming or Failed state | never |

Warm advancement polls load and render completion in both GPU-import and
CPU Servo builds (today only `servo-gpu-import` advances,
`effect/traits.rs:233`, `renderer.rs:238`). Every placeholder is deleted:
a renderer with nothing real to show returns `Pending` in every build.

Pool preparation errors (`effect/pool.rs:139`), missing assets,
initialization errors, and controls an effect rejects become a `Failed`
incarnation for that one layer, never a scene-pass error that clears
runtime state and publishes black (`frame_composer.rs:465`).

### 2.3 Deadlines and session sleep

Each new incarnation has a first-output deadline: the Servo load deadline
plus three of its own frame intervals for HTML effects, and five seconds
from the source starting for capture and media sources. Session sleep,
whether the machine sleeps or the daemon's idle policy turns output off
(`session.rs:248`), suspends every deadline owner, not just the outer
one: session creation, navigation (`servo/worker.rs:1158`), caller
receive timeouts (`servo/worker_client.rs:524`), handshake waits, and
production ages (`servo/session.rs:269`), finite-media waits, and
stream sample ages. An incarnation created or loading before sleep
resumes its deadlines on wake and is never failed for the time asleep.

A **preparation deadline** is separate from the load deadline. Each time
an authored control change bumps an incoming incarnation's preparation
revision (§3.3), a new deadline starts: three of its own frame intervals
plus the worker's script round trip for SDK pages, and one frame
interval for builtins. Timers and acknowledgements carry the revision
they belong to, so an answer for an obsolete revision is ignored. A
revision that misses its deadline fails the incarnation with
`Failed { reason: "prepare_timeout" }`; the barrier then treats it like
any failed incarnation.

### 2.4 Health

`LayerHealth`, reported with the incarnation, with readiness exposed
beside it:

| State | Meaning |
| --- | --- |
| `Loading` | not presenting yet: warming, or ready and waiting for its graph to swap |
| `Active` | presenting real output |
| `Degraded { reason }` | presenting, but a recoverable fault occurred (readback failure, transient media error); the last good pixels stay up |
| `Stalled` | a periodic producer missed four consecutive expected completions |
| `Failed { reason }` | terminal for this incarnation |
| `AssetMissing` | unchanged |

Health is keyed by incarnation, so a rebuilt layer starts at `Loading`
(it survives rebuilds today, `layer_runtime.rs:49`). A fatal Servo
polling error that removes the session (`servo/renderer/frame_poll.rs:126`)
is `Failed`; a recoverable media error that keeps useful pixels
(`layer_rendering.rs:292`) is `Degraded`. A failure is reported once per
incarnation.

Producers report a completion generation, completion time, and a
production mode: `Periodic { interval }`, `Static`, `Paused`, or
`Finished`. Only periodic producers can stall, judged against their own
interval. Returning cached output is never a completion
(`renderer.rs:214`).

## 3. Hold-then-swap

Phase 2.

### 3.1 Graphs per zone

For each zone the render thread keeps **committed** (the latest
published plan), **presenting** (the producers and per-layer render state
composition, sampling, and display finalization use this frame), and
**incoming** (committed layers whose incarnation is not presenting). A
zone may instead present a **hold still** (§3.5, §3.6).

A layer in both committed and presenting with the same incarnation is
**shared**: it advances exactly once per frame, keeps its clock, and
takes control updates immediately. Replacing `[A, B]` with `[A, C]` warms
only C.

Reconcile stops dropping excluded slots at once (`effect/pool.rs:224`)
and stops installing new media, surface, projection, and spatial state
immediately (`render_zones/reconcile.rs:441`, `:503`). It stages the
incoming graph's resources beside the presenting graph's and releases
the presenting ones when ownership transfers.

### 3.2 Input demand

Demand (audio, screen, interaction, media, network, sensors) is the union
over presenting and incoming graphs (`scene_snapshot.rs:543` counts only
committed layers today). Incoming producers advance on every render-loop
iteration, including iterations where the scene composes nothing visible
(`frame_composer.rs:205`) and while output is paused, subject to their
source class (§4), and stop only in session sleep.

### 3.3 The swap barrier

The incoming graph becomes presenting on the first frame where every new
incarnation in it is terminal: `Ready` for its current preparation
revision, or `Failed`. An authored control change to an incoming layer
bumps its preparation revision and returns it to `Warming` until real
output for the new values exists, so a held layer never presents pixels
that disagree with its committed controls. Values that arrive through
bindings or inputs (audio, sensors) are live data, not authored state,
and do not invalidate readiness.

- Ready incarnations present their prepared output (§4).
- Failed incarnations composite as transparent and their siblings
  present normally. If every new incarnation failed, the zone still
  swaps, so a failure is visible rather than hidden.
- Retiring incarnations stop presenting on that frame and, outside a
  scene transition (§3.6), are destroyed then.

### 3.4 Supersession

If the committed stack changes again while an incoming graph warms,
incoming incarnations absent from the newest stack are discarded without
ever presenting, those still present keep warming, and the presenting
graph (or hold still) stays until the newest incoming graph reaches the
barrier. A rapid series of applies never shows an intermediate effect.

### 3.5 Geometry changes and the hold still

A change to canvas size, zone dimensions, zone membership, or a display
target rebuilds producers (`effect/pool.rs:516`) and replaces projection
and sampling resources (`render_zones/reconcile.rs:503`). Before reconcile
replaces anything, the render thread captures the zone's **hold still**:
an immutable, owned copy of the zone's last completed frame.

| Zone path | Hold still |
| --- | --- |
| CPU-composed zone | a clone of the zone canvas |
| Compositor GPU zone | a copy into a dedicated texture the compositor never writes again (projected snapshots reuse writable storage, `sparkleflinger/gpu/snapshot.rs:40`) |
| Imported GPU frame | a copy, releasing the import lease |
| Direct display zone | the face's pre-finalization pixels, so finalization still applies brightness and geometry exactly once (`display_lane.rs:237`) |

Holds are captured per zone before composition into the aggregate, so
overlapping zones each keep their own frame (`render_zones/scene_retention.rs:37`
retains only the aggregate and available zone canvases today). The zone
presents its still, resampled to the new geometry, until every rebuilt
and new incarnation reaches the barrier. It never presents empty output
and never samples old content through old geometry.

A hold still freezes composition inside the zone. Output policy applied
after zone composition (zone brightness, display brightness, geometry)
still applies to the still immediately, which is why direct display
holds are taken before finalization. Changes inside the zone (layer
opacity, blend, and the bindings that drive them,
`binding_eval.rs:126`) cannot be represented by a flattened still and
take effect when the hold ends.

### 3.6 Scene activation

Activation renders and delivers per physical target, not per scene
canvas:

- The presenting scene keeps presenting, with all its zones, until every
  new incarnation across the incoming scene's zones is terminal.
- On that frame the configured transition (`duration_ms`, easing, Oklab)
  starts. Each graph is sampled and finalized with its own geometry, the
  contributions are reconciled by physical target (an LED, a display),
  blended once per target, and published as one output per target.
  Today CPU LEDs blend against a frozen sampled base
  (`frame_sampling.rs:797`) merged by output-zone identity (`:322`) and
  direct displays receive one face and one scene (`display_lane.rs:237`);
  both move to per-target blending.
- A target only the outgoing scene feeds fades toward the incoming
  scene's unassigned behavior; a target only the incoming scene feeds
  fades from the outgoing scene's unassigned behavior; a display fed by
  different face zones in the two scenes blends their pre-finalization face
pixels and finalizes once, so geometry and brightness apply exactly once.
- The outgoing graph keeps its producers, demand, and routing until the
  transition completes, then is destroyed.
- **Interruption.** If a newer activation arrives mid-transition, the
  current blended output of every target is captured as a hold still
  (as the planner freezes the last composed frame today,
  `composition_planner.rs:84`), and both contributing graphs are
  released. The still is taken before output policy: LED targets keep
  their blended colors before brightness, and displays keep their
  blended pre-finalization pixels, because finalization bakes geometry
  and brightness into pixels irreversibly (`display_lane.rs:237`,
  `display_finalize.wgsl:263`). Brightness and geometry changes during
  the hold therefore still apply. The newest target warms behind the still and transitions
  from it when it reaches the barrier. Each further interruption replaces
  the still and releases whatever was warming, so a zone never holds more
  than one still and one incoming graph.

## 4. The preparation handshake and source classes

A new incarnation should start where the audience first sees it, not
somewhere into a run nobody watched. How close each source can get
depends on its class.

| Class | Behavior while held | First presented frame |
| --- | --- | --- |
| prepared (SDK pages) | suspended after its origin (Phase 2) | the origin, rendered at effect time zero |
| stateful synchronous builtins | not advanced once Ready | its first frame; its clock starts at presentation |
| restartable (finite media, images) | paused at logical playback zero | the frame at logical playback zero, honoring the layer's authored `start_offset_secs`, speed, loop, and autoplay (`hypercolor-types/src/layer.rs:273`, `media/producer.rs:314`) |
| live page (legacy HTML, remote web viewport) | keeps executing (it cannot be suspended) | its latest live frame |
| live stream, live capture | keeps decoding or capturing (`media/producer.rs:451`, `:918`) | its latest live frame |
| stateless | nothing to hold | its frame |

**The SDK handshake.** SDK pages (built on `sdk/packages/core`) speak it;
capability detection uses the worker's pre-navigation injection
(`servo/worker/runtime_html.rs:52`) and its script callback
(`servo/worker.rs:1108`), which today discards the value it receives and
must carry the handshake's versioned state back.

1. **Init acknowledgement** (Phase 1). The page reports when its
   asynchronous initialization finishes (`base-effect.ts:109`); load
   completion alone (`servo/worker.rs:1163`) is not enough.
2. **Gated startup draw** (Phase 1). Under host control the page does
   not draw on startup (`base-effect.ts:125`) until asked.
3. **Origin draw** (Phase 1). The host requests one frame at effect time
   zero with the incarnation's controls and preparation revision. That
   frame, tagged with incarnation and revision, makes the incarnation
   `Ready` in Phase 1.
4. **Suspension acknowledgement** (Phase 2). The page pauses its
   animation loop (`base-effect.ts:135`), its delta clock
   (`canvas-effect.ts:45`), and SDK-managed timers, and acknowledges. In
   Phase 2, `Ready` requires both the origin for the current revision and
   this acknowledgement. Servo's own throttling slows timers rather than
   stopping them (`servo-0.6.0/webview.rs:730`), which is why suspension
   is cooperative.
5. **Resume** (Phase 2). At presentation the host resumes the page from
   the origin time. The first authored delta after the origin is one
   frame interval of logical time; the Servo cadence gate's
   previous-submission time (`frame_queue.rs:39`) is set to the origin
   and the frame counter restarts at zero.

Pages without the handshake keep executing while held and present live
state; their health is still truthful and their receipts report what was
delivered.

## 5. Cut receipts

`Active` is health, not proof that the audience saw the cut: health is
published before final composition (`frame_composer.rs:355`), deferred
sampling can replace samples (`frame_sampling.rs:481`, `:616`, `:685`),
device and display queues coalesce and deduplicate pending frames
(`device/output_queue.rs:742`, `:1552`, `:1733`, `:1745`;
`display_output/worker.rs:467`, `:474`, `:522`), and several logical
routes can share one physical payload (`device/manager/output_frame.rs:213`).

### 5.1 Operations

Every request that can change what renders gets an **operation id**,
minted when the HTTP request or WebSocket command arrives, together with
a monotonic arrival `Instant`. The id and instant travel with the
request through the session's command queue, synthetic REST dispatch
(`api/ws/command.rs:78`), the domain mutation's provenance
(`domain/mod.rs:536`, which carries only a trigger today), and the commit.
REST responses return the id in their response metadata, and WebSocket
command responses carry it too (they drop that metadata today,
`api/ws/command.rs:159`). Latency is always measured from the arrival
`Instant`, never by subtracting wall-clock stamps (`core/bus/mod.rs:118`
stamps `SystemTime`).

### 5.2 Cuts and target sets

A **cut** is one change in what a zone presents: a stack swap, a hold
starting or ending, or an activation transition. Each cut gets a
generation for its zone, the operation ids whose commits it carries, and
the composed **swap frame number** (`frame_executor.rs:731`). Activation
creates one cut per affected zone in both scenes, including zones only
the outgoing scene has, and owns the targets those zones fed.

At the swap the cut snapshots its **target set**: every logical target
routed at that moment (and, for activations, the union of outgoing and
incoming targets, including unassigned-behavior contributions),
identified by layout output identity and, when resolved, backend and
physical device. Frames carry **contribution metadata** keyed by scene,
zone, logical output, and cut generation through sampling, display
finalization, and physical aggregation, so a payload that combines zone
A's generation 5 with zone B's generation 9 advances each receipt
independently.

### 5.3 Terminal outcomes

For each target, a cut's generation is tracked across every attempt that
carries it, pending or in flight. Each target reaches exactly one
outcome, decided by these rules in order:

1. An in-flight attempt's acknowledgement is authoritative: transport
   completion gives `Completed`, a backend decline gives `Suppressed`,
   an error gives `Failed`.
2. A host-side decision not to dispatch is terminal as `Suppressed`,
   recorded with the stage (LED dedupe, display unchanged-frame or
   zero-brightness skip, direct-control ownership,
   `device/manager/output_frame.rs:125`) and the provenance of the
   payload the target keeps showing. Dedupe and direct control keep
   working as they do today.
3. `Superseded { by_generation }` applies only when a newer generation
   replaces the target's last pending attempt for this cut and no
   attempt for it remains in flight; if one is in flight, its
   acknowledgement decides.
4. Disconnection, route removal or remapping after the swap, and a
   missing physical mapping at the swap give `Disconnected`,
   `RouteRemoved`, and `Unmapped`; a pending attempt cancelled by them
   ends there, while an in-flight one still follows rule 1.
5. A backend whose acknowledgements do not reflect what the device
   accepted reports `Unverified`. The Blocks backend reports a negative
   acknowledgement as success today (`device/blocks/connection.rs:143`,
   `device/blocks/backend.rs:247`), and the display coordinator reduces
   its acknowledgements to `Result<()>` (`device/manager/display_output.rs:202`);
   both carry real outcomes once fixed.

```rust
pub enum DeliveryOutcome {
    Completed,
    Suppressed { stage: SuppressionStage, kept: Option<FrameProvenance> },
    Superseded { by_generation: u64 },
    Failed { reason: String },
    Disconnected,
    RouteRemoved,
    Unmapped,
    Unverified,
}
```

The backend families already expose usable acknowledgements (USB actors,
SMBus, Hue DTLS, Nanoleaf and WLED UDP, Govee LAN and cloud, OpenRGB
socket writes). Transport completion means the daemon finished sending;
for UDP and socket transports it is not proof of what the remote device
displayed.

### 5.4 Receipts

When every target in a cut's set has an outcome, the daemon publishes:

```rust
CutPresented {
    scene_id: SceneId,
    zone_id: ZoneId,
    cut_generation: u64,
    operation_ids: Vec<OperationId>,
    incarnations: Vec<ProducerIncarnation>,
    swap_frame_number: u32,
    /// Monotonic, from the earliest carried operation's arrival to the swap frame's composition.
    swap_latency_us: u64,
    targets: Vec<TargetDelivery>,
}

pub struct TargetDelivery {
    pub layout_output_id: String,
    pub backend_id: Option<String>,
    pub device_id: Option<DeviceId>,
    pub outcome: DeliveryOutcome,
    /// Monotonic, from the same arrival: dispatch start and transport completion.
    pub dispatched_latency_us: Option<u64>,
    pub completed_latency_us: Option<u64>,
    /// The frame actually delivered, which may be later than the swap frame.
    pub frame: Option<FrameProvenance>,
}
```

An operation whose commit is superseded before any swap presents it
(§3.4) is reported as `OperationSuperseded { operation_id, by }`, so a
client can tell a cut it will never see from a receipt it is still
waiting for. A clear-to-empty cut has no incarnations; its operation ids
identify it.

The origin guarantee holds at composition: the swap frame composes the
prepared output. A slow target may deliver a later frame of the same
generation first, and its `frame` says which. The vocabulary lives in
`hypercolor-types`.

## 6. Failure fallback

The fallback listener consumes `EffectError` and, under `ClearZones`,
clears every zone containing the failed effect
(`startup/lifecycle.rs:884`, `domain/effect.rs:1092`). Under this spec
only a presenting incarnation's failure is eligible, fallback is scoped
to that layer in its zone of the active scene, and a failed layer
compositing transparent satisfies `ClearZones` for that layer. Healthy
siblings and other zones running the same effect are untouched. Incoming
failures never trigger fallback.

## 7. Runtime snapshot and client migration

Events are not replayed across reconnects, and coalescing means a
reconnecting client may never see a past `Failed`
(`hypercolor-ui/src/ws/connection.rs:308`). `GET /scene/runtime` returns
the runtime truth with a generation:

```json
{ "generation": 412, "zones": [ { "zone_id": "…", "cut_generation": 9,
  "presenting": [ { "layer_id": "…", "incarnation": 77, "health": "active", "readiness": "ready", "since": "…" } ],
  "incoming":   [ { "layer_id": "…", "incarnation": 81, "health": "loading", "readiness": "warming", "since": "…" } ],
  "hold": "barrier" } ] }
```

`hold` is `none`, `barrier`, `geometry`, or `transition`. Health and
receipt events carry the generation; clients fetch the snapshot on
connect and on `resync_required` and apply newer events. Nothing polls.

Per Spec 78 lockstep, each phase lands its shared types, the WebSocket
protocol manifest, the OpenAPI operation inventory test
(`daemon/tests/openapi_tests.rs:353`), the generated Python client, and
the UI, TUI (whose bridge ignores health today,
`hypercolor-tui/src/bridge.rs:209`), and MCP consumers in the same
change. Spec 78 Appendix A gains `GET /scene/runtime`.

## 8. Resources

Per zone, at most: the presenting graph, one incoming graph, the outgoing
graph during an uninterrupted scene transition, and one hold still.
Supersession discards incoming graphs and interruption releases both
transition graphs behind a still, so repeated changes cannot accumulate
live graphs. That is the transient footprint applying effects already
creates, extended by load time and bounded by the number of zones changing
together. No admission is added and no live action is refused.

Logical removal is not release: a dropped load task keeps its helper
running (`servo/renderer/load.rs:35`), detached destruction drops its
acknowledgement (`servo/worker_client.rs:712`), and imported frames keep
their allocations through their own leases
(`hypercolor-gpu-frame/src/lib.rs:130`). Phase 2 therefore adds an
acknowledged detached-destruction path and measures overlap through
actual release, counting presenting, incoming, outgoing, hold stills,
cancellation-pending loads, destruction-pending sessions, and GPU frames
in flight, and reports it in `/scene/runtime`. Those measurements are
what cue admission (§10) needs.

## 9. Phases and acceptance

**Phase 0: prerequisites (in flight).** Remove redundant per-commit
durable writes and give each WebSocket session its own command worker;
measure Servo stall time from dequeue; make the Servo worker serve
renders while page loads are in progress; fix the Blocks acknowledgement.

**Phase 1: incarnations, readiness, truthful health.** §2, §4 steps 1
to 3, §6, §7, without holding. Acceptance:

- Each source in §2.2 has tests for the states it can reach and
  assertions that it cannot reach the others (ColorFill never warms or
  fails).
- An SDK page is Ready only after its origin frame; a page without the
  handshake only after a completed render.
- A failed page load reports `Failed` once and composites transparent;
  no build shows a placeholder.
- A rebuilt layer starts at `Loading`; a recoverable fault reports
  `Degraded`; a stuck periodic producer reports `Stalled`; static and
  paused media never do.
- Session sleep with loads, navigations, and renders in flight fails
  nothing on wake.
- A reconnecting client recovers health and readiness from
  `/scene/runtime`.
- Fallback never clears a healthy sibling or another zone.

**Phase 2: hold-then-swap, suspension, receipts.** §3, §4 steps 4 and 5,
§5, §8. Acceptance:

- Frame-by-frame daemon tests: the zone composites the presenting graph
  until the swap frame; shared layers advance once and keep their
  clocks; superseded incoming graphs never present; a control change to
  a held layer re-prepares it; a geometry change presents the hold still
  and never empty output; overlapping GPU zones each hold their own
  still; scene activation blends per physical target and keeps the
  outgoing graph live until completion; interrupting a transition
  releases both graphs behind a still.
- Timing is asserted at each boundary separately: authored logical time
  (the second frame after an origin is one interval later in effect
  time), Servo submission, render completion, composition, queue
  acceptance, and transport completion. The wall-time requirement is
  that a fresh frame composes at the first composition after its render
  completes.
- Receipts: every target in the cut's snapshot reaches exactly one
  terminal outcome, including the sequence where one attempt is in
  flight, another pending, and a newer generation replaces the pending
  one (the in-flight acknowledgement decides); host dedupe,
  unchanged-frame and zero-brightness skips, and direct control end in
  `Suppressed` with their stage; a payload aggregating two zones'
  generations advances each receipt separately; disconnects and
  remapping terminalize correctly; latencies are monotonic.
- Two separate bounds per target, each tested at an adverse phase offset
  (the swap landing just after the target's previous slot): dispatch of
  the new generation starts at the target's next output slot, so within
  one target interval of the swap frame (16.7 ms for 60 FPS USB targets,
  100 ms for a 10 FPS Nanoleaf); transport completes within that plus the
  target's measured p99 service time (encoding and transfer), which the
  receipt reports. Capture and reconcile frames are measured alongside
  swap frames, since hold-still copies happen before the swap.
- On the rig, the show runner joins each request's operation id to its
  `CutPresented` (or `OperationSuperseded`) through the swap frame
  number, judges completed deliveries against those bounds, asserts
  suppressed, superseded, and failed outcomes separately, and finds no
  frame over budget on capture, reconcile, or swap frames at the
  configured FPS.

## 10. Follow-up: zone cues

Preparing an effect before a request and going live on the next frame
boundary is the only way a cut to an HTML effect can land on a musical
beat. A cue is an incoming incarnation staged before its stack is
committed, so it reuses incarnations, readiness, the handshake, holds,
and receipts from this spec. Two review rounds on drafts that included
cues (rev 1 at `0e1c16259`, rev 2 at `b93364c56`, whose §6 and §7 remain
the starting text) settled these requirements for that spec:

**Lifecycle and identity**
- A cue's layer id is reserved at creation and becomes a layer id only at
  go; discarded ids are never reused.
- A cue stores the full canonical `LayerSource::Effect`, including the
  control-binding map same-effect apply preserves
  (`core/scene/mod.rs:1423`); layer-parameter bindings stay distinct.
- Cues are immutable; a zone change that alters what go would commit
  makes a cue `Stale`, and go refuses it.
- A content generation and the activation epoch fence every cue
  operation.
- Cues are runtime-only: tests assert `scenes.json` and the runtime
  session are byte-identical before and after creating and deleting
  them.

**Go**
- One scene-domain transaction (`domain/scene.rs:558`): replay
  recognition, then `If-Match`, then claim (state, generation, snapshot
  lock, active-scene membership), then commit through the replacement
  primitive with the reserved id, then publish the slot transfer with the
  plan. Delete takes the same lock, so exactly one of delete and go wins.
- After go, ownership of the staged producer moves to the layer; nothing
  done to the cue record can destroy it.
- Apply is an implicit cue plus go in one transaction; a refused apply
  rolls the implicit cue back.

**Leases and retention**
- Leases with explicit renewal and no maximum; expiry discards.
- Compact tombstones with a replay horizon tied to the lease, separate
  from producer payloads, with a defined response after the horizon;
  tombstone memory counts in admission.

**Admission**
- Atomic reservations against one ledger, released by resource owner
  (destroy acknowledgement, last frame lease, GPU completion), never on
  logical discard.
- Servo memory attributed per session where possible, with honest
  uncertainty where browser costs cannot be attributed
  (`effect/servo/memory.rs:5`); no estimate presented as a measurement.
- Worker service latency and burst cost (create, navigate, origin,
  suspend, destroy) judged against producer deadlines, not steady-state
  occupancy.
- Native GPU budgets chosen by the active backend, not the OS
  (`render_thread/gpu_device.rs:43`).
- Only cue creation can be refused for capacity; live actions never are.

**Integration**
- Warming cues join input demand; held cues follow their source class.
- `if_match` on the WebSocket `command` message (`api/ws/protocol.rs:519`,
  `api/ws/command.rs:60`).
- The playlist runtime becomes a domain service that cues item N+1 ahead
  and goes live at absolute monotonic deadlines (it drifts today,
  `api/library/playlists.rs:408`), and rejects `transition_ms` until
  crossfade exists.

## 11. Open questions

1. **Go at a timestamp,** after §10 and a clock-sync handshake.
2. **Geometry reprojection.** §3.5 holds a still across geometry changes;
   live re-projection of presenting producers is the alternative if
   shows resize zones mid-performance.
3. **Crossfade within a zone,** using the per-target blending §3.6
   introduces.

## 12. Review history

- **rev 1 (2026-10-12):** initial draft including zone cues.
- **rev 2:** Codex round 1 returned NEEDS_CHANGES (20 findings, 5
  blocking). Added incarnations, three graphs per zone, per-source
  readiness, an activation origin, receipts, go as one transaction,
  scoped fallback, a runtime snapshot, and a measured admission ledger.
- **rev 3:** Codex round 2 closed 10 of round 1's findings and left 10
  partial, and raised 18 more (4 blocking). Most concerned cues and
  admission, so rev 3 narrowed the spec to truthful health and
  hold-then-swap and recorded the cue requirements.
- **rev 4:** Codex round 3 accepted the narrowing and raised 14 findings
  (2 blocking). Dispositions: interrupted transitions release both
  graphs behind a hold still, bounding each zone to one still and one
  incoming graph (r3-1); every routed target reaches a terminal outcome,
  with `Superseded` for host-side replacement and a target snapshot at
  the swap (r3-2, r3-9); hold stills are immutable owned copies captured
  per zone before reconcile, per pipeline path (r3-3); activation blends
  per physical target, with defined old-only and new-only targets (r3-4);
  overlap is measured through actual release with an acknowledged
  detached destroy (r3-5); readiness is tied to a preparation revision
  that authored control changes bump, and Phase 2 readiness requires the
  suspension acknowledgement (r3-6); every deadline owner suspends
  across session sleep (r3-7); sources are classed, with stateful
  builtins frozen once ready and live sources presenting live (r3-8);
  `Unverified` for backends whose acknowledgements are untrue, starting
  with Blocks (r3-10); acceptance asserts each timing boundary
  separately and the runner correlates cuts with receipts (r3-11); the
  minimal handshake moves to Phase 1 (r3-12); §10 now carries the full
  cue requirements (r3-13); evidence separates client round trips from
  the server audit log and the counts are corrected (r3-14).
- **rev 5:** Codex round 4 closed 9 of the 14 and left 5 partial, and
  raised 8 (1 blocking). Dispositions: host-side suppression (dedupe,
  unchanged or zero-brightness skips, direct control) is a terminal
  `Suppressed` with its stage and the kept payload (r4-1);
  `Superseded` requires that no attempt for the generation remain in
  flight, and outcomes follow a stated precedence (r4-2); hold stills
  freeze composition inside a zone while post-composition brightness and
  geometry still apply, and interruption stills are taken before output
  policy (r4-3); re-preparation gets its own revision-fenced deadline
  (r4-4); finite media start at logical playback zero honoring authored
  offset, speed, loop, and autoplay, and stream-backed Media layers are
  classed as live streams (r4-5); contribution metadata survives
  physical aggregation, and activation cuts own the union of outgoing
  and incoming targets (r4-6); operation ids from ingress and the swap
  frame number join requests to receipts, with `OperationSuperseded` for
  operations no swap presents (r4-7); dispatch and transport completion
  have separate per-target bounds tested at adverse phase (r4-8).
