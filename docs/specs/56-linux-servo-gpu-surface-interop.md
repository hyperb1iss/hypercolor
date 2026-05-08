# 56 - Linux Servo GPU Surface Interop

**Status:** Implemented, live soak pending before default-on
**Author:** Nova
**Date:** 2026-05-08
**Crates:** `hypercolor-core`, `hypercolor-daemon`
**Related:** Specs 48, 57, 58;
`docs/design/34-servo-perf-and-crash-isolation.md`,
`docs/design/45-graphics-pipeline-unification-plan.md`

## 1. Goal

Make Linux Servo HTML effects publish GPU-resident frames into the render
pipeline without a full-frame `glReadPixels` readback and without uploading the
same pixels back into SparkleFlinger.

The first implementation target is not perfect zero-copy. The first target is:

1. Servo paints into a hardware GL framebuffer.
2. The Servo worker imports that framebuffer into the same Vulkan-backed
   `wgpu::Device` used by SparkleFlinger.
3. SparkleFlinger samples the imported texture directly.
4. CPU `Canvas` readback remains available as a fallback.

The first milestone is complete when one Linux Servo effect can reach
SparkleFlinger as a `wgpu::Texture` with no call to
`read_pixels_into_buffer()` on the successful GPU path.

## 2. Current Problem

The current Servo path is CPU-bound by type:

- `ServoWorker` paints the page, calls `read_framebuffer_into_canvas()`, then
  returns a `Canvas`.
- `Canvas` owns CPU RGBA bytes.
- `ProducerFrame` exposes frames through `rgba_bytes()`.
- `PublishedSurfaceStorage` only supports `CpuRgba`.
- `GpuSparkleFlinger` uploads producer bytes with `queue.write_texture()`.

That means a Servo frame currently does:

```text
Servo GL framebuffer
  -> glReadPixels
  -> CPU Canvas
  -> queue.write_texture
  -> SparkleFlinger texture
```

The GPU compositor and GPU zone sampler can already avoid full-frame output
readback in several cases. Servo cannot benefit from that because its producer
output has already been collapsed into CPU memory.

## 3. Non-goals

- Do not solve macOS or Windows in the first milestone.
- Do not modify `hypercolor-types` to depend on `wgpu`, `surfman`, `ash`, or
  platform GPU handles.
- Do not remove the CPU Servo path.
- Do not promise LED device output is GPU-only. Physical LED writes still
  require CPU device packets.
- Do not solve display JPEG encoding, display USB transfer, or WebSocket canvas
  publication as part of this milestone.
- Do not vendor `wgpu-graft` unchanged. Treat it as reference architecture.
- Do not add a Servo-specific lower-resolution upscale path. Canvas dimensions
  still come from the render group or display surface contract.

## 4. Hard Constraints

### 4.1 `hypercolor-types` stays pure

`hypercolor-types` is the shared data crate. It must not gain GPU backend
dependencies. GPU frame handles should live in `hypercolor-core` or
`hypercolor-daemon` behind feature and platform gates.

### 4.2 Current `EffectRenderer` output is CPU-only

`EffectRenderer::render_into()` writes into caller-owned `Canvas`. A GPU frame
cannot flow through that method without immediately reading pixels to CPU.

This work requires a new output contract. The compatibility path can keep
`render_into()`, but the GPU path needs an output enum that can represent either
CPU canvas bytes or an imported GPU texture.

### 4.3 Servo GL commands are thread-affine

The import path needs GL commands: framebuffer binding, external-memory texture
setup, and framebuffer blits. Those commands must run on the Servo worker
thread while the Servo GL context is current.

The Servo worker should import into a `wgpu::Texture` using the shared
SparkleFlinger device, then send the imported texture handle back to the render
thread.

### 4.4 Linux import requires a Vulkan-backed `wgpu` device

The Linux path depends on Vulkan external memory and GL external-memory
extensions. It must be disabled when SparkleFlinger is running on a non-Vulkan
backend.

### 4.5 Unsafe is a policy gate

The implementation probably needs `wgpu-hal`, raw Vulkan handles, and raw GL
external-memory calls. That is an unsafe boundary.

Hypercolor currently forbids `unsafe_code` in workspace crates. Before landing
the real import module, choose one of these approaches:

1. Keep the unsafe code in a small external dependency with a safe Hypercolor
   wrapper.
2. Create a tiny optional interop crate with an explicit audited unsafe
   exception and keep the rest of the workspace on `unsafe_code = "forbid"`.

This decision must be explicit before implementation. A spec-compliant patch
does not hide unsafe inside unrelated render-thread code.

## 5. Target Architecture

```text
EffectPool
  -> ServoRenderer
  -> ServoWorker
  -> Linux Surfman GPU context
  -> Servo paint()
  -> LinuxGpuFrameImporter
  -> ImportedServoTexture
  -> ProducerFrame::Gpu
  -> GpuSparkleFlinger compose/sample
```

### 5.1 Shared GPU device

Add a daemon-owned `GpuRenderDevice` wrapper around:

- `wgpu::Instance`
- `wgpu::Adapter`
- `wgpu::Device`
- `wgpu::Queue`
- backend metadata
- capability metadata

`GpuSparkleFlinger` should receive this wrapper instead of creating an
unshareable private device. Servo GPU import receives a clone or `Arc` to the
same wrapper.

Linux Servo import is enabled only when:

- the selected backend is Vulkan
- the device can expose the required `wgpu-hal` Vulkan handles
- the Servo GL context exposes the required external-memory GL entry points
- the configured mode allows GPU import

### 5.2 Servo GPU context

Add a Linux-only Servo rendering context backed by a Surfman generic GPU
surface. It must implement Servo's `RenderingContext` trait and keep
`read_to_image()` working for fallback.

The context must expose an internal frame acquisition method for the Servo
worker:

```rust
pub(crate) struct NativeServoGpuFrame {
    pub width: u32,
    pub height: u32,
    pub framebuffer_id: u32,
    pub gl: Arc<glow::Context>,
    pub extension_loader: Arc<dyn Fn(&str) -> *const c_void + Send + Sync>,
}
```

This type is illustrative, not final API. The important contract is that the
Servo worker can acquire the current GL framebuffer and import it while the GL
context is current.

### 5.3 Imported frame output

Introduce a render-output type outside `hypercolor-types`:

```rust
pub enum EffectRenderOutput {
    Cpu(Canvas),
    Gpu(ImportedEffectFrame),
}

pub struct ImportedEffectFrame {
    pub width: u32,
    pub height: u32,
    pub format: ImportedFrameFormat,
    pub storage_id: u64,
    pub texture: Arc<wgpu::Texture>,
    pub view: Arc<wgpu::TextureView>,
}
```

The exact module placement is open, but the type must not force `wgpu` into
`hypercolor-types`.

### 5.4 Producer queue contract

Extend daemon-local producer frames:

```rust
pub(crate) enum ProducerFrame {
    Canvas(Canvas),
    Surface(PublishedSurface),
    Gpu(ImportedEffectFrame),
}
```

`ProducerFrame::rgba_bytes()` should become CPU-only. Call sites that need CPU
bytes must explicitly request CPU fallback or CPU materialization. GPU-aware
call sites should branch on `ProducerFrame::Gpu`.

### 5.5 SparkleFlinger contract

SparkleFlinger must accept GPU layers without `queue.write_texture()`.

For `CompositionMode::Replace` and opacity `1.0`, it may use the imported
texture view as the current source when dimensions and format match. For blend
modes, it should bind the imported source view directly into the compute
pipeline.

If a CPU canvas is required by downstream consumers and GPU sampling cannot
satisfy it, SparkleFlinger may use its existing readback path. That readback is
after composition, not Servo-specific.

## 6. Linux Import Strategy

The Linux first implementation should use the proven external-memory shape:

1. Create a Vulkan image with exportable opaque-FD memory using the same Vulkan
   device behind SparkleFlinger's `wgpu::Device`.
2. Export the memory FD.
3. Import that FD into GL through `GL_EXT_memory_object_fd`.
4. Create a GL texture backed by that memory with `TexStorageMem2DEXT`.
5. Attach the GL texture to a temporary draw framebuffer.
6. Blit from Servo's source framebuffer into the shared texture framebuffer.
7. Wrap the Vulkan image as a `wgpu::Texture`.
8. Return `ImportedEffectFrame` to the render thread.

This removes CPU readback. It still performs a GPU-to-GPU blit. Call it
"GPU-resident import", not "perfect zero-copy", until profiling proves a lower
copy path.

### 6.1 Required Linux capabilities

At startup or first use, detect and record:

- `wgpu` backend is Vulkan
- Vulkan external memory support for opaque FD
- GL entry points:
  - `glCreateMemoryObjectsEXT`
  - `glImportMemoryFdEXT`
  - `glTexStorageMem2DEXT`
  - `glDeleteMemoryObjectsEXT`
- framebuffer blit support
- compatible RGBA8 format support

Capability failure must explain the exact missing feature.

### 6.2 Synchronization

Milestone 1 may use conservative synchronization:

```text
Servo paint
  -> GL blit into shared texture
  -> glFlush or glFinish
  -> imported texture sent to render thread
  -> SparkleFlinger samples texture
```

Start with correctness. Measure `servo_gpu_import_sync_us` separately so we can
replace conservative waits with explicit fence/semaphore synchronization later.

Milestone 2 should investigate GL sync objects and Vulkan external semaphores.
Do not mix synchronization improvements into the first proof unless the
conservative path is incorrect.

### 6.3 Format and orientation

The imported frame is non-premultiplied sRGB RGBA, matching the canonical
surface contract.

Servo GL framebuffers use a bottom-left origin. The import blit may Y-flip into
the target texture. SparkleFlinger should see top-left origin, matching
`Canvas`.

Pixel parity tests must compare the imported path against the current CPU
readback path for deterministic fixtures.

## 7. Configuration and Fallback

Add a Servo GPU import mode:

```toml
[rendering.servo_gpu_import]
mode = "auto" # auto | on | off
```

Mode behavior:

- `off`: Always use the current CPU path.
- `auto`: Try GPU import. If capabilities are missing, use CPU and report the
  reason. If a frame import fails transiently, reuse the last valid frame or
  fall back to CPU for that frame.
- `on`: Require GPU import. Capability failure marks Servo GPU import
  unavailable and reports a hard diagnostic error. The renderer may still use
  CPU fallback only if explicitly requested by a test or debug override.

Default should be `auto` only after the path has soak coverage. During early
implementation, keep it opt-in.

## 8. Telemetry

Add metrics before enabling the path by default:

- `servo_gpu_import_enabled`
- `servo_gpu_import_backend`
- `servo_gpu_import_capability`
- `servo_gpu_import_failures_total`
- `servo_gpu_import_fallback_reason`
- `servo_gpu_import_blit_us`
- `servo_gpu_import_sync_us`
- `servo_gpu_import_total_us`
- `servo_readback_us`
- `producer_gpu_frames_total`
- `producer_cpu_frames_total`
- `sparkleflinger_gpu_source_upload_skipped_total`

The performance dashboard should make it obvious which path is active.

## 9. Verification Contract

Every implementation wave must prove one specific thing.

Minimum checks:

- `cargo check -p hypercolor-core --features servo`
- `cargo check -p hypercolor-daemon --features servo,wgpu`
- focused Servo worker tests
- focused SparkleFlinger GPU tests
- Linux-only import tests behind a feature gate
- one deterministic pixel parity test comparing CPU readback and GPU import
- one benchmark showing `servo_readback_us == 0` on the successful GPU import
  path

`just verify` remains the broad gate before merge.

## 10. Implementation Waves

### Wave 0: Baseline and control metrics

**Files:** `crates/hypercolor-core/src/effect/servo/worker.rs`,
`crates/hypercolor-daemon/src/performance/`, render metrics plumbing.

Implementation:

- Ensure Servo paint, readback, and total worker frame timings are visible in
  metrics.
- Add a counter for CPU Servo frames emitted.
- Add a benchmark or scripted scenario for one deterministic Servo HTML effect.

Verify:

- Baseline scenario reports `servo_readback_us`.
- CPU path behavior is unchanged.
- Baseline numbers can be compared before and after GPU import.

### Wave 1: Shared GPU device

**Files:** `crates/hypercolor-daemon/src/render_thread/sparkleflinger/gpu.rs`,
new daemon GPU device module, related tests.

Implementation:

- Extract `GpuRenderDevice`.
- Make `GpuSparkleFlinger::new()` accept or create a shared device wrapper.
- Record backend metadata.
- Add a Linux-only capability check for Vulkan.

Verify:

- Existing SparkleFlinger tests pass.
- GPU compositor reports the active backend.
- CPU compositor fallback behavior is unchanged.

### Wave 2: Linux interop module

**Files:** new interop module or crate, Linux-only build config, tests.

Implementation:

- Build the safe public wrapper for GL FBO to Vulkan/wgpu import.
- Hide raw Vulkan, GL external-memory calls, and unsafe code inside one audited
  boundary.
- Return a typed `ImportedEffectFrame`.
- Provide a deterministic raw-GL solid-color fixture before involving Servo.

Verify:

- Linux raw-GL fixture imports into `wgpu`.
- Readback from the imported `wgpu::Texture` matches expected pixels.
- Capability failure returns a precise reason, not a generic error.

### Wave 3: Linux Servo GPU context

**Files:** `crates/hypercolor-core/src/effect/servo_bootstrap.rs`,
`crates/hypercolor-core/src/effect/servo/worker.rs`, new Servo context module.

Implementation:

- Add a Linux Surfman generic-surface rendering context.
- Keep `read_to_image()` available.
- Expose a worker-only native-frame acquisition method.
- Gate with Servo GPU import config and capabilities.

Verify:

- Existing Servo CPU tests still pass.
- A Servo fixture can render through the GPU context and still read CPU pixels
  through fallback.
- Context resize preserves current Servo dimension semantics.

### Wave 4: Effect output contract

**Files:** `crates/hypercolor-core/src/effect/traits.rs`,
`crates/hypercolor-core/src/effect/pool.rs`, Servo renderer/session/client
modules, daemon render-group plumbing.

Implementation:

- Introduce `EffectRenderOutput`.
- Preserve the old `render_into()` behavior for native and CPU renderers.
- Let Servo return `Gpu(ImportedEffectFrame)` when import succeeds.
- Reuse last valid GPU or CPU frame when a Servo frame is late.

Verify:

- Native effects still render into CPU canvases unchanged.
- Servo CPU fallback still emits `Canvas`.
- Servo GPU path emits `ImportedEffectFrame` without allocating a full CPU
  canvas.

### Wave 5: GPU producer frames and SparkleFlinger composition

**Files:** `crates/hypercolor-daemon/src/render_thread/producer_queue.rs`,
`crates/hypercolor-daemon/src/render_thread/composition_planner.rs`,
`crates/hypercolor-daemon/src/render_thread/sparkleflinger/gpu.rs`,
SparkleFlinger tests.

Implementation:

- Add `ProducerFrame::Gpu`.
- Make CPU-only code paths explicitly reject or materialize GPU frames.
- Bind imported texture views directly in SparkleFlinger.
- Skip `queue.write_texture()` for GPU producer frames.

Verify:

- GPU producer frame test proves no source upload occurs.
- Blended composition from a GPU source matches CPU parity fixtures.
- CPU compositor either receives CPU fallback or reports unsupported input
  cleanly.

### Wave 6: Config, diagnostics, and fallback policy

**Files:** config structs, daemon diagnostics, API status/metrics surfaces,
docs.

Implementation:

- Add `rendering.servo_gpu_import.mode`.
- Add capability and fallback diagnostics.
- Make `auto`, `on`, and `off` behavior explicit and test-covered.

Verify:

- `off` never attempts import.
- `auto` falls back to CPU with a precise reason.
- `on` reports a hard diagnostic when import is unavailable.

### Wave 7: Benchmarks and soak

**Files:** benchmarks, perf docs, optional stress fixtures.

Implementation:

- Compare CPU Servo readback versus Linux GPU import for the same effect.
- Track p50, p95, p99, copy bytes, import sync time, and total render-thread
  frame time.
- Run mixed scenes with Servo LED effects, display faces, preview streaming,
  and GPU zone sampling.

Verify:

- Successful GPU path reports `servo_readback_us == 0`.
- Render-thread p95 improves or the path remains opt-in.
- Soak test does not leak textures, Vulkan memory, GL memory objects, or Servo
  sessions.

## 11. Acceptance Criteria

The feature is ready to enable by default on Linux when:

1. CPU fallback is always available.
2. Missing capabilities produce clear diagnostics.
3. The successful GPU path avoids Servo full-frame CPU readback.
4. SparkleFlinger composes imported Servo textures without CPU re-upload.
5. Pixel parity passes against the CPU path for deterministic fixtures.
6. Metrics prove the active path and the fallback reason.
7. Linux soak shows no GPU memory leak.
8. `just verify` passes.

## 11.1 Implementation Notes

The Linux opt-in vertical slice is implemented on 2026-05-08. The core path is:

- `hypercolor-linux-gpu-interop` owns the audited unsafe GL/Vulkan boundary.
- `GpuRenderDevice` provides the shared Vulkan-backed `wgpu` device.
- Servo emits `EffectRenderOutput::Gpu(ImportedEffectFrame)` when GPU import
  succeeds and falls back to CPU `Canvas` output otherwise.
- Daemon-local `ProducerFrame::Gpu` flows imported textures into
  SparkleFlinger without `queue.write_texture()` source upload.
- Metrics expose Servo GPU import timings, fallback reason, producer CPU/GPU
  frame counts, and skipped SparkleFlinger source uploads.

Verification receipts from the implementation pass:

- `cargo check --locked -p hypercolor-core --features servo` passed.
- `cargo check --locked -p hypercolor-daemon --features servo,wgpu` passed.
- `cargo test --locked -p hypercolor-core --features servo-gpu-import
  servo_gpu_import` passed.
- `cargo test --locked -p hypercolor-daemon --features servo-gpu-import gpu`
  passed with 72 daemon library GPU tests plus focused binary/integration
  tests.
- `cargo test --locked -p hypercolor-linux-gpu-interop` passed.
- `HYPERCOLOR_RUN_GPU_INTEROP_FIXTURE=1 cargo test --locked -p
  hypercolor-linux-gpu-interop --features raw-gl-fixture
  raw_gl_solid_color_import_matches_wgpu_readback -- --nocapture` passed.
- `scripts/servo-gpu-import-proof.sh` passed and printed
  `servo_readback_us_delta=0` on the successful GPU import path.
- `just verify` passed.
- `just ui-test` passed.

The path remains opt-in/default-off until a live mixed-scene soak confirms no
texture, Vulkan memory, GL memory object, or Servo-session leak under real
preview, display, and GPU spatial sampling load.

## 12. Recommendation

Build this as a Linux-only opt-in vertical slice first:

1. Shared Vulkan `GpuRenderDevice`.
2. Raw GL fixture import into `wgpu`.
3. Servo GPU context import.
4. `ProducerFrame::Gpu`.
5. SparkleFlinger direct GPU source binding.

This keeps the work honest. We prove the hard interop path before reshaping the
entire render pipeline around it, and we keep the existing CPU path as the
reversibility lever.
