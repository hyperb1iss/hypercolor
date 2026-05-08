# 58 - Windows Servo GPU Surface Interop

**Status:** Research-backed plan, highest risk platform
**Author:** Nova
**Date:** 2026-05-08
**Crates:** `hypercolor-core`, `hypercolor-daemon`, optional interop crate
**Related:** Specs 32, 48, 56, 57;
`docs/design/34-servo-perf-and-crash-isolation.md`

## 1. Goal

Define the Windows path for Servo HTML effects to publish GPU-resident frames
into SparkleFlinger without a full-frame CPU readback.

Windows is not the first implementation target. It should follow Linux and
macOS because its path combines Servo, ANGLE, D3D11 shared resources, Vulkan or
DX12 import, and cross-API synchronization.

The likely path is:

```text
Servo ANGLE framebuffer
  -> D3D11 texture backing ANGLE/EGL surface
  -> shared handle
  -> Vulkan or DX12 import into wgpu
  -> SparkleFlinger source texture
```

## 2. What We Know

Hypercolor already treats Windows specially:

- `servo_bootstrap.rs` creates a hidden Tao window.
- It builds Servo `WindowRenderingContext`.
- It uses Servo `OffscreenRenderingContext`.
- Servo is compiled with `no-wgl`, which routes through ANGLE instead of WGL.

Surfman Windows code exposes D3D11-backed surfaces in its WGL path:

- generic surfaces create `ID3D11Texture2D`
- textures use `D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX`
- DXGI share handles are available internally
- Surfman comments still question keyed mutex handling in at least one path

The reference `wgpu-graft` tree contains two Windows ideas:

1. ANGLE D3D11 KMT handle import through
   `EGL_ANGLE_query_surface_pointer`.
2. Non-ANGLE GL external-memory import through
   `GL_EXT_memory_object_win32`.

The second path is less relevant to Hypercolor's current Windows Servo stack
because Servo uses ANGLE there. The first path is the one to investigate.

## 3. Non-goals

- Do not implement Windows before Linux and macOS prove the shared pipeline.
- Do not remove the hidden-window Servo path unless a replacement is proven.
- Do not require DX12 if Vulkan import is the only reliable ANGLE path.
- Do not promise Windows GPU import in `auto` mode until real hardware is
  tested.
- Do not bypass CPU fallback.
- Do not hide synchronization uncertainty behind broad retries.

## 4. Hard Constraints

### 4.1 ANGLE is the current Servo GL provider

On Windows, Servo uses ANGLE. That means the rendered frame is backed by D3D11,
not native OpenGL memory.

Any Windows plan that assumes `GL_EXT_memory_object_win32` must first prove the
active Servo context is not ANGLE. With current Hypercolor settings, assume
ANGLE.

### 4.2 Vulkan may be required even on Windows

The ANGLE KMT path imports the D3D11 backing texture into Vulkan using
`VK_EXTERNAL_MEMORY_HANDLE_TYPE_D3D11_TEXTURE_KMT_BIT`. That means
SparkleFlinger may need a Vulkan-backed `wgpu::Device` on Windows for Servo GPU
import, even though DX12 is often the natural Windows backend.

This has product consequences:

- GPU compositor backend selection must be explicit and observable.
- `auto` mode may choose DX12 for compositor stability and disable Servo GPU
  import.
- `on` mode may force Vulkan or fail with a precise diagnostic.

### 4.3 Shared handle flavor matters

ANGLE surfaces may expose legacy KMT handles, not NT handles. Many higher-level
D3D sharing helpers expect NT handles. The importer must detect and document the
handle type it uses.

Do not assume `wgpu_hal::texture_from_d3d11_shared_handle` is sufficient without
checking whether it accepts the actual ANGLE handle type.

### 4.4 Synchronization is the hardest part

ANGLE D3D11 resources may use keyed mutex synchronization. Vulkan import of the
KMT handle does not automatically prove correct synchronization with ANGLE's GL
timeline.

Milestone 1 may use conservative `glFlush` plus retained-frame fallback, but the
spec-compliant implementation must measure and report sync behavior. If the
texture tears, samples stale pixels, or intermittently fails, the path remains
opt-in.

### 4.5 Unsafe and Win32 handles stay boxed in

The importer will need raw Win32 handles, ANGLE/EGL function pointers, Vulkan or
DX12 HAL access, and possibly keyed mutex handling. Keep this in one small
interop crate or module with an audited safe wrapper.

## 5. Target Architecture

Windows reuses the shared architecture from Spec 56:

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

The Windows native frame descriptor should be explicit about backend and handle
type:

```rust
pub(crate) enum WindowsServoNativeFrame {
    AngleD3d11Kmt {
        width: u32,
        height: u32,
        share_handle: RawWin32Handle,
        format: NativeFrameFormat,
    },
    GlMemoryObjectWin32 {
        width: u32,
        height: u32,
        framebuffer_id: u32,
    },
}
```

The second variant is optional research scaffolding. The current production
candidate is `AngleD3d11Kmt`.

## 6. Candidate Import Paths

### 6.1 Path A: ANGLE D3D11 KMT to Vulkan

Flow:

1. Ensure the ANGLE EGL context is current on the Servo worker thread.
2. Load `eglQuerySurfacePointerANGLE` from `libEGL.dll`.
3. Query `EGL_D3D_TEXTURE_2D_SHARE_HANDLE_ANGLE`.
4. Create a Vulkan image using the D3D11 KMT external handle type.
5. Allocate Vulkan memory by importing the KMT handle.
6. Wrap the Vulkan image as a `wgpu::Texture`.
7. Normalize BGRA/top-left if needed.
8. Return `ImportedEffectFrame`.

Required capabilities:

- `wgpu` backend is Vulkan
- `VK_KHR_external_memory_win32`
- ANGLE exposes `EGL_ANGLE_query_surface_pointer`
- `eglQuerySurfacePointerANGLE` returns a non-null D3D11 texture handle
- GPU/driver supports D3D11 KMT external memory import

Primary risks:

- synchronization correctness
- handle lifetime
- backend selection conflicts with DX12
- multi-GPU mismatches between ANGLE D3D11 device and Vulkan device

### 6.2 Path B: non-ANGLE GL memory object to Vulkan or DX12

Flow:

1. Use native WGL/OpenGL instead of ANGLE.
2. Create exportable Vulkan or DX12 texture memory.
3. Import that memory into GL through `GL_EXT_memory_object_win32`.
4. Blit Servo FBO into the shared GL texture.
5. Wrap the Vulkan or DX12 image as `wgpu::Texture`.

This path is lower priority because current Hypercolor Windows Servo explicitly
uses ANGLE. Keep it as a research fallback only if ANGLE KMT import fails.

## 7. Configuration and Backend Selection

Use the shared mode:

```toml
[rendering.servo_gpu_import]
mode = "auto" # auto | on | off
```

Windows needs additional diagnostic fields:

- `servo_gpu_import_backend = "angle_d3d11_kmt_to_vulkan"`
- active `wgpu` backend
- ANGLE DLL presence and version if available
- `eglQuerySurfacePointerANGLE` availability
- share handle type
- Vulkan external memory support
- synchronization mode
- fallback reason

Potential backend policy:

- `off`: CPU path only.
- `auto`: Use the normal compositor backend. If it is not compatible with
  Servo GPU import, use CPU path and report why.
- `on`: Require a compatible backend. If Vulkan is required, request Vulkan
  explicitly and fail Servo GPU import when unavailable.

Do not silently switch the entire compositor backend in `auto` without
diagnostics.

## 8. Synchronization Strategy

Milestone 1 correctness path:

```text
Servo paint
  -> ANGLE/EGL current surface
  -> glFlush
  -> query D3D11 KMT handle
  -> Vulkan import
  -> SparkleFlinger samples
```

Metrics:

- `servo_gpu_import_query_handle_us`
- `servo_gpu_import_wrap_us`
- `servo_gpu_import_sync_us`
- `servo_gpu_import_total_us`
- `servo_gpu_import_stale_frame_total`
- `servo_gpu_import_tearing_detected_total`

Milestone 2 research:

- keyed mutex acquisition and release
- Vulkan external semaphores
- ANGLE fence objects
- cross-adapter rejection

The path cannot become default until synchronization is proven under animation,
resize, and mixed device load.

## 9. Implementation Waves

### Wave 0: Shared lane from Linux and macOS

**Depends on:** Specs 56 and 57 shared output work.

Implementation:

- Reuse `GpuRenderDevice`.
- Reuse `EffectRenderOutput::Gpu`.
- Reuse `ProducerFrame::Gpu`.
- Reuse SparkleFlinger direct source binding.
- Reuse diagnostics and fallback policy.

Verify:

- Windows builds with Servo GPU import disabled.
- Existing Windows CPU Servo path still works.

### Wave 1: Windows capability probe

**Files:** Windows interop module or crate, diagnostics.

Implementation:

- Probe `wgpu` backend.
- Probe Vulkan external memory support.
- Probe ANGLE EGL functions.
- Probe share-handle query availability while the Servo context is current.
- Return structured failure reasons.

Verify:

- Probe succeeds or fails deterministically on Windows.
- Diagnostics distinguish backend mismatch, missing ANGLE function, null handle,
  and missing Vulkan support.

### Wave 2: ANGLE handle import spike

**Files:** Windows interop module or crate.

Implementation:

- Import a synthetic ANGLE or D3D11 KMT texture into Vulkan-backed `wgpu`.
- Normalize format and origin when required.
- Avoid Servo until handle lifetime and import semantics are understood.

Verify:

- Imported texture readback matches expected pixels.
- Handle cleanup is correct.
- Failure paths do not leak Vulkan memory or Win32 handles.

### Wave 3: Servo integration

**Files:** `crates/hypercolor-core/src/effect/servo_bootstrap.rs`,
Servo worker/session modules, Windows interop module.

Implementation:

- Query the current ANGLE surface after Servo paint.
- Import the backing texture into `wgpu`.
- Return `ImportedEffectFrame`.
- Fall back to CPU in `auto`.

Verify:

- A deterministic Servo fixture emits `ImportedEffectFrame`.
- Pixel parity matches CPU readback.
- Successful GPU import avoids Servo `glReadPixels`.

### Wave 4: Synchronization hardening

**Files:** Windows interop module, metrics, tests.

Implementation:

- Add stale-frame detection fixtures.
- Test resize and rapid animation.
- Investigate keyed mutex or explicit fence synchronization.
- Reject unsupported multi-GPU configurations.

Verify:

- No tearing or stale frame reuse under stress.
- Sync metrics are visible.
- Unsupported sync configurations fall back clearly.

### Wave 5: Benchmarks and soak

**Files:** benchmarks, perf docs.

Implementation:

- Compare CPU readback versus ANGLE import.
- Measure p50, p95, p99, import overhead, sync overhead, and fallback rate.
- Soak mixed Servo/display/preview scenes.

Verify:

- `servo_readback_us == 0` on successful GPU import.
- p95 improves or path stays opt-in.
- No D3D11, Vulkan, Win32 handle, or Servo session leaks.

## 10. Acceptance Criteria

Windows Servo GPU import is ready to enable by default only when:

1. Linux and macOS shared lanes have landed.
2. CPU fallback works.
3. ANGLE capability probing is precise.
4. Imported frames reach SparkleFlinger without CPU readback.
5. Pixel parity passes against CPU readback.
6. Synchronization is proven under animation and resize.
7. Backend selection is explicit and observable.
8. Soak shows no handle or GPU memory leaks.

## 11. Recommendation

Treat Windows as a dedicated research milestone, not a simple port.

The likely first real path is ANGLE D3D11 KMT to Vulkan because it matches
Servo's current Windows backend. Keep `auto` conservative. If the path works
only with forced Vulkan and strict single-GPU assumptions, it should remain
opt-in until the product tradeoff is obvious.
