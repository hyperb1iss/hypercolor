# 85 -- Zone Cues: Show-Grade Cuts

> Every effect change today replaces a zone's layer stack and lets the new
> producer warm up in public: HTML effects flash a placeholder, the layer
> reports Active before it shows a real pixel, and a cut cannot be timed
> to a beat. This spec makes every stack replacement hold the outgoing
> stack until the incoming one has real pixels, makes layer health tell
> the truth, and adds zone cues: effects prepared ahead of time that go
> live on a frame boundary within one frame of the request.

**Status:** DRAFT rev 1
**Crates:** `hypercolor-core` (effect pool, renderer readiness), `hypercolor-daemon` (render zones, scene domain, API), `hypercolor-types` (cue and health contracts)
**Amends:** Spec 78 §1.4, §2.3, Appendix A; LayerHealth semantics
**Author:** Nova
**Date:** 2026-10-12

---

## Table of Contents

0. [Goals, non-goals, doctrine](#0-goals-non-goals-doctrine)
1. [Evidence](#1-evidence)
2. [Hold-then-swap](#2-hold-then-swap)
3. [Truthful layer health](#3-truthful-layer-health)
4. [Zone cues](#4-zone-cues)
5. [Resources](#5-resources)
6. [Spec 78 amendments](#6-spec-78-amendments)
7. [Phases and acceptance](#7-phases-and-acceptance)
8. [Open questions](#8-open-questions)
9. [Review history](#9-review-history)

---

## 0. Goals, non-goals, doctrine

**Goals**

1. No stack replacement ever presents a placeholder or an empty frame
   while the incoming producers warm up. The outgoing stack keeps
   presenting, live, until every incoming producer has produced a real
   frame.
2. A cut can land within one frame of the request that triggers it. A
   client (a show runner, a playlist, a MIDI pad) prepares the effect
   early and asks for it to go live at the moment that matters.
3. `LayerHealth` means what it says. Active is a composited real frame,
   Failed is emitted when a load fails, and Stalled is emitted when a
   presenting producer stops producing.
4. One door. Apply is defined as an implicit cue plus an immediate go,
   playlists prepare their next item through the same mechanism, and no
   second staging path exists.

**Non-goals:** crossfade transitions (reserved in §7, Phase 3), Servo
worker sharding (Design 70), go-at-a-future-timestamp scheduling (§8),
cues for display faces (§8), and any persistence of cues.

**Doctrine:** Spec 78 lockstep and one name per concept. Performance
baselines are product contracts: nothing here lowers render cadence,
resolution, or tier to make a cut look better. Resource bounds derive
from measured capacity, never from a convenient constant.

## 1. Evidence

A scripted rehearsal of the demo film (`demo/show/runner.ts`, branch
`nova/demo-video`, 2026-10-12) against the live rig measured:

| Observation | Measured | Cause |
| --- | --- | --- |
| Cue to layer Active, warm effects | 63 to 108 ms | ~43 to 60 ms of synchronous scene commit, then admission on the next frame |
| What Active meant | admitted, not visible | any effect frame marks the layer Active (`render_zones/layer_rendering.rs:178-188`), including the Servo placeholder (`effect/servo/renderer.rs:137-151`, `:233-235`) |
| A failed page load | Active forever, placeholder on the LEDs | `load_failed` is recorded (`renderer/load.rs:219-229`) but never checked (`renderer.rs:304-315`) |
| `LayerHealth::Stalled` | never emitted | no emitter exists (`hypercolor-types/src/event.rs:183`) |
| Five faces assigned together | one running face hard-stalled 516 ms | one Servo worker blocks on every page load (`servo/worker.rs:519-594`, `:694-739`); queue wait counts as stall |
| Re-applying the effect a zone already runs | full Servo session rebuild | pool slots key on `(zone_id, layer_id)` (`effect/pool.rs:61-65`) and apply mints a new layer id |

The commit cost is being removed separately (branch
`nova/scene-commit-cost`). Even at zero commit cost, a cut to an HTML
effect cannot land on a beat today, because the new producer starts
loading only when the cut is requested.

## 2. Hold-then-swap

Phase 1. No API change; every stack replacement benefits, including
apply, layer replace, scene activation, and playlist advancement.

### 2.1 Renderer readiness

`EffectRenderer` gains:

```rust
/// Whether this renderer has produced a real frame yet.
fn readiness(&self) -> Readiness { Readiness::Ready }

pub enum Readiness {
    /// Loading; frames it returns are not real output.
    Warming,
    /// Has produced at least one real frame.
    Ready,
    /// Will never produce a real frame.
    Failed { reason: String },
}
```

The default is `Ready`, so CPU builtins (which initialize synchronously
and whose first frame is real) need no change. `ServoRenderer` reports
`Warming` until its first real frame, `Ready` after it, and `Failed`
when the load fails or exceeds the existing load deadline. The
frame-counter placeholder is deleted: a warming scene effect returns
`Pending`, as display-category effects already do
(`renderer.rs:228-231`, `:367-372`).

### 2.2 The swap

When a zone's committed stack changes, the render thread keeps the
retiring slots alive and keeps compositing the **retiring** stack, live,
while the incoming slots warm without being composited (the existing
`advance_output` path, `effect/traits.rs:233-238`). On the first frame
where every incoming producer is `Ready`, the zone presents the incoming
stack and the retiring slots are destroyed (detached, as today).

Rules:

- **Superseded while warming.** If another replacement arrives before
  the incoming stack is ready, the incoming slots are discarded without
  ever presenting, and the retiring stack keeps presenting until the
  newest stack is ready. A rapid series of applies never shows the
  intermediate effects.
- **Failure.** If any incoming producer reports `Failed`, the zone swaps
  on that frame: failed layers composite as transparent, healthy layers
  in the same stack present normally, and §3 events fire. The zone never
  holds a stale stack indefinitely to hide a failure, and never shows a
  placeholder.
- **Builtins** are `Ready` on their first frame, so a builtin-to-builtin
  replacement swaps on the next frame exactly as today.
- **Effect clock.** An incoming producer's elapsed time restarts at the
  swap frame, so an effect starts its animation when it becomes visible,
  not when it began loading.
- **Display zones.** Direct display zones already hold their previous
  frame while a display effect is `Pending`
  (`render_zones/render_pass.rs:255-268`, `display_retention.rs:56-76`),
  but the held frame is frozen. Under this rule the retiring face keeps
  rendering live until the incoming face is ready.

### 2.3 Prerequisite

While a zone holds a live retiring HTML layer, its renders queue on the
same Servo worker that is loading the incoming page. Today that worker
blocks inside page loads (`worker/scheduler.rs:104-110`, `:153-157`),
so the retiring layer would trip the 500 ms hard-stall check
(`renderer/frame_poll.rs:96-110`), and that error fails the whole scene
pass (`render_pass.rs:132-140`). Phase 1 therefore requires that the
worker serve renders while page loads are in progress (tracked as "Servo
worker blocks on page loads, so concurrent loads and renders queue
serially") and that stall time be measured from dequeue, not submission
(branch `nova/servo-cut-papercuts`).

## 3. Truthful layer health

`LayerHealth` keeps its variants and changes meaning:

| State | Emitted when |
| --- | --- |
| `Loading` | the layer is committed and its producer is warming; the zone is still presenting the previous stack |
| `Active` | the zone composites this layer's first real frame (the swap frame) |
| `Stalled` | a presenting producer has produced no new frame for four of its own frame intervals; cleared by the next frame |
| `Failed { reason }` | the producer reports `Failed`; an `EffectError` with the same reason is published once |
| `AssetMissing` | unchanged |

`Active` and `Failed` are emitted from the frame that decides them, so a
client measuring cut latency from `LayerHealthChanged { Active }`
measures what an audience sees. Health events stay coalesced per layer
(`layer_runtime.rs:79-122`).

## 4. Zone cues

Phase 2.

### 4.1 Model

A **cue** is a runtime-only staged effect for one zone of the active
scene:

```rust
pub struct Cue {
    /// Reserved at creation; becomes the layer id when the cue goes live.
    pub id: SceneLayerId,
    pub zone_id: ZoneId,
    pub effect_id: EffectId,
    pub preset_id: Option<PresetId>,
    pub controls: BTreeMap<String, ControlValue>,
    pub state: CueState,
    pub created_at: EventTimestamp,
    pub expires_at: EventTimestamp,
}

pub enum CueState { Warming, Ready, Live, Failed { reason: String }, Discarded { reason: DiscardReason } }
```

Cues live on a **cue board** owned by the scene domain service, beside
the scene plan, and reach the render thread through the plan reader (the
board's generation joins the dependency key,
`render_thread/scene_snapshot.rs:355-383`). The render thread builds a
staged slot for each warming cue. Cues are never stored in
`SceneManager.scenes`, never written to `scenes.json` or the runtime
session, and never appear in the `/scene` document, which stays the
authored state.

### 4.2 Lifecycle

```
           create
             │
             ▼
   ┌──── Warming ────┐
   │        │        │
   │  first real     │ load fails
   │     frame       │
   │        ▼        ▼
   │      Ready    Failed
   │        │
   │       go
   │        ▼
   │      Live  (the cue id is now a layer id)
   │
   └── delete, TTL, scene activation, zone removal ──► Discarded
```

- **Warming to Ready** is decided by the render thread from renderer
  readiness (§2.1).
- **A ready cue holds still.** After its first real frame it stops
  rendering and keeps that frame, so idle cues cost memory, not render
  time or Servo worker time. Its clock restarts at go.
- **Go** commits a `SceneMutation` replacing the zone's stack with one
  layer whose id is the cue id and whose source is the cue's effect,
  preset, and controls: the same mutation apply performs
  (`replace_effect_layer_stack`). The render thread adopts the cue's
  staged slot for that layer id instead of building a new one, so a
  ready cue swaps on the next frame boundary. A cue that is still
  warming at go follows §2.2: the zone keeps presenting until it is
  ready.
- **Rebuild triggers** that would rebuild a live slot (registry change
  for that effect, canvas resize, descriptor change,
  `effect/pool.rs:516-531`) return a ready cue to Warming rather than
  discarding it.
- **Discard** happens on delete, TTL expiry, activation of any scene,
  or removal of the cue's zone. Discarded cue ids are never reused.

### 4.3 Routes

All under the live tree, per Spec 78 §1.5:

| Route | Body | Response |
| --- | --- | --- |
| `POST /scene/zones/{zone}/cues` | `{ effect_id, preset_id?, controls?, ttl_ms? }` | `202` cue resource |
| `GET /scene/zones/{zone}/cues` | | `ListResponse<Cue>` |
| `GET /scene/zones/{zone}/cues/{cue}` | | cue resource |
| `DELETE /scene/zones/{zone}/cues/{cue}` | | `200` discarded cue resource |
| `POST /scene/zones/{zone}/cues/{cue}/go` | `{ transition? }` | `200` zone resource |

- Create validates the effect, zone, and controls exactly as apply does
  and returns without waiting for readiness.
- Go is a structural mutation: it honors `If-Match` against the scene
  revision (Spec 78 §1.6) and returns the canonical zone resource
  carrying the layer, whose id equals the cue id.
- Go is idempotent per cue: repeating it after success returns `200`
  with the current zone when the zone's stack is still exactly that
  layer, and `409 cue_consumed` otherwise.
- Errors: `404 cue_not_found`, `409 cue_failed` (go on a failed cue,
  with the reason), `409 cue_budget_exhausted` (§5), `409
  snapshot_locked` on create and go for snapshot-locked scenes,
  `412` on an `If-Match` mismatch.
- `ttl_ms` defaults to 60 000 and is capped at 600 000.
- Every route works over the WebSocket `command` message without extra
  code (`api/ws/command.rs:23-99`).

### 4.4 Events

`CueStateChanged { scene_id, zone_id, cue_id, state, reason }` on the
broadcast bus, once per transition. Go also emits the ordinary scene
events for the stack replacement, and `LayerHealthChanged { Active }`
fires on the swap frame per §3.

### 4.5 Apply and playlists

`POST /effects/{id}/apply` is defined as creating an implicit cue in the
target zone and going live immediately. Its request and response do not
change; the observable differences are §2 and §3. Implicit cues never
appear in cue listings.

The playlist runtime creates a cue for item N+1 one lead time before
item N ends and goes live at item N's deadline, scheduled against a
monotonic clock rather than by sleeping after each apply
(`api/library/playlists.rs:355-437` drifts today).

## 5. Resources

A cue for an HTML effect is a full Servo session with its own rendering
context and exportable GPU images (`hypercolor-linux-gpu-interop` session
allocation, `servo/worker.rs:538-547`). The bound is capacity, not a
count:

- Cue creation consults the GPU memory budget the producers already use
  (`render_thread/gpu_device.rs:5-6`, 80% soft and 95% hard thresholds).
  If admitting the cue's estimated allocation would cross the soft
  threshold, creation returns `409 cue_budget_exhausted` with the
  current usage and the estimate in `details`.
- Nothing is evicted silently. A client frees capacity by deleting cues
  or letting them expire.
- The estimate per producer comes from the producer's declared surface
  sizes, measured once per effect kind and cached, and is reported in
  the cue resource so clients can plan.

## 6. Spec 78 amendments

- **§1.4 Layer identity.** A cued layer's id is minted when the cue is
  created and becomes a layer id only at go. Before go it names a cue,
  never a layer; a discarded cue's id is never reused.
- **§2.3 Apply.** Apply is an implicit cue plus an immediate go. Request
  and response are unchanged.
- **LayerHealth.** Semantics per §3 of this spec.
- **Appendix A.** Adds the five cue operations in §4.3.
- **§6 MCP.** Adds `cue_effect` and `go_cue` tools mirroring §4.3.

## 7. Phases and acceptance

**Phase 0 (prerequisites, in flight).** Per-commit durable writes cut
and a per-session WebSocket command worker (`nova/scene-commit-cost`);
Servo stall measured from dequeue (`nova/servo-cut-papercuts`); Servo
worker serving renders during page loads (§2.3).

**Phase 1: hold-then-swap and truthful health.** §2 and §3, plus the
failed-load fix. Acceptance:

- Daemon tests drive a fake renderer through Warming, Ready, and Failed
  and assert, frame by frame, that the zone composites the retiring
  stack until the swap frame, never a placeholder, and that `Active`
  fires on the swap frame.
- A superseded incoming stack never presents.
- A failed load emits `Failed` and one `EffectError`, and the zone
  composites the failed layer as transparent.
- The demo rehearsal (`just show play demo/shows/rehearsal.ts --shot
  S02,S04`) reports cut-to-active as the visible latency, with no frame
  over budget on the swap frame.

**Phase 2: zone cues.** §4 and §5. Acceptance:

- Lifecycle, TTL, budget, idempotent go, snapshot lock, and rebuild
  transitions covered by daemon tests.
- `scenes.json` and the runtime session are byte-identical before and
  after creating and deleting cues.
- With cues prepared one beat ahead, the rehearsal's cut-per-beat shot
  at 124 BPM lands every cut within one frame of its go
  acknowledgement, at the configured FPS, with no tier change.

**Phase 3: crossfade (reserved).** `TransitionType` gains
`crossfade { duration_ms, easing }` once the engine can compose the
retiring and incoming stacks separately and blend them with an eased
alpha in Oklab, reusing the scene transition clock
(`scene_state.rs:17-53`) and two-layer alpha plans
(`composition_planner.rs:149-249`). An optional beat quantization may
use `AudioData.beat_detected`. Until then the request field accepts only
`cut`.

## 8. Open questions

1. **Go at a timestamp.** Network jitter on a LAN is small next to a
   frame, so go-on-receipt is the first rev. A `go_at` against the
   daemon's monotonic clock would make cuts frame-exact for remote
   clients; it needs a clock-sync handshake first.
2. **Live cue previews.** A ready cue holds one frame. A VJ-style UI may
   want cues animating in thumbnails; that would be a preview cadence on
   the preview path, never the LED path.
3. **Display face cues.** Faces gain hold-then-swap in Phase 1. Cueing a
   face (preparing a face and going live on a beat) fits the same model
   but needs a display-domain route under `/displays/{id}/face`.
4. **Zones in other scenes.** Cues belong to the active scene and are
   discarded on activation. A show that cues across a scene change would
   need scene-level cues; nothing in the film needs it yet.

## 9. Review history

- **rev 1 (2026-10-12):** initial draft from the demo-film rehearsal and
  a read-only architecture map of the render-thread layer lifecycle.
