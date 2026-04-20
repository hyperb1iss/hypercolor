# 04 · Guest Languages for Hypercolor WASM Effects

Research snapshot: April 2026. This document evaluates the developer experience
of writing WebAssembly effect plugins in every guest language with plausible
2026 viability, specifically sized for a Hypercolor effect: a compute-bound
plugin that reads a `FrameInput` (timing, audio spectrum, interaction state),
writes into a pixel buffer, and ships as a `.wasm` under a few hundred LOC.

## TL;DR Recommendation

Hypercolor effects are short, pure-compute, numerics-heavy. Binary size and
startup latency matter because we expect hundreds of effects loaded per
session. We cannot do all nine languages well in year one. The prioritized
slate:

**First-class DX (ship with a dedicated toolchain, templates, `just` recipes,
error-message quality gate):**

1. **Rust** via a Hypercolor-authored proc-macro crate (`hypercolor-effect`)
   that wraps `wit-bindgen` and generates the glue automatically. This is
   the most important investment, because most serious effect authors will
   choose Rust for speed + ecosystem + the same language the daemon is in.
2. **AssemblyScript** for the much larger population of non-systems authors
   coming from SignalRGB, Shadertoy, and generative art tooling. TypeScript-
   shaped syntax, ~4 KB runtime, zero pain to install (`npm i -g assemblyscript`).
   This is the democratization lane; it is what makes Hypercolor a polyglot
   ecosystem in practice rather than in theory.

**Supported but not optimized (documented, one working sample each, we
ship `wit` files they can consume but we do not hand-hold):**

3. **Zig** for size fanatics who want sub-1 KB binaries. The community is
   small but passionate and its output is genuinely the smallest.
4. **C / C++** via `wasi-sdk` for the shadertoy / GLSL-to-C porters. Works
   fine with the same raw-export ABI Rust uses; no special tooling required.
5. **TinyGo** as a courtesy for Go developers. Binary sizes are acceptable
   (100-400 KB), Component Model works since v0.33.

**Skip for v1, revisit when the story improves:**

6. **ComponentizeJS / Jco**: ~8 MB per component because StarlingMonkey is
   bundled whole. Would balloon the effects directory from MBs to GBs. Wait
   for shared-engine embedding to land.
7. **componentize-py**: ~35 MB per component (entire CPython bundled) with
   no practical path to smaller. Not viable for a library of effects.

**Standardization decision:** ship a **raw-export ABI** first, add Component
Model support as an opt-in second lane. The raw ABI is 4 exported functions
over a shared linear memory buffer. It works identically in every language
today, is trivial to document, and costs about 500 bytes of per-effect
overhead. Component Model is the right long-term story but the guest-side
tooling is still jagged in 2026 for several of the languages we care about.

The rest of this document shows the work.

---

## 1 · Rust → WASM in 2026

### Target landscape

Rust has three WASM targets that matter. Picking the right one is the first
decision effect authors will make.

| Target | Tier | Stdlib | Use Case |
|---|---|---|---|
| `wasm32-unknown-unknown` | Tier 2 with host tools | Partial (many functions panic or no-op) | Browser-style raw-export modules, Extism-style plugins |
| `wasm32-wasip1` | Tier 2 with host tools | Full (WASI preview 1 syscalls) | Legacy WASI host integration |
| `wasm32-wasip2` | Tier 2 | Full (WASI preview 2, Component Model) | 2026 default for components |
| `wasm32v1-none` | Tier 2 | `core` + `alloc` only | Embedded-style, stable `no_std`, smallest binaries |

Rust 1.82 made `wasm32-wasip2` a native rustc target, meaning plain `cargo
build --target wasm32-wasip2` emits a Component Model component. You no
longer need `cargo-component` unless you have custom (non-WASI) WIT
interfaces, which Hypercolor does. For Hypercolor effects we *do* have a
custom WIT world (our `effect` interface), so `cargo-component` or the
`wit-bindgen::generate!` macro is still the right entry point.

### Tooling stack for a Component Model guest

In 2026 the mainstream recommendation converges on:

- **`wit-bindgen`** (`generate!` proc-macro) for Rust guests that implement
  a custom WIT world. This is what a `hypercolor-effect` plugin will use.
- **`cargo-component`** (last release 0.21.1, April 2024, still current as
  of April 2026) for scaffolding, binding subcommands, and registry
  workflows. The project is in maintenance mode because most of what it
  did for Rust is now native in rustc, which is actually healthy.
- **`wasm-bindgen`** only matters if you are targeting JS interop on the
  web. For server / daemon hosts like Hypercolor, it is the wrong tool.
- **`wasm-tools`** for composing, validating, and optimizing components.

For the non-Component path (raw exports), you skip all of the above and just
use `#[unsafe(no_mangle)] pub extern "C" fn` on whatever entry points the
host expects. This is what Extism and most existing plugin systems do, and
it has the virtue that it works on stable Rust with zero extra tooling.

### Binary size

For a representative "simple Rust function compiled with
`opt-level = "z"` + LTO + `wasm-opt -Oz`":

- Minimal no-panic function: ~800 bytes on `wasm32v1-none`
- Same function with panic infrastructure: ~17-27 KB
- A realistic effect with color-math helpers and no other deps: **30-60 KB**
- Same effect as a Component Model component with WASI adapter: ~90-150 KB

The panic-handler cliff is real. Effect authors should get `panic = "abort"`
in the template, and our proc-macro should wire up `#[panic_handler]` for
`wasm32v1-none` users so they start at 30 KB instead of 100 KB.

### `no_std` compatibility

`wasm32v1-none` is the clean path for effects that want to be tiny. We can
offer `core` + `alloc` and a tiny heap (wee-alloc or dlmalloc) in the
effect template. Most effects do not need anything beyond `libm` for
trig/exp and our own color helpers.

### Compile time

Cold build of a minimal effect: ~8-15 seconds on a modern laptop (mostly
LLVM). Incremental rebuild: ~2-4 seconds. Both are livable but feel slow
compared to AssemblyScript's ~200 ms incrementals. For a tight inner loop
("edit, save, see LEDs change") we need to keep this under 5 seconds total.

### "Hello pixel" skeleton (raw-export ABI)

```rust
// Cargo.toml
// [package]
// name = "effect-rainbow-wave"
// [lib]
// crate-type = ["cdylib"]
// [profile.release]
// opt-level = "z"
// lto = true
// panic = "abort"
// strip = true

#![no_std]
extern crate alloc;

use core::f32::consts::TAU;

const W: usize = 320;
const H: usize = 200;
const BYTES: usize = W * H * 4;

static mut CANVAS: [u8; BYTES] = [0; BYTES];

#[unsafe(no_mangle)]
pub extern "C" fn canvas_ptr() -> *mut u8 { unsafe { CANVAS.as_mut_ptr() } }

#[unsafe(no_mangle)]
pub extern "C" fn canvas_len() -> usize { BYTES }

#[unsafe(no_mangle)]
pub extern "C" fn render(time_ms: u32, audio_rms: f32) {
    let t = time_ms as f32 / 1000.0;
    for y in 0..H {
        for x in 0..W {
            let hue = (x as f32 / W as f32 + t * 0.25) % 1.0;
            let v = 0.5 + 0.5 * audio_rms * libm::sinf(TAU * y as f32 / H as f32);
            let (r, g, b) = hsv_to_rgb(hue, 1.0, v);
            let i = (y * W + x) * 4;
            unsafe {
                CANVAS[i] = r; CANVAS[i + 1] = g; CANVAS[i + 2] = b; CANVAS[i + 3] = 255;
            }
        }
    }
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (u8, u8, u8) { /* ... */ todo!() }

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { core::arch::wasm32::unreachable() }
```

The full Component Model version replaces `#[unsafe(no_mangle)]` with a
`wit_bindgen::generate!` macro and a `Guest` trait implementation. The
proc-macro approach (section 8) collapses both to a single `#[effect]`
attribute.

---

## 2 · Zig → WASM in 2026

### State of the toolchain

Zig's freestanding wasm32 story has been strong for years. `zig build-lib
-target wasm32-freestanding -dynamic -rdynamic -O ReleaseSmall` produces
tiny modules, the `export` keyword is clean, and there is no runtime beyond
what you explicitly link. A Mandelbrot kernel lands at **232 bytes** on
`ReleaseSmall`. There is no hidden cost: no GC, no allocator by default,
no panic infrastructure unless you opt in. You get what you write.

WASI support (`wasm32-wasi`) exists but lags. Component Model guest
support is not native in the Zig compiler as of early 2026. The community
workaround is to use `wit-bindgen` with its **C backend**, then compile the
generated C headers together with Zig source through `zig build`, and
finally stitch the result into a component with `wasm-tools component new`
and the preview1 adapter. This works but is fiddly. It is not the kind of
thing we can tell casual effect authors to do.

The implication for Hypercolor: Zig fits the **raw-export ABI** lane
beautifully and the **Component Model** lane badly. If we ship a raw ABI
as the primary path, Zig is a first-class citizen. If we ship only
Component Model, Zig authors will wait for native bindgen support.

### Binary size

Zig is the binary-size champion. For an effect kernel:

- Trivial function: 100-500 bytes
- Full rainbow-wave effect with HSV math and a 320x200 buffer: **~1-3 KB**
- Same effect with WASI output: +2-5 KB

This is an order of magnitude smaller than Rust release + `opt-z`, and two
orders of magnitude smaller than TinyGo or ComponentizeJS.

### "Hello pixel" skeleton (raw-export ABI)

```zig
// build: zig build-lib effect.zig -O ReleaseSmall \
//        -target wasm32-freestanding -dynamic -rdynamic

const W: u32 = 320;
const H: u32 = 200;
var canvas: [W * H * 4]u8 = undefined;

export fn canvas_ptr() [*]u8 { return &canvas; }
export fn canvas_len() u32 { return W * H * 4; }

export fn render(time_ms: u32, audio_rms: f32) void {
    const t = @as(f32, @floatFromInt(time_ms)) / 1000.0;
    var y: u32 = 0;
    while (y < H) : (y += 1) {
        var x: u32 = 0;
        while (x < W) : (x += 1) {
            const hue = @mod(@as(f32, @floatFromInt(x)) / @as(f32, @floatFromInt(W)) + t * 0.25, 1.0);
            const v = 0.5 + 0.5 * audio_rms * @sin(2.0 * std.math.pi * @as(f32, @floatFromInt(y)) / @as(f32, @floatFromInt(H)));
            const rgb = hsvToRgb(hue, 1.0, v);
            const i = (y * W + x) * 4;
            canvas[i] = rgb[0];
            canvas[i + 1] = rgb[1];
            canvas[i + 2] = rgb[2];
            canvas[i + 3] = 255;
        }
    }
}

fn hsvToRgb(h: f32, s: f32, v: f32) [3]u8 { /* ... */ }

const std = @import("std");
```

The global-buffer + `canvas_ptr`/`canvas_len` pattern is idiomatic in the
Zig WASM community (see `minimal-zig-wasm-canvas`, `zig-wasm-audio-
framebuffer`). It maps one-to-one to the Rust raw-export pattern, which is
why a single Hypercolor ABI spec covers both.

---

## 3 · C / C++ → WASM in 2026

### Toolchain choice

There are three credible toolchains. The choice is unambiguous for us:

- **`wasi-sdk`** (clang + LLVM, maintained by the Bytecode Alliance). The
  right answer for server / daemon-hosted wasm. Produces a standalone
  `.wasm` with no JS glue. Latest wasi-sdk releases target the `wasm32-
  wasip2` triple and support reactor-mode components via
  `-mexec-model=reactor`. This is what Hypercolor should document.
- **Emscripten** is focused on the browser + Node case. Emits JS glue.
  Wrong tool for our host.
- **Cheerp** is a niche proprietary-friendly alternative, no reason to
  prefer it for an open-source plugin ecosystem.

`wasi-sdk` binaries can be larger than Emscripten's for equivalent code,
not because wasi-sdk is worse but because Emscripten aggressively strips
libc and relies on JS for missing functions. For a compute-bound effect
with minimal libc dependency, wasi-sdk numbers are fine.

### Binary size

- Trivial effect, no libc calls: 5-15 KB
- Effect with `libm` (sin, cos, pow): 20-50 KB
- Same effect after `wasm-opt -Oz` + `strip`: shave ~30-40%

### "Hello pixel" skeleton (raw-export ABI)

```c
// clang --target=wasm32-unknown-unknown -O2 -nostdlib \
//   -Wl,--no-entry -Wl,--export-all effect.c -o effect.wasm

#include <math.h>
#include <stdint.h>

#define W 320
#define H 200
#define BYTES (W * H * 4)

static uint8_t canvas[BYTES];

uint8_t* canvas_ptr(void) { return canvas; }
uint32_t  canvas_len(void) { return BYTES; }

static void hsv_to_rgb(float h, float s, float v, uint8_t out[3]);

void render(uint32_t time_ms, float audio_rms) {
    float t = time_ms / 1000.0f;
    for (uint32_t y = 0; y < H; y++) {
        for (uint32_t x = 0; x < W; x++) {
            float hue = fmodf((float)x / W + t * 0.25f, 1.0f);
            float val = 0.5f + 0.5f * audio_rms * sinf(6.28318f * (float)y / H);
            uint8_t rgb[3]; hsv_to_rgb(hue, 1.0f, val, rgb);
            uint32_t i = (y * W + x) * 4;
            canvas[i] = rgb[0]; canvas[i+1] = rgb[1];
            canvas[i+2] = rgb[2]; canvas[i+3] = 255;
        }
    }
}
```

For authors coming from Shadertoy / GLSL, this pattern is the most natural
(it is basically "run a shader across a framebuffer"). They do not need
anything WASI-specific; we host them with the same linker flags the Zig
and Rust raw-export authors use.

---

## 4 · AssemblyScript → WASM in 2026

### Maturity

AssemblyScript is healthy: ~50k weekly npm installs, 29k GitHub projects
use it, and the compiler continues to ship updates under active
maintenance. What it is *not* is part of the Bytecode Alliance standards
track. Starting in v0.21 the project removed first-party WASI support from
its stdlib and published a standards-objections document explaining why it
views WASI and the Component Model as problematic for open standards. The
community maintains `@assemblyscript/wasi-shim` and `as-wasi` for users
who want WASI anyway. This is a political divide, not a technical dead
end, but it matters for us: **AssemblyScript will not give you a
Component Model component out of the box**, and there is no official plan
for that to change.

The implication for Hypercolor is the opposite of the political implication
for the wider ecosystem. If we are shipping a raw-export ABI for Rust, C,
and Zig authors anyway, AssemblyScript lives naturally in the same lane
with no special accommodation. The WASI/Component friction does not affect
us because we are not using WASI-style imports for effect plugins.

### Why AssemblyScript matters for Hypercolor

This is the lane for the large population of effect authors who:

- Already write TypeScript and have `npm` installed
- Came from SignalRGB, WebGL / Shadertoy, or generative-art tools
- Do not want to install a Rust toolchain
- Do not care about the 2x performance delta vs Rust (effects are small)

The install + build story is, frankly, the best of any language in this
document:

```
npm install --save-dev assemblyscript
npx asinit .
npx asc assembly/index.ts --target release -o effect.wasm
```

The result is a sub-5 KB `.wasm` with no separate runtime files needed for
Hypercolor's raw-export ABI (the usual AssemblyScript loader is for
browser JS interop, which we don't do).

### Binary size

Benchmarks comparing identical sort algorithms: AssemblyScript **4.7 KB**
(3.5 KB binary + 1.2 KB runtime) vs Rust 44 KB + 74 KB bootstrap. For
pixel-pushing effects of a few hundred LOC the numbers land at ~3-10 KB.
This is smaller than Rust by a factor of ~4-10x and within shouting
distance of Zig.

Runtime choice matters: `--runtime stub` (no GC, no `new`) gets you the
smallest binaries but forbids allocation. `--runtime minimal` (minimal GC)
is the typical effect target. `--runtime incremental` is for long-running
programs and is overkill for per-frame effects.

### "Hello pixel" skeleton

```typescript
// assembly/index.ts
// npx asc assembly/index.ts --target release --runtime minimal -o effect.wasm

const W: u32 = 320;
const H: u32 = 200;
const BYTES: u32 = W * H * 4;
const canvas = new Uint8Array(BYTES);

export function canvasPtr(): usize { return changetype<usize>(canvas.buffer); }
export function canvasLen(): u32 { return BYTES; }

export function render(timeMs: u32, audioRms: f32): void {
    const t: f32 = <f32>timeMs / 1000.0;
    for (let y: u32 = 0; y < H; y++) {
        for (let x: u32 = 0; x < W; x++) {
            const hue: f32 = ((<f32>x / <f32>W) + t * 0.25) % 1.0;
            const v: f32 = 0.5 + 0.5 * audioRms * Mathf.sin(Mathf.PI * 2 * <f32>y / <f32>H);
            const rgb = hsvToRgb(hue, 1.0, v);
            const i = (y * W + x) * 4;
            canvas[i + 0] = rgb.r;
            canvas[i + 1] = rgb.g;
            canvas[i + 2] = rgb.b;
            canvas[i + 3] = 255;
        }
    }
}

class RGB { r: u8 = 0; g: u8 = 0; b: u8 = 0; }
function hsvToRgb(h: f32, s: f32, v: f32): RGB { /* ... */ return new RGB(); }
```

Notice this is "TypeScript you squinted at": types are stricter (`u32`,
`u8`, `f32`), arithmetic requires casts, no `any`, no dynamic objects.
This is precisely why it compiles to tiny wasm, and it is also precisely
why an author who knows TypeScript can be productive in an hour.

---

## 5 · Go / TinyGo → WASM in 2026

### Why not stdlib Go?

Standard Go's `GOOS=js GOARCH=wasm` (the browser target) and `GOOS=wasip1
GOARCH=wasm` emit binaries of 2-10 MB for trivial programs because the Go
runtime (goroutine scheduler, GC, reflection tables) ships whole. That is
a non-starter for a plugin ecosystem.

### TinyGo

TinyGo is the answer. Different compiler entirely, LLVM-based, with a
conservative mark-sweep GC designed for small heaps and aggressive dead
code elimination. WASIP2 (Component Model) support has been stable since
v0.33, and the current release (v0.40 as of late 2025) brings GC
improvements up to 10% faster, LLVM 20 support, and an experimental Boehm
GC variant for WebAssembly.

Building a TinyGo component looks like:

```
tinygo build -target=wasip2 \
    --wit-package ./wit \
    --wit-world effect \
    -o effect.wasm main.go
```

### Binary size

Numbers from real measurements:

- Plain Go `GOOS=wasip1`: **2-10 MB** for trivial programs. Not viable.
- TinyGo default: **100-400 KB** for a real effect
- TinyGo after `-no-debug -opt=z -panic=trap`: down to ~60-200 KB
- Community claim (Fermyon): 1.1 MB reduced to 377 KB with optimization

This is 10-50x larger than Zig or AssemblyScript and 3-10x larger than
Rust, but not catastrophic. If you are already a Go programmer, TinyGo is
an acceptable price to pay. If you are choosing from scratch with binary
size as a criterion, you would not pick Go.

### "Hello pixel" skeleton

```go
// tinygo build -o effect.wasm -target wasip1 -no-debug -opt=z main.go
package main

import "math"

const W, H = 320, 200
var canvas = make([]byte, W*H*4)

//go:wasmexport canvas_ptr
func canvasPtr() uintptr { return uintptr(unsafe.Pointer(&canvas[0])) }

//go:wasmexport canvas_len
func canvasLen() uint32 { return W * H * 4 }

//go:wasmexport render
func render(timeMs uint32, audioRms float32) {
    t := float32(timeMs) / 1000.0
    for y := 0; y < H; y++ {
        for x := 0; x < W; x++ {
            hue := math.Mod(float64(x)/W + float64(t)*0.25, 1.0)
            v := 0.5 + 0.5*float64(audioRms)*math.Sin(2*math.Pi*float64(y)/H)
            r, g, b := hsvToRGB(float32(hue), 1.0, float32(v))
            i := (y*W + x) * 4
            canvas[i] = r; canvas[i+1] = g; canvas[i+2] = b; canvas[i+3] = 255
        }
    }
}

func hsvToRGB(h, s, v float32) (byte, byte, byte) { /* ... */ return 0, 0, 0 }
func main() {}
```

`//go:wasmexport` is the TinyGo-specific directive for raw exports; for
Component Model guests you use `wit-bindgen-go` and implement generated
interfaces. Both work in v0.40.

---

## 6 · JavaScript → WASM (ComponentizeJS / Jco) in 2026

### State of the tooling

Jco (Bytecode Alliance) reached 1.0 with full WASI 0.2 support. The JS-to-
wasm compilation path (ComponentizeJS) is still labeled experimental but
shipped and production-used at Fastly / Fermyon. Under the hood it is
**StarlingMonkey** (a SpiderMonkey fork, compiled to wasm, targeting WASI
0.2) embedded into every component. `Wizer` pre-initializes the engine,
parses your source, and snapshots the state so instantiation is fast at
runtime. Optional **Weval AOT** precompiles the inline caches for further
speedup.

### The size problem

StarlingMonkey bundled into every component costs **~8 MB** per `.wasm`.
A "hello world that adds two numbers" is ~8 MB. This is not a
"JavaScript is slow" problem; it is a "we bundled a full JS engine into
each effect" problem. The Bytecode Alliance roadmap explicitly plans to
let components share a single SpiderMonkey embedding so that N effects
cost 8 MB plus N * (user code), but that is not shipped today.

For Hypercolor, which wants ~100-500 effects in a library, this would
mean ~0.8-4 GB of redundant JS engine binaries. Not viable.

### Startup

Thanks to Wizer snapshots, startup is sub-millisecond after the initial
engine load. So the cost is pure disk + memory footprint, not runtime
latency. If we had only a handful of JS effects in the whole ecosystem,
the tradeoff would be fine; at scale it is not.

### "Hello pixel" skeleton

```javascript
// componentize-js componentize effect.js -w wit/effect.wit -o effect.wasm
const W = 320, H = 200;
const canvas = new Uint8Array(W * H * 4);

export function render(timeMs, audioRms) {
    const t = timeMs / 1000;
    for (let y = 0; y < H; y++) {
        for (let x = 0; x < W; x++) {
            const hue = ((x / W) + t * 0.25) % 1;
            const v = 0.5 + 0.5 * audioRms * Math.sin(2 * Math.PI * y / H);
            const [r, g, b] = hsvToRgb(hue, 1, v);
            const i = (y * W + x) * 4;
            canvas[i] = r; canvas[i+1] = g; canvas[i+2] = b; canvas[i+3] = 255;
        }
    }
    return canvas;
}
```

The code is lovely, the binary is 8 MB. Pass for v1.

---

## 7 · Python → WASM (componentize-py) in 2026

### State of the tooling

`componentize-py` (Bytecode Alliance) bundles CPython, wasi-libc, and any
native extensions into a single component. Recent releases track WASI 0.3
RC work (Feb 2026 milestone). It works, the WIT bindings are clean, the
API is unsurprising to Python developers.

### The size problem, worse

Hello-world output: **~35 MB**. That is a static CPython + stdlib in
every component. There is an open issue proposing host-provided libpython
so components could import instead of bundle, but it has not shipped and
would require runtime cooperation we do not have.

### "Hello pixel" skeleton

```python
# componentize-py --wit-path wit -w effect componentize app -o effect.wasm
import math

W, H = 320, 200

class Effect:
    def __init__(self):
        self.canvas = bytearray(W * H * 4)

    def render(self, time_ms: int, audio_rms: float) -> bytes:
        t = time_ms / 1000
        for y in range(H):
            for x in range(W):
                hue = ((x / W) + t * 0.25) % 1
                v = 0.5 + 0.5 * audio_rms * math.sin(2 * math.pi * y / H)
                r, g, b = hsv_to_rgb(hue, 1, v)
                i = (y * W + x) * 4
                self.canvas[i:i+4] = bytes([r, g, b, 255])
        return bytes(self.canvas)
```

For a 320x200 canvas, that double-nested loop in pure Python will miss
every frame budget we have. NumPy is not available without more bundling
pain. The right way to use Python in WASM is for orchestration / glue,
not per-pixel kernels. Wrong fit for effects.

---

## 8 · `hypercolor-effect` Proc-Macro: What We Build Ourselves

### Goal

Collapse the entire effect skeleton (exports, panic handler, buffer
management, HSV helpers, control-value decoding) into a single macro so
an author writes:

```rust
use hypercolor_effect::{effect, FrameInput, Canvas, Color, hsv};

#[effect]
fn render(frame: &FrameInput, canvas: &mut Canvas) {
    let t = frame.time_secs();
    for (x, y, pixel) in canvas.iter_mut() {
        let hue = (x / canvas.width() as f32 + t * 0.25) % 1.0;
        let v = 0.5 + 0.5 * frame.audio.rms * (std::f32::consts::TAU * y / canvas.height() as f32).sin();
        *pixel = hsv(hue, 1.0, v);
    }
}
```

That is it. The whole file.

### What the macro generates

- `#[unsafe(no_mangle)] extern "C" fn hypercolor_init()` that allocates
  the pixel buffer (size from `#[effect(canvas = "640x480")]` attr or
  defaulting to 320x200)
- `hypercolor_canvas_ptr()` / `hypercolor_canvas_len()` exports
- `hypercolor_render(time_ms: u32, audio_ptr: *const u8, audio_len: u32,
  ...)` that decodes the `FrameInput` struct from linear memory, calls
  the user's `render`, and returns
- A `#[panic_handler]` that logs via a host import and traps
- A `wasm_bindgen`-style extern block for the 2-3 host functions we
  actually allow (log, request control update, read controls buffer)
- A static `HYPERCOLOR_EFFECT_MANIFEST` containing name / description /
  declared controls as a serialized JSON blob in a custom section, so
  the daemon can read metadata without instantiating the wasm

### What else the crate provides

- `Canvas` wrapper with `.iter_mut()`, `.set(x, y, color)`, `.fill(color)`
- `Color`, `hsv()`, `oklab()`, gradient helpers (we want perceptual color
  available by default, LEDs look awful with naive sRGB)
- `FrameInput` mirroring the daemon's struct, with `.audio.rms`, `.audio.
  spectrum(band)`, `.interaction.pointer`, etc.
- `#[effect(controls(speed(0.0..=1.0), palette("rainbow")))]` for declaring
  user-tweakable controls; the macro emits both runtime accessors and
  manifest metadata
- Feature-gated `std` vs `no_std` (default `no_std` with `alloc`)

### Estimated boilerplate reduction

From ~80 lines of raw-export Rust (plus `Cargo.toml` ceremony) to
**the `#[effect]` function body + Cargo dep on `hypercolor-effect`**. A
trivial effect becomes ~8 lines. The macro is maybe ~400 lines of
proc-macro code for us to write and maintain, which is a reasonable
investment for the DX win and is the single most impactful thing we can
do to make Rust-authored effects pleasant.

### Parallel for other languages

- AssemblyScript: publish `@hypercolor/effect-sdk` on npm, a `.d.ts`
  + tiny runtime mirror of the same API. `import { effect, hsv } from
  '@hypercolor/effect-sdk'` with the same shape.
- Zig: a `hypercolor_effect.zig` that can be `@import`ed; provides
  `Canvas`, `hsv`, export macros via Zig comptime.
- C: a `hypercolor_effect.h` single-header. Old-school but inevitable.

One spec document, four SDK implementations, one ABI.

---

## 9 · WIT vs Raw Exports: Standardization Decision

### Where Component Model is in April 2026

- WASI Preview 2 is stable ("island of stability, relied on indefinitely"
  per the rustc docs).
- Component Model is production-used at Fastly, Fermyon, Shopify.
- Rust support is native in rustc (1.82+) with `wasm32-wasip2`.
- TinyGo supports wasip2 since v0.33.
- Jco 1.0 ships for JS hosting; ComponentizeJS 1.0 ships for JS guests.
- componentize-py tracks WASI 0.3 RCs.
- **Zig does not have native Component Model bindgen support.** Community
  uses `wit-bindgen` C backend + Zig's C interop.
- **AssemblyScript actively opposes WASI/Component Model on standards
  grounds.** No support planned.
- wasi-sdk (C/C++) supports reactor components with `-mexec-model=reactor`.

### What this means for us

Committing to Component Model as the only ABI excludes AssemblyScript
(the non-systems-programmer lane) and makes Zig awkward. Both are
languages we want in the ecosystem.

The raw-export ABI (export a few functions over linear memory, use one
simple `FrameInput` / `Canvas` binary encoding) works identically in
every language today with no extra tooling. It is also what Extism has
shipped in production for years with 9-language PDK coverage, which is
useful social proof.

### Recommendation

Two-lane ABI:

- **Lane A (raw exports, default):** Every language works. The
  `hypercolor-effect` SDKs generate this. No WIT. No preview-2 adapter.
  The wasm is portable to any runtime that can call exported functions
  and poke at linear memory. Effects are tiny.
- **Lane B (Component Model, opt-in):** Ship a `hypercolor:effect@0.1.0`
  WIT world for authors who want typed bindings and language-agnostic
  interoperability. Rust and TinyGo authors get this through their
  respective bindgen tooling. We host these components with wasmtime's
  component API. C/C++ works via reactor modules. AssemblyScript and
  Zig opt out, which is fine.

Start shipping Lane A first. Add Lane B in a follow-up once Lane A is
proven. The two lanes can coexist; the daemon can sniff which one the
loaded module implements.

---

## 10 · Cross-Language Ergonomics

### Edit-save-reload loop speed

| Language | Cold build | Incremental | Notes |
|---|---|---|---|
| AssemblyScript | ~2 s | **~200 ms** | Fastest by a large margin |
| Zig | ~1-3 s | ~500 ms | LLVM-based but small input |
| C (wasi-sdk) | ~1-2 s | ~300 ms | Clang is fast on small files |
| Rust (debug) | ~8-15 s | ~2-4 s | LLVM + generics + monomorphization |
| Rust (release) | ~20-40 s | ~5-10 s | LTO and `opt-z` cost time |
| TinyGo | ~5-15 s | ~3-8 s | Full link every time |
| ComponentizeJS | ~5 s | ~2 s | Wizer snapshot dominates |
| componentize-py | ~10-20 s | ~5-10 s | Bundling CPython is slow |

For "save and see LEDs respond instantly" the daemon needs to watch the
effect `.wasm` directory, re-instantiate on change, and swap atomically.
This is straightforward with wasmtime's `Module::deserialize`. The build
side is the bottleneck, and AssemblyScript wins it outright.

Hypercolor should ship `just effect-dev <name>` that watches source files
and triggers the right compiler. We can also provide `hypercolor effect
repl` that hot-reloads without needing a file-system watcher.

### Error message quality

Subjective ranking for "developer pastes a mistake, reads the compiler
output, figures out the fix":

1. **Rust** - industry-leading diagnostics, though the proc-macro layer
   can degrade quality if we are sloppy. Spending time on the macro's
   error messages is worth it.
2. **AssemblyScript** - inherits TypeScript's solid diagnostic pedigree,
   occasionally awkward when wasm types mismatch.
3. **Zig** - very clear for type errors and stack traces; comptime errors
   can be cryptic.
4. **TinyGo** - Go's errors are plain and serviceable.
5. **C/C++** - better than their historical reputation but still terse.
6. **componentize-py** - Python tracebacks are fine, but errors that
   happen during componentization land in Rust-layer stack traces that
   are unfriendly.
7. **ComponentizeJS** - similar issue, JS-layer errors are clear,
   componentization errors are not.

We can materially improve the experience on every language by wrapping
their tools in `just effect-check <name>` recipes that re-format errors
consistently, surface the right source file, and add a one-line "fix
hint" for the top ~10 common mistakes per language. Small investment,
significant DX win.

---

## 11 · Binary Size Targets

Realistic sizes for a ~200 LOC effect with color math, no external
dependencies, compiled release + optimized:

| Language | Typical .wasm | With `wasm-opt -Oz` | Verdict |
|---|---|---|---|
| Zig (`ReleaseSmall`) | 1-3 KB | 1-2 KB | Champion |
| AssemblyScript (`release`, `minimal` runtime) | 3-10 KB | 3-7 KB | Excellent |
| C (`wasi-sdk`, `-O2`) | 10-30 KB | 6-20 KB | Excellent |
| Rust (`opt-z`, LTO, `panic=abort`) | 30-80 KB | 20-60 KB | Good |
| Rust (component with WASI adapter) | 90-180 KB | 70-140 KB | Acceptable |
| TinyGo (`-opt=z`, `-no-debug`) | 100-400 KB | 80-300 KB | Tolerable |
| ComponentizeJS | ~8 MB | ~8 MB | Unacceptable for a library |
| componentize-py | ~35 MB | ~35 MB | Unacceptable |

A **target effect size ceiling of 500 KB** covers everything except
Python and JavaScript and reflects what distribution looks like if we
ship, say, 500 effects: 250 MB total, fits comfortably in a repository
or a tarball. The cap also gives us a clean rejection story for
ComponentizeJS and componentize-py that is not subjective ("your binary
is too big" not "we do not like your language").

---

## 12 · Final Recommendation

### Invest heavily

**Rust + `hypercolor-effect` proc-macro (section 8).** Most serious
effect authors will choose Rust because it is the daemon's language,
because it is the systems-programming lingua franca, and because the
proc-macro lets us offer an authoring experience nobody else in the RGB
space has. This is our flagship.

**AssemblyScript + `@hypercolor/effect-sdk` npm package.** The lane for
non-systems authors. TypeScript-shaped, tiny binaries, one npm install
away. Without this we are not actually a polyglot ecosystem, we are
"Rust with leftovers". The SDK is small to write and to maintain.

### Document and sample-but-don't-chase

**Zig**, **C / C++**, **TinyGo.** Publish one working sample effect for
each, document the raw-export ABI so the language's existing WASM
tooling works with no Hypercolor-specific extensions, link to community
resources. No dedicated SDK, no first-class `just` recipes, but we ship
effects authored in these languages proudly if someone contributes one.

### Skip for v1

**ComponentizeJS** (~8 MB per component, balloons the effect library).
Revisit when Bytecode Alliance ships shared-engine embedding.

**componentize-py** (~35 MB per component, pure-Python per-pixel loops
are too slow anyway). Effects are the wrong use case for Python in wasm.

### Standardization

Raw-export ABI **first** (Lane A). Add Component Model **second** (Lane
B) as an opt-in. The daemon can load both.

### Build order

1. Specify the raw-export ABI (`FrameInput` binary layout, exported
   function names, host imports). One short spec doc.
2. Build `hypercolor-effect` proc-macro + core types.
3. Build `@hypercolor/effect-sdk` AssemblyScript package.
4. Write 3-5 real effects in each to dogfood the SDKs.
5. Write the Zig / C / TinyGo sample effects against the same ABI.
6. Publish WIT for Lane B. Wait for real user demand before investing
   more in Component Model tooling.

This plan lets us ship a credible polyglot WASM effect system in a
quarter, with first-class DX for the two languages that matter most and
a no-stigma supported path for everyone else.

---

## Sources

- [wasm32-wasip2 — The rustc book](https://doc.rust-lang.org/nightly/rustc/platform-support/wasm32-wasip2.html)
- [wasm32-wasip1 — The rustc book](https://doc.rust-lang.org/rustc/platform-support/wasm32-wasip1.html)
- [wasm32-unknown-unknown — The rustc book](https://doc.rust-lang.org/beta/rustc/platform-support/wasm32-unknown-unknown.html)
- [wasm32v1-none — The rustc book](https://doc.rust-lang.org/beta/rustc/platform-support/wasm32v1-none.html)
- [Changes to Rust's WASI targets — Rust Blog](https://blog.rust-lang.org/2024/04/09/updates-to-rusts-wasi-targets/)
- [cargo-component — Bytecode Alliance](https://github.com/bytecodealliance/cargo-component)
- [wit-bindgen — Bytecode Alliance](https://github.com/bytecodealliance/wit-bindgen)
- [wit-bindgen generate! macro docs](https://docs.rs/wit-bindgen/latest/wit_bindgen/macro.generate.html)
- [The wasm-bindgen Guide](https://rustwasm.github.io/docs/wasm-bindgen/)
- [Shrinking .wasm Size — Rust and WebAssembly Book](https://rustwasm.github.io/book/game-of-life/code-size.html)
- [WASM Size Diet: Rust Binaries Under One Megabyte](https://medium.com/beyond-localhost/wasm-size-diet-rust-binaries-under-one-megabyte-9104c1bc30b2)
- [Pixel Buffer Rendering in WASM with Rust — Cogs and Levers](https://tuttlem.github.io/2024/12/07/pixel-buffer-rendering-in-wasm-with-rust.html)
- [Building Native Plugin Systems with WebAssembly Components — Sy Brand](https://tartanllama.xyz/posts/wasm-plugins/)
- [Plugins with Rust and WASI Preview 2](https://benw.is/posts/plugins-with-rust-and-wasi)
- [Zig for WebAssembly guide — vExcess](https://vexcess.github.io/blog/zig-for-webassembly-guide.html)
- [Zig in WebAssembly — Fermyon](https://developer.fermyon.com/wasm-languages/zig)
- [Using Zig with WebAssembly — Send numbers, strings, objects](https://blog.mjgrzymek.com/blog/zigwasm)
- [minimal-zig-wasm-canvas](https://github.com/daneelsan/minimal-zig-wasm-canvas)
- [zig-wasm-audio-framebuffer](https://github.com/ringtailsoftware/zig-wasm-audio-framebuffer)
- [Zig and the WASM Component Model — Vigoo](https://blog.vigoo.dev/posts/zig-wasm-component-model/)
- [WASI-SDK — WebAssembly C/C++ toolchain](https://github.com/WebAssembly/wasi-sdk)
- [C/C++ — The WebAssembly Component Model](https://component-model.bytecodealliance.org/language-support/building-a-simple-component/c.html)
- [The C language in WebAssembly — Fermyon](https://developer.fermyon.com/wasm-languages/c-lang)
- [WASI Command and Reactor Modules — Dylibso](https://dylibso.com/blog/wasi-command-reactor/)
- [What is the difference between wasi-sdk and emscripten?](https://github.com/WebAssembly/wasi-sdk/issues/222)
- [AssemblyScript Book](https://www.assemblyscript.org/)
- [AssemblyScript Implementation Status](https://www.assemblyscript.org/status.html)
- [AssemblyScript Standards Objections](https://www.assemblyscript.org/standards-objections.html)
- [AssemblyScript wasi-shim](https://github.com/AssemblyScript/wasi-shim)
- [WebAssembly: TinyGo vs Rust vs AssemblyScript — Ecostack](https://ecostack.dev/posts/wasm-tinygo-vs-rust-vs-assemblyscript/)
- [AssemblyScript vs Rust — Suborbital](https://blog.suborbital.dev/assemblyscript-vs-rust-for-your-wasm-app)
- [Wasm By Example: Reading and Writing Graphics (AssemblyScript)](https://wasmbyexample.dev/examples/reading-and-writing-graphics/reading-and-writing-graphics.assemblyscript.en-us.html)
- [TinyGo WebAssembly WASI guide](https://tinygo.org/docs/guides/webassembly/wasi/)
- [TinyGo Release v0.38.0](https://github.com/tinygo-org/tinygo/releases/tag/v0.38.0)
- [TinyGo Release v0.40.0](https://github.com/tinygo-org/tinygo/releases/tag/v0.40.0)
- [Shrink Your TinyGo WebAssembly Modules by 60% — Fermyon](https://www.fermyon.com/blog/optimizing-tinygo-wasm)
- [Compile Go directly to WebAssembly components with TinyGo and WASI P2 — wasmCloud](https://wasmcloud.com/blog/compile-go-directly-to-webassembly-components-with-tinygo-and-wasi-p2/)
- [Writing components in Go with TinyGo compiler](https://dev.to/topheman/webassembly-component-model-writing-components-in-go-with-tinygo-compiler-2914)
- [Announcing Jco 1.0 — Bytecode Alliance](https://bytecodealliance.org/articles/jco-1.0)
- [Jco GitHub](https://github.com/bytecodealliance/jco)
- [ComponentizeJS GitHub](https://github.com/bytecodealliance/ComponentizeJS)
- [JavaScript — The WebAssembly Component Model](https://component-model.bytecodealliance.org/language-support/javascript.html)
- [componentize-py GitHub](https://github.com/bytecodealliance/componentize-py)
- [Python — The WebAssembly Component Model](https://component-model.bytecodealliance.org/language-support/python.html)
- [Introducing Componentize-Py — Fermyon](https://www.fermyon.com/blog/introducing-componentize-py)
- [componentize-py issue #28 — host-provided libpython](https://github.com/bytecodealliance/componentize-py/issues/28)
- [componentize-py issue #98 — smaller binaries](https://github.com/bytecodealliance/componentize-py/issues/98)
- [Wizer — WebAssembly Pre-Initializer](https://github.com/bytecodealliance/wizer)
- [Extism plugin quickstart](https://extism.org/docs/quickstart/plugin-quickstart/)
- [Extism Rust PDK](https://github.com/extism/rust-pdk)
- [Extism Zig PDK](https://github.com/extism/zig-pdk)
- [Intro to Extism — InfoWorld](https://www.infoworld.com/article/2336970/intro-to-extism-a-webassembly-library-for-extendable-apps-and-plugins.html)
- [Watt — Rust proc macros as WebAssembly](https://github.com/dtolnay/watt)
- [wasmer-plugin — proc macro wrapper](https://github.com/freemasen/wasmer-plugin)
- [WebAssembly Component Model — Bytecode Alliance](https://component-model.bytecodealliance.org/)
- [WASI and the WebAssembly Component Model: Current Status — eunomia](https://eunomia.dev/blog/2025/02/16/wasi-and-the-webassembly-component-model-current-status/)
- [The State of WebAssembly — 2025 and 2026 — Platform.Uno](https://platform.uno/blog/the-state-of-webassembly-2025-2026/)
- [WebAssembly Component Model: 2026 Developer Cheat Sheet](https://techbytes.app/posts/wasm-component-model-cheat-sheet/)
- [WASI Preview 2 vs WASIX (2026)](https://wasmruntime.com/en/blog/wasi-preview2-vs-wasix-2026)
- [WebAssembly in 2026: Three Years of "Almost Ready"](https://www.javacodegeeks.com/2026/04/webassembly-in-2026-three-years-of-almost-ready.html)
- [WebAssembly 2026: Server-Side Runtimes, WASI, and the Universal Binary Revolution](https://www.programming-helper.com/tech/webassembly-2026-server-side-runtime-wasi-universal-binary)
- [WebAssembly as an ecosystem for programming languages — 2ality](https://2ality.com/2025/01/webassembly-language-ecosystem.html)
