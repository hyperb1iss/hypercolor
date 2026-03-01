# Web Rendering Engine Analysis for Hypercolor

> Which engine renders 320x200 Canvas/WebGL at 60fps with pixel readback on Linux?

---

## The Requirement

Render HTML Canvas 2D and/or WebGL effects at 60fps, extract raw RGBA pixel data every frame, push it to OpenRGB SDK + WLED DDP. Must work on Linux (GNOME primary), be open source friendly, and not weigh 200MB.

---

## Tier 1: Best Fits

### wgpu + Custom Shader Renderer

**The performance ceiling.** Skip web engines entirely.

- **What**: Rust GPU abstraction over Vulkan/Metal/D3D12/OpenGL
- **Pixel readback**: Render to texture → staging buffer (MAP_READ) → CPU. Trivial for 320x200 (256KB/frame)
- **Performance**: Thousands of FPS. 5 million pixel updates/sec demonstrated
- **Memory**: ~5-10MB. Just your binary + GPU driver
- **License**: MIT/Apache 2.0
- **Tradeoff**: No HTML/JS/CSS. Effects written as WGSL/GLSL shaders, not web pages
- **Best for**: New effect format designed for maximum performance. Uchroma-style Rust projects
- **When to pick**: Building a custom effect system, not reusing SignalRGB HTML effects

### Servo (Mozilla's Rust Engine)

**The most promising open source web engine.**

- **What**: Rust browser engine, MPL 2.0, v0.0.5 (Jan 2026)
- **Canvas 2D + WebGL + WebGPU**: All supported. Recent additions: `toDataURL`, `toBlob`, `transferControlToOffscreen`, WebGL2 `blitFramebuffer`
- **Offscreen rendering**: Via surfman -- cross-platform GPU surface management with near-zero-copy transfer
- **Pixel readback**: surfman offscreen surfaces → OpenGL texture → readPixels
- **Memory**: Rust-native, lighter than Chromium
- **Embedding API**: Being reworked (WebView delegate pattern like Apple's WebKit). Still early
- **Risk**: v0.0.5 -- real edge cases will surface. Web standards incomplete for general browsing, but likely sufficient for canvas-heavy LED effects
- **When to pick**: Want web compatibility + Rust + open source. Willing to ride the maturity curve

### WPE WebKit

**The sleeper pick. Purpose-built for embedded Linux.**

- **What**: WebKit without GTK. Designed for digital signage, set-top boxes, automotive
- **Offscreen**: Native via custom backends. DMA-BUF / EGLStream for zero-copy frame export
- **Canvas 2D + WebGL**: Full support. Skia GPU rendering (since WebKit 2.46)
- **Memory**: ~30MB. No GTK dependency
- **License**: LGPL 2.1
- **Battle-tested**: Widely deployed on embedded Linux for exactly this kind of use case
- **Tradeoff**: Harder to set up than Servo, less community tooling
- **When to pick**: Production-proven web rendering on Linux without the Chromium tax

---

## Tier 2: Viable

### CEF (Chromium Embedded Framework)

- **OnPaint callback**: Delivers BGRA pixel buffer with dirty rects. Purpose-built for this
- **GPU OSR**: Reintroduced in M125. 200-1800 FPS on benchmarks. Linux has NVIDIA driver issues
- **Software mode**: ~30fps, but for 320x200 that's only 256KB/frame -- fast enough
- **Memory**: ~60-80MB (lighter than Electron, still Chromium-class)
- **License**: BSD
- **Rust integration**: [wef](https://github.com/longbridge/wef) wraps CEF with OSR in Rust
- **When to pick**: Need bullet-proof Chromium compatibility, willing to carry the weight

### Electron (Offscreen Mode)

- **Shared texture mode**: Zero-copy GPU texture sharing. 240fps capable
- **paint event**: Delivers dirty rects + NativeImage every frame
- **Canvas + WebGL**: Full Chromium. Everything works
- **Memory**: ~90MB+ base. Full Chromium + Node.js
- **License**: MIT
- **When to pick**: Rapid prototyping. The API is exactly right (`setFrameRate(60)` + `paint` event), just heavy

### node-canvas + headless-gl

- **Canvas 2D**: Cairo-backed, CPU rendering. `canvas.toBuffer()` for pixel data
- **WebGL**: ANGLE-based via headless-gl. `gl.readPixels()` for readback
- **Works with**: Three.js via node-canvas-webgl
- **Memory**: ~40MB (V8 + native modules)
- **60fps at 320x200**: Yes. CPU overhead is negligible at this resolution
- **License**: MIT (all components)
- **When to pick**: Simplest path to running existing JS/TS effects headlessly. Good for prototyping

---

## Tier 3: Not Recommended

| Engine | Why Not |
|---|---|
| **Ultralight** | Proprietary license ($3K/year for Pro). No WebGL. Great tech, wrong license for OSS |
| **Tauri / wry** | No offscreen rendering API. Canvas FPS drops to 5fps on Linux WebKitGTK mouse move bug |
| **webview/webview** | No pixel readback API. JS bridge serialization overhead kills 60fps |
| **WebKitGTK** | Possible but complex offscreen setup. `willReadFrequently` hint helps but GTK coupling is awkward |
| **Headless Chrome** | CDP round-trip adds ms per frame. 2fps headless without GPU. Overkill |
| **Deno** | No WebGL. Canvas 2D via Skia works but limited ecosystem |

---

## Comparison Matrix

| Engine | 60fps | Pixel Readback | Memory | WebGL | License | Embed Difficulty |
|---|---|---|---|---|---|---|
| **wgpu** | 1000s fps | Buffer readback | ~5MB | N/A (native GPU) | MIT/Apache | Hard (custom) |
| **Servo** | Likely | surfman offscreen | Light | Yes | MPL 2.0 | Medium (Rust) |
| **WPE WebKit** | Yes | DMA-BUF zero-copy | ~30MB | Yes | LGPL 2.1 | Medium-Hard |
| **CEF** | Yes | OnPaint callback | ~60-80MB | Yes | BSD | Medium (C++) |
| **Electron** | 240fps | paint/shared texture | ~90MB+ | Yes | MIT | Easy |
| **node-canvas** | Yes | toBuffer/readPixels | ~40MB | WebGL 1 | MIT | Easy |

---

## Recommendation: The Hybrid Approach

SignalRGB does this -- they offer **Ultralight** for fast Canvas 2D and **Qt WebEngine** for WebGL. Follow the same pattern:

### Fast Path: wgpu
- Native WGSL/GLSL shader effects
- Maximum performance, minimum overhead
- Rust-native, integrates with uchroma
- For effects designed specifically for Hypercolor

### Compatibility Path: Servo (or WPE WebKit as fallback)
- Run existing SignalRGB HTML/Canvas/JS effects unmodified
- Open source, embeddable, supports Canvas 2D + WebGL
- Servo if you want Rust-native; WPE WebKit if you want battle-tested

### Prototype Path: node-canvas + headless-gl
- Get it working fast with your existing TypeScript effects
- openrgb-sdk (npm) for hardware output
- Upgrade to wgpu/Servo later when the architecture solidifies

```
┌─────────────────────────────────┐
│         Hypercolor Engine       │
├────────────────┬────────────────┤
│   wgpu Path    │   Web Path     │
│  (WGSL/GLSL)   │  (HTML/JS/CSS) │
│  Native shaders │  Servo or WPE  │
│  1000s fps     │  60fps Canvas  │
├────────────────┴────────────────┤
│     Spatial Layout Engine       │
│  (Canvas coords → LED positions)│
├─────────────────────────────────┤
│        Output Transports        │
│  OpenRGB SDK │ WLED DDP │ HA    │
└─────────────────────────────────┘
```
