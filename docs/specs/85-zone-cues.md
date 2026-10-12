# 85 -- Zone Cues: Show-Grade Cuts

> Every effect change today replaces a zone's layer stack and lets the new
> producer warm up in public: HTML effects flash a placeholder, the layer
> reports Active before it shows a real pixel, and a cut cannot be timed
> to a beat. This spec keeps the presenting graph on screen until the
> incoming one has real pixels, makes runtime health tell the truth,
> reports when a cut actually reached the devices, and adds zone cues:
> effects prepared ahead of time that go live on the next frame boundary.

**Status:** DRAFT rev 2
**Crates:** `hypercolor-core` (effect pool, renderers, readiness), `hypercolor-daemon` (render zones, scene domain, API, admission), `hypercolor-types` (health, cue, and receipt contracts), `sdk` (page suspension)
**Amends:** Spec 78 §1.4, §2.3, Appendix A, and the WebSocket `command` message; LayerHealth semantics; effect-error fallback policy
**Author:** Nova
**Date:** 2026-10-12

---

## Table of Contents

0. [Goals, non-goals, doctrine](#0-goals-non-goals-doctrine)
1. [Evidence](#1-evidence)
2. [Producer incarnations and the presenting graph](#2-producer-incarnations-and-the-presenting-graph)
3. [Readiness](#3-readiness)
4. [The activation origin](#4-the-activation-origin)
5. [Truthful health and presentation receipts](#5-truthful-health-and-presentation-receipts)
6. [Zone cues](#6-zone-cues)
7. [Admission](#7-admission)
8. [Contract amendments and client migration](#8-contract-amendments-and-client-migration)
9. [Phases and acceptance](#9-phases-and-acceptance)
10. [Open questions](#10-open-questions)
11. [Review history](#11-review-history)

---

## 0. Goals, non-goals, doctrine

**Goals**

1. No layer-stack change ever presents a placeholder or an empty frame
   while new producers warm up. The presenting graph keeps rendering,
   live, until every new producer has produced a real frame or failed.
2. A cut lands on the next frame boundary after the request that
   triggers it, measured from the request's arrival to the frame that
   reached the device backends.
3. Runtime health means what it says, per producer incarnation, and any
   client can read the current truth after a reconnect.
4. One door. Apply is an implicit cue plus an immediate go, a stack
   change under hold is the same mechanism, and playlists prepare their
   next item through it.

**Non-goals:** crossfade transitions (reserved, §9 Phase 4), Servo worker
sharding (Design 70), go at a future timestamp (§10), cues for display
faces (§10), and any persistence of cues.

**Doctrine:** Spec 78 lockstep and one name per concept. Performance
baselines are product contracts: nothing here lowers render cadence,
resolution, or tier. Every resource bound derives from measured
capacity; where a platform cannot measure, the engine fails the
allocation that cannot be satisfied rather than inventing a cap. A user
action on the live scene is never refused for capacity.

## 1. Evidence

A scripted rehearsal of the demo film against the live rig measured the
following. Reproduce with `just show play demo/shows/rehearsal.ts` from
branch `nova/demo-video` (runner `64b00f0d7`); the 2026-10-12 runs used
the internal daemon v0.5.2 on engine `268261199`, preview profile, GPU
compositor, 640x480 canvas at a configured 60 FPS, on the owner's rig
(24 devices, 7,483 LEDs).

| Observation | Measured | Cause |
| --- | --- | --- |
| Request to layer Active, warm effects | 63 to 108 ms | 43 to 60 ms of synchronous scene commit, then admission on the next frame |
| What Active meant | admitted, not visible | any effect frame marks the layer Active (`render_zones/layer_rendering.rs:178-188`), including the Servo placeholder (`effect/servo/renderer.rs:137-151`, `:183-201`, `:233-235`) |
| A failed page load | Active forever, placeholder on the LEDs | `load_failed` is recorded (`renderer/load.rs:219-229`) but the degradation guard never checks it (`renderer.rs:304-315`) |
| `LayerHealth::Stalled` | never emitted | no production emitter exists |
| Five faces assigned together | a running face hard-stalled 516 ms | one Servo worker blocks on every page load (`servo/worker.rs:519-594`, `:694-739`); queue wait counts as stall |
| Re-applying the effect a zone runs | full Servo session rebuild | pool slots key on `(zone_id, layer_id)` (`effect/pool.rs:61-65`) and apply mints a new layer id |

The commit cost is being removed separately. Even at zero commit cost, a
cut to an HTML effect cannot land on a beat today, because its producer
starts loading only when the cut is requested.

## 2. Producer incarnations and the presenting graph

### 2.1 Incarnations

Every runtime producer instance gets a `ProducerIncarnation` (a
daemon-monotonic `u64`) distinct from its authored layer id. The pool
maps `(zone_id, layer_id)` to the current incarnation. Building a slot,
rebuilding it for a registry, canvas, or descriptor change
(`effect/pool.rs:516-531`), and adopting a cue's staged producer each
mint or transfer an incarnation explicitly. Health, failure
notifications, presentation receipts, and admission reservations are all
keyed by incarnation.

### 2.2 Three graphs per zone

For each zone the render thread keeps:

- **committed:** the stack in the latest published scene plan;
- **presenting:** the producers and per-layer render state that
  composition, sampling, and display finalization use this frame;
- **incoming:** committed layers whose incarnation is not in presenting.

A layer in both committed and presenting with the same incarnation is
**shared**: it advances exactly once per frame, keeps its clock, and
receives control updates immediately. Replacing `[A, B]` with `[A, C]`
therefore warms only C, while A keeps running untouched.

Reconcile no longer drops excluded slots at once (`effect/pool.rs:224`)
or installs new media, surface, projection, and spatial state
immediately (`render_zones/reconcile.rs:441`, `:503`). It stages the
incoming graph's resources beside the presenting graph's, and the
presenting resources are released only when presentation transfers.

### 2.3 The swap barrier

The incoming graph becomes presenting on the first frame where every new
incarnation in it is terminal: `Ready` (§3) or `Failed`. Then:

- Ready incarnations present their activation-origin frame (§4).
- Failed incarnations composite as transparent; their siblings present
  normally. If every new incarnation failed, the zone still swaps,
  showing whatever the shared layers produce, so a failure is visible
  rather than hidden behind a stale stack.
- Retiring incarnations are destroyed (detached, as today) and release
  their admission reservations when the worker acknowledges the destroy.

A new incarnation that has not reached a terminal state within its
first-real-frame deadline (§3.4) is marked `Failed { first_frame_timeout }`.

### 2.4 Supersession

If the committed stack changes again while an incoming graph is warming,
incoming incarnations absent from the newest stack are discarded without
ever presenting, incoming incarnations still present in it keep warming,
and the presenting graph stays until the newest incoming graph reaches
the barrier. A rapid series of applies never shows an intermediate
effect.

### 2.5 Geometry changes end a hold

A hold is a content transition. A change to canvas size, zone dimensions,
zone membership, or a display target is an authoring action, and
presenting old content at new geometry would sample the wrong LEDs or
fail display retention (`display_retention.rs:67`). So a geometry change
during a hold ends it: the zone presents the incoming graph at the new
geometry on the next frame, warming incarnations composite transparent
and report `Loading`, and the barrier no longer applies to that change.
Zone brightness and opacity are not geometry; they apply to the
presenting graph immediately.

### 2.6 Scene activation

Activation selects the incoming scene's zones immediately today
(`render_thread/scene_snapshot.rs:264`) and runs its transition clock
regardless of readiness (`scene_state.rs:28`). Under this spec:

- The presenting scene keeps presenting, with all its zones, until every
  new incarnation across the incoming scene's zones is terminal.
- The configured activation transition (`duration_ms`, easing, Oklab)
  starts on that frame, blending from the presenting scene's live output
  rather than a frozen canvas taken at request time.
- If a newer activation arrives first, the newest target wins and the
  presenting scene stays presenting until that target reaches the
  barrier. An activation interrupted mid-transition continues from the
  blended state it reached.

### 2.7 Demand and execution

Input demand (audio, screen, sensors, interaction) is the union over
presenting and incoming graphs (today only committed enabled layers
count, `scene_snapshot.rs:543`), so a retiring audio-reactive layer
keeps its audio until it stops presenting, and a warming screen-reactive
layer gets capture before it is shown. Incoming and staged producers
advance on every render-loop iteration, including iterations where the
scene composes nothing visible (`frame_composer.rs:205`) and while the
user has paused output. They stop only in session sleep
(`frame_executor.rs:366`), and resume warming when the session wakes.

## 3. Readiness

### 3.1 Every source declares readiness

`EffectRenderer` gains a required method, with no default, so every
renderer declares its own behavior:

```rust
fn readiness(&self) -> Readiness;

pub enum Readiness {
    /// Not yet producing real output.
    Warming,
    /// Has produced its activation-origin frame (§4).
    Ready,
    /// This incarnation will never produce real output.
    Failed { reason: String },
}
```

Readiness for every layer source:

| Source | Ready when | Failed when |
| --- | --- | --- |
| Servo HTML effect | the activation-origin frame completes | page load fails, the session dies, or the first-frame deadline passes |
| CPU builtin with synchronous init | after its first `tick` | init returns an error |
| `media_player` builtin | the first decoded sample is drawn (`effect/builtin/media_player.rs:103`) | the source cannot be resolved or opened |
| `web_viewport` builtin | the first completed navigation draws (`effect/builtin/web_viewport.rs:207`) | navigation fails |
| Media layer | the first decoded frame or the static image is drawn | the asset is missing or undecodable |
| ScreenRegion layer | the first capture sample for its region arrives | capture is unavailable or denied |
| ColorFill layer | immediately | never |

### 3.2 Servo in every build

Warm advancement polls load and render completion in both GPU-import and
CPU Servo builds; today only the `servo-gpu-import` override advances
(`effect/traits.rs:233`, `renderer.rs:238`). Every frame-counter
placeholder, scene and CPU paths alike, is deleted: a Servo renderer
that has nothing real to show returns `Pending` in every build.

### 3.3 Construction failures are contained

Pool preparation errors (`effect/pool.rs:139`), missing assets,
initialization errors, and controls the effect rejects become a `Failed`
incarnation for that one layer. None of them propagates as a scene-pass
error that clears runtime state and publishes black
(`frame_composer.rs:465`).

### 3.4 Deadlines

Each new incarnation has a first-real-frame deadline counted from its
creation: the Servo load deadline plus three of its own frame intervals,
and for other sources the source's own open or first-sample timeout.
This is separate from page-load completion: a page that loads but never
renders still fails.

## 4. The activation origin

A new incarnation renders exactly one frame at effect time zero, its
activation origin, and then holds. When it presents, that origin frame is
the first thing on the LEDs, and its clock starts advancing from zero on
that frame. The reset contract:

- **Cadence gate.** The Servo cadence gate compares effect time with the
  previous submission time (`servo/renderer/frame_queue.rs:39`); at
  presentation its previous-submission time is set to the origin, so the
  next submission is due one interval later.
- **Queued and in-flight work.** A held incarnation has no queued or
  in-flight renders: the origin render completes before Ready, and none
  is submitted until presentation.
- **Frame counters and cached output.** The frame counter restarts at
  zero; the cached output is the origin frame.
- **Page state.** The SDK gains a suspend and resume message. A held page
  pauses its animation loop (`sdk/packages/core/src/effects/base-effect.ts:135`)
  and its delta clock (`canvas-effect.ts:45`); resume continues from the
  origin time, so the first presented delta is one frame, not the hold
  duration. The worker throttles a held webview and unthrottles it at
  presentation; today it unthrottles during rendering and never
  rethrottles (`servo/worker.rs:836`).

A held incarnation therefore costs memory, not render time or Servo
worker time, and cannot drift while it waits.

## 5. Truthful health and presentation receipts

### 5.1 Health per incarnation

`LayerHealth`, keyed by incarnation and reported with it:

| State | Meaning |
| --- | --- |
| `Loading` | warming; not presenting |
| `Active` | presenting real output |
| `Degraded { reason }` | presenting, but a recoverable fault occurred (readback failure, transient media error); the last good pixels stay up |
| `Stalled` | a periodic producer missed four consecutive expected completions (§5.2) |
| `Failed { reason }` | terminal for this incarnation |
| `AssetMissing` | unchanged |

Today health survives producer rebuilds because it is keyed by authored
identity (`layer_runtime.rs:49`); keyed by incarnation, a rebuilt layer
starts at `Loading`. A fatal Servo polling error that removes the
session (`servo/renderer/frame_poll.rs:126`) is `Failed`; a recoverable
media error that keeps useful pixels (`layer_rendering.rs:292`) is
`Degraded`, not `Failed`. A failure is reported once per incarnation.

### 5.2 Production metadata

Producers report a completion generation, the completion time, and a
production mode: `Periodic { interval }`, `Static`, `Paused`, or
`Finished`. Only `Periodic` producers can stall, against their own
interval (a 30 FPS Servo face is judged at 30 FPS, not at the
compositor's 60). Returning cached output is never a completion
(`renderer.rs:214`), so a renderer stuck behind cached frames cannot look
healthy. Static images, paused playback, and finished media never stall.

### 5.3 Presentation receipts

`Active` is health; it is not proof that a cut reached the audience,
because health is published before final composition
(`frame_composer.rs:355`) and deferred LED sampling can reuse previously
published colors (`frame_sampling.rs:481`, `:685`). So each swap
allocates a **cut generation** for its zone. The generation travels with
the composed frame through GPU and CPU sampling (a pending sample that
reuses earlier colors carries the earlier generation, not the new one),
display finalization, and the hand-off to device backends. When the
first frame carrying generation G for a zone is handed to every backend
that zone feeds, the daemon publishes:

```rust
CutPresented {
    scene_id: SceneId,
    zone_id: ZoneId,
    cut_generation: u64,
    incarnations: Vec<ProducerIncarnation>,
    frame_number: u32,
    /// When the request that caused the cut arrived at the daemon.
    requested_at: EventTimestamp,
    presented_at: EventTimestamp,
}
```

`requested_at` is stamped when the HTTP request or WebSocket command
arrives. `presented_at - requested_at` is the cut latency this spec is
judged on. Delivery past the backend hand-off (USB transfer, network,
the device's own refresh) is per-device and outside the daemon; the
film measures it on camera.

### 5.4 Failure fallback is scoped

The fallback listener consumes `EffectError` and, under the `ClearZones`
policy, clears every zone containing the failed effect
(`startup/lifecycle.rs:884`, `domain/effect.rs:1092`). Under this spec:

- Only a presenting incarnation's failure is eligible for fallback.
  Incoming and staged failures never trigger it.
- Fallback is scoped to the failed layer in its zone of the active
  scene. Healthy siblings and other zones running the same effect are
  untouched. A failed layer compositing transparent already satisfies
  `ClearZones` for that layer; the policy narrows to that.

### 5.5 Runtime health snapshot

Events are not replayed across reconnects, and coalescing means a
reconnecting client may never see a past `Failed`
(`hypercolor-ui/src/ws/connection.rs:308`). `GET /scene/runtime` returns
the runtime truth with a generation:

```json
{ "generation": 412, "zones": [ { "zone_id": "…", "cut_generation": 9,
  "presenting": [ { "layer_id": "…", "incarnation": 77, "health": "active", "since": "…" } ],
  "incoming":   [ { "layer_id": "…", "incarnation": 81, "health": "loading", "since": "…" } ],
  "cues": [ … ] } ] }
```

Health, cue, and receipt events carry the generation. Clients fetch the
snapshot on connect and on `resync_required`, and apply events newer
than it. Nothing polls.

## 6. Zone cues

### 6.1 Model

A **cue** is a runtime-only staged effect for one zone of the active
scene. It carries the complete canonical source that going live will
commit, resolved at creation by the same rules the replacement primitive
uses:

```rust
pub struct Cue {
    /// Reserved at creation; becomes the layer id at go.
    pub id: SceneLayerId,
    pub zone_id: ZoneId,
    pub effect_id: EffectId,
    pub preset_id: Option<PresetId>,
    /// Normalized values, preset already resolved.
    pub controls: BTreeMap<String, ControlValue>,
    /// Bindings the replacement would carry (same-effect apply preserves
    /// bindings today, `core/scene/mod.rs:1423`).
    pub bindings: Vec<LayerBinding>,
    pub incarnation: ProducerIncarnation,
    /// Bumped whenever the cue's staged producer changes.
    pub generation: u64,
    pub state: CueState,
    pub reservation: AdmissionReservation,
    pub created_at: EventTimestamp,
    pub lease_expires_at: EventTimestamp,
}

pub enum CueState {
    Warming,
    Ready,
    /// Consumed by go; the id now names a layer.
    Live,
    /// The zone changed in a way that alters what go would commit.
    Stale { reason: String },
    Failed { reason: String },
    Discarded { reason: DiscardReason },
}
```

A cue is immutable. Changing what it would show means deleting it and
creating another. A cue becomes `Stale` when the zone changes in a way
that alters what go would commit (for example a binding added to the
layer the cue would replace); go on a stale cue is refused.

Cues live on a **cue board** inside the scene domain's state, under the
same lock as the scene manager. They are never stored in
`SceneManager.scenes`, never written to `scenes.json` or the runtime
session, and never appear in the `/scene` document, which remains the
authored state. Their runtime view is in `/scene/runtime` (§5.5).

### 6.2 Lifecycle

```
            create
              │
              ▼
   ┌───── Warming ─────┐
   │         │         │
   │   origin frame    │ failure or deadline
   │         ▼         ▼
   │       Ready     Failed
   │         │
   │        go ────────► Live (ownership moves to the layer)
   │
   ├── zone change alters the commit ──► Stale
   │
   └── delete, lease expiry, scene activation, zone removal ──► Discarded
```

- A rebuild trigger (registry, canvas, descriptor) gives a cue a new
  incarnation and returns it to `Warming`; its generation bumps.
- `Ready` cues hold their origin frame (§4).
- Go is accepted from `Warming` or `Ready`. From `Warming`, the zone
  holds per §2 until the adopted incarnation is terminal.
- At go, ownership of the staged producer moves to the layer. The cue's
  lease no longer applies, and nothing that happens to the cue record can
  destroy the live renderer.
- Leases default to 60 s, are set per cue with `lease_ms`, and are
  renewed with `POST .../cues/{cue}/renew`. A lease exists so a crashed
  client cannot strand staged producers; there is no maximum, because
  capacity is governed by admission (§7), not by age.

### 6.3 Go is one transaction

Go runs as a single scene-domain transaction under the manager lock:

1. **Replay.** If the cue is `Live` and the zone's stack is exactly one
   layer whose id is the cue id, return `200` with the current zone
   resource. Replay recognition precedes `If-Match`, so a client that
   lost the first response can safely retry.
2. **Precondition.** Evaluate `If-Match` against the scene revision
   (`412` on mismatch).
3. **Claim.** The cue must be `Warming` or `Ready`, its generation must
   equal the generation the client last saw (when supplied), the scene
   must not be snapshot-locked, and the cue's zone must belong to the
   active scene. A concurrent delete takes the same lock, so exactly one
   of delete and go wins.
4. **Commit.** Replace the zone's stack with one layer whose id is the
   cue id and whose source is the cue's canonical source, through the
   canonical replacement primitive parameterized with the reserved id
   (`core/scene/mod.rs:1830` mints a fresh id today).
5. **Publish.** Publish the scene plan together with the transfer of the
   cue's incarnation to the new layer, so the render thread adopts the
   staged producer in the same plan generation that introduces the
   layer.

Implicit cues (§6.6) are created inside the same transaction, so a
refused apply leaves nothing behind.

### 6.4 Routes

All under the live tree, per Spec 78 §1.5:

| Route | Body | Response |
| --- | --- | --- |
| `POST /scene/zones/{zone}/cues` | `{ effect_id, preset_id?, controls?, lease_ms? }` | `202` cue resource |
| `GET /scene/zones/{zone}/cues` | | `ListResponse<Cue>` |
| `GET /scene/zones/{zone}/cues/{cue}` | | cue resource |
| `DELETE /scene/zones/{zone}/cues/{cue}` | | `200` discarded cue resource |
| `POST /scene/zones/{zone}/cues/{cue}/renew` | `{ lease_ms? }` | `200` cue resource |
| `POST /scene/zones/{zone}/cues/{cue}/go` | `{ transition?, generation? }` | `200` zone resource |

Create validates the effect, zone, and controls exactly as apply does,
reserves admission (§7), and returns without waiting for readiness.
Errors: `404 cue_not_found`; `409 cue_failed`, `409 cue_stale`,
`409 cue_consumed` (go on a cue whose layer has since been replaced),
`409 cue_generation_mismatch`; `409 snapshot_locked` on create and go;
`409 admission_exhausted` (§7) on create; `412` on an `If-Match` mismatch.

Records of consumed cues are kept while their layer id is in the zone's
stack, which is exactly as long as a replay can be recognized. Records of
discarded cues are kept until the next scene activation. Neither needs a
timer.

### 6.5 Events

`CueStateChanged { scene_id, zone_id, cue_id, incarnation, generation,
state, reason, runtime_generation }` once per transition. Go also emits
the ordinary scene events for the replacement, `LayerHealthChanged`
on the swap frame, and `CutPresented` (§5.3) when the cut reaches the
backends.

### 6.6 Apply and playlists

`POST /effects/{id}/apply` is an implicit cue plus an immediate go, in
one transaction. Its request and response do not change; implicit cues
never appear in cue listings.

The playlist runtime moves out of the API layer into a domain service.
It creates a cue for item N+1 one lead time before item N ends and goes
live at item N's deadline, scheduled against a monotonic clock rather
than by sleeping after each apply (`api/library/playlists.rs:355-437`
drifts today). `PlaylistItem.transition_ms` is honored once crossfade
exists and rejected until then, rather than stored and ignored.

## 7. Admission

Phase 2 prerequisite. Every new incarnation (live, incoming, or staged)
holds an **admission reservation** across the resources it consumes:

| Resource | Cost measured as | Capacity measured from |
| --- | --- | --- |
| Servo worker time | render milliseconds per second at the producer's cadence, an EWMA per effect from its own completions | 1000 ms per second per worker, minus measured worker overhead |
| GPU memory | bytes of surfaces and exportable images the incarnation allocates, tracked at allocation | the platform budget: `VK_EXT_memory_budget` on Linux, `IDXGIAdapter3::QueryVideoMemoryInfo` on Windows, `recommendedMaxWorkingSetSize` on macOS |
| System memory | resident-set growth measured across the incarnation's creation | available memory from the OS |

- Reservations are taken atomically against a single ledger, so
  concurrent creations cannot collectively overshoot.
- A reservation is released when the resource is actually released: for
  Servo, when the worker acknowledges the session destroy
  (`servo/worker_client.rs:712`), not when the cue is logically
  discarded. Retiring and destroy-pending incarnations stay reserved.
- An effect's first incarnation is admitted on a conservative estimate
  (its declared surface sizes and the worker cost of the most expensive
  HTML effect measured so far) and its measured cost replaces the
  estimate after its first completions.
- Only cue creation is refused for capacity: `409 admission_exhausted`
  with the ledger's per-resource usage, capacity, and the cue's estimate
  in `details`. A live apply or activation is never refused; if it would
  exceed capacity, it proceeds and the ledger reports the overcommit in
  `/scene/runtime`, because refusing the user's own action would be a
  nerf.
- Where a platform cannot report a GPU budget, GPU admission is skipped
  and an allocation that fails marks that incarnation
  `Failed { out_of_memory }` and releases its reservation. No constant
  stands in for the missing measurement.

The ledger needs measurement interfaces the daemon does not expose today
(`render_thread/gpu_device.rs` configures allocation-error thresholds but
reports no usage). Building them is the first Phase 2 task.

## 8. Contract amendments and client migration

- **Spec 78 §1.4 Layer identity.** A cued layer's id is minted when the
  cue is created and becomes a layer id only at go. Before go it names a
  cue; discarded cue ids are never reused.
- **Spec 78 §2.3 Apply.** Apply is an implicit cue plus an immediate go,
  one transaction. Request and response are unchanged.
- **Spec 78 Appendix A.** Adds the six cue operations and
  `GET /scene/runtime`.
- **WebSocket `command`.** Gains an optional `if_match` field, mapped to
  the `If-Match` header on dispatch (`api/ws/command.rs:60`), so WS
  clients can fence go like REST clients (`api/ws/protocol.rs:519`).
- **LayerHealth.** Gains `Degraded`; semantics per §5.1, reported with
  the incarnation.
- **Events.** Adds `CutPresented` and `CueStateChanged`; health, cue,
  and receipt events carry the runtime generation.
- **Effect-error fallback.** Scoped per §5.4.
- **MCP.** Adds `cue_effect`, `go_cue`, and runtime health in status.

Per Spec 78 lockstep, each phase lands its shared types in
`hypercolor-types`, the WebSocket protocol manifest, the OpenAPI
operation inventory test (`daemon/tests/openapi_tests.rs:353`), the
generated Python client, and the UI, TUI (whose bridge ignores health
today, `hypercolor-tui/src/bridge.rs:209`), and MCP consumers in the
same change.

## 9. Phases and acceptance

**Phase 0: prerequisites (in flight).** Remove redundant per-commit
durable writes and give each WebSocket session its own command worker;
measure Servo stall time from dequeue; make the Servo worker serve
renders while page loads are in progress, so a held presenting HTML
layer cannot trip the hard-stall check (`renderer/frame_poll.rs:96-110`)
while its successor loads.

**Phase 1: incarnations, readiness, and truthful health.** §2.1, §3,
§5.1, §5.2, §5.4, §5.5, without holding. Acceptance:

- Every source in §3.1 has tests driving Warming, Ready, and Failed.
- A failed page load reports `Failed` once and composites transparent;
  no build shows a placeholder.
- A rebuilt layer starts at `Loading`; a recoverable fault reports
  `Degraded`; a stuck periodic producer reports `Stalled`; static and
  paused media never do.
- A reconnecting client recovers current health from `/scene/runtime`.
- Fallback never clears a healthy sibling or another zone.

**Phase 2: the presenting graph.** §2.2 to §2.7, §4, and §5.3.
Acceptance:

- Frame-by-frame daemon tests: the zone composites the presenting graph
  until the swap frame, shared layers advance once and keep their
  clocks, superseded incoming graphs never present, a geometry change
  ends the hold, and scene activation starts its transition on the
  barrier frame.
- The first presented frame of a new incarnation is its origin frame,
  and its second is one interval later.
- `CutPresented` reports `presented_at - requested_at`; the rehearsal's
  cut shots (`--shot S02,S04`) show no frame over budget on swap frames.

**Phase 3: admission and zone cues.** §6 and §7. Acceptance:

- Ledger tests: atomic reservation under concurrent creates, release on
  destroy acknowledgement, overcommit reported for live actions, and
  `out_of_memory` handling where no budget exists.
- Cue tests: lifecycle, lease and renew, stale detection, go replay
  before `If-Match`, delete racing go, generation fencing over REST and
  WebSocket, snapshot lock, and implicit-cue rollback on refusal.
- `scenes.json` and the runtime session are byte-identical before and
  after creating and deleting cues.
- With cues prepared one beat ahead, every cut in the rehearsal's
  cut-per-beat shot at 124 BPM has `presented_at` within one frame
  interval of `requested_at` plus that frame's own work, at the
  configured FPS, with no tier change.

**Phase 4: crossfade (reserved).** `TransitionType` gains
`crossfade { duration_ms, easing }` once the presenting and incoming
graphs can be composed separately and blended with an eased alpha in
Oklab, reusing the scene transition clock (`scene_state.rs:17-53`) and
two-layer alpha plans (`composition_planner.rs:149-249`), optionally
quantized to `AudioData.beat_detected`. Until then the field accepts only
`cut`.

## 10. Open questions

1. **Go at a timestamp.** Go on arrival is the first rev. A `go_at`
   against the daemon's monotonic clock would make cuts frame-exact for
   remote clients and needs a clock-sync handshake first.
2. **Live cue previews.** A ready cue holds its origin frame. A VJ-style
   UI may want animated cue thumbnails; that would run at a preview
   cadence on the preview path, reserved by admission, never on the LED
   path.
3. **Display face cues.** Faces gain the presenting graph in Phase 2.
   Cueing a face needs a display-domain route under `/displays/{id}/face`.
4. **Cues across scenes.** Cues belong to the active scene and are
   discarded on activation. Scene-level cues would let a show prepare
   across a scene change.
5. **Geometry under hold.** §2.5 ends a hold on geometry change. If live
   shows turn out to resize zones mid-performance, the alternative is
   re-projecting presenting producers at the new geometry.

## 11. Review history

- **rev 1 (2026-10-12):** initial draft from the demo-film rehearsal and
  a read-only architecture map of the render-thread layer lifecycle.
- **rev 2 (2026-10-12):** Codex review of rev 1 returned NEEDS_CHANGES
  with 20 findings (5 blocking). Dispositions: producer incarnations and
  three graphs per zone (findings 1, 11); geometry ends a hold (2);
  scene activation handoff and transition start on the barrier (3);
  readiness required for every source with a per-source table (4); warm
  advancement and placeholder removal in CPU Servo builds (5); terminal
  barrier and contained construction failures (6); the activation
  origin and page suspension contract (7, 18); presentation receipts
  carrying a cut generation to the backends, and acceptance measured
  from request arrival (8); demand from presenting and incoming graphs
  and staging through empty and paused frames (9); production metadata
  for Stalled (10); scoped fallback (12); go as one transaction with
  replay before `If-Match` and ownership transfer (13, 14); cues carry
  the complete canonical source and are immutable, with a Stale state
  (15); `if_match` on the WebSocket command (16); a measured admission
  ledger replacing the cited thresholds (17); runtime health snapshot
  and lockstep client migration (19); lease without a maximum, estimates
  in the reservation, and a reproduction block for the evidence (20).
