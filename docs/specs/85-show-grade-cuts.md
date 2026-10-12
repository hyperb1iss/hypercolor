# 85 -- Show-Grade Cuts: Truthful Health and Hold-Then-Swap

> Every effect change today replaces a zone's layer stack and lets the new
> producer warm up in public: HTML effects flash a placeholder, the layer
> reports Active before it shows a real pixel, a failed load stays Active
> forever, and nothing reports when a cut actually reached the devices.
> This spec makes runtime health tell the truth, keeps the presenting
> graph on screen until the incoming one has real pixels, and reports each
> cut's delivery per device with monotonic latency. Preparing effects
> ahead of time (zone cues) builds on this and is specified separately
> once this lands and its costs are measured (§10).

**Status:** DRAFT rev 3
**Crates:** `hypercolor-core` (effect pool, renderers, readiness, device output), `hypercolor-daemon` (render zones, scene domain, API, WebSocket), `hypercolor-types` (health, readiness, and receipt contracts), `hypercolor-driver-api` (delivery outcomes), `sdk` (preparation handshake)
**Amends:** LayerHealth semantics; the effect-error fallback policy; Spec 78 Appendix A (one route)
**Author:** Nova
**Date:** 2026-10-12

---

## Table of Contents

0. [Goals, non-goals, doctrine](#0-goals-non-goals-doctrine)
1. [Evidence](#1-evidence)
2. [Incarnations, readiness, and truthful health](#2-incarnations-readiness-and-truthful-health)
3. [Hold-then-swap](#3-hold-then-swap)
4. [The preparation handshake and activation origin](#4-the-preparation-handshake-and-activation-origin)
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
   every new producer has produced real output or failed; across a
   geometry change, the last presented frame holds instead (§3.5).
2. Health means what it says, per producer incarnation, and a client can
   read the current truth after a reconnect.
3. Every cut reports, per device target, when its first frame was
   accepted and when transport completed, with latency measured on a
   monotonic clock from the request's arrival.

**Non-goals:** preparing effects ahead of a request (zone cues, §10);
capacity admission (§10); crossfade transitions; Servo worker sharding
(Design 70); camera-side physical latency, which the film measures.

**Doctrine:** Spec 78 lockstep and one name per concept. Performance
baselines are product contracts: nothing here lowers render cadence,
resolution, or tier. A user action on the live scene is never refused for
capacity.

## 1. Evidence

Raw reports: `demo/evidence/2026-10-12-rehearsal-full.json` (runner
`c50a2498e`) and `demo/evidence/2026-10-12-rehearsal-cuts-warm.json`
(runner `6478d05a1`) on branch `nova/demo-video` at `28c037693`, with
the daemon revision, rig, and caveats in that directory's README. Both
were taken against the internal daemon v0.5.2 on engine `268261199`,
preview profile, GPU compositor, 640x480 canvas at a configured 60 FPS,
on a 24-device, 7,483-LED rig.

| Observation | Measured (report field) | Cause |
| --- | --- | --- |
| Request to layer Active, warm effects | 63 to 108 ms (`cues[].activeMs`, warm report) | 43 to 60 ms of synchronous scene commit (`cues[].ackMs`; server-side in the daemon's audit log), then admission on the next frame |
| What Active meant | admitted, not visible | any effect frame marks the layer Active (`render_zones/layer_rendering.rs:178-188`), including the Servo placeholder (`effect/servo/renderer.rs:137-151`, `:183-201`, `:233-235`) |
| A failed page load | Active forever, placeholder on the LEDs | `load_failed` is recorded (`renderer/load.rs:219-229`) but never checked (`renderer.rs:304-315`) |
| `LayerHealth::Stalled` | never emitted | no production emitter exists |
| Five faces assigned together | a running face hard-stalled 516 ms (`events[]` effect_error, full report) | one Servo worker blocks on every page load (`servo/worker.rs:519-594`, `:694-739`); queue wait counts as stall |

## 2. Incarnations, readiness, and truthful health

Phase 1. No behavior change to when stacks swap; everything here makes
the current behavior observable and honest.

### 2.1 Incarnations

Every runtime producer instance gets a `ProducerIncarnation` (a
daemon-monotonic `u64`) distinct from its authored layer id. The pool
maps `(zone_id, layer_id)` to the current incarnation. Building a slot
and rebuilding it for a registry, canvas, or descriptor change
(`effect/pool.rs:516-531`) each mint a new incarnation. Health, failure
notifications, receipts, and deadlines are keyed by incarnation.

### 2.2 Readiness is declared per source

`EffectRenderer` gains a required method with no default, so every
renderer states its own behavior:

```rust
fn readiness(&self) -> Readiness;

pub enum Readiness {
    /// No real output yet.
    Warming,
    /// Has produced real output (for prepared producers, its origin frame, §4).
    Ready,
    /// This incarnation will never produce real output.
    Failed { reason: String },
}
```

"Real output" means pixels the source authored, not a cleared or
placeholder canvas.

| Source | Ready when | Failed when |
| --- | --- | --- |
| Servo HTML effect, SDK page | the preparation handshake's origin frame completes (§4) | load fails, the session dies, or the first-output deadline passes |
| Servo HTML effect, page without the handshake | the first completed render after load | as above |
| `screen_cast` builtin | it draws from a renderable capture publication for its selected source (`effect/builtin/screen_cast.rs:46-52` clears and succeeds without one today) | capture is denied or unavailable past the deadline |
| `media_player` builtin | the first decoded sample is drawn (`effect/builtin/media_player.rs:103`) | the source cannot be resolved or opened |
| `web_viewport` builtin and the authored WebViewport layer source (`effect/pool.rs:842`) | the first completed navigation draws (`effect/builtin/web_viewport.rs:207`) | navigation fails |
| Other CPU builtins | their first `tick` returns, since they draw synchronously | init returns an error |
| Media layer | the first decoded frame or the static image is drawn | the asset is missing or undecodable |
| ScreenRegion layer | the first capture sample for its region arrives (`render_zones/frame_helpers.rs:157` returns nothing today) | capture is denied or unavailable past the deadline |
| ColorFill layer | immediately; it has no Warming or Failed state | never |

Warm advancement polls load and render completion in both GPU-import and
CPU Servo builds (today only `servo-gpu-import` advances,
`effect/traits.rs:233`, `renderer.rs:238`). Every placeholder is deleted:
a renderer with nothing real to show returns `Pending` in every build.

Pool preparation errors (`effect/pool.rs:139`), missing assets,
initialization errors, and controls an effect rejects become a `Failed`
incarnation for that one layer, never a scene-pass error that clears
runtime state and publishes black (`frame_composer.rs:465`).

### 2.3 Deadlines

Each new incarnation has a first-output deadline: the Servo load
deadline plus three of its own frame intervals for HTML effects, and
five seconds from the source starting for capture and media sources.
Deadlines run on a clock that pauses during session sleep
(`frame_executor.rs:366`), so an incarnation created before the machine
sleeps is not failed for the time it slept.

### 2.4 Health

`LayerHealth`, reported with the incarnation, and readiness exposed
alongside it:

| State | Meaning |
| --- | --- |
| `Loading` | not presenting yet: warming, or ready and waiting for its graph to swap (§3) |
| `Active` | presenting real output |
| `Degraded { reason }` | presenting, but a recoverable fault occurred (readback failure, transient media error); the last good pixels stay up |
| `Stalled` | a periodic producer missed four consecutive expected completions |
| `Failed { reason }` | terminal for this incarnation |
| `AssetMissing` | unchanged |

`Loading` covers both warming and ready-but-held; the runtime snapshot
(§7) reports the incarnation's `Readiness` beside its health so clients
can tell the two apart. Today health survives rebuilds because it is
keyed by authored identity (`layer_runtime.rs:49`); keyed by
incarnation, a rebuilt layer starts at `Loading`. A fatal Servo polling
error that removes the session (`servo/renderer/frame_poll.rs:126`) is
`Failed`; a recoverable media error that keeps useful pixels
(`layer_rendering.rs:292`) is `Degraded`. A failure is reported once per
incarnation.

Producers report a completion generation, completion time, and a
production mode: `Periodic { interval }`, `Static`, `Paused`, or
`Finished`. Only periodic producers can stall, judged against their own
interval (a 30 FPS display face at 30 FPS, a 10 FPS Nanoleaf-bound layer
at its own rate). Returning cached output is never a completion
(`renderer.rs:214`).

## 3. Hold-then-swap

Phase 2.

### 3.1 Three graphs per zone

For each zone the render thread keeps **committed** (the latest
published plan), **presenting** (the producers and per-layer render state
composition, sampling, and display finalization use this frame), and
**incoming** (committed layers whose incarnation is not presenting).

A layer in both committed and presenting with the same incarnation is
**shared**: it advances exactly once per frame, keeps its clock, and
takes control updates immediately. Replacing `[A, B]` with `[A, C]` warms
only C.

Reconcile stops dropping excluded slots at once (`effect/pool.rs:224`)
and stops installing new media, surface, projection, and spatial state
immediately (`render_zones/reconcile.rs:441`, `:503`). It stages the
incoming graph's resources beside the presenting graph's and releases
the presenting ones only when ownership transfers (§3.3, §3.6).

### 3.2 Input demand

Demand (audio, screen, interaction, media, network, sensors) is the union
over presenting and incoming graphs (`scene_snapshot.rs:543` counts only
committed layers today), so a retiring audio-reactive layer keeps audio
until it stops presenting and a warming screen-reactive layer gets
capture before it is shown. Incoming producers advance on every
render-loop iteration, including iterations where the scene composes
nothing visible (`frame_composer.rs:205`) and while output is paused, and
stop only in session sleep.

### 3.3 The swap barrier

The incoming graph becomes presenting on the first frame where every new
incarnation in it is terminal: `Ready` or `Failed`.

- Ready incarnations present their first real output, which for prepared
  producers is their origin frame (§4).
- Failed incarnations composite as transparent and their siblings
  present normally. If every new incarnation failed, the zone still
  swaps, so a failure is visible rather than hidden behind a stale stack.
- Retiring incarnations stop presenting on that frame. Outside a scene
  transition (§3.6) they are destroyed then, detached as today.

### 3.4 Supersession

If the committed stack changes again while an incoming graph warms,
incoming incarnations absent from the newest stack are discarded without
ever presenting, those still present keep warming, and the presenting
graph stays until the newest incoming graph reaches the barrier. A rapid
series of applies never shows an intermediate effect.

### 3.5 Geometry changes

A change to canvas size, zone dimensions, zone membership, or a display
target rebuilds producers (`effect/pool.rs:516`) and replaces projection
and sampling resources (`render_zones/reconcile.rs:503`). The zone then
holds its last presented composed frame, resampled to the new geometry,
as a still image until every rebuilt and new incarnation reaches the
barrier. It never presents empty output and never samples old content
through old geometry. Display zones whose target dimensions change hold
the same way, replacing the retained-frame check that rejects a size
change today (`display_retention.rs:67`). Brightness and opacity are not
geometry and apply to the presenting graph immediately.

### 3.6 Scene activation

- The presenting scene keeps presenting, with all its zones, until every
  new incarnation across the incoming scene's zones is terminal.
- The configured activation transition (`duration_ms`, easing, Oklab)
  starts on that frame and blends from the presenting scene's live
  output rather than a frozen canvas taken at request time
  (`composition_planner.rs:211`).
- The outgoing graph keeps its producers, demand, and routing until the
  transition completes, and only then is destroyed.
- A newer activation that arrives mid-transition makes the current
  blended output the presenting state: both contributing graphs stay
  alive and keep rendering until the newest target reaches the barrier
  and its own transition completes.

## 4. The preparation handshake and activation origin

A new incarnation should present an animation that starts at time zero
on the frame it becomes visible, not one that has been running unseen
while it loaded. That needs the page's cooperation, because Servo's
throttling slows timers rather than stopping them
(`servo-0.6.0/webview.rs:730`, `servo-script-0.6.0/dom/window/window.rs:3642`).

**SDK pages** (built on `sdk/packages/core`) implement a handshake:

1. **Init acknowledgement.** The page reports when its asynchronous
   initialization finishes (`base-effect.ts:109`); load completion alone
   (`servo/worker.rs:1163`) is not enough.
2. **Gated startup draw.** Under host control the page does not draw on
   startup (`base-effect.ts:125`); it waits for the host.
3. **Origin draw.** The host requests one frame at effect time zero with
   the incarnation's authoritative controls. That frame is the origin,
   tagged with the incarnation, and makes the incarnation `Ready`.
4. **Suspension acknowledgement.** The page pauses its animation loop
   (`base-effect.ts:135`), its delta clock (`canvas-effect.ts:45`), and
   SDK-managed timers, and acknowledges. Nothing renders or advances
   until resume.
5. **Resume.** At presentation the host resumes the page from the origin
   time; the first delta after the origin is one frame interval. The
   Servo cadence gate's previous-submission time (`frame_queue.rs:39`)
   is set to the origin, and the frame counter restarts at zero.

**Pages without the handshake** (legacy HTML effects and remote pages in
the web viewport, which get no engine globals,
`effect/builtin/web_viewport.rs:103`) are `Ready` after their first
completed render following load. They keep executing while held, so
they present live state, not an origin frame; their health is still
truthful, and the receipt reports what was delivered. The handshake is
capability-detected, so both kinds coexist.

**Builtins and media** have no hidden execution: their first real output
is the origin, and their clock starts at presentation.

## 5. Cut receipts

`Active` is health, not proof that the audience saw the cut: health is
published before final composition (`frame_composer.rs:355`), deferred
sampling can reuse or replace samples (`frame_sampling.rs:481`, `:616`,
`:685`), and device and display queues coalesce pending frames
(`device/output_queue.rs:1552`, `:1745`; `display_output/worker.rs:467`).

Each swap allocates a **cut generation** for its zone. Frames carry the
generation and their source incarnations through composition, GPU and
CPU sampling, display finalization, and output. A sample or queued frame
that is replaced by a newer frame of the same generation passes the
generation on; one that reuses colors from before the swap carries the
earlier generation. Each device target reports a delivery outcome for
the first frame of generation G it handled, using the backend delivery
acknowledgements that already distinguish completion, suppression, and
failure (`hypercolor-driver-api/src/backend.rs:164`).

When every target the zone feeds has an outcome, the daemon publishes:

```rust
CutPresented {
    scene_id: SceneId,
    zone_id: ZoneId,
    cut_generation: u64,
    incarnations: Vec<ProducerIncarnation>,
    /// Monotonic, from request arrival to the swap frame.
    swap_latency_us: u64,
    targets: Vec<TargetDelivery>,
}

pub struct TargetDelivery {
    pub backend_id: String,
    pub device_id: DeviceId,
    pub outcome: DeliveryOutcome, // Completed | Suppressed | Failed | Disconnected | Unmapped
    /// Monotonic, from request arrival to queue acceptance and to transport completion.
    pub accepted_latency_us: Option<u64>,
    pub completed_latency_us: Option<u64>,
    pub frame_number: u32,
}
```

Latencies are measured from a monotonic `Instant` captured when the HTTP
request or WebSocket command arrived and carried through the session's
command queue and dispatch, never by subtracting wall-clock timestamps
(`core/bus/mod.rs:118` stamps `SystemTime`). The shared vocabulary for
this lives in `hypercolor-types`, below core. Delivery past transport
completion (USB transfer, network, the device's own refresh) is outside
the daemon; the film measures it on camera.

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

`hold` is `none`, `barrier` (waiting on incoming incarnations), or
`geometry` (§3.5). Health and receipt events carry the generation;
clients fetch the snapshot on connect and on `resync_required` and apply
newer events. Nothing polls.

Per Spec 78 lockstep, each phase lands its shared types, the WebSocket
protocol manifest, the OpenAPI operation inventory test
(`daemon/tests/openapi_tests.rs:353`), the generated Python client, and
the UI, TUI (whose bridge ignores health today,
`hypercolor-tui/src/bridge.rs:209`), and MCP consumers in the same
change. Spec 78 Appendix A gains `GET /scene/runtime`.

## 8. Resources

Hold-then-swap keeps at most two graphs per zone alive at once
(presenting and incoming; superseded incoming graphs are discarded, and
a mid-transition activation keeps the two graphs already blending). That
is the transient footprint applying effects already creates, extended by
load time, and it is bounded by the number of zones changing together.
No new unbounded resource is introduced, so no admission is added here,
and a user's live action is never refused. Phase 2 does measure the hold
overlap (presenting plus incoming incarnations, their Servo sessions,
and GPU allocations) and reports it in `/scene/runtime`, which supplies
the measurements cue admission needs (§10).

## 9. Phases and acceptance

**Phase 0: prerequisites (in flight).** Remove redundant per-commit
durable writes and give each WebSocket session its own command worker;
measure Servo stall time from dequeue; make the Servo worker serve
renders while page loads are in progress, so a presenting HTML layer
cannot trip the hard-stall check (`renderer/frame_poll.rs:96-110`) while
its successor loads.

**Phase 1: incarnations, readiness, truthful health.** §2, §6, §7
without holding. Acceptance:

- Each source in §2.2 has tests for the states it can reach and
  assertions that it cannot reach the others (ColorFill never warms or
  fails).
- A failed page load reports `Failed` once and composites transparent;
  no build shows a placeholder.
- A rebuilt layer starts at `Loading`; a recoverable fault reports
  `Degraded`; a stuck periodic producer reports `Stalled`; static and
  paused media never do.
- A deadline spanning session sleep does not fail a healthy incarnation.
- A reconnecting client recovers health and readiness from
  `/scene/runtime`.
- Fallback never clears a healthy sibling or another zone.

**Phase 2: hold-then-swap, preparation, receipts.** §3, §4, §5, §8.
Acceptance:

- Frame-by-frame daemon tests: the zone composites the presenting graph
  until the swap frame; shared layers advance once and keep their
  clocks; superseded incoming graphs never present; a geometry change
  holds the last frame and never presents empty output; scene activation
  starts its transition on the barrier frame and keeps the outgoing graph
  live until it completes.
- An SDK page presents its origin frame first and its second frame one
  interval later; a page without the handshake presents live output and
  reports Ready only after a completed render.
- Receipts: for the rehearsal's cut shots (`just show play
  demo/shows/rehearsal.ts --shot S02,S04`), every target reports an
  outcome, `swap_latency_us` and per-target `accepted_latency_us` come
  from monotonic clocks, and each target's first delivery of the new
  generation lands within that target's own output interval after the
  swap frame (60 FPS USB targets at the compositor's interval, a 10 FPS
  Nanoleaf at 100 ms), with no frame over budget on swap frames at the
  configured FPS.

## 10. Follow-up: zone cues

Preparing an effect ahead of a request and going live on the next frame
boundary is the only way a cut can land on a musical beat when the
incoming effect is an HTML page. It builds directly on this spec: a cue
is an incoming incarnation that is staged before its stack is committed.
Two review rounds on an earlier draft that included cues established
what that spec must settle, and it is deferred until Phase 2 has
produced real measurements:

- **Admission.** Cues introduce unbounded staging, so they need a
  capacity model: reservations released by resource owner (session
  destroy acknowledgement, last frame lease, GPU completion callback,
  `hypercolor-gpu-frame/src/lib.rs:130`,
  `linux-gpu-interop/src/linux/vulkan_export.rs:170`), Servo memory that
  is attributed rather than inferred from process RSS
  (`effect/servo/memory.rs:5`), worker service latency and burst cost
  (create, navigate, origin, suspend, destroy) rather than steady-state
  occupancy, and native GPU budgets selected by the active backend, not
  the OS (`render_thread/gpu_device.rs:43`).
- **Canonical source.** A cue stores the full `LayerSource::Effect`,
  including its control-binding map, which same-effect apply preserves
  (`core/scene/mod.rs:1423`); layer-parameter bindings stay distinct.
- **One transaction.** Go claims the cue, checks `If-Match` (after
  replay recognition), the snapshot lock, and active-scene membership,
  commits through the replacement primitive with the reserved layer id,
  and publishes the slot transfer with the plan, all under the scene
  domain lock (`domain/scene.rs:558`).
- **Retention.** Compact tombstones with a replay horizon tied to the
  cue's lease, separate from producer payloads, so `409 cue_consumed` is
  answerable and create/delete churn does not grow without bound.
- **Demand.** Warming cues join the demand union; held cues state their
  input policy.
- **Transport.** `if_match` on the WebSocket `command` message
  (`api/ws/protocol.rs:519`, `api/ws/command.rs:60`).

## 11. Open questions

1. **Go at a timestamp** (needs §10 first, then a clock-sync handshake).
2. **Geometry reprojection.** §3.5 holds a still across geometry changes.
   If live shows resize zones mid-performance, presenting producers could
   instead be re-projected live at the new geometry.
3. **Crossfade.** Once two graphs can present together (§3.6 already
   blends during scene transitions), `TransitionType` can gain
   `crossfade { duration_ms, easing }` for stack replacements within a
   zone.

## 12. Review history

- **rev 1 (2026-10-12):** initial draft including zone cues.
- **rev 2 (2026-10-12):** Codex round 1 returned NEEDS_CHANGES (20
  findings, 5 blocking); rev 2 added incarnations, three graphs per zone,
  per-source readiness, an activation origin, presentation receipts, go
  as one transaction, scoped fallback, a runtime snapshot, and a measured
  admission ledger.
- **rev 3 (2026-10-12):** Codex round 2 closed 9 of the 20, left 11
  partial, and raised 18 more (4 blocking). Most of what remained
  concerned cues and their admission, which cannot be designed honestly
  before hold-then-swap exists to measure, so rev 3 narrows the spec to
  truthful health and hold-then-swap and records the cue requirements in
  §10. In-scope dispositions: geometry holds the last frame instead of
  presenting empty output (r2 finding 1); `screen_cast` and the
  WebViewport layer get capture- and navigation-backed readiness (2);
  receipts carry the generation through replacement and report per-target
  delivery outcomes from backend acknowledgements (3); the outgoing graph
  lives until the activation transition completes, including interrupted
  ones (5); a preparation handshake with init and suspension
  acknowledgements, and honest behavior for pages without it (6, 7);
  deadlines pause during session sleep, with concrete first-output
  policies (9); latency is monotonic from request arrival (14);
  acceptance separates swap, acceptance, and completion per target and
  per target cadence, and tests only reachable states (15); `Loading`
  covers ready-but-held with readiness reported separately (16); phases
  no longer depend on later work (17); raw evidence is committed (18).
  Moved to §10: admission and memory attribution (4, 11, 12), cue
  bindings (10), tombstone retention (13), and staged-cue demand (8).
