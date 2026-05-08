# 57 - macOS Servo GPU Surface Interop

**Status:** Planned after Linux proof
**Author:** Nova
**Date:** 2026-05-08
**Crates:** `hypercolor-core`, `hypercolor-daemon`, optional interop crate
**Related:** Specs 48, 56; `docs/design/34-servo-perf-and-crash-isolation.md`,
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

Linux should prove the shared Hypercolor architecture first: shared GPU device,
GPU effect output, `ProducerFrame::Gpu`, SparkleFlinger direct source binding,
fallback diagnostics, and parity tests. This spec defines the macOS importer and
platform work needed after that shared lane exists.

## 2. What We Know

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

## 3. Non-goals

- Do not implement macOS before Linux proves the shared GPU surface lane.
- Do not add `IOSurface`, `metal`, or `objc2` dependencies to
  `hypercolor-types`.
- Do not remove CPU Servo readback.
- Do not support non-Metal `wgpu` backends on macOS.
- Do not add a separate macOS-only effect output contract.
- Do not vendor `wgpu-graft` unchanged.

## 4. Hard Constraints

### 4.1 macOS requires a Metal-backed SparkleFlinger device

The imported texture must be created from the same Metal device behind
SparkleFlinger's `wgpu::Device`.

Import is unavailable when:

- the active `wgpu` backend is not Metal
- the Metal HAL device cannot be accessed
- an IOSurface-backed Servo surface is unavailable
- Metal cannot create a texture from the IOSurface

### 4.2 Current macOS Servo bootstrap is not enough

Hypercolor currently uses Servo `SoftwareRenderingContext` on non-Windows
targets. A macOS GPU path needs a hardware Surfman context and a generic
IOSurface-backed surface.

The CPU path must remain available because some machines or CI environments may
not have the required GL/Metal interop path.

### 4.3 Pixel format needs explicit normalization

macOS IOSurface and Metal paths commonly expose BGRA-native textures. Hypercolor
canonical surfaces are non-premultiplied sRGB RGBA with top-left origin.

The importer must state exactly where it performs:

- BGRA to RGBA conversion, if needed
- vertical flip, if needed
- sRGB versus unorm interpretation
- alpha representation preservation

### 4.4 Unsafe stays boxed in

The importer will likely require Objective-C messaging, raw `IOSurfaceRef`,
`wgpu-hal`, and raw Metal texture wrapping. Keep that code in one small audited
interop boundary with a safe Hypercolor wrapper.

## 5. Target Architecture

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

## 6. macOS Import Strategy

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

## 7. Synchronization

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

## 8. Configuration and Diagnostics

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

## 9. Implementation Waves

### Wave 0: Shared lane from Linux

**Depends on:** Spec 56 Waves 1, 4, 5, and 6.

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

**Files:** optional interop crate or macOS interop module.

Implementation:

- Write the minimal IOSurface-to-`wgpu::Texture` wrapper using the exact
  `wgpu-hal` version in Hypercolor.
- Use `objc2_metal` types consistently. Do not mix incompatible `metal` crate
  texture wrappers unless the conversion is proven.
- Add a tiny IOSurface fixture independent of Servo.

Verify:

- `cargo check` passes on macOS.
- A synthetic IOSurface imports into a `wgpu::Texture`.
- A readback from the imported texture matches expected pixels.

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

## 10. Acceptance Criteria

macOS Servo GPU import is ready to enable by default when:

1. Linux shared GPU surface lane has landed.
2. CPU fallback works.
3. Metal raw texture wrapping compiles against Hypercolor's pinned `wgpu`.
4. Servo can render into an IOSurface-backed hardware context.
5. Imported frames reach SparkleFlinger without CPU readback.
6. Pixel parity passes against CPU readback.
7. Diagnostics identify every fallback reason.
8. Soak shows no IOSurface or Metal texture leak.

## 11. Recommendation

Do macOS second, after Linux proves the shared render-pipeline shape.

The likely winning path is IOSurface-to-Metal, but the first macOS task should
be a focused raw-texture compatibility spike. The reference implementation
already failed there once, and we should kill that risk before threading Servo
through it.
