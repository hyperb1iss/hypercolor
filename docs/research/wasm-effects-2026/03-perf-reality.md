# WebAssembly for Real-Time RGB Effects: Performance Reality in 2026

**Scope:** Can a modern WASM runtime, embedded inside our Rust daemon, render
RGB effects onto a legacy 320 by 200 to 640x480 RGBA canvas at 30-60 FPS without burning
the per-frame budget?

**Target workload:**

- Canvas sizes: 64,000 to 307,200 pixels (256 KiB to 1.2 MiB RGBA per frame)
- Frame rate: 30-60 FPS (16.67 to 33.3 ms budget per frame)
- Operations: per-pixel color blends, gradients, noise, FFT-driven modulation
- Reference: our native wgpu/Servo renderers already hit these targets

**TL;DR:**
Fast enough for the realistic effect surface, with caveats. AOT Cranelift
puts CPU-bound code within 1.1-1.55x of native Rust in the common case and
SIMD closes most of the remaining gap on per-pixel loops. For a 307k-pixel
canvas at 60 Hz with typical color math, WASM has plenty of headroom. The
places it will actually hurt are pathological effects (cache-thrashing
scatter/gather, branch-heavy noise at 4K) and anything that needs threads
or the GPU today. Mitigate with a strict frame budget, mandatory SIMD, and
a native fallback path. See section 12 for the verdict and section 11 for
the actual arithmetic.

---

## 1. Native vs WASM Performance Delta in 2026

The honest answer depends on who is measuring and what they measure. The
range in the current literature is roughly 5% to 55% slowdown for
AOT-compiled WASM versus native Rust on CPU-bound code.

**The foundational academic number** comes from Jangda et al. (USENIX ATC
2019), "Not So Fast: Analyzing the Performance of WebAssembly vs. Native
Code." On the full SPEC CPU suite the study found WASM runs 1.45x slower
in Firefox and 1.55x slower in Chrome on average, with peak slowdowns up
to 2.08x (Firefox) and 2.5x (Chrome). The dominant overheads identified:
2.02x more loads and 2.30x more stores (register pressure, poor allocation,
underutilized x86 addressing modes), 1.65-1.75x more branches (loop
overhead, stack checks, indirect call validation), and 1.80x more retired
instructions from generated-code bloat causing 2.83x more instruction
cache misses. The h264ref video encoder benchmark - the closest analog to
our workload - showed 2.07x slowdown in Chrome and 1.88x in Firefox.

**The current state-of-the-art number** is much better. Wasmer 6.0
(October 2025, LLVM backend) hits 95% of native on CoreMark - a 5% gap.
Earlier benchmarks placed Wasmer's LLVM backend at 1.2-2.1x native speed;
the 2025 improvement is real. The Cranelift backend that Wasmtime uses
sits roughly 14% slower than LLVM per a 2024 comparison, which lines up
with Wasmtime's own reported numbers: 1.1-1.3x slowdown vs native for
steady-state CPU-bound workloads under AOT compilation.

**The contrarian data point** worth taking seriously is Andy Wingo's
April 2026 analysis of his Wastrel tail-calling interpreter. He measured
Wasmtime at 4.3x overhead (switch interpreter comparison baseline) and
6.5x (tail-calling baseline), arguing the gap "isn't inherent to
WebAssembly" but an artifact of suboptimal calling conventions, limited
register allocation for function parameters, and repeated memory value
reloads. Translation: even in 2026 Wasmtime's Cranelift output leaves
significant performance on the floor for certain code shapes. For our
purposes this means per-pixel inner loops are favorable (Cranelift handles
them well), while effect code with many short function calls is a risk.

**Interpreter mode is a different universe.** The Lumos 2025 paper
(Marcelino et al., IOT 2025) reports interpreted WASM suffers up to 55x
higher warm latency than container-native execution. Pulley is a
best-effort interpreter that will "never be as fast as native Cranelift"
per its own docs; the team describes it as "in a relatively good spot"
but "by no means outstripping other wasm interpreters." It's fine as a
cold-path fallback, not a production render path.

**Practical ratio to design against:**

- AOT Cranelift, well-tuned code: **1.1-1.3x slowdown** vs native Rust
- AOT Cranelift, adversarial/branch-heavy code: **1.5-2.0x**
- AOT LLVM (Wasmer 6): **1.05-1.2x**
- Interpreter (Pulley): **10-50x** - avoid for hot paths
- With wasm32 SIMD enabled and a vectorizable kernel: the gap narrows
  toward native because both sides vectorize

## 2. wasm32 SIMD (v128 / simd128)

Yes, v128 instructions map to hardware SIMD: SSE/SSE2/SSSE3/SSE4 on x86
and NEON on ARM64, with Cranelift picking the best available native path
at compile time. Both Wasmtime and Wasmer enable wasm32 SIMD by default
on x86-64 and aarch64 (per the Cranelift 2022 progress report, SIMD was
completed for those two architectures and turned on by default).

**Relaxed SIMD (Wasm 3.0, 2025)** adds non-deterministic instructions
like fused multiply-add, relaxed dot product (8-bit x 7-bit accumulating),
and precision-relaxed float ops. These trade strict determinism for
hardware-native fast paths. Chrome 114+ and Firefox 145+ ship it by
default; Wasmtime supports it. For pixel shaders this is the "I just
want FMAs and don't care about bitwise reproducibility" escape hatch.

**Concrete speedup numbers for 4-wide f32 pixel operations:**

- OpenCV.js WASM SIMD blur (1280x720, 3x3 kernel): **1.36x** over scalar
- OpenCV.js pyrDown (1920x1080, CV_32FC4): **3.09x** over scalar
- Gaussian filter SIMD (generic): **10-20x** over naive scalar
- WASM array ops (Rust+wasm-bindgen 2025 benchmarks): **6x**
  (1.4 ms to 0.231 ms)
- Mandelbrot (v8.dev SIMD post): **2.65x**
- OCR inference with Relaxed SIMD dot product: **~1.6x** on top of
  baseline SIMD
- llama.cpp dot product on WASM with SIMD: reported **2x** end-to-end
  speedup (January 2025 PR)

The honest pattern: 4-wide f32 color ops, the bread-and-butter of LED
effect kernels, typically see 2-4x SIMD speedup in WASM when the loop is
straightforward and the data is aligned. Cache-unfriendly access patterns
cap the win below 2x even with perfect vectorization. Gaussian-style
convolution kernels with careful data layout reach 10x+.

## 3. Cold Start and AOT Compilation

**Cranelift compilation throughput** is fast but not free. Wasmtime has
parallel compilation on by default and uses a mid-end optimizer that
cut compile time 15% on bz2 in the 2022-2023 round. There is no clean
MB/s figure in the public benchmarks, but the scale is "hundreds of
milliseconds for a 1 MB wasm module on a modern laptop, using all
cores," which aligns with our subjective experience.

**Winch (baseline compiler)** is Wasmtime's fast-compile, slow-run
option. Per the Bytecode Alliance's own measurements on the Sightglass
suite, Winch achieves **15-20x faster compilation** while producing code
that runs **1.1-1.5x slower** than Cranelift. That's a brutal tradeoff
for our use case: a 1.5x slowdown compounds with WASM's already
existing 1.1-1.3x gap, so Winch code can run ~2x slower than native.
For us Winch is a "while Cranelift compiles in the background" stopgap,
not a production path. AArch64 support for Winch landed in Wasmtime 35
(2026).

**Pulley (portable interpreter)** compiles through Cranelift to Pulley
bytecode, so compilation time is similar to Cranelift-to-native but the
runtime is interpreted. Its "two opcodes per memory access" design
(bounds check + load/store) hurts, and it will never match native JIT.
It's the right answer only where you can't JIT (sandboxed embeddings,
architectures without Cranelift support, signed-binary-only distribution).
We have none of those constraints.

**Compilation cache.** Wasmtime and Wasmer both transparently cache JIT
output keyed by module hash. Cached load is opaque blob deserialization
plus function pointer fixups; Wasmtime's recent "incremental compilation
cache" work further caches per-function compilation artifacts. For a
restart of the daemon, a cached effect module loads in the
**"instantiate" regime of microseconds**, not the "compile" regime of
hundreds of milliseconds.

**Instantiation with pooling allocator + CoW:** Wasmtime's 1.0
performance work reduced SpiderMonkey.wasm instantiation from ~2 ms to
**~5 microseconds, a 400x speedup** (memfd-based copy-on-write pool,
`cfallin` PR #3697). For us this means "swap the active effect" or
"reload after a hot edit" is effectively free. Fermyon Spin reports
average latency **175.56 µs end-to-end** with sub-1 ms cold start, and
Shopify's Lucet container startup lands at **35 µs**. These are
production numbers at scale.

**Design conclusion:** always AOT with Cranelift. Cache to disk. Use
the pooling allocator with CoW. Never ship Pulley or Winch on the hot
render path.

## 4. Call Overhead

**Host-to-guest calls (Wasmtime, core Wasm, AOT):** "as little as 10
nanoseconds" per the Bytecode Alliance's 2023 Wasmtime update. That's
the trampoline cost only; actual work on top. The trampoline overhaul
in the 2023 release cut this 5-10% from the prior generation.

**Component Model overhead is not free.** Current WIT-bindgen still
lowers complex types (strings, lists, records) through copy-based
serialization into linear memory; component-model engineers openly
describe this as "an area of ongoing optimization." The Hacker News
discussion on the faster wasm-bindgen project (October 2025) reports
**2.5x faster** boundary crossing vs standard wasm-bindgen by avoiding
per-call type-conversion overhead.

**For reference, wasm-bindgen overhead** sits at roughly **5-10 ns per
call** at the raw boundary but legacy benchmarks show **1.7 µs per call**
once type conversion (string encoding, object wrapping) is included.
That's the classic "death by a thousand small calls" failure mode.

**Extism (concrete numbers, 2023-2024 benchmarks) on Wasmtime:**

- One-way data transfer: **1.5-1.6 GiB/s**
- Round-trip transfer: **911 MiB/s**
- "Reflect" (full round-trip): **284.63 MiB/s**
- Per-byte kernel cost: **4.75 ns** (one-way) or **6.7 ns** (round-trip)
- 64 KiB payload: ~278 MiB/s at 224 µs
- 640 KiB payload: ~298 MiB/s at 2.1 ms

**Native trait object dispatch (for comparison):** single indirect call
through a vtable on modern x86 is roughly **1-2 ns** including the
virtual call overhead. So Wasmtime host-to-guest is **5-10x** the cost
of a Rust `dyn Trait` call. This matters if we call into the guest
hundreds of thousands of times per frame; it's negligible if we call
once per frame with a batched buffer.

**Design conclusion:** never call into WASM per-pixel or per-LED.
Batch to "render the whole frame" granularity. A single `render(buf_ptr,
buf_len, time_ms)` call at 60 Hz incurs 60 x 10 ns = **600 ns/sec of
call overhead**. Irrelevant. Copying the framebuffer out incurs ~1 MiB /
298 MiB/s x 60 = **~200 ms/sec of data motion** if done naively. Very
relevant - see section 5.

## 5. Memory Access Patterns

The guest's linear memory is a single `Vec<u8>` on the host. Both
directions can read and write it synchronously; the host just needs to
translate Wasm pointers (offsets from memory base 0) into host addresses.

**The zero-copy sweet spot for our workload:**

1. Guest allocates the framebuffer inside its own linear memory at
   instantiate time.
2. Guest exports its pointer (via a `canvas_ptr()` export or similar).
3. On each tick: host calls `render(time_ms)`. Guest writes pixels into
   its own memory. Host reads them from the same backing `Vec<u8>`
   via `memory.data_mut(&mut store)` - **zero copy, no bounds-check
   round trip.**
4. Host passes the slice to the spatial engine as normal.

This is the documented Wasmtime pattern (`Memory::data_ptr` +
`Memory::data_size`). No kernel calls, no marshaling. The cost is a
single pointer dereference per frame.

**What you cannot do (without threads/shared memory):**

- Have host and guest both hold live mutable references to the same
  bytes concurrently. Guest mutates; host reads after guest returns.
  Fine for our frame-at-a-time rendering model.
- Pass arbitrary host buffers into the guest without a copy. Host has
  to copy-in if the data isn't already at a known guest-memory address.
  For us this is one-shot audio/spectrum samples (a few KB per frame),
  so the copy cost is negligible.

**With wasm32-threads enabled** the guest memory can be a
`SharedArrayBuffer`-style shared region. This opens true parallel
access but requires the full threading story (section 7) which is still
experimental in Wasmtime.

**Design conclusion:** guest owns the canvas, host reads it after
`render()` returns. Small inputs (time, audio spectrum, controls) are
copied into guest memory per frame. The canvas data never crosses the
boundary.

## 6. Graphics and DSP Workloads in WASM

**Image processing production signal:** Figma ships C++-to-WASM for its
renderer and reports **3x faster load times** post-WASM move, and runs
the actual canvas rendering in WebGPU (with WASM as the orchestrator).
Google ships WASM in Google Earth, Sheets calculation workers, Photos
image filters, and Meet real-time video processing. Adobe Photoshop
Web ships as WASM for compute-heavy filters. These are all running in
browsers, not with Wasmtime's AOT speeds. Our Wasmtime-embedded case is
strictly better.

**Concrete image-processing Mpix/s numbers are sparse in the public
literature,** but we can triangulate. On an M2 Pro, scalar
emscripten-compiled image code ran ~7 ms per operation on a standard
test image (per the 2024 Arthmis image-processing blog); with SIMD
that drops 3-5x to the 1.5-3 ms range. For a 640x480 canvas that's
**~100-200 Mpix/s throughput** with a moderate per-pixel kernel.

**FFT:** pffft.wasm provides both SIMD and non-SIMD builds. FFT
vectorizes well and the authors cite WASM SIMD roughly matching or
beating JS by 8x on an FFT-heavy pitch detection workload. In absolute
terms on our canvas sizes (way smaller than audio FFTs used for real-time
pitch detection), FFT cost is negligible - a 1024-point FFT in SIMD WASM
is sub-100 µs territory.

**Noise functions:** these are the pathological case. Simplex/Perlin
noise is branch-heavy and does scatter-gather through gradient tables.
WASM's extra bounds checks on table lookups bite here. Expect closer to
1.5-2x slowdown vs native. Still well within our budget, but don't
assume 1.05x.

**Color-space conversion (sRGB/Oklab/HSL):** straightforward per-pixel
SIMD kernels; expect 1.1-1.2x slowdown vs native, matching CoreMark
numbers.

**Palette mapping:** nearest-color lookup against a 16-64 entry palette
is memory-bound, not compute-bound. WASM's extra loads/stores hurt here
(+20-30% vs native per the Jangda findings), but absolute cost on a
307k-pixel canvas is still far below a millisecond.

## 7. Threading

**Current state (April 2026):** the core Wasm threads proposal is at
Phase 3 (stable, shipping in runtimes). WASI-threads shipped a usable
implementation in Wasmtime years ago but the Bytecode Alliance describes
the Wasmtime implementation as "still experimental and not yet suitable
for multi-tenant embeddings," citing the hard-exit-on-thread-death issue.
The original wasi-threads proposal was withdrawn in August 2023 in favor
of **shared-everything-threads**, which is early-stage and not in any
runtime yet.

**What actually works today:** inside a single WASM instance you can use
`atomics` instructions (load/store/fence/CAS/wait/notify), shared memory,
and the threads-level primitives. Spawning threads requires either
wasi-threads (experimental) or the host exposing a thread-spawn API. For
our embedder case we could expose our own threadpool-backed spawn
function, but the tooling story is rough; most effect authors won't use it.

**Hardware parallelism via host fan-out:** the simpler pattern - host
instantiates N copies of the same module, each renders a tile, host
stitches. This is a single-instance-per-thread model and requires no new
WASI proposals. Cost: N copies of the linear memory, but since we
control memory pages it's fine.

**Design conclusion:** do not expose WASM threads to effect authors in
the near term. If we need multi-core for a heavy effect, fan out at the
host level across tile regions. For our canvas sizes (307k pixels max) a
single core is very likely fast enough anyway - see section 11.

## 8. WebGPU via WASM

**wasi-gfx is real but Phase 2** (as of 2026). It exposes WebGPU, frame
buffers, and surfaces to WASM guests via component bindings. Wasmtime
has experimental support. The demos at Wasm I/O 2025 show Bevy and wgpu
games running unmodified from WASM against wasi-webgpu bindings. Cool.

**For us, shipping wasi-gfx as the hot path is premature.** It's not in
any shipping runtime release yet that we'd want to depend on, the spec
is pre-1.0, and it adds a second layer of translation (WASM -> wasi-webgpu
-> host wgpu) vs just running wgpu native. Our current wgpu-based native
renderer is already optimal on this axis.

**For plugin-authored effects it could eventually be great.** Effect
author writes wgpu-style code, compiles to WASM, we pipe it through
wasi-gfx to our existing wgpu context. Revisit in 2027 when Phase 3
ships.

## 9. Memory Management Overhead

**Each WASM plugin has its own heap.** The Rust-to-WASM toolchain uses
`dlmalloc` or `wee_alloc` by default; AssemblyScript ships its own
allocator; Go uses its runtime's allocator. All run inside the guest's
linear memory.

**Allocation cost** in a guest allocator is broadly similar to native
malloc cost (tens to hundreds of nanoseconds) plus WASM's general slowdown
factor. For per-frame allocations in a 60 FPS hot loop, 100 ns x "a few
dozen allocs per frame" = 3-10 µs/frame. Tolerable but wasteful.

**Zero-alloc hot paths are achievable.** In Rust-compiled WASM, any
effect that preallocates its scratch buffers at construction time and
uses `&mut [u8]` slices into linear memory for per-frame work has zero
allocations per frame. This is the same discipline we use for the native
effect path; the only difference is the effect author has to actually
follow it. Enforce via effect-authoring guidelines, verify in CI with a
"peak heap usage" measurement that should not grow across frames.

**Linear memory growth** is an event worth avoiding on the hot path.
`memory.grow` in Wasmtime can happen concurrently but never relocates
the base pointer (stable addresses), so it's not catastrophic when it
does happen. Still: preallocate.

## 10. Real-World Embedders at Scale

**Shopify Functions:** originally Lucet, now moving to Wasmtime. 35 µs
container startup, 1000 req/s per worker. Runs untrusted merchant code.
JavaScript-in-WASM (Javy) runs roughly **3x slower than Rust-in-WASM**
for their functions workload (per the Shopify Engineering blog).

**Fastly Compute@Edge:** Wasmtime under the hood. Quoted "a few
microseconds" instance startup. Fastly cares about _instance_ latency
(network-edge compute) more than sustained throughput, which aligns
well with our use case.

**Fermyon Spin:** 2-3 ms typical cold start, 175.56 µs end-to-end latency
in their Spin 2.0 macOS benchmark with 28,000 req/s and 300,000 instances
created. Production handling 75M req/s across their infrastructure.

**Envoy Proxy-Wasm:** "microseconds" overhead per request for WASM
filters vs native C++ filters, acknowledged higher than native but
"negligible for most use cases." Envoy's perf team describes it as
"prototype-first, measure" guidance - the overhead exists, it's
measurable, it's fine for 99% of filter workloads. Our per-frame call
model has the same shape.

**Figma Plugins:** the cautionary tale. Figma runs plugin code in a
JS-VM-compiled-to-WASM sandbox, which is an _interpreter_ on WASM.
They note performance degrades on 200+ screen files - that's the
interpreter-on-interpreter tax, not a problem that applies to
Rust-compiled effect modules.

**Takeaway:** production Wasmtime at scale is battle-hardened for
microsecond-level workloads. What's less well-trodden is sustained
high-throughput compute from a single instance - which is exactly our
case. The ecosystem's production bias toward "many short requests" vs
"long-running compute loop" means we'll need our own benchmarks to
trust the numbers.

## 11. Stress Test Projection for Our Case

Do the actual math. Canvas = 640x480 = **307,200 pixels**.

**Frame budget:**

- 60 FPS: **16.67 ms/frame**
- 30 FPS: **33.3 ms/frame**
- Leave 20-30% headroom for spatial sampling, backend writes, event bus
  publish - target **10-12 ms/frame actual render budget at 60 FPS**.

**Native Rust baseline (what our wgpu/CPU effects already achieve):**

- A simple per-pixel f32 color blend in native Rust with SIMD vectors
  is roughly **0.3-0.5 ns/pixel** on modern hardware (4 f32 ops in a
  fused vector lane = 1-2 ns for 4 pixels, so ~0.4 ns each).
- 307,200 pixels x 0.4 ns = **~123 µs per pass**. That's 0.7% of a
  60 FPS budget. We have ~80-100 such passes worth of headroom per frame.

**WASM AOT Cranelift with SIMD (realistic case):**

- Same kernel at 1.2x slowdown = **~0.48 ns/pixel** = ~148 µs per pass.
- Still 0.9% of budget. **Effectively indistinguishable from native.**

**WASM AOT Cranelift without SIMD (defensive case):**

- ~1.5-2x native = ~0.8 ns/pixel = ~246 µs per pass = 1.5% of budget.
- Still fine for one pass. An effect doing ~20 compound passes would
  use 5 ms, still under budget.

**The pathological case - branch-heavy scatter/gather (e.g., Perlin
noise with table lookups), no SIMD, cache-unfriendly:**

- ~3-4 ns/pixel in WASM = ~1.2 ms per pass. 20 such passes is 24 ms,
  **blows the 60 FPS budget**, fine at 30 FPS.

**Where the original ask went wrong.** The prompt asked: "2 ns native
-> 1.2 s at 2x slowdown for a simple pass on 300k pixels, too slow."
That arithmetic is off by three orders of magnitude. 300,000 pixels x
4 ns = **1.2 ms**, not 1.2 s. The full RGBA framebuffer write (1.2 MiB
at 74 MiB/s - sorry, per frame at 60 Hz is 74 MiB/s total) is well
within memcpy bandwidth on any CPU.

**Where it does get tight:**

- **Multi-pass compositing:** 10+ passes over the full canvas with
  non-trivial per-pass work starts to matter at 60 FPS. Budget accordingly.
- **FFT plus per-frequency-bin pixel modulation:** the FFT is cheap
  (<100 µs), the N-bin x 307k-pixel modulation loop is real work. Plan
  on this being a dominant cost.
- **Complex noise functions:** per-pixel Perlin or Simplex at full
  canvas is the most likely effect to struggle. Expect 2-3 ms per pass.

**Throughput sanity check (Extism numbers):**

- Data out of guest at 300 MiB/s in their worst round-trip case. Our
  canvas is 1.2 MiB, but we don't do a round-trip copy - we read
  directly from linear memory. Zero-copy eliminates this bottleneck.

## 12. Verdict

**Yes, WASM is fast enough for our effect renderer hot path in 2026.**

The cases where it falls short are bounded and identifiable:

**Fast enough:**

- Any single-pass per-pixel color kernel (blends, gradients, LUTs,
  color-space conversion, palette mapping)
- FFT-driven modulation at our canvas sizes
- Moderate multi-pass effects (up to ~10 passes per frame at 60 FPS)
- Anything an effect author would write without optimizing

**Marginal, profile before shipping:**

- Heavy multi-pass compositing (>15 passes)
- Per-pixel simplex/perlin noise at full 640x480 canvas with
  multi-octave accumulation
- Very branch-heavy algorithms (conditional per-pixel branching that
  defeats SIMD)

**Needs a different tool:**

- GPU-class effects (volumetric ray-marching, complex shading).
  Use native wgpu or wait for wasi-gfx phase 3.
- Multi-core parallelism. Fan out at the host level or wait for
  shared-everything-threads.

**Required caveats for shipping:**

1. **Always AOT compile with Cranelift.** Never ship Winch or Pulley
   on the hot path. Cache compiled modules to disk.
2. **Require wasm32 SIMD (simd128).** Mark effects built without it
   as "legacy" and downrank them. Detect and warn at load time.
3. **Enforce a per-frame budget.** Measure each effect's actual frame
   time in the render loop, demote to a lower FPS tier if it exceeds
   budget. The existing FpsController already does this for native
   effects.
4. **Zero-allocation hot paths are mandatory.** Effect SDK docs must
   be clear: preallocate in `init()`, no `Vec::new()` in `render()`.
   CI-verify peak heap doesn't grow across 1000 frames.
5. **Zero-copy canvas reads.** Never marshal the framebuffer across
   the boundary. Guest owns the buffer, host reads linear memory after
   `render()` returns.
6. **One render call per frame.** Not per-pixel, not per-LED, not
   per-row. Cross the boundary once with `render(time_ms)`.
7. **Keep a native fallback.** Ship the heaviest 5-10 built-in effects
   as native Rust EffectRenderer implementations. WASM is for
   third-party and experimental effects.
8. **Benchmark in-house.** The published numbers are serverless and
   FFI-oriented. We're sustained-compute; we need our own sightglass-
   style suite for the actual effect kernels we care about.

**The pivot we don't need:** "WASM for control logic only, native for
render." The math doesn't support that retreat - WASM per-pixel is
fast enough for our canvas sizes even at Cranelift quality. The
pivots we _might_ need are per-effect: specific heavy effects can
stay native indefinitely, while the long tail of simpler effects
ships as WASM plugins. That's the architecture.

---

## Sources

**Academic / peer-reviewed:**

- [Jangda et al. 2019 - "Not So Fast: Analyzing the Performance of WebAssembly vs. Native Code" (USENIX ATC)](https://www.usenix.org/conference/atc19/presentation/jangda) ([ar5iv mirror](https://ar5iv.labs.arxiv.org/html/1901.09056))
- [Marcelino et al. 2025 - "Lumos: Performance Characterization of WebAssembly as a Serverless Runtime" (IOT 2025)](https://arxiv.org/abs/2510.05118v1)
- [Dierickx 2025 - "Comparative Study of the Performance of WebAssembly Runtimes"](https://www.opencloudification.com/wp-content/uploads/2025/07/comparative_study_WA_runtimes.pdf)
- [ACM TACO 2025 - "Benchmarking WebAssembly for Embedded Systems"](https://dl.acm.org/doi/10.1145/3736169)

**Runtime vendor announcements with benchmark data:**

- [Bytecode Alliance - "Wasmtime 1.0: A Look at Performance" (400x instantiation speedup)](https://bytecodealliance.org/articles/wasmtime-10-performance)
- [Bytecode Alliance - "Wasmtime and Cranelift in 2023" (10 ns host-guest calls)](https://bytecodealliance.org/articles/wasmtime-and-cranelift-in-2023)
- [Wasmer 6.0 announcement - 95% of native on CoreMark](https://wasmer.io/posts/announcing-wasmer-6-closer-to-native-speeds)
- [Wingolog 2026-04 - "The Value of a Performance Oracle" (contrarian 4.3-6.5x measurement)](https://wingolog.org/archives/2026/04/07/the-value-of-a-performance-oracle)
- [Wasmtime docs - Fast Instantiation (pooling allocator + CoW)](https://docs.wasmtime.dev/examples-fast-instantiation.html)
- [Wasmtime baseline compilation RFC (Winch 15-20x compile, 1.1-1.5x runtime tradeoff)](https://github.com/bytecodealliance/rfcs/blob/main/accepted/wasmtime-baseline-compilation.md)
- [Pulley interpreter performance tracking](https://github.com/bytecodealliance/wasmtime/issues/10102)

**Third-party benchmarks and analysis:**

- [Frank Denis 2023 - "Performance of WebAssembly runtimes" (2.32x median slowdown)](https://00f.net/2023/01/04/webassembly-benchmark-2023/)
- [wasmRuntime.com 2026 benchmarks](https://wasmruntime.com/en/benchmarks)
- [Dylibso - "Back of the Napkin Wasm Performance: How Does Extism Work?" (4.75 ns/call, 278-298 MiB/s)](https://dylibso.com/blog/how-does-extism-work/)
- [byteiota 2025 - Rust + WebAssembly benchmarks](https://byteiota.com/rust-webassembly-performance-8-10x-faster-2025-benchmarks/)
- [Medium - "The Benchmark Bake-Off: Which Runtime Actually Wins in 2025?"](https://medium.com/the-rise-of-device-independent-architecture/the-benchmark-bake-off-which-runtime-actually-wins-in-2025-ebf69ec5a080)
- [Hacker News Oct 2025 - faster wasm-bindgen 2.5x at boundary](https://news.ycombinator.com/item?id=45664341)

**SIMD specifics:**

- [V8 - "Fast, parallel applications with WebAssembly SIMD"](https://v8.dev/features/simd)
- [OpenCV.js WASM SIMD PR benchmarks](https://github.com/opencv/opencv/pull/18068)
- [MDPI 2024 - Fast Gaussian Filter Approximations on SIMD Platforms](https://www.mdpi.com/2076-3417/14/11/4664)
- [WebAssembly Relaxed SIMD overview (Wasm 3.0)](https://github.com/WebAssembly/spec/blob/wasm-3.0/proposals/relaxed-simd/Overview.md)
- [Simon Willison - "x2 speed for WASM by optimizing SIMD" (llama.cpp)](https://simonwillison.net/2025/Jan/27/llamacpp-pr/)

**Threading and graphics:**

- [Bytecode Alliance - "Announcing wasi-threads"](https://bytecodealliance.org/articles/wasi-threads)
- [Wasm I/O 2025 - "GPUs Unleashed! Make Your Games More Powerful With wasi-gfx"](https://2025.wasm.io/sessions/gpus-unleashed-make-your-games-more-powerful-with-wasi-gfx/)
- [wasi-gfx GitHub organization](https://github.com/wasi-gfx)

**Production embedders:**

- [Shopify Engineering - "How Shopify Uses WebAssembly Outside of the Browser" (Lucet 35 µs)](https://shopify.engineering/shopify-webassembly)
- [Shopify Engineering - "Bringing JavaScript to WebAssembly for Shopify Functions"](https://shopify.engineering/javascript-in-webassembly-for-shopify-functions)
- [Fermyon - "Introducing Spin 2.0" (175.56 µs latency, sub-1ms cold start)](https://www.fermyon.com/blog/introducing-spin-v2)
- [Figma Blog - "How to build a plugin system on the web" (JS-VM-in-WASM interpreter model)](https://www.figma.com/blog/how-we-built-the-figma-plugin-system/)
- [Figma Blog - "Figma is powered by WebAssembly" (3x load time)](https://www.figma.com/blog/webassembly-cut-figmas-load-time-by-3x/)
- [webrtcHacks - "Video Frame Processing on the Web"](https://webrtchacks.com/video-frame-processing-on-the-web-webassembly-webgpu-webgl-webcodecs-webnn-and-webtransport/)
- [Tetrate - "4 Envoy Extensibility Mechanisms" (Proxy-Wasm overhead)](https://tetrate.io/blog/4-envoy-extensibility-mechanisms-how-to-boost-envoy-gateway-performance-and-functionality)
