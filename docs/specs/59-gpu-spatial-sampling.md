# 59 - GPU Spatial Sampling

**Status:** Planned
**Author:** Nova
**Date:** 2026-05-08
**Crates:** `hypercolor-core`, `hypercolor-daemon`
**Related:** Specs 48, 56; `docs/design/45-graphics-pipeline-unification-plan.md`

---

## 1. Goal

Keep the render pipeline GPU-resident through spatial sampling. When
SparkleFlinger has a GPU scene texture, Hypercolor should sample LED colors
with a compute pass and read back only packed LED RGB data.

In this spec, `sample_count` means the total number of LED sample points across
all zones in the prepared plan. Today that is normally one sample point per LED.

The target successful path is:

```text
Servo imported GPU texture or CPU producer upload
  -> SparkleFlinger GPU composition texture
  -> GPU spatial sampling compute pass
  -> sample_count * 4 byte readback buffer
  -> Vec<ZoneColors>
  -> BackendManager device packets
```

The first milestone is complete when a Servo GPU-imported effect can drive LED
hardware without a full canvas readback on the successful path. CPU still owns
device packet encoding and physical I/O.

## 2. Current Problem

The CPU spatial sampler is semantically correct, but it is the wrong hot-path
boundary once effects and composition are GPU-resident.

If the renderer has to materialize a CPU canvas for sampling, the frame pays
for:

1. full scene texture readback, usually `canvas_width * canvas_height * 4`
   bytes
1. CPU nearest, bilinear, area, or Gaussian sampling for every LED
1. CPU allocation and cache pressure around the sampled `ZoneColors`
1. possible upload again if a downstream preview or compositor path needs GPU
   data

At the default 640x480 canvas, a full RGBA readback is about 1.2 MiB per frame.
The LED payload is usually orders of magnitude smaller. For 2,000 sample
points, the readback target is 8 KiB before zone regrouping.

## 3. Non-goals

- Do not move USB, HID, network, or display transport packet writes onto the
  GPU.
- Do not remove the CPU spatial sampler. It remains the semantic reference and
  fallback.
- Do not silently lower Gaussian-area sampling to bilinear or box area.
- Do not make `hypercolor-types` depend on `wgpu` or platform GPU handles.
- Do not require canvas preview, JPEG preview, or display-face output to become
  zero-readback in this milestone.
- Do not make this Servo-only. Servo GPU import is the forcing function, but
  the sampler should work for any SparkleFlinger GPU output texture.

## 4. Current State

The codebase already has most of the right pieces:

- `SpatialEngine` owns layout normalization and produces immutable
  `PreparedZonePlan` values.
- `GpuSamplingPlan` can flatten nearest, bilinear, and area-average prepared
  zones into GPU sample points.
- `sample.wgsl` runs one compute invocation per LED, samples the scene texture,
  applies fade attenuation, and writes packed RGBA bytes to a storage buffer.
- `GpuSpatialSampler` copies only the used packed LED bytes into a small
  readback ring.
- Render-thread `DeferredSamplingState` tracks pending, retired, and scratch
  sampling work so the render loop can consume completed results without
  forcing a full-frame stall.
- The frame composer can avoid requiring a CPU sampling canvas when
  SparkleFlinger reports that it can sample the current prepared plan.
- Servo GPU import has already introduced `EffectRenderOutput::Gpu`,
  `ProducerFrame::Gpu`, and GPU producer counters behind the
  `servo-gpu-import` feature. This spec assumes those contracts exist and
  focuses on keeping that GPU producer path connected through LED sampling.

This spec does not ask for a rewrite. It turns the existing path into the
canonical architecture and defines the missing hardening work.

## 5. Target Architecture

### 5.1 Ownership split

Keep `SpatialEngine` as the CPU-side plan compiler:

- generate LED topology positions
- apply zone transform, rotation, scale, and edge behavior
- precompute sample positions and fade attenuation
- expose a stable prepared plan for CPU and GPU backends

Keep SparkleFlinger as the GPU-side executor:

- compose producer layers into a final scene texture
- encode the prepared spatial plan into GPU buffers
- dispatch the sampling compute pass
- manage LED readback buffers and deferred completion
- return `ZoneColors` to the render thread

This keeps layout semantics in `hypercolor-core` and GPU resource ownership in
`hypercolor-daemon`.

### 5.2 GPU plan format

The GPU sampler should consume a flat point buffer plus zone ranges:

```text
SamplePoint {
  x: f32,
  y: f32,
  method: u32,
  extra: u32,
}

GpuZoneRange {
  zone_id: String,
  start: usize,
  len: usize,
}
```

`extra` packs method-specific parameters:

- low 16 bits: fade attenuation
- high 16 bits: area radius for area-average sampling

The output buffer is flat `u32` RGBA, one element per LED. CPU regrouping into
`ZoneColors` uses `GpuZoneRange`.

Plan cache keys should include a generation, revision, or content hash in
addition to pointer identity and zone count. Pointer-plus-length is fast, but
allocator reuse can theoretically produce an ABA cache hit after layout
replacement. A tiny monotonically increasing plan generation on `SpatialEngine`
is enough.

That generation must flow through cached plans, uploaded point buffers, cached
sample results, `PendingZoneSampling`, and the predicate that decides whether a
pending readback still matches current work. A stale readback for a replaced
layout must never be promoted into the current frame.

### 5.3 Sampling dispatch

For every GPU-composed scene frame that has LED zones:

1. Ensure the prepared plan is supported.
1. Upload the sample point buffer only when the plan key changes.
1. Bind the current output texture view, point buffer, output buffer, and
   uniform params.
1. Dispatch `ceil(sample_count / 64)` workgroups.
1. Copy `sample_count * 4` bytes from the GPU output buffer into the next
   readback slot.
1. Return `PendingZoneSampling` when the map is not ready.

The render loop should consume a completed previous sample while queueing the
next sample for the current output texture. This is mandatory. If completion
and follow-up dispatch alternate across frames, physical output falls to half
rate.

### 5.4 Readback policy

Readback is asynchronous by default:

- use a small ring of mappable buffers
- poll with zero-timeout before waiting
- keep stale pending work in a retired queue when it may still release a
  readback slot
- drop stale results whose output generation or sampling plan no longer match
  current work

Blocking waits are allowed only when there is no acceptable retained frame to
reuse and hardware output would otherwise have no valid frame. That path must
be observable through metrics.

### 5.5 CPU materialization policy

`requires_cpu_sampling_canvas` should be false whenever the current
SparkleFlinger backend can sample the prepared zone plan on GPU.

Canvas materialization is still valid for:

- CPU compositor backend
- unsupported sampling modes
- explicit preview publication
- error fallback
- tests that intentionally exercise CPU parity

Late full-frame readback for CPU fallback must stay isolated in the fallback
path. It should never be needed for the successful GPU Servo path.

### 5.6 GPU producer integration

Spec 56 gives Servo a GPU-resident `EffectRenderOutput::Gpu` path on Linux.
This spec depends on GPU producer frames arriving at SparkleFlinger without
calling CPU byte APIs, whether the producer is Servo or a future GPU-native
effect backend.

GPU producer frames must:

- expose width, height, storage identity, texture, and texture view
- fail explicitly if a CPU-only caller asks for RGBA bytes
- compose directly in SparkleFlinger without `queue.write_texture()`
- become sampleable by the same GPU spatial sampler used for uploaded CPU
  producer frames

The preferred failure mode for accidental CPU byte access is a typed error that
degrades only the affected route. A render-thread panic is acceptable only as an
audited temporary guard while all CPU-only callers are being converted to branch
on the producer frame variant.

## 6. Sampling Semantics

The CPU prepared sampler remains the semantic source of truth. GPU output must
match CPU output within small rounding tolerance for the same canvas and layout.

### 6.1 Supported modes

GPU-supported for the first milestone:

- nearest
- bilinear
- area-average using the prepared area radius

CPU fallback remains required for:

- Gaussian-area sampling until a GPU kernel/weights buffer exists
- any future sampling mode that has not been ported to WGSL

### 6.2 Edge behavior

Clamp, wrap, mirror, and fade-to-black are resolved during CPU plan
preparation. The shader may clamp positions defensively, but it must not
replace the prepared edge behavior with shader-local behavior that disagrees
with the CPU path.

Fade-to-black is represented as per-sample attenuation. The compute shader
applies attenuation after sampling in linear light.

### 6.3 Color handling

The canonical scene texture is non-premultiplied sRGB RGBA. The sampler should:

1. load sRGB texture channels
1. decode to linear light
1. interpolate or average in linear light
1. apply attenuation
1. encode sampled RGB back to sRGB bytes

The returned `ZoneColors` remain `[u8; 3]` sRGB values. Device output policy may
later decode and shape those bytes for hardware-specific brightness or gamma.

### 6.4 Canvas resize parity

GPU sampling must match CPU output when runtime canvas dimensions differ from
the dimensions stored in a prepared plan. `SceneTransaction::ResizeCanvas`
lands at a frame boundary, but stale prepared dimensions can still appear during
transition or fallback paths.

A parity test should prepare a layout at one canvas size, sample a live canvas
at another size through both CPU and GPU paths, and assert the results stay
within the normal rounding tolerance.

## 7. Implementation Plan

### Phase 1: Make plan identity durable

**Files:** `crates/hypercolor-core/src/spatial/mod.rs`,
`crates/hypercolor-daemon/src/render_thread/sparkleflinger/gpu_sampling.rs`

- Add a stable prepared-plan generation or hash.
- Carry it into `GpuSamplingPlanKey`.
- Invalidate uploaded point buffers and cached sample results when generation
  changes.

**Verify:**

- Calling `SpatialEngine::update_layout()` between dispatches invalidates
  cached plans, uploaded point buffers, cached sample results, and any
  in-flight pending-readback match.
- `just test-crate hypercolor-daemon sparkleflinger` passes.

### Phase 2: Declare GPU sampling capability precisely

**Files:** `crates/hypercolor-daemon/src/render_thread/sparkleflinger/mod.rs`,
`crates/hypercolor-daemon/src/render_thread/frame_composer.rs`,
`crates/hypercolor-daemon/src/render_thread/frame_sampling.rs`

- Keep `can_sample_zone_plan()` tied to actual supported sampling modes.
- Add the missing composer-level Gaussian fallback coverage.
- Ensure GPU backend availability and plan support both feed
  `requires_cpu_sampling_canvas`.

**Verify:**

- Composer tests prove CPU canvas is not requested for supported GPU plans.
- Composer tests prove Gaussian-only plans request CPU fallback and never
  attempt GPU dispatch.

### Phase 3: Verify the GPU producer-to-LED path

**Files:** `crates/hypercolor-core/src/effect/`,
`crates/hypercolor-daemon/src/render_thread/producer_queue.rs`,
`crates/hypercolor-daemon/src/render_thread/sparkleflinger/gpu.rs`,
`crates/hypercolor-daemon/src/render_thread/render_groups.rs`

- Preserve GPU producer frames through the producer queue.
- Compose imported texture views directly without CPU materialization.
- Dispatch GPU spatial sampling against the composed output texture.
- Avoid every CPU byte API on the success path.
- Treat existing `LedSamplingOutcome` GPU flags as the sampling-dispatch
  telemetry contract unless a later metrics spec replaces them.
- Convert any temporary panic guard around GPU producer CPU byte access into a
  typed `ProducerFrameError` once this phase's success-path verification is
  green.

**Verify:**

- A GPU producer frame reaches `ZoneColors` without `rgba_bytes()`.
- Linux Servo GPU import covers the real producer path when available.
- A non-Servo GPU output texture covers the backend-general path in tests.
- Metrics count GPU producer frames and successful GPU sample dispatches.
- `cpu_sampling_late_readback` remains false on the successful path.

### Phase 4: Harden deferred readback behavior

**Files:** `crates/hypercolor-daemon/src/render_thread/frame_sampling.rs`,
`crates/hypercolor-daemon/src/render_thread/pipeline_runtime.rs`,
`crates/hypercolor-daemon/src/render_thread/sparkleflinger/gpu_sampling.rs`

- Resolve completed samples without blocking when possible.
- Queue follow-up sampling for the current frame after consuming a completed
  deferred result.
- Retire or drop stale work without leaking readback slots.
- Surface saturation and wait-blocked state in performance metrics.
- Keep preview readback metrics distinct from LED sampling readback metrics.

**Verify:**

- Tests cover the no-half-rate cadence: consume previous, dispatch current.
- Tests cover readback ring saturation and slot release.
- Metrics distinguish GPU sample deferred, retry hit, wait blocked, and CPU
  fallback.
- Metrics distinguish unsupported sampling mode fallback from GPU readback
  saturation or GPU execution failure.

### Phase 5: Optional Gaussian GPU support

**Files:** `crates/hypercolor-core/src/spatial/plan.rs`,
`crates/hypercolor-daemon/src/render_thread/sparkleflinger/gpu_sampling.rs`,
`crates/hypercolor-daemon/src/render_thread/sparkleflinger/sample.wgsl`

- Add a Gaussian sample method only after parity tests exist.
- Upload kernel weights and per-sample kernel metadata.
- Keep CPU fallback until Gaussian GPU output matches CPU tolerance.

Do not overload the current `SamplePoint.extra` field for Gaussian support. Add
either a versioned sample-point struct with additional fields or a sidecar
`GaussianSampleMeta` buffer keyed by sample index. The shader needs, at
minimum, attenuation, weight offset, weight count or radius, and weight sum.

**Verify:**

- CPU/GPU parity tests cover Gaussian radius and sigma combinations.
- Unsupported Gaussian plans remain explicit fallback until parity is green.

## 8. Verification Strategy

Use small deterministic canvases for semantic parity and larger canvases for
performance checks.

Required automated checks:

- `just test-crate hypercolor-core spatial`
- `just test-crate hypercolor-daemon sparkleflinger`
- `just test-crate hypercolor-daemon frame_sampling`
- `just test-one gpu_sampling_matches_cpu_after_canvas_resize`
- `just check`

Required behavioral checks:

- supported plans do not materialize a CPU sampling canvas
- unsupported plans materialize CPU fallback and preserve semantics
- deferred GPU sampling does not halve device update cadence
- readback copies are `sample_count * 4`, not full canvas size
- Servo GPU import success path does not call CPU `rgba_bytes()` or
  framebuffer readback
- non-Servo GPU output textures can also drive the GPU sampler
- canvas resize parity matches the CPU sampler within tolerance

Useful benchmark targets:

- sampled microseconds for CPU sampler vs GPU sampler
- readback bytes per frame
- render-thread stalls caused by sample readback waits
- device output cadence under 30/45/60 FPS targets

## 9. Risks

### 9.1 GPU readback latency can still stall

Small readbacks are much cheaper than full-frame readbacks, but mapping can
still block. The ring buffer and deferred sampling path are required, not
polish.

### 9.2 Preview demand can hide the win

Canvas preview and JPEG preview may still request surfaces. Metrics must
distinguish preview readback from LED sampling readback so we do not blame the
sampler for UI demand.

### 9.3 Plan cache identity can go stale

Pointer identity is not enough as the long-term cache contract. Add generation
or content identity before treating GPU sampling as canonical.

### 9.4 Sampling parity can drift

The WGSL path has its own color and rounding math. Keep CPU/GPU tolerance tests
close to the sampler so shader changes cannot silently alter hardware color.

### 9.5 Unsupported modes can reintroduce full readback

Gaussian and future modes must be visible capability misses. They should emit
clear metrics instead of looking like normal GPU sampling.

### 9.6 CPU byte misuse can panic the render thread

GPU producer frames should make accidental CPU materialization impossible to
ignore, but the long-term contract should be typed failure and degraded
fallback, not an uncontrolled render-thread panic.

## 10. Recommendation

Adopt GPU spatial sampling as the canonical LED sampling path whenever
SparkleFlinger owns the composed scene texture and the prepared zone plan is
supported.

Do not replace `SpatialEngine`. It is the right place to compile layout
semantics. The sharp architecture is CPU plan compiler, GPU sample executor,
and tiny LED-byte readback. That gives Servo GPU import the payoff it deserves
without coupling pure layout types to `wgpu` or sacrificing CPU fallback
correctness.
