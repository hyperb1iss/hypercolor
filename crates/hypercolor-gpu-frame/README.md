# hypercolor-gpu-frame

*Platform-neutral vocabulary for imported GPU frames.*

Servo and the platform screen-capture backends all hand Hypercolor a GPU
texture that some native API owns: a Vulkan image on Linux, an IOSurface on
macOS, a D3D11 texture on Windows. Every one of those paths needs to describe
the same things to the compositor, and none of them should leak a platform
handle into shared code. This crate is that shared description. The three
`*-gpu-interop` crates re-export these types rather than defining their own, and
`hypercolor-core` consumes them without knowing which platform produced the
frame.

## Workspace position

**Depends on:** `wgpu`; optionally `anyhow` and the top-level `servo` API behind
`servo-context`. No other Hypercolor crates.

**Depended on by:** `hypercolor-core`, `hypercolor-daemon`,
`hypercolor-linux-gpu-interop`, `hypercolor-macos-gpu-interop`, and
`hypercolor-windows-gpu-interop`.

## Key types

- `ImportedEffectFrame` — the frame itself: dimensions, format, allocation
  identity, content generation, row origin, the imported `wgpu::Texture` and its
  default view, a lifetime lease, and timing counters.
- `ImportedFrameAllocationId` — opaque, producer-scoped identity of one GPU
  allocation. Deliberately separate from `content_generation`, so a consumer can
  tell "same allocation, new contents" from "new allocation".
- `ImportedFrameFormat` — neutral pixel format (`Rgba8Unorm`, `Bgra8Unorm`,
  `Rgba16Float`, `R8Unorm`, and more). Non-exhaustive; each interop crate adds
  its own extension trait to project it onto native format constants.
- `FrameOrigin` — `TopLeft` or `BottomLeft` row convention, so the compositor
  never has to guess whether a producer flips.
- `ImportedFrameLease` — a cloneable lifetime token holding
  `Arc<dyn Send + Sync>`. It retains a platform-owned resource for as long as
  the frame lives without exposing the native type, and `retains_same_owner`
  compares two frames by allocation.
- `ImportedFrameTimings` — optional `blit_us`, `wrap_us`, and `sync_us` phases
  plus a required `total_us`, for import observability.
- `GpuFrameImportFallbackReason` and the `GpuFrameImportError` trait — the
  stable diagnostic vocabulary a platform error maps into so shared code can log
  and route a failure without matching on a platform error type.

## Feature flags

| Feature | What it gates |
|---|---|
| `servo-context` | The Servo rendering-context and frame-import seam (`servo` module) shared by the GPU interop crates and the Servo worker. Pulls in `anyhow` and `servo`. |
| `default` | Empty. |

---

Part of [Hypercolor](https://github.com/hyperb1iss/hypercolor) — open-source
RGB lighting orchestration for Linux, Windows, and macOS. Licensed under
Apache-2.0.
