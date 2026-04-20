# 00 — Synthesis: Native WASM Effect Loader for Hypercolor

> Turbo-optimized, sandboxed, polyglot effect plugins loaded dynamically into the Hypercolor daemon. Research synthesis, April 2026.

## TL;DR

A native WASM effect loader is a clear build, not a maybe. Wasmtime 36.0 LTS hits 95% of native performance with SIMD, epoch interruption gives us bulletproof per-frame budgets, and `#[derive(Effect)]` ergonomics collapse boilerplate to ~8 lines. This unlocks a polyglot ecosystem where anyone can ship a sandboxed effect in Rust, AssemblyScript, Zig, or C, distributed through a git-native signed registry, loaded and swapped live at microsecond granularity. The existing `docs/design/09-plugin-ecosystem.md` already specifies Phase 2 (WIT + Wasmtime for non-effect plugins), so we inherit the runtime decision and extend it across the effect boundary.

## The Shift

The prior design doc flagged WASM effects as "acceptable for some formats, not others" because of hot-path latency concerns. That conclusion was written against the 2022-2023 state of the ecosystem. Three things have changed since then:

1. **Wasmer 6.0 (October 2025) hit 95% of native on CoreMark.** Wasmtime Cranelift sits at 1.1-1.3x steady-state. The "WASM is 1.5-2x slower" rule of thumb is retired.
2. **WebAssembly 3.0 ratified in September 2025** with fixed 128-bit SIMD, relaxed SIMD, tail calls, GC, exception handling, and 64-bit memory. Wasmtime enables relaxed SIMD by default.
3. **Epoch interruption in wasmtime** gives us a ~10% overhead primitive that lets a render thread walk away from a rogue effect inside 1ms. This is the missing piece for real-time safety.

The arithmetic changes too. A naive per-pixel kernel on 640×480 runs in ~150 µs in WASM under 1% of a 60fps budget. SIMD-aware kernels hit 10-20x over scalar. We are not fighting the runtime anymore.

## Architecture

```
                         Hypercolor render thread
                                   │
                                   ▼
                  ┌────────────────────────────────────┐
                  │         EffectPool (Mutex)         │
                  └──────┬───────────┬──────────┬──────┘
                         │           │          │
             Box<dyn EffectRenderer> instances (swappable)
                         │           │          │
           ┌─────────────┘           │          └──────────────┐
           ▼                         ▼                         ▼
   WgpuRenderer (GPU)      ServoRenderer (HTML)      WasmEffectRenderer (NEW)
           │                         │                         │
           │                         │                         │
           │                         │           ┌──────────────┴──────────────┐
           │                         │           │   wasmtime Engine (shared)  │
           │                         │           │   .cwasm AOT cache (XDG)    │
           │                         │           │   epoch + fuel metering     │
           │                         │           │   memfd-backed linear mem   │
           │                         │           │   pooling allocator         │
           │                         │           └──────────────┬──────────────┘
           │                         │                          │
           ▼                         ▼                          ▼
                            Canvas (RGBA, 320x200 → 640x480)
                                       │
                                       ▼
                             SpatialEngine → devices
```

### The tiered model

The `09-plugin-ecosystem.md` hint that WASM is "acceptable for some formats" becomes a formal tier:

| Tier | Renderer        | Use for                                             | Budget target |
|------|-----------------|-----------------------------------------------------|---------------|
| 0    | `WgpuRenderer`  | Native GPU shaders for the hottest 5-10 effects     | <100 µs       |
| 1    | `WasmEffectRenderer` | Everything else authored today in Rust/TS + new effects | <5 ms     |
| 2    | `ServoRenderer` | HTML/WebGL effects with full DOM surface            | <15 ms        |

Tier 1 is the new default. Tier 0 stays for performance-critical natives. Tier 2 stays because Servo is already the path for effects that want real HTML (canvas 2D, webgl, full CSS).

### Runtime pick: wasmtime 36.0 LTS

Direct crate dependency, not wrapped in Extism. Extism's bytes-in/bytes-out model forces kernel-call-per-8-bytes staging and is the wrong shape for streaming 1.2 MB canvas buffers at 60fps. Wasmer's modern `MemoryView` is a copying cursor, a regression for zero-copy use without unsafe (workspace-forbidden). Wasmi is an interpreter at 200-500ns per call, fine for control code but slow for pixel loops.

Wasmtime wins on four specific primitives we need:

- **Zero-copy canvas lending** via `Memory::data_mut` returning `&mut [u8]`
- **Epoch interruption** for per-frame deadlines with ~10% overhead
- **Pooling allocator + CoW heap images** for microsecond instance swap
- **Custom `MemoryCreator`** so we can back the guest's linear memory with a memfd shared with the host

Pin to 36.0 LTS now (24-month security window), migrate to 48.x LTS when it ships in August 2026.

### ABI strategy: raw first, Component Model second

Ship v0.1 with a handful of raw `wasm32` exports: `init`, `render_into`, `set_control`, `destroy`. All polyglot languages work today: Rust, AssemblyScript, Zig, C, C++, TinyGo. Raw exports dodge two real blockers:

- **AssemblyScript actively opposes WASI/Component Model** on standards grounds. If we commit to Component Model alone, we cut the one lane that is trivial for web-native effect authors.
- **Zig has no wit-bindgen** support in 2026. Raw exports work, Component Model does not.

Define `hypercolor:effect/renderer@0.1.0` WIT world for v0.2, as an opt-in layer for authors who want the ergonomics. Both ABIs live side-by-side in the host loader.

## The API

`#[derive(Effect)]` over a struct whose fields are the parameters. One `render` method. The struct is the schema, the macro generates everything the daemon needs to wire controls, UI, presets, and MIDI binding.

```rust
use hypercolor_effect::{Effect, Frame, Canvas, palette, easing};

#[derive(Effect)]
#[effect(
    name = "Aurora Drift",
    category = "ambient",
    author = "Nova"
)]
pub struct AuroraDrift {
    #[control(min = 0.1, max = 4.0, default = 1.0)]
    pub speed: f32,

    #[control(palette = "aurora", default = "aurora_classic")]
    pub palette: palette::Handle,

    #[control(default = 0.35)]
    pub audio_modulation: f32,

    // Private state persists across frames automatically.
    phase: f32,
}

impl AuroraDrift {
    pub fn render(&mut self, f: &Frame, canvas: &mut Canvas) {
        self.phase += f.delta * self.speed * (1.0 + self.audio_modulation * f.audio.loudness);

        canvas.for_each_pixel_simd(|x, y, out| {
            let u = x as f32 / canvas.width as f32;
            let v = y as f32 / canvas.height as f32;
            let t = self.phase + u * 1.4 - v * 0.3;
            let mix = easing::smooth(0.5 + 0.5 * (t * std::f32::consts::TAU).sin());
            *out = self.palette.sample(mix);
        });
    }
}
```

Thirty lines. Every control appears as a UI widget automatically. Hot reload replaces the module at the next frame boundary with sub-millisecond swap. Audio, screen, sensors, keyboard all land in `f.audio` / `f.screen` / etc., same shape the native `FrameInput` already uses.

The macro generates the raw wasm exports that the host calls. A hand-written AssemblyScript effect implements the same four exports directly:

```typescript
// aurora-drift.ts
import { Frame, Canvas } from "@hypercolor/effect-sdk";

export const META = { name: "Aurora Drift", category: "ambient" };
export const CONTROLS = [
    { name: "speed", min: 0.1, max: 4.0, default: 1.0 },
    { name: "audio_modulation", default: 0.35 }
];

let phase: f32 = 0.0;
let speed: f32 = 1.0;
let audio_mod: f32 = 0.35;

export function set_control(idx: i32, value: f32): void {
    if (idx == 0) speed = value;
    if (idx == 1) audio_mod = value;
}

export function render(frame_ptr: usize, canvas_ptr: usize): void {
    const frame = Frame.at(frame_ptr);
    const canvas = Canvas.at(canvas_ptr);
    phase += frame.delta * speed * (1.0 + audio_mod * frame.audio_loudness);
    // ... pixel loop
}
```

Both produce ~4-80 KB `.wasm` files. Rust with the macro targets ~50 KB release. AssemblyScript hits ~4 KB.

## The Witchery Inventory

What this architecture actually unlocks, sorted by how delicious each one is.

### Performance witchery

1. **memfd-backed canvas**. The host allocates the RGBA buffer once, backs the guest's linear memory with the same fd, guest writes pixels directly into host memory. Zero copy, ever.
2. **AOT `.cwasm` cache at `$XDG_CACHE_HOME`**. Compile once on first load, cache bytecode, subsequent loads are memory-map-and-run in <1 ms.
3. **Pooling allocator + CoW heap images**. Hot-reloading an effect becomes a `Mutex` lock plus an instance swap. Microseconds, not milliseconds.
4. **Epoch deadline per frame tick**. Render thread bumps the epoch counter; if the effect blows its budget, wasmtime interrupts cleanly. A rogue effect cannot stall the render loop.
5. **simd128 pixel kernels**. wasm32 v128 maps to SSE/NEON; Gaussian-shape kernels hit 10-20x scalar. Palette sampling, gradient blends, and noise all vectorize cleanly.
6. **Fuel metering for deterministic replay**. Turn on fuel budgets, record an effect session frame-by-frame, replay bit-exact on any machine.

### Authoring witchery

7. **`#[derive(Effect)]` is the schema**. Struct fields become UI controls, presets, MIDI CC bindings, and OSC addresses with zero duplication. NIH-plug and Bevy converged on this pattern for a reason.
8. **Hot reload at frame boundaries**. File watcher sees `aurora_drift.wasm` change, host queues the swap for the next render tick, no tearing, no dropped frames, live coding becomes trivial.
9. **`cargo generate hypercolor-effect`**. Scaffold a working effect project with one command. Thirty seconds from nothing to a running effect on a real device.
10. **Polyglot without pain**. Rust authors get the macro, AssemblyScript authors get the SDK, Zig/C authors get a C header. The same daemon loads them all because the raw ABI is four functions.

### Composition witchery

11. **Effect chaining**. A `compose` manifest lets the host run effect A's output as effect B's input canvas. Think ISF chains or Resolume layers, but as WASM modules plugged together by config.
12. **Named subcanvases**. A meta-effect owns multiple child canvases and composites them with blend modes. Split a 640x480 into four 320x240 effects and mix them live.
13. **Shared palette and LUT library** on the host side. Effects reference palettes by handle; the host manages them, keeps them warm, swaps them in response to scene changes without reloading the effect.
14. **Live parameter automation**. An LFO node reads `audio.loudness` or `audio.mel[3]` and drives a control input on any effect, authored declaratively in the scene config, no effect code change.

### Distribution witchery

15. **Git-native signed registry**. Steal the Zed extension model: GitHub PR to a central repo, CI builds and signs the `.wasm` + `.cwasm` bundle, daemon installs from URL. No central server to run.
16. **Content-addressed effect bundles**. Effects are identified by the hash of their bundle. Sharing is sending a URL or a 50 KB file. Reproducible, cacheable, diffable.
17. **Capability whitelist per effect**. `requires = ["audio", "screen", "network"]` in the manifest. Effects cannot read the filesystem, touch the network, or capture the screen without explicit opt-in at install time. VRChat learned this the hard way, so we start there.
18. **Browser preview runs the same binary**. The same `.wasm` effect can run in a web preview via `wasm-bindgen` shim, so effect galleries and pairing UIs show the real thing, not a mock.

### Advanced witchery (v0.3+)

19. **Dual-target effects**. An effect author marks a function as shader-safe and the build pipeline compiles it to a wgpu compute shader in addition to the WASM module. Native path uses the shader; WASM path is the portable fallback. One source of truth, two execution paths.
20. **WASI-NN for ML-driven effects**. Load a small audio classifier inside an effect so it can react to genre, mood, or vocal presence without host-side integration. Speculative but feasible by 2027.
21. **Snapshot / restore for stateful effects**. Serialize the guest's linear memory slice corresponding to effect state. Reset a particle system on beat, scrub back to an earlier moment, share a "performance state" as a savegame.

## Integration points

The new module lives at `crates/hypercolor-core/src/effect/wasm/` with five files:

- `mod.rs` exports `WasmEffectRenderer` which implements `EffectRenderer`
- `loader.rs` handles `.wasm` / `.cwasm` discovery, AOT compile, caching
- `host.rs` defines the host functions exposed to guests (logging, palette sampling, noise)
- `memory.rs` owns the custom `MemoryCreator` that backs linear memory with memfd
- `schema.rs` parses the declarative control schema exported by the guest

The existing `EffectPool` factory grows a third arm next to wgpu and Servo. Effects declare their renderer in metadata: `renderer = "wasm"`. The daemon routes accordingly.

A new `hypercolor-effect-sdk` crate (separate workspace member, publishable to crates.io) provides the guest-side types: `Frame`, `Canvas`, `ControlValue`, `AudioData`, `palette::Handle`, plus re-exports from `hypercolor-effect-macros` with the `#[derive(Effect)]` proc-macro.

A new `@hypercolor/effect-sdk` npm package mirrors the Rust SDK for AssemblyScript authors.

## Acid test

Port `color_wave.rs` from the builtin effects to the new WASM path. If the equivalent WASM effect cannot land in under ~120 lines with identical behavior, the API has failed and needs another iteration.

## Implementation phases

**v0.1 — Proof**. wasmtime loader, raw exports ABI, one hand-written WASM effect port of `color_wave`. Budget enforcement via epoch. No hot reload. No SDK macros. No registry. Ship to confirm the arithmetic in this doc.

**v0.2 — SDK**. `hypercolor-effect-sdk` + `#[derive(Effect)]`. Port 5 more builtins. Hot reload at frame boundaries. AOT cache. Capability manifest. AssemblyScript SDK alpha.

**v0.3 — Ecosystem**. Git-native registry, signed bundles, CLI install command, MIDI/OSC routing to control inputs. Browser preview shim.

**v0.4 — Composition**. Effect chaining, named subcanvases, shared palette library, parameter automation nodes.

**v0.5+ — Advanced**. Dual-target (WASM + wgpu shader), snapshot/restore, WASI-NN exploration.

## Risks and what we do not know

- **Some code shapes in wasmtime hit 4-6x slowdown** per Wingolog's April 2026 analysis. Those are call-heavy patterns, not per-pixel loops, but we should benchmark our real workloads early. If a specific builtin regresses, keep it on wgpu or native.
- **Hot reload at frame boundaries needs careful state handoff.** A stateful effect that owns a particle system should either export a state serializer or accept a reset on reload. SDK macro can auto-implement this for `#[derive(Effect)]` but hand-written guests have to opt in.
- **Guest language fragmentation risk.** If we over-invest in non-Rust guest SDKs before the core is proven, we will have three half-broken SDKs instead of one excellent one. Rust first, AssemblyScript second, everything else is documented-but-unofficial for v0.1.
- **wgpu/WASM duality (v0.5)** is ambitious. It might not land. The rest of the plan stands on its own regardless.
- **Registry operations cost time**. Zed's registry is git-based for a reason: no central server to run. We should copy the shape exactly. Running our own server is a distraction.

## Sources

Wave 1 research docs, all at `docs/research/wasm-effects-2026/`:

- `01-runtimes.md` — runtime landscape, wasmtime 36.0 LTS recommendation, embedding snippets
- `02-api-elegance.md` — API shape survey, Shape A "derive-the-contract" recommendation
- `03-perf-reality.md` — 95% of native verdict, SIMD numbers, call overhead, stress math
- `04-guest-languages.md` — polyglot toolchain matrix, raw exports first, skip ComponentizeJS/Py
- `05-hypercolor-prior-art.md` — complete internal synthesis of existing plugin and effect docs
- `06-prior-art-ecosystems.md` — Zed, Figma, Shopify, Envoy, WAM, Extism, Hyperlight patterns

Key external references:

- [Wasmer 6.0 announcement (95% of native)](https://wasmer.io/posts/announcing-wasmer-6-closer-to-native-speeds)
- [Wasmtime PoolingAllocationConfig docs](https://docs.wasmtime.dev/api/wasmtime/struct.PoolingAllocationConfig.html)
- [Life of a Zed Extension (WIT + wasmtime reference)](https://zed.dev/blog/zed-decoded-extensions)
- [Composing Components with Spin 2.0](https://www.fermyon.com/blog/composing-components-with-spin-2)
- [Proxy-Wasm spec (epoch-based per-request budgets)](https://github.com/proxy-wasm/spec)
- [Web Audio Modules 2 (SHM ring buffer patterns)](https://www.webaudiomodules.com/docs/intro/)
- [Wingolog April 2026 contrarian WASM perf analysis](https://wingolog.org/archives/2026/04/07/the-value-of-a-performance-oracle)
