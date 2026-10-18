# 86 -- GPU-Resident Multi-Zone Scenes

> A scene with more than one LED zone leaves the GPU today. Unless every
> zone's layout samples Nearest with Clamp, each zone asks SparkleFlinger for
> a CPU canvas, GPU producer frames (Servo, media) reach the CPU only through
> one compositor-wide readback latch, and depending on frame timing a zone
> gets nothing (black) or another zone's pixels. On a rig with display faces
> the result is every LED black and the render loop at its 10 fps floor. This
> spec finishes what Spec 64 §12.5 deferred: each zone composes into its own
> retained GPU texture, LEDs are sampled per zone on the GPU from that
> texture with that zone's own plan, and the scene canvas and zone previews
> become GPU outputs built for the people looking at them.

**Status:** rev 2, revised after cross-model review (§10).
**Crates:** `hypercolor-daemon` (render zones, frame sampling, frame publication, SparkleFlinger GPU compositor and sampler), `hypercolor-core` (spatial plan identity)
**Amends:** Spec 64 §12.1 and §12.5 (per-group sampling moves to the GPU), Spec 48 §4.2 (multi-group preview derivation)
**Author:** Nova
**Date:** 2026-10-18

---

## Table of Contents

0. [Goals, non-goals, doctrine](#0-goals-non-goals-doctrine)
1. [Evidence](#1-evidence)
2. [Per-zone retained textures](#2-per-zone-retained-textures)
3. [Per-zone LED sampling](#3-per-zone-led-sampling)
4. [Transitions, retention, and unassigned outputs](#4-transitions-retention-and-unassigned-outputs)
5. [Scene canvas and zone previews](#5-scene-canvas-and-zone-previews)
6. [Unsupported plans and the CPU path](#6-unsupported-plans-and-the-cpu-path)
7. [Telemetry](#7-telemetry)
8. [Phases and acceptance](#8-phases-and-acceptance)
9. [Open questions](#9-open-questions)
10. [Review history](#10-review-history)

---

## 0. Goals, non-goals, doctrine

### Goals

- A scene with any number of LED zones, each running any mix of HTML, media,
  screen, web viewport, and native layers, renders at the same tier a
  single-zone scene does.
- Every zone's LEDs are sampled from that zone's own composed pixels with
  that zone's own sampling plan, so overlapping zones never read each other
  (Spec 64 §2.3 stays fixed on the GPU, where it is broken today).
- LED output reads back only sampled LED colors, never a whole image (Spec 48
  §4.1 hard residency rules).
- Display faces, the UI scene preview, Studio's per-zone previews, and
  `GET /api/v1/canvas/frame` keep receiving frames at their current cadences.

### Non-goals

- Per-zone frame rates. Zones share the render-loop tier (Spec 64 §12.5).
- Changing zone authoring, the REST surface, or the scene document format.

### Doctrine

1. **The zone texture is the source of truth for that zone's LEDs.** The
   scene canvas is derived from zone textures for people to look at. It never
   feeds LED output.
2. **No CPU materialization of GPU producer frames.** A GPU frame that cannot
   be sampled on the GPU reuses retained output or produces an explicit,
   reported fallback. It is never read back as a whole image and it is never
   silently cleared to black. Preview transports read back only their
   transport-sized payload, as the scene preview does today.
3. **CPU-backed work that works today keeps working.** A zone whose frames
   are already CPU canvases may still compose and sample on the CPU when the
   GPU cannot take its plan.
4. **Performance baselines are product contracts.** Nothing here lowers a
   tier, cadence, resolution, or preview rate to make the multi-zone path fit.

---

## 1. Evidence

Reproduced live on 2026-10-18 on the owner's rig (daemon built from OSS
`b02ec3db7`): an O11D layout with a 640x630 canvas and bilinear sampling, a
Default zone of 61 outputs running Bubble Garden (HTML), and five display-face
zones (Push 2, iCUE LINK LCD, three Lian Li TL LCDs). Adding one custom zone
through `POST /api/v1/scene/zones`, with either Touch Ripple alone or Bubble
Garden plus Touch Ripple in `screen` blend, turned every LED black, made
`GET /api/v1/canvas/frame` return an all-black image, and dropped the render
loop to `tier=10fps (minimal)`. Deleting the zone restored Bubble Garden and
the loop climbed back to 60 fps. The Studio zone-layout preview bug from
2026-09-06 (one zone whose canvas differed from the scene) is the same
failure reached another way.

### 1.1 Why it goes black

1. `render_zones/scene_assembly.rs` takes the GPU projected path only when
   every zone passes `projection_supports_composition`
   (`render_zones/projection.rs:141`), which requires an affine output with
   `SamplingMode::Nearest` and `EdgeBehavior::Clamp`. Bilinear layouts fail
   it, and `reconcile.rs:357` then also withholds the per-zone GPU textures.
   The rule exists for CPU and GPU parity: the GPU target-space fetch rounds
   to the nearest texel (`blend.wgsl`), while the CPU raster applies each
   output's sampling mode (`projection.rs:353-358`).
2. The fallback composes each zone through `SparkleFlinger::compose_for_outputs`
   with `requires_cpu_sampling_canvas = true` (`render_pass.rs:136-149`).
   For keyless plans (GPU producer frames) the only CPU canvas source is
   `latch_sampling_surface_readback`
   (`sparkleflinger/gpu/compositor/sampling_state.rs:197-232`): one latch per
   compositor, keyed only by width and height, at most one readback in
   flight, returning whatever an earlier compose staged.
3. Outcomes depend on timing. Scene zones compose back to back, so a second
   equal-size zone can resolve the first zone's readback and receive its
   pixels. Any surface size change on the main compositor clears the latch
   (`gpu/compositor/bypass.rs:57-63`); display zones that compose through it
   at their own sizes (`render_pass.rs:78-86`) do that, though display
   passthrough and retained-frame paths do not. When the latch is empty,
   `composed_frame_to_producer_frame` falls back to a `GpuTexture`,
   `copy_producer_frame_to_canvas` refuses it
   (`render_zones/frame_helpers.rs:322-333`), and the zone target is cleared
   (`render_pass.rs:198-205`). `gpu_readback_failed` stays false, so nothing
   reports the loss.
4. Every materialized zone costs a whole-canvas readback, a surface copy, and
   a canvas clone, then the scene costs a CPU projection raster and a
   re-upload; size swaps also reallocate readback buffers. Live FPS
   attribution was not measured, but this is the only path that changed when
   the tier fell.

### 1.2 What the GPU projected path gets wrong today

When the Nearest gate passes, LEDs are sampled from the composited scene
through `combined_led_spatial_engine` (`scene_assembly.rs:85-106`). That
reintroduces the Spec 64 §2.3 overlap hazard (where two zones' outputs
overlap in scene space, the later zone's pixels win for both). It resolves
outputs that carry no sampling mode against the combined layout's Bilinear
default instead of their own zone's default (`sampler/mod.rs:127`). And
LED-space scene transitions are skipped because the strategy is not
`PreSampled` (`frame_sampling.rs:442-445`).

### 1.3 What already exists

- Each zone's composed output can already live in its own persistent texture:
  `GpuProjectionSnapshot` per zone (`sparkleflinger/gpu/projected.rs:108-155`),
  refreshed by `stabilize_projected_zone_frame` (`gpu/snapshot.rs:40-89`),
  which bumps a `content_generation` on every call.
- The GPU sampler already concatenates several zones' points with
  `GpuZoneRange` segments (`sparkleflinger/gpu_sampling.rs:407-472`),
  supports Nearest, Bilinear, and AreaAverage plus every edge behavior, and
  reads back 4 bytes per LED. `sample_texture_into` accepts any texture view
  (`gpu_sampling.rs:873`).
- The compositor already keeps surface sets per size
  (`gpu/compositor/bypass.rs:49`), but switching size clears output,
  preview, composition, and sample state (`bypass.rs:61`).

Three things stop the sampler from reading K zone textures per frame: bind
groups are cached by `GpuSampleSource { Front, Back, Diagnostic }` rather
than by texture (`gpu_sampling.rs:1126-1141`, `gpu_area_sat.rs:318-328`), so
a new view under an old key samples the stale texture; one params uniform
serves every pass (`gpu_sampling.rs:475-497`); and the reconcile gate in
§1.1 item 1.

---

## 2. Per-zone retained textures

Whenever SparkleFlinger supports GPU output frames, reconcile emits a
`ProjectedZoneTextureRequirement` for every scene-contributing zone that
composes on the GPU (§6 names the CPU exception), sized to that zone's own
`layout.canvas_width` and `layout.canvas_height`. Projection eligibility no
longer gates the textures.

### 2.1 Composition key

Each zone carries a composition key: the ordered list of its enabled layers'
`(layer id, producer frame identity, blend, opacity, transform)`, where
producer frame identity is whatever already distinguishes a new producer
frame from a retained one (texture storage id plus producer frame number for
GPU frames, the CPU replay key for canvases). When the key matches the
previous frame's, the zone skips composition and stabilization, and its
snapshot keeps its `content_generation`. A retained Servo frame
(`hypercolor-core/src/effect/servo/renderer.rs:218`) therefore costs nothing
downstream, the way it already costs nothing on the single-zone path through
`layer_reuses_current_output_texture`. When the key changes, the zone
composes and stabilizes, and the generation advances.

### 2.2 Lifetime

Snapshots are written only by stabilization and read only by passes queued
in the same frame (sampling, scene projection, zone preview scaling), and the
wgpu queue executes submissions in order. No consumer holds a zone snapshot
across frames: per-frame sampling sets are dropped once dispatched (a pending
readback holds only its buffer), and retained results store sampled colors,
not textures (§4.2). The existing snapshot lease needs no new enforcement for
this design.

---

## 3. Per-zone LED sampling

### 3.1 Strategy

`LedSamplingStrategy` gains one variant, built fresh every frame:

```rust
SparkleFlingerZones(Arc<ZoneSamplingSet>)

struct ZoneSamplingSet {
    // Scene-contributing LED zones in scene zone order.
    zones: Vec<ZoneSampling>,
    // Union layout, used only to shape and route ZoneColors.
    combined_layout: Arc<SpatialLayout>,
}

struct ZoneSampling {
    zone_id: ZoneId,
    engine: SpatialEngine, // the zone's own engine and prepared plan
    source: ZoneSampleSource,
}

enum ZoneSampleSource {
    // GPU-admitted plan over the zone's snapshot.
    Gpu { texture: GpuTextureFrame, content_key: (u64, u64) },
    // CPU-backed zone whose plan the GPU cannot take (§6).
    Cpu { canvas: Canvas },
    // GPU-backed zone whose plan the GPU cannot take (§6).
    Unsupported { reason: GpuSamplingRefusal },
}
```

The multi-zone branch of `render_scene` returns this variant whenever the
backend supports GPU output frames. The single-zone fast path keeps
`SparkleFlinger(SpatialEngine)` except during the transitions in §4.1.
`combined_led_spatial_engine` leaves the LED path entirely, as Spec 64 §12.1
always intended.

### 3.2 Dispatch

One encoder, one submit, one readback per frame for all `Gpu` sources:

- The points buffer holds every `Gpu` zone's points, concatenated in zone
  order.
- One compute pass per zone texture. Each pass binds its zone's texture and
  reads its parameters through a dynamic offset into one params buffer, with
  the existing `_pad` field carrying the pass's `point_offset`
  (`sample.wgsl:33-38`). Passes write disjoint ranges of one output buffer.
- Area-average zones need a summed-area table per sampled texture: one SAT
  buffer per distinct texture size, rebuilt in each pass that needs it.
- One copy of the whole output and one `map_async`, through the existing
  three-slot readback ring and its pending, stale, and saturated handling.

`Cpu` sources sample with their own engine on the CPU (`sample_append_into_at`).
`Unsupported` sources take §6's behavior. The final `ZoneColors` are
assembled in zone order across all three kinds, which matches the CPU
per-zone path (`render_zones/scene_output.rs:28-46`), so device routing is
unchanged.

### 3.3 Caching and freshness

- Bind groups are keyed by `(texture storage id, SAT buffer id)`, not by
  `GpuSampleSource`, and survive main-compositor surface switches; entries
  retire when their snapshot is released.
- The plan key becomes an ordered key over every constituent engine: each
  engine's process-unique id plus its plan generation, in zone order. Today
  the key is `(pointer, length, generation)` built from the first output's
  generation (`gpu_sampling.rs:390`, `:1320`), every fresh engine starts at
  generation 1 (`hypercolor-core/src/spatial/mod.rs:219`), and uploaded
  points are reused on a key match (`gpu_sampling.rs:1118`), so a reused
  allocation or a change in a later zone could reuse stale points.
- The cached-result key is `(content digest, plan key)`, where the digest
  hashes every `Gpu` zone's `(storage_id, content_generation)` in order. With
  §2.1, an unchanged scene is a cache hit.

### 3.4 Consumption

`resolve_led_sampling` treats `SparkleFlingerZones` like `SparkleFlinger` for
deferral, stale retry, saturation, held-frame reuse, and zone-shape
signatures, with the shape signature computed over every zone in the set.
Pending GPU sampling state is owned by the sampler, not the compositor's
surface state, so a display zone switching the compositor's size mid-frame
cannot discard it. The strategy does not force full composition of the
primary plan (`frame_sampling.rs:43-45`): LED output no longer depends on the
composed scene, so the scene composes only when a consumer in §5 asks.

---

## 4. Transitions, retention, and unassigned outputs

### 4.1 Scene transitions

A scene transition blends in LED space whenever the outgoing or incoming
scene has more than one scene-contributing zone. The base is the last stable
published LED colors, which the LED-space blend already captures
(`frame_sampling.rs:787-833`); the current side is the incoming scene sampled
before any canvas crossfade. For an incoming multi-zone scene that is the
`SparkleFlingerZones` result. For an incoming single-zone scene, the
single-zone path reports a one-zone `SparkleFlingerZones` over its own
stabilized scene frame for the length of the transition, so LEDs never sample
the crossfaded composite, whose base is a preview image with overlap baked
in. The `uses_scene_sampled_zones` check (`frame_sampling.rs:442-445`)
extends to `SparkleFlingerZones`. Single-zone to single-zone transitions keep
today's canvas crossfade. The preview's canvas transition base is unchanged.

### 4.2 Retained scenes

A successful multi-zone frame retains its sampled `ZoneColors` and layout as
`RetainedLedSamplingStrategy::PreSampled` (`render_zones/scene_retention.rs`),
never the sampling set. `reuse_last_good_scene` (`scene_assembly.rs:74-84`)
and the render-error path (`frame_composer.rs:468`) resume from those colors
as `RetainedPreSampled`, so no retained result re-samples a snapshot that has
moved on.

### 4.3 Unassigned outputs

`Off` and `Hold` behave as today; they do not read zone pixels.
`UnassignedBehavior::Fallback(zone)` today samples unassigned outputs from
`render_stage.zone_canvases`, which are CPU canvases
(`unassigned_output.rs:52`, `:100`; `frame_executor.rs:594-599`). For a GPU
zone, the synthesized fallback outputs are appended to the target zone's
`Gpu` plan as an extra `GpuZoneRange` against that zone's snapshot, and their
colors join the frame's readback. For a `Cpu` zone the existing path stays.

---

## 5. Scene canvas and zone previews

### 5.1 Scene canvas

The scene canvas is composed on the GPU from zone snapshots with the existing
projection layers (`projection.rs:150-180`, one layer per output over the
opaque black base) whenever a consumer requests it: a preview receiver, a
display face that blends the scene, or a canvas capture. The primary
composition plan, including active transitions, runs over it exactly as
today, so display backdrops see transitions. It never feeds LED output, so
admission drops the sampling-mode and edge-behavior requirements and keeps
the affine requirement. Overlapping outputs keep zone-order overwrite, which
Spec 64 already calls a harmless detail of a preview image.

The target-space fetch rounds to the nearest texel. That is exact for nearest
outputs and close for others, but Spec 48 §4.2 asks the preview to follow LED
sampling semantics, and equal-size bilinear or area zones can differ on
patterned content. Phase 3 closes that gap; until then the diagnostic
snapshot reports `preview_projection_filter = nearest`.

### 5.2 Zone previews

Studio's addressable zone previews (Spec 64 §10, §12.3) are built from
`zone_canvases`, whose converter rejects GPU frames (`frame_io.rs:421`,
`:478`, `:546`). For `Gpu` zones, when `zone_preview_due` admits a frame, the
render thread scales the zone's snapshot on the GPU to the zone-preview
demand's `max_width` and `max_height` (full size when a subscriber asks for
full resolution), reads back that transport-sized payload, and publishes it
as a `ProducerFrame::Surface` entry. Cadence and demand gating are unchanged.
`Cpu` zones keep publishing their canvases as today.

---

## 6. Unsupported plans and the CPU path

- **CPU compositor backend.** Unchanged. Frames are CPU canvases, and the
  Spec 64 §12.1 CPU per-zone path stays the implementation.
- **GPU backend, zone plan admitted.** `Gpu` source, §3.2.
- **GPU backend, plan refused, zone frames CPU-backed.** The zone composes on
  the CPU and samples on the CPU as a `Cpu` source. A native or color-fill
  zone with Gaussian sampling keeps working exactly as it does today.
- **GPU backend, plan refused, zone frames GPU-backed** (a Gaussian zone
  running an HTML effect, or a plan past device limits). An `Unsupported`
  source: the zone holds its last published colors when the published frame
  carries them with a matching shape, and otherwise outputs black. Its layers
  report `LayerHealth::Failed { reason: "gpu_sampling_unsupported:<mode>" }`,
  and diagnose warns under `multi_zone_sampling`, naming the zone. This
  follows Spec 48 §4.3 and is never silent.
- **The keyless readback latch leaves the scene-zone path.**
  `requires_cpu_sampling_canvas` is no longer requested for GPU scene zones.
  The latch stays for its other callers until Spec 56's stage collapse
  retires it.

---

## 7. Telemetry

- `gpu_zone_sampling` reports true for frames whose set has any `Gpu` source.
- New frame metrics: `gpu_sampled_zone_count`, `cpu_sampled_zone_count`,
  `unsupported_zone_count`, `gpu_sampling_passes`, and
  `zone_composition_skips` (zones whose composition key matched).
- `producer_full_frame_copy` must not record
  `surface_to_zone_canvas_materialization` for GPU-backed zones; a test
  asserts it.
- The diagnose `render` category gains `multi_zone_sampling`: pass when no
  zone is `Unsupported`, warn naming each `Unsupported` zone.

---

## 8. Phases and acceptance

### Phase 1: GPU-resident multi-zone (unblocks the rig)

§2, §3, §4, §5.1 admission, §5.2, §6, and §7.

GPU tests run with `HYPERCOLOR_REQUIRE_GPU_TESTS=1`, so a missing adapter
fails rather than skips (`sparkleflinger/gpu/tests.rs:74`). Required cases:

1. Two and three zones with distinct solid fills and overlapping output
   rectangles: each zone's LEDs show that zone's color.
2. Zones with different canvas sizes, plus a display zone composing at a
   third size in the same frame, across several frames, including pending
   and deferred sampling.
3. Parity: for Nearest, Bilinear, and AreaAverage plans, per-zone GPU colors
   match the CPU per-zone path within the existing sampler parity tolerance.
4. A zone whose producer returns a retained frame keeps its generation and
   hits the sampling cache; a zone whose layer changes advances it.
5. Transitions single to multi, multi to single, and multi to multi with
   overlapping zones: overlapped LEDs blend from their own old color to
   their own new color, never through another zone's color.
6. Render failure resumes from retained colors; `Off`, `Hold`, and
   `Fallback(zone)` produce the same LED output as the CPU path.
7. A Gaussian native zone beside a Bilinear HTML zone: the native zone
   samples on the CPU and stays correct; a Gaussian HTML zone reports
   `Unsupported` with health and diagnose surfaces.
8. Media, screen-region, and web-viewport layers in a non-primary zone.
9. Zone previews publish for `Gpu` zones at the demanded size and cadence;
   scene preview and a blended display backdrop are non-black and include
   transitions.
10. The CPU backend produces the same per-zone output as before.
11. No whole-image readback: the scene-zone path never calls
    `latch_sampling_surface_readback`, §7's materialization assertion holds,
    and test-level allocation counters for snapshots, bind groups, and SAT
    buffers stay flat after warmup.
12. The reproduction test from the investigation is un-ignored and passes.

Live acceptance on the evidence rig: the Default zone running Bubble Garden
plus a ROLI zone running Bubble Garden with Touch Ripple in `screen` holds
the 60 fps tier with all five display faces active for ten minutes;
`frame_liveness`, `led_freshness`, and `multi_zone_sampling` pass;
`gpu_zone_sampling` is true; the Lightpad and LUMI show ripples over
bubbles; every other device shows Bubble Garden; and frame `total_us` p95
stays within 25 percent of the same rig running the single-zone scene.

### Phase 2: preview fidelity

Linear-light bilinear and area filtering in the target-space projection path,
so the scene canvas and zone previews match the CPU raster within tolerance
for every admitted mode on patterned content, including zones whose layout
comes from `zone_layout_preview` (`scene_snapshot.rs:267`).

### Phase 3: GPU Gaussian sampling

A GPU Gaussian-area sampler with matching preview reconstruction, which
retires the `Unsupported` source for Gaussian plans.

---

## 9. Open questions

1. **One pass per zone or one pass for all zones.** One pass with a texture
   binding array would avoid K pass setups but needs
   `TEXTURE_BINDING_ARRAY`, which is not universal. K passes in one encoder is
   the portable baseline; measure before reaching for arrays.
2. **Snapshot copy cost.** Stabilizing copies each zone's compositor output
   into its snapshot, about 1.6 MB per 640x630 zone per changed frame on the
   GPU. Composing straight into the zone's snapshot would remove the copy.
   Worth doing if Phase 1 profiling shows it; Spec 56's stage work may own it.
3. **Compositor size switches.** Size-keyed surface sets already exist
   (§1.3), and Phase 1 decouples sampling from them (§3.4). Whether the
   preview and composition state cleared on a switch also needs decoupling is
   settled by Phase 1 test case 2 and the live run.

---

## 10. Review history

**rev 1 to rev 2** (two-lens cross-model review of rev 1, both NEEDS_CHANGES):

- Transitions now blend in LED space in both directions whenever either end is
  multi-zone (§4.1); rev 1 covered only multi-zone targets.
- Retained results store sampled colors, never snapshot textures, and the
  proposed lease assertion is dropped (§2.2, §4.2); rev 1 contradicted itself.
- A per-zone composition key keeps generations stable for retained producer
  frames (§2.1); rev 1's digest would have missed the cache every frame.
- The plan key is an ordered key over every constituent engine (§3.3).
- The blackout mechanism is stated as timing-dependent (§1.1).
- Retention and unassigned outputs moved into Phase 1 (§8).
- Zone previews are preserved from the GPU (§5.2); rev 1 dropped them.
- Each zone carries an explicit sample source, so mixed admissible and
  unsupported zones have defined outcomes (§3.1, §6).
- CPU-backed zones with GPU-unsupported plans stay on the CPU (§6); rev 1
  would have broken Gaussian native zones.
- Preview parity is required for all admitted modes in a later phase (§8).
- Acceptance names executable gates, GPU-required test runs, and the cases
  rev 1 missed (§8).
