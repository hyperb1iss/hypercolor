# 58 - Windows Servo GPU Surface Interop

**Status:** Implemented and Windows-fixture verified; cross-vendor soak and performance characterization ongoing
**Author:** Nova
**Date:** 2026-05-22 (FBO revision 2026-06-12)
**Crates:** `hypercolor-core`, `hypercolor-daemon`, `hypercolor-windows-gpu-interop`
**Related:** Specs 32, 48, 56, 57; `docs/design/45-graphics-pipeline-unification-plan.md`

## 0.0 FBO Revision (2026-06-12)

After Linux and macOS moved to the FBO-based GPU import architecture, the
Windows path was rebuilt to match. The architecture below this note is
preserved for history; the following supersedes the producer design in
Sections 5.2, 7, and 8:

- Servo renders into **one stable GL framebuffer** (texture color attachment
  plus DEPTH24_STENCIL8 renderbuffer) owned by
  `WindowsAngleRenderingContext`, never directly into a shared surface.
  WebRender keeps a stable render-target identity across frames; the context
  surface is a plain generic pbuffer that is never rebound.
- `present()` publishes by **blitting the render FBO into the next available
  ring slot** — a D3D11 `SHARED | SHARED_NTHANDLE` texture bound into ANGLE
  as a GL texture via Surfman `create_surface_texture_from_texture` and
  wrapped in a slot framebuffer — then inserts a GL fence and flushes. No
  `glFinish`.
- `native_frame()` idempotently returns the newest slot whose fence has
  signaled, carrying a monotonic `content_generation`. Repeated calls without
  a new publish return the same slot and generation. The first call eagerly
  publishes so warmup needs no special casing.
- The importer caches one wgpu texture per NT handle (ring depth 3) and
  stamps each frame's `storage_id` with the producer `content_generation`,
  matching macOS semantics. The previous design's per-handle cached
  `storage_id` made repeated publishes look unchanged to SparkleFlinger's
  upload-skip logic, which the multi-zone / multi-layer stack relies on.
- mozangle quirks found on hardware: gleam's GLES `read_buffer` is an
  unconditional panic (the slot framebuffer's read buffer already defaults to
  `COLOR_ATTACHMENT0`), and `glClientWaitSync` does not reliably block on a
  nonzero timeout, so the bounded fence wait polls with an explicit deadline
  (50ms cap, 500µs interval).
- Surfman is now upstream 0.12.2 from crates.io; the `hyperb1iss/surfman`
  fork described in Section 3.5 has been dropped. Both
  `create_surface_from_texture` and `create_surface_texture_from_texture`
  exist upstream as `pub unsafe fn`.

Verification receipts on the Windows workstation (2026-06-12):

- `HYPERCOLOR_RUN_WINDOWS_D3D11_FIXTURE=1 cargo test -p
  hypercolor-windows-gpu-interop imports_synthetic_d3d11_shared_texture_into_wgpu_texture`
  passed with `1 passed`.
- `HYPERCOLOR_RUN_WINDOWS_ANGLE_CONTEXT_FIXTURE=1 cargo test -p
  hypercolor-windows-gpu-interop --features servo-context
  angle_context_renders_into_importable_d3d11_ring` passed with `1 passed`,
  covering five publish generations, ring wrap-around, idempotent
  `native_frame`, and pixel-verified imports.

## 0. Research Pass Summary

This revision replaces the earlier draft after a verification pass against the
pinned Surfman fork, `wgpu-hal 29.0.1` source, ANGLE's D3D11 backend, Servo
upstream, and external prior art. The earlier draft's first-order unknown is now
resolved, which collapses a two-branch decision tree into a single path.

What changed:

- The shared-handle flavor is **settled, not unknown**. ANGLE's own surface
  exposes only a legacy/KMT handle. The owned-NT-handle texture path is the only
  importable path, so it is now the plan, not a fallback.
- The Surfman work is mostly already done. The pinned fork already exposes the
  ANGLE client-buffer surface API, DXGI-adapter pinning, and `native_device()`.
  The earlier "Surfman fork extension" wave nearly disappears.
- DX12-default risk was overstated. `wgpu` selects Vulkan first on Windows for a
  typical discrete GPU, so a compatible compositor device is the common case.
- Synchronization is sharpened. A keyed mutex cannot synchronize a Vulkan
  consumer. Phase 1 is a texture ring plus `glFinish`; Phase 2 is a shared
  timeline fence.
- A real `wgpu 29.0.1` footgun is recorded: `create_texture_from_hal` wraps
  imports in a content-discarding state.

### 0.1 Implementation Closeout

As of 2026-05-22, the Windows Servo GPU import path is implemented in
`hypercolor-windows-gpu-interop`, wired through `hypercolor-core`, exposed by the
daemon diagnostics API, and surfaced in the UI renderer diagnostics.

The implemented path matches the architecture below:

- Hypercolor creates a ring of owned D3D11 textures with
  `D3D11_RESOURCE_MISC_SHARED | D3D11_RESOURCE_MISC_SHARED_NTHANDLE`.
- Surfman/ANGLE renders Servo directly into the active ring slot through the
  ANGLE D3D texture client-buffer surface path.
- The producer synchronizes with `glFinish`, then the NT shared handle is
  imported into the Vulkan-backed `wgpu` device.
- SparkleFlinger consumes the imported GPU texture while CPU readback remains
  the fallback path for unsupported or failed imports.

Verification receipts on the Windows workstation:

- `cargo check --locked -p hypercolor-windows-gpu-interop --features servo-context`
  passed from the Visual Studio developer shell.
- `HYPERCOLOR_RUN_WINDOWS_D3D11_FIXTURE=1 cargo test --locked -p
  hypercolor-windows-gpu-interop
  imports_synthetic_d3d11_shared_texture_into_wgpu_texture -- --nocapture`
  passed with `1 passed`.
- `HYPERCOLOR_RUN_WINDOWS_ANGLE_CONTEXT_FIXTURE=1 cargo test --locked -p
  hypercolor-windows-gpu-interop --features servo-context
  angle_context_renders_into_importable_d3d11_ring -- --nocapture` passed with
  `1 passed`.
- `cargo test --locked -p hypercolor-core --no-default-features --features
  servo-gpu-import gpu_import_metrics_accumulate_timings_and_fallback_reason`
  passed with `1 passed`.
- `cargo check --locked -p hypercolor-daemon --no-default-features --features
  wgpu --lib` passed; the no-Servo build emits only the expected dead-code
  warning for the Servo-import-only Vulkan preference variant.

The synthetic fixture proves that `wgpu 29.0.1` preserves imported D3D11 texture
content on this machine. That closes the local hardware spike, but not
cross-vendor validation: NVIDIA, AMD, Intel, hybrid-GPU, and broken-Vulkan-ICD
coverage still belongs to soak.

## 1. Goal

Bring Servo GPU readback elimination to Windows by publishing Servo-rendered
HTML effect frames as GPU-resident `wgpu::Texture`s instead of reading the
framebuffer back through CPU memory.

Linux and macOS already prove the shared product path:

```text
Servo render target
  -> native GPU surface
  -> wgpu texture import/wrap
  -> EffectRenderOutput::Gpu
  -> ProducerFrame::Gpu
  -> SparkleFlinger GPU source
```

Windows joins the same lane, but its native surface model is different. Servo
renders through ANGLE on Windows, and ANGLE backs its EGL surface with a D3D11
texture. Hypercolor cannot import ANGLE's own surface texture (Section 2), so the
Windows path renders Servo into a Hypercolor-owned D3D11 texture:

```text
Hypercolor-owned D3D11 texture (D3D11_RESOURCE_MISC_SHARED_NTHANDLE)
  -> ANGLE renders Servo into it (EGL client-buffer pbuffer surface)
  -> NT shared handle (IDXGIResource1::CreateSharedHandle)
  -> Vulkan external-memory import (wgpu-hal texture_from_d3d11_shared_handle)
  -> wgpu::Texture on the SparkleFlinger device
  -> EffectRenderOutput::Gpu -> ProducerFrame::Gpu
  -> SparkleFlinger GPU source
```

The implementation must preserve CPU fallback, precise diagnostics, and the
existing `auto | on | off` Servo GPU import policy.

## 2. Executive Decision

Render Servo into a **Hypercolor-owned D3D11 texture** carrying an NT shared
handle, then import that handle into a **Vulkan-backed** `wgpu` device via
`wgpu-hal`'s `texture_from_d3d11_shared_handle`.

This is one path, not a decision tree. The earlier draft treated the
shared-handle flavor as a first-order unknown, with a fork between importing
ANGLE's surface directly and creating an owned texture. Research closed that
question:

- ANGLE's own pbuffer/swap-chain surface exposes only a **legacy (KMT) share
  handle**. ANGLE's D3D11 backend (`SwapChain11.cpp`) creates its offscreen
  texture with `D3D11_RESOURCE_MISC_SHARED` (or `..._SHARED_KEYEDMUTEX`) and
  obtains the handle via `IDXGIResource::GetSharedHandle`. It never uses
  `D3D11_RESOURCE_MISC_SHARED_NTHANDLE`.
- `wgpu-hal 29.0.1`'s `texture_from_d3d11_shared_handle` imports as
  `VK_EXTERNAL_MEMORY_HANDLE_TYPE_D3D11_TEXTURE_BIT`, which requires an **NT
  handle**. A legacy/KMT handle does not match it, and `wgpu-hal` exposes no KMT
  importer.
- Therefore the only D3D11 handle Hypercolor can import is one it mints itself,
  from a texture it created with `D3D11_RESOURCE_MISC_SHARED_NTHANDLE` via
  `IDXGIResource1::CreateSharedHandle`.

Directly importing ANGLE's internal surface is rejected. Writing a custom KMT
Vulkan importer is rejected. The owned-texture path is the implementation.

This is not speculative. The Slint team shipped the same family of technique for
embedding Servo on Windows in May 2026: a D3D11 texture with NT-handle flags, an
ANGLE client-buffer surface path, and import of the NT handle into the host
renderer. Their integration blits Servo's framebuffer into the shared texture;
Hypercolor's plan goes one step closer to the metal by binding the owned texture
as Servo's render surface so that no per-frame blit is required. Wave 4 must
verify that direct rendering works with Servo's `RenderingContext`.

Reference:
<https://slint.dev/blog/servo-with-slint-update>

The remaining first-order risk is no longer a fork in the design. It is the
synthetic-import hardware spike (Wave 3): proving that a Hypercolor-created
NT-handle D3D11 texture round-trips pixels through `wgpu` Vulkan on real Windows
GPUs, including the `create_texture_from_hal` content-state footgun (Section 4.6).

## 3. Repo Truth

### 3.1 Current Windows Servo bootstrap

`crates/hypercolor-core/src/effect/servo_bootstrap.rs` has a Windows path that
creates a hidden Tao window, obtains raw display/window handles, builds Servo's
`WindowRenderingContext`, and derives an `OffscreenRenderingContext` from it.

This path is the CPU-readback path. It stays as the non-import fallback. It is
not the GPU path: `OffscreenRenderingContext` blits Servo's output into a parent
GL framebuffer and exposes no shareable GPU texture, and the path is tied to an
OS window. The current bootstrap also carries a known live defect noted in its
own comment: WebGL effects panic during ANGLE surface import on this path. That
is tracked separately (Section 12) and must not block GPU import for non-WebGL
effects.

The GPU path adds a separate, custom Surfman-ANGLE rendering context
(Section 5.2), selected when `servo_gpu_import_should_attempt()` is true,
mirroring how Linux and macOS branch in `bootstrap_rendering_context`.

### 3.2 Current cfg split is not Windows-safe

Several Servo GPU import paths use `not(target_os = "macos")` to mean "Linux".
That is correct only while Windows import is absent. `worker.rs` is the clearest
case: its `#[cfg(all(feature = "servo-gpu-import", not(target_os = "macos")))]`
importer block hardcodes `hypercolor_linux_gpu_interop::` types.

Before adding Windows support, split these paths explicitly:

```rust
#[cfg(all(feature = "servo-gpu-import", target_os = "linux"))]
#[cfg(all(feature = "servo-gpu-import", target_os = "macos"))]
#[cfg(all(feature = "servo-gpu-import", target_os = "windows"))]
```

Known surfaces:

- `crates/hypercolor-core/src/effect/traits.rs`
- `crates/hypercolor-core/src/effect/servo/worker.rs`
- `crates/hypercolor-core/src/effect/servo/telemetry.rs`
- `crates/hypercolor-core/src/effect/servo_bootstrap.rs`
- `crates/hypercolor-daemon/src/render_thread/gpu_device.rs`
- `crates/hypercolor-daemon/src/render_thread/sparkleflinger/gpu.rs`

The Windows bootstrap cfg must also be reshaped so that
`cfg(all(windows, feature = "servo-gpu-import"))` can host the import path while
`cfg(all(windows, not(feature = "servo-gpu-import")))` keeps today's
`WindowRenderingContext` path.

### 3.3 Current backend policy excludes Windows

`GpuRenderDeviceInfo::servo_gpu_import_backend_compatible()` returns Linux ->
Vulkan, macOS -> Metal, other -> false. A legacy
`linux_servo_gpu_import_backend_compatible()` method also exists and should be
deprecated in favor of the generic name (Section 9).

Windows becomes compatible only when the compositor `wgpu` device is
Vulkan-backed and exposes `Features::VULKAN_EXTERNAL_MEMORY_WIN32`.

### 3.4 `wgpu-hal` already has the Vulkan import helper

`wgpu-hal 29.0.1` (verified in `wgpu-hal/src/vulkan/device.rs`) provides:

```rust
#[cfg(windows)]
pub unsafe fn texture_from_d3d11_shared_handle(
    &self,
    d3d11_shared_handle: windows::Win32::Foundation::HANDLE,
    desc: &crate::TextureDescriptor,
) -> Result<super::Texture, crate::DeviceError>
```

Verified behavior:

- Imports with `vk::ExternalMemoryHandleTypeFlags::D3D11_TEXTURE` (the NT-handle
  type), not the KMT variant.
- Gated on `wgt::Features::VULKAN_EXTERNAL_MEMORY_WIN32`; returns an error if the
  Vulkan device lacks `VK_KHR_external_memory_win32`.
- Uses a dedicated allocation (`VkMemoryDedicatedAllocateInfo`) and binds the
  imported memory to a freshly created image.
- `VULKAN_EXTERNAL_MEMORY_WIN32` is a public `wgpu::Features` flag, so the daemon
  detects support via `adapter.features()` and requests it in
  `DeviceDescriptor.required_features`.

The workspace pins `wgpu-hal = "29.0.1"` with only the `vulkan` feature. The new
crate keeps `vulkan` (no `dx12` feature needed; see Section 6).

### 3.5 The Surfman fork already has the ANGLE client-buffer primitives

The workspace pins `surfman` to `hyperb1iss/surfman` rev `e75590f` (`[patch]` in
the root `Cargo.toml`), which is approximately upstream Surfman 0.11 plus one
swap-chain fix. Its Windows ANGLE backend
(`src/platform/windows/angle/surface.rs`, `device.rs`) already provides:

- `Device::create_surface_from_texture(&context, &size, ComPtr<ID3D11Texture2D>)`
  -- creates an ANGLE EGL pbuffer surface from a caller-owned D3D11 texture via
  `eglCreatePbufferFromClientBuffer(EGL_D3D_TEXTURE_ANGLE, ...)`. Servo renders
  directly into the supplied texture; no blit is required.
- `Device::create_surface_texture_from_texture(...)` -- the read-side
  equivalent.
- `Adapter::from_dxgi_adapter(ComPtr<IDXGIAdapter>)` plus
  `Connection::create_device(&adapter)` -- lets Hypercolor pin ANGLE to the DXGI
  adapter whose LUID matches the Vulkan `wgpu` device.
- `Device::native_device()` -- exposes the resulting ANGLE `ID3D11Device` so
  Hypercolor can create the shared texture on the exact same D3D11 device.
- `Connection::create_device_from_native_device(NativeDevice { ... })` -- can
  wrap a pre-existing ANGLE native device, but `NativeDevice` includes both an
  `EGLDisplay` and an `ID3D11Device`; it is not just a raw D3D11 device pointer.
- A `Synchronization` enum (`KeyedMutex` / `GLFinish` / `None`). For a
  caller-owned texture without the keyed-mutex flag, Surfman uses
  `Synchronization::None` and leaves synchronization to the caller.

The Surfman work the earlier draft scoped as a "fork extension" wave is
therefore essentially already present. `Device::native_device()` is public in the
pinned fork, so Hypercolor can recover the ANGLE `ID3D11Device` after creating a
Surfman device on the selected DXGI adapter. No new pbuffer or shared-handle
machinery is required.

Upstream Surfman 0.12.1 (2026-05-08) added a public `Surface::share_handle()`
accessor and already contains the swap-chain fix the fork carries. The
owned-texture path does not need `share_handle()` -- Hypercolor mints its own NT
handle -- but a future Servo bump that pins Surfman 0.12.x would let Hypercolor
drop the fork entirely. Track that as cleanup, not a blocker for this spec.

### 3.6 Servo and ANGLE facts

- `servo` is a published crate, `servo = "0.1.0"` from crates.io, along with the
  `servo-base`, `servo-paint-api`, `webrender_api`, and related component
  crates. WebRender 0.68 still renders through OpenGL; there is no wgpu WebRender
  backend and none is imminent.
- The Windows `servo` dependency enables the `no-wgl` feature, which forces
  Servo/Surfman onto the ANGLE backend rather than native WGL. `mozangle 0.5.5`
  is in the lockfile. The ANGLE-on-Windows assumption is confirmed and stable.
- Servo's `RenderingContext` is a GL-centric trait
  (`prepare_for_rendering`, `read_to_image`, `size`, `resize`, `present`,
  `make_current`, `gleam_gl_api`, `glow_gl_api`, `create_texture`,
  `destroy_texture`, `connection`, plus defaulted `size2d` / `refresh_driver`).
  It has no GPU-texture-export method. The Windows GPU path implements this
  trait with a custom Surfman context (Section 5.2), exactly as macOS does with
  `MacosHardwareRenderingContext`.

### 3.7 macOS interop crate is the structural template

`hypercolor-macos-gpu-interop` is the closest analog and the template for the
new crate: an audited `unsafe` boundary with `unsafe_code = "allow"` and
`undocumented_unsafe_blocks = "deny"` / `unwrap_used = "deny"`, a `windows`/`macos`
module plus a non-platform `stubs` module, a `servo_context` module implementing
Servo's `RenderingContext`, and the canonical `ImportedEffectFrame` shape.

## 4. Verified External Facts

### 4.1 ANGLE D3D client-buffer extension

`EGL_ANGLE_d3d_texture_client_buffer` defines `EGL_D3D_TEXTURE_ANGLE`.
`eglCreatePbufferFromClientBuffer` with that buffer type takes a live
`ID3D11Texture2D` pointer and makes the EGL surface render into that texture.
Surfman's `create_surface_from_texture` uses exactly this path.

Texture requirements for the client-buffer surface:

- `D3D11_USAGE_DEFAULT` (mandated by the extension).
- `BindFlags = D3D11_BIND_RENDER_TARGET | D3D11_BIND_SHADER_RESOURCE`.
- Accepted formats include `DXGI_FORMAT_B8G8R8A8_UNORM` and
  `DXGI_FORMAT_R8G8B8A8_UNORM`.
- The texture must be created on ANGLE's `ID3D11Device` (the same-device
  requirement of `EGL_ANGLE_device_d3d`). Hypercolor satisfies this by creating
  the Surfman ANGLE device on the LUID-matched DXGI adapter, then recovering
  ANGLE's `ID3D11Device` through `Device::native_device()`.

Reference:
<https://github.com/google/angle/blob/main/extensions/EGL_ANGLE_d3d_texture_client_buffer.txt>

### 4.2 NT handles versus legacy global handles

`IDXGIResource1::CreateSharedHandle` creates an **NT handle** for a shared
resource; it owns a reference to the underlying memory.
`IDXGIResource::GetSharedHandle` creates the older **legacy/KMT global handle**;
it owns no reference. Vulkan external memory mirrors this split:

- `VK_EXTERNAL_MEMORY_HANDLE_TYPE_D3D11_TEXTURE_BIT` -- NT handle.
- `VK_EXTERNAL_MEMORY_HANDLE_TYPE_D3D11_TEXTURE_KMT_BIT` -- legacy/KMT handle.

`wgpu-hal` imports only the NT-handle type. The texture Hypercolor creates must
carry `D3D11_RESOURCE_MISC_SHARED_NTHANDLE`, and the handle must come from
`CreateSharedHandle`.

References:

- <https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_2/nf-dxgi1_2-idxgiresource1-createsharedhandle>
- <https://registry.khronos.org/vulkan/specs/latest/man/html/VkExternalMemoryHandleTypeFlagBits.html>

### 4.3 D3D11 texture sharing flags

The owned texture is created with:

- `D3D11_RESOURCE_MISC_SHARED_NTHANDLE` -- required for an NT handle.
- Paired with `D3D11_RESOURCE_MISC_SHARED` or
  `D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX`. The NT-handle flag must be combined
  with one of these. Which one is a Wave 3 hardware-probe decision: some drivers
  expect `SHARED_KEYEDMUTEX` for cross-API NT-handle import, others accept plain
  `SHARED`.

Reference:
<https://learn.microsoft.com/en-us/windows/win32/api/d3d11/ne-d3d11-d3d11_resource_misc_flag>

### 4.4 Keyed mutex is producer-internal, not the cross-API primitive

ANGLE does not drive an `IDXGIKeyedMutex` around its own rendering; the keyed
mutex, if present, is the caller's to acquire and release. Surfman's ANGLE
context does acquire/release the keyed mutex on the producer (ANGLE) side for
`Synchronization::KeyedMutex` surfaces.

A keyed mutex cannot synchronize Hypercolor's Vulkan consumer: `IDXGIKeyedMutex`
is a D3D-only interface, and neither a Vulkan image nor a D3D12 resource exposes
it. Producer/consumer correctness on Windows therefore comes from a `glFinish`
plus the texture ring (Section 7), not from the keyed mutex.

### 4.5 `wgpu` backend selection on Windows

`wgpu 29.0.1` registers backends in priority order Vulkan, Metal, DX12, GLES,
and `request_adapter` stable-sorts candidates by device type. For a single
discrete GPU exposed by both the Vulkan and DX12 backends, the Vulkan adapter
wins the tiebreaker. On typical Windows discrete-GPU hardware the compositor
`wgpu` device is already Vulkan. A broken Vulkan ICD, or an integrated/discrete
ordering quirk, can still select DX12; `on` mode defends against that by
requesting `Backends::VULKAN` explicitly.

### 4.6 `wgpu 29.0.1` `create_texture_from_hal` content-state footgun

In `wgpu 29.0.1`, `Device::create_texture_from_hal` wraps the imported texture
in `TextureUses::UNINITIALIZED`. Under the Vulkan spec, the first use can
transition from `UNDEFINED` layout and discard existing content. For a texture
that already holds a rendered ANGLE frame, this risks a discard on first sample.

The fix (`create_texture_from_hal` gaining an `initial_state` parameter) is in
unreleased `wgpu 30.x`. Wave 3 must verify empirically on real hardware whether
content survives the wrap on the pinned 29.0.1 stack, and the result drives
whether a `wgpu 30.x` upgrade becomes a prerequisite (Section 12).

## 5. Target Architecture

### 5.1 New crate

Add `crates/hypercolor-windows-gpu-interop`, mirroring
`hypercolor-macos-gpu-interop`:

- `lib.rs` selects `windows` on Windows and `stubs` elsewhere; `servo_context`
  is Windows-only.
- `[lints.rust] unsafe_code = "allow"`, `[lints.clippy]
  undocumented_unsafe_blocks = "deny"`, `unwrap_used = "deny"`.
- Windows-only dependencies: the `windows` crate (version aligned to the one
  `wgpu-hal 29.0.1` uses, so `HANDLE` types match
  `texture_from_d3d11_shared_handle`), `wgpu`, `wgpu-hal` with the `vulkan`
  feature, `surfman`, `servo-paint-api`, `webrender_api`, `gleam`, `glow`.

Responsibilities:

- Own all Win32/D3D11/Vulkan `unsafe` interop behind audited functions.
- Create the owned D3D11 shared-texture ring and its NT handles.
- Import an NT handle into a Vulkan-backed `wgpu::Texture`.
- Validate dimensions, format, usage, feature support, and adapter LUID.
- Expose safe Rust descriptors to `hypercolor-core`; the core app never traffics
  in raw `HANDLE`s or COM pointers.
- Provide Windows-only real-GPU fixture tests.

The public frame and importer types mirror the macOS crate exactly so that the
shared `EffectRenderOutput` / `ProducerFrame` plumbing stays platform-agnostic.
The daemon consumes `.texture`, `.view`, `.width`, `.height`, `.format`, and
`.storage_id`; the Windows `ImportedEffectFrame` must carry those fields with
those names:

```rust
pub struct ImportedEffectFrame {
    pub width: u32,
    pub height: u32,
    pub format: ImportedFrameFormat,
    pub storage_id: u64,
    pub texture: Arc<wgpu::Texture>,
    pub view: Arc<wgpu::TextureView>,
    pub timings: ImportedFrameTimings,
}

pub enum ImportedFrameFormat {
    Rgba8Unorm,
    Bgra8Unorm,
}

pub struct ImportedFrameTimings {
    pub wrap_us: u64,
    pub sync_us: u64,
    pub total_us: u64,
}
```

The `windows` module also owns the producer-side types -- the Surfman/ANGLE
device, its underlying D3D11 device, the shared-texture ring, NT handles, and a
`WindowsD3d11SharedTextureImporter` that turns an NT `HANDLE` plus a validated
descriptor into an `ImportedEffectFrame`. Naming may change during
implementation, but the boundary must not: COM and Win32 handles never escape
this crate.

### 5.2 Windows ANGLE rendering context

Add a Windows-only `WindowsAngleRenderingContext` in the new crate's
`servo_context` module, implementing Servo's `RenderingContext`, mirroring
`MacosHardwareRenderingContext`:

- Creates a Surfman ANGLE `Connection`, pins an `Adapter` to the DXGI adapter
  matching the Vulkan device LUID, then creates a Surfman `Device` and `Context`.
- Owns a ring of D3D11 textures (Section 7) created with NT-handle sharing flags
  through the `ID3D11Device` recovered from `Device::native_device()`.
- Binds the current ring slot's texture as the ANGLE render target via
  `create_surface_from_texture`.
- Rotates the ring on `present()`.
- Exposes a worker-only accessor returning the just-completed slot's identity
  and NT handle.
- Keeps `read_to_image` working for CPU fallback.

`servo_bootstrap.rs` selects this context when `servo_gpu_import_should_attempt()`
is true, and otherwise keeps the current `WindowRenderingContext` path. The
hidden Tao window is not part of the GPU path.

### 5.3 Imported frame integration

`hypercolor-core` re-exports the platform frame type explicitly:

```rust
#[cfg(all(feature = "servo-gpu-import", target_os = "linux"))]
pub use hypercolor_linux_gpu_interop::{...};

#[cfg(all(feature = "servo-gpu-import", target_os = "macos"))]
pub use hypercolor_macos_gpu_interop::{...};

#[cfg(all(feature = "servo-gpu-import", target_os = "windows"))]
pub use hypercolor_windows_gpu_interop::{...};
```

The `servo-gpu-import` feature in `hypercolor-core/Cargo.toml` adds
`dep:hypercolor-windows-gpu-interop` alongside the existing Linux and macOS
interop crate dependencies.

`ServoWorkerRuntime`'s GPU import becomes platform-specific:

- Linux: GL framebuffer -> Vulkan external-memory FD.
- macOS: IOSurface -> Metal texture.
- Windows: owned D3D11 NT-handle texture -> Vulkan texture.

## 6. Backend Policy

### 6.1 Compatibility matrix

| Platform | Compatible `wgpu` backend | Native source | Import method |
| --- | --- | --- | --- |
| Linux | Vulkan | GL framebuffer / external memory FD | Vulkan external memory FD |
| macOS | Metal | IOSurface | Metal texture wrap |
| Windows | Vulkan | Owned D3D11 NT-handle texture | Vulkan external memory Win32 |

The consumer backend is Vulkan. The DX12 backend is not used as the consumer,
for two reasons. `wgpu-hal`'s DX12 backend has no shared-handle import helper
and no maintained shared-fence surface; an importer would hand-roll
`ID3D12Device::OpenSharedHandle`. And the imported texture must live on
SparkleFlinger's own `wgpu::Device`, which is Vulkan by default on Windows
(Section 4.5). The Slint integration consumed on DX12 because its host renderer
device was DX12; Hypercolor consumes on Vulkan because SparkleFlinger's device
is Vulkan and `wgpu-hal` has a purpose-built Vulkan helper.

### 6.2 Mode behavior

`off`: always CPU readback.

`auto`:

- Use normal compositor backend selection.
- Enable Windows Servo GPU import only if the selected backend is Vulkan and
  exposes `VULKAN_EXTERNAL_MEMORY_WIN32`.
- If the selected backend is not compatible, fall back to CPU with a precise
  backend reason. Do not silently switch the compositor backend.

`on`:

- Request a Vulkan `wgpu` device explicitly (`Backends::VULKAN`) with
  `VULKAN_EXTERNAL_MEMORY_WIN32` in `required_features`.
- This is an explicit, logged backend selection at SparkleFlinger startup, not a
  mid-run switch. It means the whole compositor runs on Vulkan; on hardware
  where `wgpu` would otherwise pick a non-Vulkan device, that is a deliberate
  consequence of enabling import, and it is logged as such.
- If a Vulkan device with the external-memory feature cannot be created, fail
  Servo GPU import with a precise diagnostic.

### 6.3 Required daemon changes

`GpuRenderDevice::new` currently requests only `Features::CLEAR_TEXTURE` and
imposes no backend. Add:

- An explicit backend preference so `on` mode can request `Backends::VULKAN`:

  ```rust
  pub(crate) enum GpuBackendPreference {
      Auto,
      VulkanRequiredForServoImport,
  }
  ```

- `VULKAN_EXTERNAL_MEMORY_WIN32` added to `required_features` on Windows when
  the adapter advertises it (required in `on`, best-effort in `auto`).

`GpuRenderDeviceInfo::servo_gpu_import_backend_compatible()` becomes:

```text
Linux   => backend == Vulkan
macOS   => backend == Metal
Windows => backend == Vulkan && VULKAN_EXTERNAL_MEMORY_WIN32 is present
```

## 7. Synchronization Strategy

Synchronization is the highest-risk part of Windows. The owned-texture model
produces a natural texture ring; the ring is the Phase 1 design, not a fallback.

### 7.1 Phase 1: texture ring plus `glFinish`

```text
Servo paint into ring slot N
  -> glFinish on the Servo GL context
  -> mark slot N ready; rotate ring to slot N+1
  -> render thread imports/samples slot N-1 (ready a frame earlier)
  -> SparkleFlinger composes from the imported texture
```

A ring of 2-3 owned D3D11 textures means the compositor samples slot N-1 while
ANGLE renders slot N. `glFinish` after Servo paint is a hard producer-side
completion barrier; the ring converts that stall into one frame of pipelined
latency rather than a per-frame blocking wait. This matches how Linux and macOS
shipped: conservative correctness first, measured separately, optimized later.

The `wgpu::Texture` wrapping each ring slot is created once at importer setup
and reused; only the `storage_id` and frame identity advance per frame, since
ANGLE renders directly into the slot's D3D11 texture (no per-frame blit, unlike
Linux).

A keyed mutex is not the consumer synchronization primitive (Section 4.4). If a
ring texture carries `D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX`, Surfman's ANGLE
context will acquire/release it around ANGLE's own rendering, which is harmless
but does not gate the Vulkan consumer. Consumer correctness is the `glFinish`
plus the ring.

### 7.2 Phase 2: shared timeline fence

The `glFinish` CPU stall is the obvious cost to remove. The follow-up is a
shared timeline fence:

- Create an `ID3D11Fence` via `ID3D11Device5::CreateFence` with
  `D3D11_FENCE_FLAG_SHARED` on Hypercolor's D3D11 device.
- Signal value N after ANGLE paints frame N; export the fence as an NT handle.
- Import it into Vulkan as `VK_EXTERNAL_SEMAPHORE_HANDLE_TYPE_D3D12_FENCE_BIT`
  and wait on value N before sampling, via `wgpu-hal`'s
  `vulkan::Queue::add_wait_semaphore`.

`add_wait_semaphore` is a `wgpu 30.x` API, unreleased as of 2026-05-22. Phase 2
is therefore gated on a `wgpu` upgrade and is explicitly out of scope for the
first milestone. The Phase 1 ring plus `glFinish` ships first; Phase 2 is a
documented optimization.

### 7.3 What not to assume

- Do not assume `glFlush` alone proves visibility to Vulkan; use `glFinish` in
  Phase 1.
- Do not assume a keyed mutex synchronizes the Vulkan consumer; it does not.
- Do not assume a non-null handle means the adapter matches the `wgpu` device.
- Do not assume `create_texture_from_hal` preserves imported content on
  `wgpu 29.0.1` (Section 4.6); verify it.

### 7.4 Stale-frame detection

Add a Servo fixture that alternates deterministic colors or frame counters every
frame. Imported GPU readback must match the expected frame index. This catches a
missing `glFinish`, ring-slot ownership bugs, import caching stale memory,
content discard on wrap, and accidental CPU fallback.

## 8. Producer: Owned-Texture Path

This section replaces the earlier draft's three-way handle decision tree. There
is one path.

### 8.1 Setup

1. Determine the SparkleFlinger Vulkan device's adapter LUID via
   `wgpu::Adapter::as_hal::<Vulkan>()` -> `VkPhysicalDeviceIDProperties.deviceLUID`.
2. Enumerate DXGI adapters with the `windows` crate and select the adapter whose
   LUID matches that Vulkan device.
3. Build a Surfman ANGLE `Adapter` with `Adapter::from_dxgi_adapter`, then call
   `Connection::create_device(&adapter)`. Surfman creates both the D3D11 device
   and the ANGLE `EGLDisplay` for that adapter, so LUID match is guaranteed by
   construction.
4. Recover the resulting ANGLE `ID3D11Device` through `Device::native_device()`
   and create the texture ring on that device. This satisfies the client-buffer
   same-device requirement (Section 4.1).
5. If Hypercolor later needs to pre-create the D3D11 device itself, it must also
   create the matching ANGLE `EGLDisplay` before using
   `Connection::create_device_from_native_device`; `NativeDevice` is not a raw
   D3D11-device-only wrapper.

### 8.2 Per ring slot

1. Create an `ID3D11Texture2D`: `D3D11_USAGE_DEFAULT`,
   `BindFlags = RENDER_TARGET | SHADER_RESOURCE`,
   `MiscFlags = D3D11_RESOURCE_MISC_SHARED_NTHANDLE` paired with `SHARED` or
   `SHARED_KEYEDMUTEX` (Section 4.3), format `B8G8R8A8_UNORM` or
   `R8G8B8A8_UNORM`.
2. Mint the NT handle once with `IDXGIResource1::CreateSharedHandle`.
3. Bind the texture as an ANGLE EGL surface via Surfman
   `create_surface_from_texture`.
4. Import the NT handle into the Vulkan `wgpu` device with
   `wgpu-hal`'s `texture_from_d3d11_shared_handle`, then wrap with
   `wgpu::Device::create_texture_from_hal::<wgpu_hal::api::Vulkan>`.

### 8.3 Per frame

Servo renders into the current slot; `glFinish`; rotate; the render thread
receives the `ImportedEffectFrame` for the just-completed slot; SparkleFlinger
samples it. No KMT importer, no direct import of ANGLE's surface, no
hand-rolled DX12 path.

## 9. Telemetry and Diagnostics

The existing `ServoGpuImportFallbackReason` enum has a stable numeric mapping
used for atomic storage (`as_u64`), currently ending at `Other = 17`. New
Windows variants append at 18 and above; the existing variant numbers must not
change. Add only the variants the owned-texture path can actually produce:

- `MissingWgpuVulkanDevice`
- `MissingVulkanExternalMemoryWin32`
- `MissingWindowsAngleContext`
- `D3d11DeviceCreateFailed`
- `D3d11SharedTextureCreateFailed`
- `D3d11SharedHandleCreateFailed`
- `AngleClientBufferSurfaceFailed`
- `AdapterLuidMismatch`
- `VulkanD3d11ImportFailed`
- `WindowsImportStaleFrame`

Do not add variants for the rejected paths (no `UnsupportedD3d11HandleFlavor`,
no `MissingAngleD3d11ShareHandle`).

Timing telemetry reuses the existing three-bucket
`record_servo_gpu_import_frame(blit_us, sync_us, total_us)`:

- `sync_us` -- `glFinish` plus ring-handoff wait.
- `blit_us` -- per-frame import/wrap work; near zero, since ANGLE renders directly
  into the shared texture and the `wgpu` wrapper is created once per slot.
- `total_us` -- total.

Do not introduce a parallel metric vocabulary. The importer's own
`ImportedFrameTimings` keeps `{ wrap_us, sync_us, total_us }`; the worker records
`record_servo_gpu_import_frame(frame.timings.wrap_us, frame.timings.sync_us,
frame.timings.total_us)`.

Add or extend counters:

- `servo_gpu_import_windows_sync_mode` (gl_finish | fence)
- `servo_gpu_import_stale_frame_total`
- `servo_gpu_import_adapter_mismatch_total`

Startup/system probes should expose the active `wgpu` backend, Windows Servo GPU
import compatibility, external-memory feature presence, ring depth, sync mode,
and fallback reason. Use generic field names; deprecate the Linux-named
`linux_servo_gpu_import_backend_compatible` surface in favor of the generic
`servo_gpu_import_backend_compatible`, keeping a legacy alias only if an API
consumer requires it.

## 10. Implementation Plan

### Wave 0: Update the spec and guardrails

**Files:** `docs/specs/58-windows-servo-gpu-surface-interop.md`.

**Status:** Complete.

**Implementation:** record the resolved owned-texture decision, the Vulkan
backend requirement, the Phase 1/Phase 2 synchronization split, and the
`wgpu 29.0.1` content-state risk. (This revision.)

**Verify:** Claude/Codex cross-model review agrees the plan is implementable and
grounded in repo and API facts.

### Wave 1: Make platform cfgs explicit

**Files:** `traits.rs`, `servo/worker.rs`, `servo/telemetry.rs`,
`servo_bootstrap.rs`, `render_thread/gpu_device.rs`,
`render_thread/sparkleflinger/gpu.rs`.

**Status:** Complete.

**Implementation:**

- Replace `not(target_os = "macos")` (currently meaning Linux) with explicit
  `target_os = "linux"` cfgs.
- Add `target_os = "windows"` arms with stubs returning a Windows-specific
  unsupported reason.
- Reshape the Windows bootstrap cfg so the import path and the legacy
  `WindowRenderingContext` path are separately gated on `servo-gpu-import`.
- Keep Linux and macOS behavior unchanged.

**Verify:**

- `cargo check --locked -p hypercolor-core --features servo-gpu-import` on
  Linux, macOS, and Windows.
- `cargo test --locked -p hypercolor-core --features servo-gpu-import servo_gpu`.
- Existing Linux/macOS GPU import tests remain green.

### Wave 2: Windows backend capability plumbing

**Files:** `render_thread/gpu_device.rs`,
`render_thread/sparkleflinger/gpu.rs`, `daemon/src/api/system.rs`,
`ui/src/api/system.rs`, `ui/src/pages/dashboard/renderer.rs`.

**Status:** Complete.

**Implementation:**

- `servo_gpu_import_backend_compatible()` Windows arm: Vulkan plus
  `VULKAN_EXTERNAL_MEMORY_WIN32`.
- Add `GpuBackendPreference` and request `Backends::VULKAN` plus the
  external-memory feature for `on` mode.
- Deprecate `linux_servo_gpu_import_backend_compatible` in favor of the generic
  name.

**Verify:**

- Unit tests cover Linux Vulkan, macOS Metal, Windows Vulkan with the feature,
  Windows Vulkan without the feature, and Windows DX12.
- UI/system JSON snapshot updates are intentional.
- `cargo test --locked -p hypercolor-daemon --features servo-gpu-import gpu`.

### Wave 3: Create `hypercolor-windows-gpu-interop` and the import spike

**Files:** `crates/hypercolor-windows-gpu-interop/{Cargo.toml,src/lib.rs,
src/windows.rs,src/stubs.rs,src/importer.rs,tests/*}`; workspace `Cargo.toml`;
`hypercolor-core/Cargo.toml`.

**Status:** Complete on the current Windows workstation; cross-vendor coverage
remains part of soak.

**Implementation:**

- Add the Windows-only crate mirroring `hypercolor-macos-gpu-interop`, with
  non-Windows stubs.
- `windows` crate bindings for `HANDLE`, DXGI, D3D11; `wgpu-hal` with `vulkan`.
- `WindowsD3d11SharedTextureImporter`: NT `HANDLE` plus descriptor ->
  `texture_from_d3d11_shared_handle` -> `create_texture_from_hal::<Vulkan>` ->
  `ImportedEffectFrame`. Validate dimensions, format, usage, and the
  `VULKAN_EXTERNAL_MEMORY_WIN32` feature.
- A synthetic fixture with no Servo dependency: create a D3D11 device, an
  `NTHANDLE` shared texture, fill it with deterministic pixels,
  `CreateSharedHandle`, import, read back through `wgpu`, compare pixels.
- Explicitly test the Section 4.6 footgun: confirm whether `wgpu 29.0.1`
  preserves the texture's existing content through `create_texture_from_hal`.

**Verify:**

- `cargo check --locked -p hypercolor-windows-gpu-interop` (Windows and
  non-Windows stub builds).
- `HYPERCOLOR_RUN_WINDOWS_D3D11_FIXTURE=1 cargo test --locked -p
  hypercolor-windows-gpu-interop
  imports_synthetic_d3d11_shared_texture_into_wgpu_texture -- --nocapture`.
- Imported pixels match expected values; repeated import/drop leaks no handles
  or GPU memory.
- The content-preservation result is recorded; if content is discarded, escalate
  the `wgpu 30.x` upgrade decision (Section 12).

### Wave 4: Windows ANGLE rendering context

**Files:** `hypercolor-windows-gpu-interop/src/servo_context.rs`;
`hypercolor-core/src/effect/servo_bootstrap.rs`.

**Status:** Complete on the current Windows workstation.

**Implementation:**

- `WindowsAngleRenderingContext` implementing Servo's `RenderingContext`,
  mirroring `MacosHardwareRenderingContext`.
- Surfman ANGLE `Connection`/`Adapter`/`Device`/`Context`; the owned texture
  ring on the LUID-matched adapter; `create_surface_from_texture` per slot; ring
  rotation on `present()`; a worker-only accessor for the completed slot.
- Keep `read_to_image` for CPU fallback.
- `bootstrap_rendering_context` selects this context when
  `servo_gpu_import_should_attempt()` is true.

**Verify:**

- Existing Servo CPU tests still pass.
- `HYPERCOLOR_RUN_WINDOWS_ANGLE_CONTEXT_FIXTURE=1 cargo test --locked -p
  hypercolor-windows-gpu-interop --features servo-context
  angle_context_renders_into_importable_d3d11_ring -- --nocapture`.
- Resize recreates the ring and surfaces.
- Diagnostics report adapter LUID, texture format, dimensions, and ring depth.

### Wave 5: Servo worker integration

**Files:** `servo/worker.rs`, `servo/session.rs`, `servo_bootstrap.rs`,
`effect/traits.rs`, `render_thread/producer_queue.rs`.

**Status:** Complete; app-level soak is ongoing.

**Implementation:**

- Add the `target_os = "windows"` arms to `warm_gpu_importer_if_available`,
  `ensure_gpu_importer`, `import_gpu_frame`, `clear_gpu_importer`, and
  `destroy_gpu_importer_for_session`.
- Windows `import_gpu_frame`: after Servo paint and `glFinish`, take the
  completed ring slot's NT handle, hand it to the importer, return
  `EffectRenderOutput::Gpu`.
- Fall back to CPU in `auto`; fail clearly in `on`.

**Verify:**

- A Servo fixture emits `ImportedEffectFrame` on compatible Windows hardware.
- The CPU readback path still works.
- `servo_readback_us == 0` on successful GPU import.
- Producer metrics count GPU frames.

### Wave 6: SparkleFlinger normalization

**Files:** `render_thread/sparkleflinger/gpu.rs`.

**Status:** Complete for the shader-copy path.

**Implementation:**

- Route Windows imported frames through the shader copy initially.
- Normalize BGRA/RGBA and Y-flip; ANGLE/GL render targets use a bottom-left
  origin, and SparkleFlinger expects top-left, matching `Canvas`.
- Only skip the shader copy once format and origin are proven compatible.

**Verify:**

- Pixel parity fixture passes against CPU readback.
- A visual fixture shows correct orientation and colors.
- The existing macOS shader-copy path is unchanged.

### Wave 7: Synchronization hardening

**Files:** `hypercolor-windows-gpu-interop/src/{windows.rs,sync.rs}`;
`servo/telemetry.rs`; Windows integration tests.

**Status:** Phase 1 complete; shared-fence Phase 2 remains a future optimization.

**Implementation:**

- Formalize the ring (depth 2-3) plus `glFinish` Phase 1 sync.
- Add stale-frame detection (alternating-color fixture).
- Reject adapter-LUID mismatch.
- Document the Phase 2 shared-fence design as a follow-up gated on a `wgpu 30.x`
  upgrade.

**Verify:**

- An alternating-color animation never samples a stale frame.
- Resize stress does not tear or reuse invalid handles.
- Mixed GPU/adapter configurations are rejected with `AdapterLuidMismatch`.

### Wave 8: Benchmarks and soak

**Files:** benchmark scripts, telemetry docs, Windows fixture docs.

**Status:** Ongoing.

**Implementation:**

- Measure CPU readback versus GPU import; track p50/p95/p99 import time, sync
  overhead, and fallback rate.
- Run a long soak with Servo HTML effects.

**Verify:**

- `servo_readback_us == 0` on imported frames.
- GPU import materially beats CPU readback.
- No handle, Vulkan memory, D3D11 texture, or Servo session leaks.
- `just verify` passes before merge.

## 11. Acceptance Criteria

Implementation-complete criteria:

1. Windows uses an explicit platform cfg path, not Linux-by-negation.
2. `auto` preserves stable CPU fallback and never silently switches the
   compositor backend.
3. `on` fails loudly when a Vulkan device with `VULKAN_EXTERNAL_MEMORY_WIN32`
   is unavailable.
4. The importer validates backend, format, dimensions, and adapter LUID.
5. Servo returns `EffectRenderOutput::Gpu` on Windows via the owned-texture ring.
6. SparkleFlinger consumes the imported texture without CPU upload.
7. Deterministic D3D11 and ANGLE fixtures pass on Windows hardware.
8. The ring plus `glFinish` synchronization reports stale-frame failures
   precisely if the producer hands off an invalid slot.
9. Resize recreates the ring and handle lifetime is scoped to the context.
10. Telemetry reports exact fallback reasons and Windows sync mode.
11. Linux and macOS import paths remain unchanged and green.

Soak-complete criteria:

1. GPU import materially beats CPU readback under representative HTML effects.
2. No handle, Vulkan memory, D3D11 texture, or Servo session leaks appear during
   long-running Windows use.
3. NVIDIA, AMD, Intel, hybrid-GPU, and degraded Vulkan configurations either
   import correctly or fall back with precise diagnostics.
4. `just verify` passes before merge.

## 12. Risk Register

| Risk | Probability | Impact | Mitigation |
| --- | --- | --- | --- |
| `wgpu 29.0.1` `create_texture_from_hal` discards imported content on another driver | Low | High | Current Windows hardware preserves content; cross-vendor soak must keep this on the watchlist |
| Driver expects a specific `SHARED` vs `SHARED_KEYEDMUTEX` flag pairing | Low | Medium | Current Windows hardware accepts `SHARED | SHARED_NTHANDLE`; cross-vendor soak validates NVIDIA, AMD, and Intel |
| `wgpu` selects a non-Vulkan compositor device | Low | Medium | `auto` falls back to CPU with a reason; `on` requests `Backends::VULKAN` |
| `glFinish`-per-frame sync costs too much latency | Medium | Medium | Texture ring hides the stall; Phase 2 shared fence removes it |
| ANGLE client-buffer surface rejects the owned texture | Low | High | Match Slint's proven flags/format; create texture on ANGLE's `ID3D11Device` from `Device::native_device()` |
| Format/origin differs from SparkleFlinger assumptions | High | Low | Shader-copy normalization first; skip only once proven |
| Surfman adapter pinning diverges across drivers | Low | Medium | Current fixture validates `Adapter::from_dxgi_adapter`; keep pre-created-device injection as escape hatch |
| WebGL effects still panic before import | Medium | High | Probe the WebGL path separately; do not block non-WebGL import on it |
| Driver-specific external-memory bugs | Medium | High | Conservative allowlist/denylist after hardware soak |

## 13. Resolved Questions and Open Questions

Resolved by the research pass:

1. ANGLE's surface share handle is legacy/KMT, not NT. The owned-texture path is
   mandatory; direct import is rejected.
2. Surfman already exposes the client-buffer surface API, DXGI-adapter pinning,
   and `Device::native_device()`; Hypercolor owns the texture, handle, format,
   dimensions, and adapter, so it does not need Surfman to report a native frame
   descriptor.
3. Vulkan sampling is synchronized by `glFinish` plus a texture ring (Phase 1);
   a keyed mutex cannot synchronize a Vulkan consumer. A shared fence is Phase 2.
4. Adapter identity is handled by constructing the D3D11 device on the wgpu
   Vulkan adapter's LUID, so the match is guaranteed rather than checked after
   the fact.
5. `on` mode requests a Vulkan device explicitly at startup; it does not force a
   backend switch mid-run. On hardware where `wgpu` would not pick Vulkan, that
   is a deliberate, logged compositor-backend choice.
6. The surface format is Hypercolor's choice (`B8G8R8A8_UNORM` or
   `R8G8B8A8_UNORM`); normalization happens in the SparkleFlinger shader copy.
7. The GPU path uses a custom `WindowsAngleRenderingContext`. The hidden Tao
   window and `WindowRenderingContext` remain only for the CPU fallback path.

Closed by the Windows implementation pass:

- `wgpu 29.0.1` preserves imported D3D11 content on the current Windows
  workstation.
- `D3D11_RESOURCE_MISC_SHARED | D3D11_RESOURCE_MISC_SHARED_NTHANDLE` imports on
  the current Windows workstation.
- The primary `Adapter::from_dxgi_adapter` route works for the current Vulkan
  adapter identity.

Still open for soak:

- Whether the same import/content behavior holds across NVIDIA, AMD, Intel, and
  hybrid-GPU systems.
- Whether any driver requires `SHARED_KEYEDMUTEX` paired with `SHARED_NTHANDLE`.
- Whether Phase 1 `glFinish` sync is cheap enough under representative effects,
  or whether the Phase 2 shared-fence path should move up.

## 14. Review Notes

This spec was research-verified on 2026-05-22 against the pinned Surfman fork
(`hyperb1iss/surfman` rev `e75590f`), `wgpu-hal 29.0.1` source, ANGLE's D3D11
backend, Servo upstream, and external prior art (DXVK, Chromium/Dawn, Firefox,
CEF, and the Slint Servo-on-Windows integration).

Claude reviewed and revised the spec before this revision. Codex reviewed it
again on 2026-05-22 and accepted the architecture with three clarifications:

- Slint is prior art for the owned texture + ANGLE client-buffer + NT-handle
  import family, but Hypercolor's direct-render/no-blit variant still needs Wave
  4 validation.
- The primary Surfman route is adapter pinning plus `Device::native_device()`,
  not wrapping a raw D3D11 device directly.
- Windows imported-frame timings include `sync_us` so `glFinish` and ring
  handoff cost are visible in the existing telemetry buckets.

The same Windows workstation then verified the implemented interop path on
2026-05-22 with both the synthetic D3D11 shared-texture fixture and the Servo
ANGLE ring fixture. That verification proves the architecture locally; the
remaining work is cross-vendor soak and performance characterization, not a
first-order design unknown.
