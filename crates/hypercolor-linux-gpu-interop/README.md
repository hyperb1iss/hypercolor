# hypercolor-linux-gpu-interop

*Zero-copy GL-to-wgpu texture import for Servo effect frames on Linux.*

Servo renders HTML canvas effects into an OpenGL framebuffer on its own
thread. Hypercolor composites those frames using wgpu (Vulkan-backed). The
naive path is a CPU readback (`glReadPixels`) on every frame — at 640×480
and 60 fps that is roughly 74 MB/s of PCIe traffic. This crate eliminates
the readback by using `VK_KHR_external_memory_fd` and
`GL_EXT_memory_object_fd` to allocate a Vulkan image with exportable
memory, give OpenGL a view of it via a POSIX file descriptor, blit the
Servo framebuffer in with `glBlitFramebuffer`, then wrap the same Vulkan
image as a wgpu texture. The result is an `ImportedEffectFrame` the
compositor samples directly on the GPU.

**Platform scope.** This crate compiles on all platforms. On non-Linux
targets, `src/stubs.rs` provides a matching public API that always returns
`LinuxGpuInteropError::UnsupportedPlatform`. The real implementation lives
in `src/linux.rs`. This keeps the dependency graph clean for cross-compilation
without requiring feature flags in downstream crates.

**Safety.** `unsafe_code = "allow"` is set for this crate because raw Vulkan
and GL FFI are unavoidable here. `undocumented_unsafe_blocks = "deny"` and
`unwrap_used = "deny"` are both set as compensating controls.

## Workspace position

**Depends on:** `hypercolor-gpu-frame`, the neutral imported-frame vocabulary
every interop crate shares; plus `ash`, `glow`, `libc`, `thiserror`, `wgpu`,
and `wgpu-hal`.

**Depended on by:** `hypercolor-core` (optional; activated by the
`servo-gpu-import` feature).

## Key types

**Primary entry point**

- `LinuxGlFramebufferImporter` — allocates a pool of `ImportedFrameSlot`s at
  startup (default 8), then on each frame blits the GL source into the next
  available slot. Two import modes:
  - `import_framebuffer()` — blocking: blits and waits for GPU sync before
    returning.
  - `import_framebuffer_pipelined()` — non-blocking: queues a blit and returns
    the most recently completed slot, overlapping GPU work with CPU rendering.

**Output types** (defined in `hypercolor-gpu-frame`, re-exported here)

- `ImportedEffectFrame`: imported texture/view, allocation identity, content
  generation, origin, lease, and uniform timing phases.
- `ImportedFrameTimings`: optional `blit_us`, `wrap_us`, and `sync_us` phases
  plus `total_us`.
- `ImportedFrameAllocationId`, `ImportedFrameLease`, `FrameOrigin`,
  `GpuFrameImportError`, and `GpuFrameImportFallbackReason` come from the same
  crate.

**Descriptors and formats**

- `LinuxGlFramebufferImportDescriptor` — validated frame description (width,
  height, `ImportedFrameFormat`). Crate-owned.
- `ImportedFrameFormat` — re-exported from `hypercolor-gpu-frame`; currently
  only `Rgba8Unorm`. This crate adds `LinuxImportedFrameFormatExt`, which
  projects it onto GL internal formats and Vulkan formats.

**Extension loading**

- `GlExternalMemoryFunctions` — loaded `GL_EXT_memory_object_fd` entry points.
  Load via `load_from_process()` (dlopen into libGL/libEGL) or `load_from()`
  with a custom proc-address callback.
- `GlFramebufferSource` — `CurrentRead` or `Framebuffer(Option<NativeFramebuffer>)`.

**Diagnostics**

- `LinuxGpuImportCapabilities` — reports which required extensions are present
  or missing.
- `check_wgpu_vulkan_external_memory_fd()` — single capability check.
- `report_linux_gpu_import_capabilities()` — full capability report.

**One-shot imports (no pooling)**

- `import_gl_framebuffer_to_wgpu()` /
  `import_gl_framebuffer_to_wgpu_from_process()` — for low-frequency use only.

## Feature flags

| Feature | What it gates |
|---|---|
| `servo-context` | Servo GL context plumbing for the import path; also activates `hypercolor-gpu-frame/servo-context`. |
| `raw-gl-fixture` | Test infrastructure for raw GL framebuffer fixture tests. Not for production builds. |
| `default` | Empty. |

---

Part of [Hypercolor](https://github.com/hyperb1iss/hypercolor) — open-source
RGB lighting orchestration for Linux, Windows, and macOS. Licensed under
Apache-2.0.
