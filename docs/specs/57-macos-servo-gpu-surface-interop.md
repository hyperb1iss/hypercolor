# 57 - macOS Servo GPU Surface Interop

**Status:** Implemented; macOS live validation captured; `just dev` defaults to
`auto`
**Author:** Nova
**Date:** 2026-05-08
**Crates:** `hypercolor-core`, `hypercolor-daemon`, optional interop crate
**Related:** Specs 48, 56, 59;
`docs/design/34-servo-perf-and-crash-isolation.md`,
`docs/design/45-graphics-pipeline-unification-plan.md`

## 1. Goal

Make macOS Servo HTML effects publish GPU-resident frames into SparkleFlinger
without a full-frame CPU readback.

The macOS target path is:

```text
Servo GL framebuffer
  -> Surfman generic IOSurface-backed surface
  -> Metal texture created from IOSurface
  -> wgpu Metal texture
  -> SparkleFlinger source texture
```

Linux proved the shared Hypercolor architecture first: shared GPU device, GPU
effect output, `ProducerFrame::Gpu`, SparkleFlinger direct source binding,
fallback diagnostics, and parity tests. This spec defines the macOS importer and
platform work needed to reuse that lane with IOSurface and Metal.

## 2. Baseline Profile

Captured on 2026-05-18 with the daemon already running on a MacBook Pro
`Mac16,5`: Apple M4 Max, 16 CPU cores, 40 GPU cores, 48 GB RAM, Metal 4.
The active effect was `Breakthrough`, with six devices, a full-resolution RGB
canvas preview subscriber, GPU SparkleFlinger composition, and GPU spatial
sampling. Servo GPU import was off.

Artifacts:

- `/tmp/hypercolor-mac-baseline-20260518-095240.jsonl`
- `/tmp/hypercolor-daemon-ps-20260518-095443.csv`
- `/tmp/hypercolor-daemon-sample-correct-20260518-095443.txt`

60 REST samples over 65.9 seconds:

- actual FPS: mean 57.5, p95 60.0, max 60.0
- render-loop total: mean 0.63 ms, p95 0.91 ms, max 3.09 ms
- effect rendering: mean 0.41 ms, p95 0.61 ms, max 2.42 ms
- GPU spatial sampling: mean 0.17 ms, p95 0.25 ms, max 0.61 ms
- process CPU from `ps`: mean 24.1%, p95 31.7%, max 34.8%
- resident set from `ps`: mean 302 MiB, p95 308 MiB

Servo cumulative counters during the same window:

- delta frames: 3786
- evaluate scripts: average 2.89 ms per frame, max 24.52 ms
- paint: average 0.63 ms per frame, max 18.92 ms
- readback: average 0.73 ms per frame, max 40.14 ms
- total Servo render: average 4.27 ms per frame, max 64.06 ms

The `sample` trace shows the render thread spending visible CPU time in
`wgpu::Queue::write_texture`, `wgpu_core::Queue::write_texture`,
`_platform_memmove`, `wgpu_hal::metal::Device::create_buffer`, Metal blit setup,
and `copy_buffer_to_texture`. That is the CPU canvas upload into the GPU
composer after Servo readback. The WebSocket preview path is also visible in
`relay_canvas`, RGB preview encoding, tungstenite framing, and `sendto`.

The macOS opportunity is therefore specific: remove Servo `glReadPixels` and
the CPU-to-GPU source texture upload. GPU import will not remove JavaScript
evaluation cost, canvas preview encoding cost, or device output cost.

## 2.1 Post-import Profile

Captured on 2026-05-18 on the same MacBook Pro after implementing the
IOSurface-to-Metal importer and running:

```bash
./scripts/servo-cache-build.sh cargo run \
  -p hypercolor-daemon \
  --bin hypercolor-daemon \
  --profile preview \
  --features "servo wgpu servo-gpu-import" \
  -- \
  --log-level debug \
  --compositor-acceleration-mode gpu \
  --servo-gpu-import-mode auto
```

The active effect was still `Breakthrough`, with six discovered devices, GPU
SparkleFlinger composition, GPU spatial sampling, and Servo GPU import in
`auto` mode. The implementation used the macOS hardware Surfman context, a
generic IOSurface-backed surface, and Metal texture wrapping. The previous
`NoWidgetAttached` present spam was removed because offscreen Surfman generic
surfaces do not present to a widget.

Artifacts:

- `/tmp/hypercolor-mac-import-20260518-115445.jsonl`
- `/tmp/hypercolor-daemon-ps-import-20260518-115445.csv`
- `/tmp/hypercolor-daemon-sample-import-20260518-1854.txt`
- `/tmp/hypercolor-mac-import-light-20260518-115807.json`
- `/tmp/hypercolor-daemon-ps-import-light-20260518-115807.csv`

The first 60-sample REST run is useful for frame-time distribution but perturbs
the daemon because `/status` enumerates CoreAudio devices on each request. It
still showed successful import:

- actual FPS: mean 58.5, p95 60.0
- render-loop total: mean 0.36 ms, p95 0.56 ms, max 1.20 ms
- effect rendering: mean 0.22 ms
- GPU spatial sampling: mean 0.10 ms
- process CPU from `ps`: mean 16.1%, p95 19.8%
- resident set from `ps`: mean 311.5 MiB, max 317.5 MiB
- Servo GPU frames: +3406
- Servo CPU frames: +0
- Servo readback: +0.0 ms
- Servo GPU import failures/fallbacks: +0/+0
- SparkleFlinger source upload skipped: +3408
- Servo GPU import total: +63.138 ms over the window
- Servo GPU import sync: +0.0 ms

The lower-perturbation run sampled process stats every second but hit `/status`
only at the start and end:

- actual FPS at end: 60.0
- frame delta: +3432
- Servo render requests: +3431
- Servo GPU frames: +3431
- Servo CPU frames: +0
- Servo soft stalls: +0
- Servo readback: +0.0 ms
- Servo GPU import failures/fallbacks: +0/+0
- SparkleFlinger source upload skipped: +3432
- Servo GPU import total: +71.609 ms over the window
- Servo GPU import sync: +0.0 ms
- latest render-loop total at end: 0.31 ms
- process CPU from `ps`: mean 13.9%, p95 16.0%
- resident set from `ps`: mean 236.3 MiB, max 251.6 MiB

The `sample` trace from the import run no longer contains Servo `read_pixels`,
`glReadPixels`, or `wgpu::Queue::write_texture` source-upload frames. The
remaining visible samples are Servo/WebRender update work, WebSocket preview
traffic, CoreAudio enumeration during status requests, USB output, and scheduler
idle/parking.

## 3. What We Know

Surfman macOS surfaces are backed by `IOSurfaceRef`.

The relevant Surfman APIs already exist:

- CGL surfaces keep a `framebuffer_object` and `texture_object`.
- `Device::native_surface(&Surface)` returns a native `IOSurface`.
- The returned IOSurface reference is retained before handoff.
- CGL can bind an IOSurface into GL with `CGLTexImageIOSurface2D`.

The reference interop shape from `wgpu-graft` is useful:

1. Create a Metal texture backed by the IOSurface.
2. Wrap that Metal texture as a `wgpu` HAL texture.
3. Normalize to Hypercolor's canonical RGBA/top-left texture if needed.

The reference crate is not ready to vendor. On this machine,
`cargo check -p wgpu-native-texture-interop` fails on macOS with `wgpu 29.0.1`
because it mixes `metal` crate texture types with the newer `objc2_metal` raw
types expected by `wgpu-hal`.

Hypercolor currently pins `wgpu = 29.0.1`. The workspace `wgpu-hal` dependency
also pins `29.0.1`, but only enables the `vulkan` feature today because the
Linux importer is the only HAL user. The macOS importer must enable `metal` for
its interop crate without making non-macOS builds pull in macOS-only code.

The current crate versions have the raw APIs needed for the spike:

- `wgpu_hal::metal::Device::texture_from_raw` accepts
  `Retained<ProtocolObject<dyn MTLTexture>>`, `TextureFormat`,
  `MTLTextureType`, layer count, mip count, and copy extent.
- `wgpu::Device::create_texture_from_hal::<wgpu_hal::api::Metal>` wraps that
  HAL texture back into a safe `wgpu::Texture`.
- `objc2-metal 0.3.2` exposes
  `MTLDevice::newTextureWithDescriptor_iosurface_plane` when the
  `objc2-io-surface` feature is enabled.
- `objc2-io-surface 0.3.2` is already present in `Cargo.lock` and exposes
  both `IOSurface` Objective-C object bindings and `IOSurfaceRef` helpers.

## 4. Non-goals

- Do not add `IOSurface`, `metal`, or `objc2` dependencies to
  `hypercolor-types`.
- Do not remove CPU Servo readback.
- Do not support non-Metal `wgpu` backends on macOS.
- Do not add a separate macOS-only effect output contract.
- Do not vendor `wgpu-graft` unchanged.

## 5. Hard Constraints

### 5.1 macOS requires a Metal-backed SparkleFlinger device

The imported texture must be created from the same Metal device behind
SparkleFlinger's `wgpu::Device`.

Import is unavailable when:

- the active `wgpu` backend is not Metal
- the Metal HAL device cannot be accessed through `wgpu::Device::as_hal`
- an IOSurface-backed Servo surface is unavailable
- Metal cannot create a texture from the IOSurface

### 5.2 Current macOS Servo bootstrap is not enough

Hypercolor currently uses Servo `SoftwareRenderingContext` on non-Windows
targets. A macOS GPU path needs a hardware Surfman context and a generic
IOSurface-backed surface.

The CPU path must remain available because some machines or CI environments may
not have the required GL/Metal interop path.

### 5.3 Pixel format needs explicit normalization

macOS IOSurface and Metal paths commonly expose BGRA-native textures. Hypercolor
canonical surfaces are non-premultiplied sRGB RGBA with top-left origin.

The importer must state exactly where it performs:

- BGRA to RGBA conversion, if needed
- vertical flip, if needed
- sRGB versus unorm interpretation
- alpha representation preservation

### 5.4 Unsafe stays boxed in

The importer will likely require Objective-C messaging, raw `IOSurfaceRef`,
`wgpu-hal`, and raw Metal texture wrapping. Keep that code in one small audited
interop boundary with a safe Hypercolor wrapper.

## 6. Target Architecture

This spec reuses the shared architecture from Spec 56:

```rust
pub enum EffectRenderOutput {
    Cpu(Canvas),
    Gpu(ImportedEffectFrame),
}

pub(crate) enum ProducerFrame {
    Canvas(Canvas),
    Surface(PublishedSurface),
    Gpu(ImportedEffectFrame),
}
```

The macOS-specific frame payload should carry enough native detail to import an
IOSurface while the Servo GL context and Surfman surface lifetime remain valid:

```rust
pub(crate) struct MacosServoNativeFrame {
    pub width: u32,
    pub height: u32,
    pub iosurface: RetainedIosurface,
    pub source_origin: FrameOrigin,
    pub source_format: NativeFrameFormat,
}
```

The exact retained wrapper type is implementation detail. The contract is that
the imported `wgpu::Texture` never outlives the native surface memory it wraps.

## 7. macOS Import Strategy

The first macOS implementation should use IOSurface-to-Metal interop:

1. Render Servo into a Surfman generic surface backed by IOSurface.
2. Acquire the retained native IOSurface from Surfman.
3. Create an `MTLTextureDescriptor` matching width, height, format, usage, and
   texture type.
4. Call Metal `newTextureWithDescriptor:iosurface:plane:` on the Metal device
   backing SparkleFlinger.
5. Wrap the returned `MTLTexture` as a `wgpu` HAL texture.
6. Normalize into an owned `wgpu::Texture` when format or origin differs.
7. Return `ImportedEffectFrame` to the render thread.

Milestone 1 may normalize every frame into a SparkleFlinger-owned texture.
That is still GPU-resident and avoids Servo CPU readback. A later optimization
can skip normalization when SparkleFlinger can consume the native format and
origin directly.

## 8. Synchronization

Milestone 1 may use conservative synchronization:

```text
Servo paint
  -> GL flush
  -> acquire IOSurface
  -> create/wrap Metal texture
  -> optional Metal normalization pass
  -> SparkleFlinger samples texture
```

Measure sync separately:

- `servo_gpu_import_flush_us`
- `servo_gpu_import_wrap_us`
- `servo_gpu_import_normalize_us`
- `servo_gpu_import_total_us`

Milestone 2 should investigate GL fence sync and Metal shared-event or command
buffer synchronization. Do not block the first correct implementation on perfect
cross-API synchronization.

## 9. Configuration and Diagnostics

Use the same user-facing mode as Spec 56:

```toml
[rendering.servo_gpu_import]
mode = "auto" # auto | on | off
```

macOS-specific diagnostics should report:

- active `wgpu` backend
- whether the Servo context is hardware Surfman or software
- whether the Surfman surface exposes IOSurface
- native IOSurface pixel format
- Metal texture creation result
- normalization requirement
- fallback reason

During development, default to `off` or a hidden opt-in. Default to `auto` only
after soak and parity pass on Apple Silicon and at least one Intel Mac if we
still support that target.

## 10. Implementation Waves

### Wave 0: Shared lane from Linux

**Status:** Landed from Spec 56.

Implementation:

- Reuse `GpuRenderDevice`.
- Reuse `EffectRenderOutput::Gpu`.
- Reuse `ProducerFrame::Gpu`.
- Reuse SparkleFlinger direct GPU source binding.
- Reuse fallback metrics and config.

Verify:

- macOS code compiles with the shared lane disabled.
- CPU Servo rendering remains unchanged.

### Wave 1: Metal raw texture compatibility spike

**Files:** new macOS interop crate or tightly scoped macOS interop module.

Implementation:

- Write the minimal IOSurface-to-`wgpu::Texture` wrapper using the exact
  `wgpu-hal` version in Hypercolor.
- Enable `wgpu-hal` `metal` support only for the macOS interop path.
- Depend on `objc2-metal 0.3.2` with `objc2-io-surface` support and
  `objc2-io-surface 0.3.2`.
- Use `objc2_metal` types consistently. Do not mix incompatible `metal` crate
  texture wrappers unless the conversion is proven.
- Create an IOSurface fixture independent of Servo.
- Fill the fixture with deterministic pixels, create an `MTLTexture` from it,
  wrap it with `wgpu_hal::metal::Device::texture_from_raw`, then import it with
  `wgpu::Device::create_texture_from_hal::<wgpu_hal::api::Metal>`.

Verify:

- `cargo check` passes on macOS.
- A synthetic IOSurface imports into a `wgpu::Texture`.
- A readback from the imported texture matches expected pixels.
- The same crate compiles to a stub on non-macOS targets.

### Wave 2: macOS Servo hardware context

**Files:** `crates/hypercolor-core/src/effect/servo_bootstrap.rs`,
new macOS Servo context module.

Implementation:

- Add a macOS hardware Surfman generic-surface context.
- Keep `read_to_image()` for CPU fallback.
- Expose native IOSurface acquisition for the Servo worker.
- Preserve current resize semantics.

Verify:

- Existing Servo CPU tests still pass.
- A Servo fixture renders through the macOS hardware context.
- CPU fallback readback from the same context matches current behavior.
- Native IOSurface acquisition reports width, height, pixel format, origin, and
  surface identity in diagnostics.

### Wave 3: IOSurface importer integration

**Files:** Servo worker/session modules, macOS interop module.

Implementation:

- Import the current Servo IOSurface into the shared Metal `wgpu::Device`.
- Normalize format and origin when required.
- Return `ImportedEffectFrame`.
- Fall back to CPU on capability or import failure in `auto` mode.

Verify:

- A deterministic Servo fixture emits `ImportedEffectFrame`.
- Pixel parity matches CPU readback.
- Successful GPU import does not call Servo `glReadPixels`.
- `producer_gpu_frames_total` increments for Servo producers.
- Render-thread samples no longer show Servo source upload as a dominant
  `queue.write_texture` cost.

### Wave 4: macOS benchmarks and soak

**Files:** benchmarks, perf docs, diagnostics.

Implementation:

- Compare CPU readback and IOSurface import on the same effect.
- Track import wrap, normalization, sync, and total frame time.
- Run mixed Servo/display/preview scenes.

Verify:

- `servo_readback_us == 0` on successful GPU import.
- p95 improves or the macOS path remains opt-in.
- No IOSurface, Metal texture, or Servo session leak appears in soak.

## 11. Acceptance Criteria

macOS Servo GPU import is ready to enable by default when:

1. CPU fallback works.
2. Metal raw texture wrapping compiles against Hypercolor's pinned `wgpu`.
3. Servo can render into an IOSurface-backed hardware context.
4. Imported frames reach SparkleFlinger without CPU readback.
5. Pixel parity passes against CPU readback.
6. Diagnostics identify every fallback reason.
7. Soak shows no IOSurface or Metal texture leak.
8. Baseline comparison shows lower readback/upload CPU cost, or the macOS path
   remains opt-in.

## 12. Recommendation

Build the IOSurface-to-`wgpu` spike first, independent of Servo.

That kills the highest-risk unknown with the smallest blast radius: whether
Hypercolor's pinned `wgpu-hal 29.0.1` can safely wrap an IOSurface-backed
`objc2_metal` texture and round-trip pixels through `wgpu`. Once that is green,
thread Servo through a hardware Surfman context and reuse the Linux GPU producer
lane.
