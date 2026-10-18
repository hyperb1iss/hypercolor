# 86 -- GPU-Resident Multi-Zone Scenes

> A scene with more than one LED zone leaves the GPU today. Unless every
> zone's layout samples Nearest with Clamp, each zone asks SparkleFlinger for
> a CPU canvas, GPU producer frames (Servo, media) reach the CPU only through
> one compositor-wide readback latch, and on a rig with display faces that
> latch never returns a frame. Every LED goes black and the render loop falls
> to its 10 fps floor. This spec finishes what Spec 64 §12.5 deferred: each
> zone composes into its own retained GPU texture, LEDs are sampled per zone
> on the GPU from that texture with that zone's own plan, and the scene canvas
> becomes a GPU projection built only for preview and display backdrops.

**Status:** rev 1, draft for review.
**Crates:** `hypercolor-daemon` (render zones, frame sampling, SparkleFlinger GPU compositor and sampler), `hypercolor-core` (spatial plan identity)
**Amends:** Spec 64 §12.1 and §12.5 (per-group sampling moves to the GPU), Spec 48 §4.2 (multi-group preview derivation)
**Author:** Nova
**Date:** 2026-10-18

---

## Table of Contents

0. [Goals, non-goals, doctrine](#0-goals-non-goals-doctrine)
1. [Evidence](#1-evidence)
2. [Per-zone retained textures](#2-per-zone-retained-textures)
3. [Per-zone GPU LED sampling](#3-per-zone-gpu-led-sampling)
4. [Transitions, retention, and unassigned outputs](#4-transitions-retention-and-unassigned-outputs)
5. [The scene canvas as a preview projection](#5-the-scene-canvas-as-a-preview-projection)
6. [Unsupported plans and the CPU path](#6-unsupported-plans-and-the-cpu-path)
7. [Telemetry](#7-telemetry)
8. [Phases and acceptance](#8-phases-and-acceptance)
9. [Open questions](#9-open-questions)

---

## 0. Goals, non-goals, doctrine

### Goals

- A scene with any number of LED zones, each running any mix of HTML, media,
  and native layers, renders at the same tier a single-zone scene does.
- Every zone's LEDs are sampled from that zone's own composed pixels with
  that zone's own sampling plan, so overlapping zones never read each other
  (Spec 64 §2.3 stays fixed on the GPU path, where it is broken today).
- LED output reads back only sampled LED colors, never a whole image (Spec 48
  §4.1 hard residency rules).
- Display faces, the UI scene preview, and `GET /api/v1/canvas/frame` keep
  receiving a composed scene at their current cadences.

### Non-goals

- Per-zone frame rates. Zones share the render-loop tier (Spec 64 §12.5).
- A GPU Gaussian-area sampler. §6 defines explicit behavior until one exists.
- Changing zone authoring, the REST surface, or the scene document format.

### Doctrine

1. **The zone texture is the source of truth for that zone's LEDs.** The
   scene canvas is derived from zone textures for people to look at. It never
   feeds LED output.
2. **No CPU materialization of GPU producer frames.** A GPU frame that cannot
   be sampled on the GPU reuses retained output or produces an explicit,
   reported fallback. It is never read back as a whole image and it is never
   silently cleared to black.
3. **Performance baselines are product contracts.** Nothing here lowers a
   tier, cadence, resolution, or preview rate to make the multi-zone path fit.

---

## 1. Evidence

Reproduced live on 2026-10-18 on the owner's rig (daemon built from OSS
`b02ec3db7`): an O11D layout with a 640x630 canvas and bilinear sampling, a
Default zone of 61 outputs running Bubble Garden (HTML), and five display-face
zones (Push 2, iCUE LINK LCD, three Lian Li TL LCDs). Adding one custom zone
through `POST /api/v1/scene/zones` with either Touch Ripple alone or Bubble
Garden plus Touch Ripple in `screen` blend turned every LED black, made
`GET /api/v1/canvas/frame` return an all-black image, and dropped the render
loop to `tier=10fps (minimal)`. Deleting the zone restored Bubble Garden and
the loop climbed back to 60 fps. The Studio zone-layout preview bug from
2026-09-06 (a single zone whose canvas differed from the scene) is the same
failure reached another way.

### 1.1 Why it goes black

1. `render_zones/scene_assembly.rs` takes the GPU projected path only when
   every zone passes `projection_supports_composition`
   (`render_zones/projection.rs:141`), which requires an affine output with
   `SamplingMode::Nearest` and `EdgeBehavior::Clamp`. Bilinear layouts fail
   it, and `reconcile.rs:357` then also withholds the per-zone GPU textures.
2. The fallback composes each zone through `SparkleFlinger::compose_for_outputs`
   with `requires_cpu_sampling_canvas = true` (`render_pass.rs:136-149`).
   For keyless plans (GPU producer frames) the only CPU canvas source is
   `latch_sampling_surface_readback`
   (`sparkleflinger/gpu/compositor/sampling_state.rs:197-232`): one latch per
   compositor, keyed only by width and height, at most one readback in
   flight, returning whatever the previous compose staged.
3. Display-face zones compose through the same compositor at their own sizes
   (`render_pass.rs:78-86`), and every surface size change clears the latch
   (`gpu/compositor/bypass.rs:57-63`). The latch is therefore empty on every
   scene-zone compose, `composed_frame_to_producer_frame` falls back to a
   `GpuTexture`, `copy_producer_frame_to_canvas` refuses it
   (`render_zones/frame_helpers.rs:322-333`), and the zone target is cleared
   (`render_pass.rs:198-205`). `gpu_readback_failed` stays false, so nothing
   reports the loss.
4. With equal sizes and no display zones the latch does return frames, but
   zones receive each other's pixels.
5. The per-frame cost of this path (a whole-canvas readback, a surface copy,
   and a canvas clone per zone, then a CPU projection raster and a re-upload)
   plus buffer reallocation on every size swap drives the tier down.

### 1.2 What the GPU projected path gets wrong today

When the Nearest gate does pass, LEDs are sampled from the composited scene
through `combined_led_spatial_engine` (`scene_assembly.rs:85-106`). That
reintroduces the Spec 64 §2.3 overlap hazard: where two zones' outputs overlap
in scene space, the later zone's pixels win for both. It also resolves
outputs that carry no sampling mode against the combined layout's Bilinear
default instead of their own zone's default, and LED-space scene transitions
are skipped because the strategy is not `PreSampled`
(`frame_sampling.rs:442-445`).

### 1.3 What already exists

- Each zone's composed output can already live in its own persistent texture:
  `GpuProjectionSnapshot` per zone (`sparkleflinger/gpu/projected.rs:108-155`),
  refreshed every frame by `stabilize_projected_zone_frame`
  (`gpu/snapshot.rs:40-89`), which bumps a `content_generation`.
- The GPU sampler already concatenates several zones' points with
  `GpuZoneRange` segments and dispatches them as one 64-wide pass
  (`sparkleflinger/gpu_sampling.rs:407-472`), supports Nearest, Bilinear, and
  AreaAverage plus every edge behavior, and reads back 4 bytes per LED.
- `sample_texture_into` accepts any texture view (`gpu_sampling.rs:873`).

Three things stop it from sampling K zone textures per frame: bind groups are
cached by `GpuSampleSource { Front, Back, Diagnostic }` rather than by texture
(`gpu_sampling.rs:1126-1141`, `gpu_area_sat.rs:318-328`), so a new view under
an old key samples the stale texture; one params uniform serves every pass
(`gpu_sampling.rs:475-497`); and the reconcile gate in item 1 above.

---

## 2. Per-zone retained textures

Whenever SparkleFlinger supports GPU output frames, reconcile emits a
`ProjectedZoneTextureRequirement` for every scene-contributing zone, sized to
that zone's own `layout.canvas_width` and `layout.canvas_height`. Projection
eligibility no longer gates the textures; it only gated a composition
shortcut that §5 replaces.

Each frame, a zone's layer stack composes as it does today, and its output is
stabilized into the zone's snapshot. The snapshot's identity for sampling is
`(storage_id, content_generation)`. A zone whose layers produced nothing this
frame keeps its previous snapshot and generation, which is exactly the
retained-frame semantics the sampler's freshness key relies on (§3.3).

Reads of a snapshot within a frame need no extra fencing: the sampling and
projection passes are queued before the next frame's stabilize copy, and the
wgpu queue executes submissions in order. The one real hazard is a consumer
that holds a zone snapshot across frames and expects its content to stay
put. Nothing in this design does that: retained multi-zone results never
re-sample snapshots (§4.2). The snapshot lease (`gpu/snapshot.rs:40-77`),
which is not enforced today, gains a debug assertion so a future consumer
that retains a zone snapshot fails loudly in tests.

---

## 3. Per-zone GPU LED sampling

### 3.1 Strategy

`LedSamplingStrategy` gains one variant:

```rust
SparkleFlingerZones(Arc<ZoneSamplingSet>)

struct ZoneSamplingSet {
    // Scene-contributing LED zones in scene zone order.
    zones: Vec<ZoneSamplingSource>,
    // Union layout, used only to shape and route ZoneColors.
    combined_layout: Arc<SpatialLayout>,
}

struct ZoneSamplingSource {
    zone_id: ZoneId,
    engine: SpatialEngine,      // the zone's own engine and prepared plan
    texture: GpuTextureFrame,   // the zone's snapshot for this frame
    content_key: (u64, u64),    // (storage_id, content_generation)
}
```

The multi-zone branch of `render_scene` returns this variant whenever the
backend supports GPU output frames and every zone's plan is GPU-admissible
(§6 covers the rest). The single-zone fast path keeps
`SparkleFlinger(SpatialEngine)` unchanged. `combined_led_spatial_engine`
leaves the LED path entirely, as Spec 64 §12.1 always intended.

### 3.2 Dispatch

One encoder, one submit, one readback per frame:

- The points buffer holds every zone's points, concatenated in zone order.
  The resulting `ZoneColors` order equals the CPU per-zone path
  (`render_zones/scene_output.rs:28-46`), so device routing is unchanged.
- One compute pass per zone texture. Each pass binds its zone's texture and
  reads its parameters through a dynamic offset into one params buffer, with
  the existing `_pad` field carrying the pass's `point_offset`
  (`sample.wgsl:33-38`). Passes write disjoint ranges of one output buffer.
- Area-average zones need a summed-area table per sampled texture. One SAT
  buffer is kept per distinct texture size and rebuilt per pass that needs it.
- One `copy_buffer_to_buffer` of the whole output and one `map_async`, through
  the existing three-slot readback ring and pending/stale/saturated machinery.

### 3.3 Caching and freshness

- Bind groups are keyed by `(texture storage_id, SAT buffer id)`, not by
  `GpuSampleSource`. A surface size change on the main compositor no longer
  clears them (`gpu/compositor/bypass.rs:77`); entries retire when their
  snapshot is released.
- The cached-result key becomes `(zone content digest, plan key)`, where the
  digest hashes every zone's `(storage_id, content_generation)` in order.
  This replaces `output_generation` for this strategy. An unchanged digest is
  a cache hit, which is how retained effects keep costing nothing.
- `GpuSamplingPlanKey` adds a process-unique engine id. Today it is
  `(pointer, length, generation)` with every fresh engine starting at
  generation 1 (`hypercolor-core/src/spatial/mod.rs:219`), so a freed plan
  whose allocation is reused can alias a new one.

### 3.4 Consumption

`resolve_led_sampling` treats `SparkleFlingerZones` like `SparkleFlinger`
for deferral, stale retry, saturation, held-frame reuse, and zone-shape
signatures, with the zone-shape signature computed over the concatenated
plans. It does not force full composition of the primary plan
(`frame_sampling.rs:43-45`): LED output no longer depends on the composed
scene, so the scene composes only when a consumer in §5 asks for it.

---

## 4. Transitions, retention, and unassigned outputs

### 4.1 Scene transitions

Zone textures are pre-transition. While a scene transition is active, the
sampled `ZoneColors` feed the existing LED-space blend
(`frame_sampling.rs:787-833`), which today runs only for `PreSampled`. The
check at `frame_sampling.rs:442-445` extends to `SparkleFlingerZones`. The
preview's canvas transition base is unchanged.

### 4.2 Retained scenes

`reuse_last_good_scene` (`scene_assembly.rs:74-84`) retains the zone sampling
set as well as the scene frame. After a render failure some snapshots may
already hold newer content than the retained LED colors, so a retained
multi-zone result resumes as `ReusePublished` rather than re-sampling
snapshots that no longer match the retained preview.

### 4.3 Unassigned fallback

`UnassignedBehavior::Fallback(zone)` today samples unassigned outputs from
`render_stage.zone_canvases`, which are CPU canvases
(`frame_executor.rs:594-599`). With GPU zones, the planner appends the
synthesized fallback outputs to the target zone's GPU plan as an extra
`GpuZoneRange` against that zone's texture. `zone_canvases` stays populated
only for CPU-backed zones.

---

## 5. The scene canvas as a preview projection

The scene canvas is composed on the GPU from zone snapshots with the existing
projection layers (`projection.rs:150-180`, one layer per output over the
opaque black base) whenever a consumer requests it: a preview receiver, a
display face that blends the scene, or a canvas capture. It never feeds LED
output, so it no longer needs the Nearest gate.

- Admission drops the sampling-mode and edge-behavior requirements and keeps
  the affine requirement.
- Overlapping outputs keep zone-order overwrite. Spec 64 already calls this a
  harmless detail of a preview image.
- The projection shader's target-space fetch rounds to the nearest texel. For
  zones whose canvas matches the scene canvas, which is every zone created
  without a layout override (`hypercolor-core/src/scene/mod.rs:813-819`),
  that is an exact copy of the zone's pixels. For a zone with a differently
  sized override canvas it is a nearest resample. Spec 48 §4.2 asks that the
  preview follow LED sampling semantics; Phase 3 adds linear-light bilinear
  and area filtering to the target-space path so the preview matches the CPU
  raster within tolerance. Until then the diagnostic snapshot reports
  `preview_projection_filter = nearest`.

---

## 6. Unsupported plans and the CPU path

- **CPU compositor backend.** Unchanged. Frames are CPU canvases, and the
  Spec 64 §12.1 CPU per-zone path stays the implementation.
- **GPU backend, plan not GPU-admissible** (a Gaussian-area zone, or a plan
  past device limits). Zones whose admitted plans pass are sampled on the
  GPU. A zone whose plan is refused and whose frame is GPU-backed reuses its
  retained LED colors when they exist and otherwise outputs black, with
  `LayerHealth::Failed { reason: "gpu_sampling_unsupported:<mode>" }` on its
  layers and a `multi_zone_sampling` diagnose warning naming the zone. This
  follows Spec 48 §4.3 and is never silent.
- **The keyless readback latch is removed from the scene-zone path.**
  `requires_cpu_sampling_canvas` is no longer requested for scene zones on
  the GPU backend. The latch remains for its single-zone callers until Spec
  56's stage collapse retires it.
- **Mixed producers.** A zone whose layers are all CPU-backed still composes
  on the GPU (upload, then the same snapshot), so a zone set is never split
  between CPU and GPU sampling on the GPU backend.

---

## 7. Telemetry

- `gpu_zone_sampling` reports true for `SparkleFlingerZones` frames.
- New frame metrics: `gpu_sampled_zone_count` and `gpu_sampling_passes`.
- `producer_full_frame_copy` must not record
  `surface_to_zone_canvas_materialization` for GPU-backed zones. A test
  asserts it.
- The diagnose `render` category gains `multi_zone_sampling` (pass when every
  scene zone samples on the GPU or is CPU-backed by design, warn when §6
  fallback is active).

---

## 8. Phases and acceptance

### Phase 1: LED output (this is what unblocks the rig)

§2, §3, §4.1, §6, §7, and the §5 admission change, with the existing
nearest target-space fetch.

Acceptance:

1. On the evidence rig, the Default zone running Bubble Garden plus a ROLI
   zone running Bubble Garden with Touch Ripple in `screen` holds the 60 fps
   tier with all five display faces active. `frame_liveness` and
   `led_freshness` pass, `gpu_zone_sampling` is true, the ROLI Lightpad and
   LUMI Keys show ripples over bubbles, and every other device shows Bubble
   Garden.
2. GPU tests: two and three zones with distinct solid fills and overlapping
   output rectangles produce each zone's own color on its own LEDs; zones
   with different canvas sizes; a display-face zone composing at a third size
   in the same frame; a zone whose effect stops producing keeps its last
   snapshot and hits the sampling cache. The ignored reproduction test from
   the investigation is un-ignored and passes.
3. Parity: for Nearest, Bilinear, and AreaAverage plans, per-zone GPU colors
   match the CPU per-zone path within the existing sampler parity tolerance.
4. No whole-image readback: the scene-zone path never calls
   `latch_sampling_surface_readback`, and §7's materialization assertion
   holds.
5. Cost: on the evidence rig, `push_us` and `producer_scene_compose_us` for
   the two-zone scene stay within 25 percent of the single-zone scene, and
   snapshot and bind-group allocation counters are flat after warmup.

### Phase 2: retention and unassigned outputs

§4.2 and §4.3, with tests for render-failure retention and `Fallback(zone)`
on GPU zones.

### Phase 3: preview fidelity

Linear-light bilinear and area filtering in the target-space projection path
(§5), and a GPU Gaussian-area sampler that retires the §6 fallback.

---

## 9. Open questions

1. **One pass per zone or one pass for all zones.** One pass with a texture
   binding array would avoid K pass setups but needs
   `TEXTURE_BINDING_ARRAY`, which is not universal. K passes in one encoder is
   the portable baseline; measure before reaching for arrays.
2. **Snapshot copy cost.** Stabilizing copies each zone's compositor output
   into its snapshot, about 1.6 MB per 640x630 zone per frame on the GPU.
   Composing straight into the zone's snapshot would remove the copy. Is the
   compositor's front/back surface model the right place to change, or does
   Spec 56's stage work own it?
3. **Display faces sharing the main compositor.** Display zones compose
   through the main SparkleFlinger at their own sizes, which churns surface
   size state every frame. Size-keyed surface sets would remove the churn for
   every multi-zone and display-face scene. In scope here or Spec 56?
