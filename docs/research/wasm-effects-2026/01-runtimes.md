# WASM Runtimes for Hypercolor Effect Plugins (2026)

Research input for the spec on dynamically loaded WASM effect renderers inside
the Hypercolor daemon. Use case: a third `EffectRenderer` backend that
loads untrusted `.wasm` modules, lends them a `&mut [u8]` canvas every frame,
runs inside a 16 to 33 millisecond budget at 30 to 60 frames per second, and
supports hot reload without restarting the daemon.

## 0. Recommendation (Lead)

**Pick `wasmtime` 36.0 LTS (pinned), or the current stable `wasmtime` tracking
release, used directly as a crate, not through Extism.** Migrate to the 48.x LTS
(August 2026) when it ships.

Justification, in order of importance for our workload:

1. **Per-frame budget enforcement.** Wasmtime is the only runtime with both
   fuel metering (deterministic, expensive) and epoch-based interruption (about
   10 percent slowdown, non-deterministic but cheap). Epoch deadlines are the
   exact primitive we need for "kill this effect if it blows 16 milliseconds."
   Sources dated 2023 to 2026.
2. **Mature AOT caching.** `Engine::precompile_module` plus `Module::deserialize_file`
   gives us disk-cached native code, `mmap`ed lazily. First load compiles,
   subsequent loads are microseconds. Host-specific artifacts, which is fine
   for our daemon.
3. **Near-native host to guest call overhead.** About 10 nanoseconds per direct
   export call after trampoline work in 2023; nothing since has regressed it.
   At 60 frames per second that is 600 nanoseconds per second of pure dispatch
   cost, negligible.
4. **SIMD that actually lowers to native.** Cranelift maps `v128` to SSE2 or
   NEON. WebAssembly 3.0 (ratified September 2025) includes 128-bit SIMD and
   relaxed SIMD; wasmtime enables relaxed-simd by default.
5. **Zero-copy `&mut [u8]`.** `Memory::data_mut` hands us a raw slice into the
   guest's linear memory for the lifetime of the store borrow. Perfect for a
   per-frame canvas lend.
6. **Binary size is controllable.** Default Rust embedding is heavy, but a
   runtime-only build (no Cranelift, precompiled `.cwasm` only) drops a
   minimal Wasmtime C API to 2.1 megabytes. Relative to Servo, this is noise.
7. **Funded and shipping.** Monthly releases (v40 through v43 landed Jan to
   March 2026), formal LTS policy (every 12th release, 24 months of security
   patches), security advisories handled promptly (April 9, 2026 coordinated
   patch of 12 CVEs). Bytecode Alliance backing.

Against the alternatives:

- **Wasmer** is pragmatic and fast, but its value proposition (LLVM backend,
  WASIX) does not outweigh wasmtime's tighter resource controls for our use
  case, and WASIX is a wasmer-specific extension we do not need.
- **Wasmi** is the right answer if we were embedding on a microcontroller. On
  a desktop with a per-frame budget measured in milliseconds, giving up JIT to
  stay pure-Rust and `no_std` is the wrong trade.
- **Extism** wraps wasmtime with a virtual memory staging area that is
  brilliant for polyglot plugin ecosystems and wrong for our workload: it
  forces kernel function calls per 8 bytes of payload, which for a 640x480x4
  canvas is over a million kernel calls per frame. The raw wasmtime path gives
  us a `*mut u8` into the plugin's memory in one operation.

The rest of this document backs those claims with numbers.

---

## 1. Current State (April 2026)

### Wasmtime (Bytecode Alliance)

- **Latest stable**: **43.0.0**, released **March 20, 2026**, with WASIp3
  snapshot `0.3.0-rc-2026-03-15` support, configurable backtrace frame limits,
  fine-grained operator cost configuration for fuel metering, and
  `wasmtime-wasi-tls` OpenSSL backend. (Prism News, March 2026.)
- **Prior stable**: **42.0.0**, released **February 20, 2026**. Cranelift
  added bitwise-on-float operations on aarch64 and NaN canonicalization for
  f16 and f128. (Prism News, March 2026.)
- **LTS track**: **36.0.0** (August 20, 2025), supported through
  **August 20, 2027**. **24.0.0** (retroactive LTS) supported through
  **August 20, 2026**. Every 12th release is an LTS, 24-month security window.
  (Bytecode Alliance LTS article.)
- **Security cadence**: Coordinated patch on **April 9, 2026** shipped
  43.0.1 / 42.0.2 / 36.0.7 / 24.0.7 covering 12 advisories. This is the model
  for how we should pin.
- **Rust MSRV** (43.0.0): **1.91.0**.
- **Component Model**: Production on WASIp2, experimental on WASIp3. The
  async proposal support landed complete in 36.0. Wasmtime is the reference
  implementation for the Component Model.

References:

- <https://github.com/bytecodealliance/wasmtime/releases>
- <https://bytecodealliance.org/articles/wasmtime-lts>
- <https://bytecodealliance.org/articles/wasmtime-security-advisories>
- <https://www.prismnews.com/hobbies/rust-programming/wasmtime-4300-arrives-with-wasip3-support-and-expanded>

### Wasmer

- **Latest stable**: **7.1.0**, released **March 27, 2026**. Redesigned
  `--enable-pass-params-opt` (now default) addressing CPU scaling issues on
  large modules; **Cranelift and LLVM** both gained Tail Call, Extended
  Constant Expression, Relaxed SIMD, and Wide Arithmetic proposal support;
  WASIX TTY and epoll rewritten for performance.
- **Prior stable**: **7.0.0**, released **January 28, 2026**. LLVM backend
  upgraded from 18 to 21; Singlepass added RISC-V 64-bit and multi-value;
  Cranelift added exception handling; WASIX context-switching (green threads);
  experimental async API behind `experimental-async` feature; dynamic linking
  in WASIX; ~90 second to ~10 second Python compile-time fix on LLVM.
- **Release candidate**: **7.2.0-alpha.1**, April 9, 2026.
- **Compilers**: Singlepass (fastest compile, slowest code), Cranelift
  (balanced, same core as wasmtime), LLVM (production, about 50 percent faster
  runtime). V8 backend exists for Chrome-parity.

References:

- <https://github.com/wasmerio/wasmer/releases>
- <https://github.com/wasmerio/wasmer/blob/main/CHANGELOG.md>

### Wasmi (Wasmi Labs)

- **Latest stable**: **1.0.9** line (released through **February 27, 2026**),
  the first API-stable release, following years of internal evolution from the
  Parity fork. v1.0 guarantees API stability.
- **Next major**: **2.0.0-beta.2** is the most recent beta in the 2.0 cycle,
  which lands a completely redesigned internal IR, fixed 64-bit cells that
  isolate SIMD footprint ("enabling the simd crate feature no longer affects
  memory consumption or execution performance of non-simd Wasm code"), and
  halved table memory (64 to 32 bit elements). New dispatch modes:
  `portable-dispatch` (any Rust target, perf trade-off), `indirect-dispatch`
  (smaller encoding at runtime cost).
- **Scope**: Pure interpreter, written in Rust, `no_std`-friendly. Register-based
  bytecode since v0.32 (May 2024) delivered up to 5x execution speedup plus
  lazy compilation for fast startup. Fuel metering and refueled resumable
  calls. Over 200 SIMD operators opt-in, verified to lower to native
  hardware SIMD (not emulated).
- **Component Model**: Not supported. Core Wasm only.

References:

- <https://github.com/wasmi-labs/wasmi/releases>
- <https://wasmi-labs.github.io/blog/posts/wasmi-v1.0/>
- <https://wasmi-labs.github.io/blog/posts/wasmi-v0.32/>

### Extism (Dylibso)

- **Latest stable**: **1.21.0**, released **March 26, 2026**. Bumps Wasmtime
  dependency to **v41**.
- **Prior**: **1.20.0**, released **March 19, 2026**. **Breaking**: `UserData`
  type must be thread-safe when used in a `Pool`. This is a 2026 pain point
  for anyone embedding pools.
- **Historical**: 1.13.0 (November 2024) exposed fuel limits on
  `CompiledPlugin`; 1.9.0 introduced `CompiledPlugin` itself.
- **Architecture**: Extism is a framework on top of wasmtime, not a runtime.
  It adds an opinionated plugin ABI, a virtual-memory staging area (host-side
  buffer the plugin reads via `load_u64` / `store_u64` kernel calls), a
  polyglot PDK ecosystem, and pool-based lifecycle.
- **License**: BSD-3-Clause.

References:

- <https://github.com/extism/extism/releases>
- <https://docs.rs/extism/latest/extism/>
- <https://dylibso.com/blog/how-does-extism-work/>

---

## 2. Embedding API Ergonomics (Rust)

All four runtimes hit the same "instantiate and call a typed export" shape.
Differences: borrowing discipline, async story, and whether the runtime owns
a buffer model for you.

### Wasmtime

```rust
use wasmtime::{Engine, Module, Store, Instance, TypedFunc};

let engine = Engine::default();
let module = Module::from_file(&engine, "effect.wasm")?;
let mut store = Store::new(&engine, ());
let instance = Instance::new(&mut store, &module, &[])?;

let render: TypedFunc<(i32, i32), ()> =
    instance.get_typed_func(&mut store, "render")?;
let memory = instance.get_memory(&mut store, "memory")
    .expect("effect module must export memory");

let canvas_ptr: i32 = /* a guest-allocated offset */;
let canvas_len: i32 = (640 * 480 * 4) as i32;
render.call(&mut store, (canvas_ptr, canvas_len))?;

let canvas: &[u8] =
    &memory.data(&store)[canvas_ptr as usize..(canvas_ptr + canvas_len) as usize];
```

Ergonomics: the `Store` lifetime is the dominant concept. `Memory::data_mut`
lends `&mut [u8]` pinned to the store borrow, so you can hand the plugin a
pointer, call in, and read back without marshalling. The type-checked export
API (`TypedFunc`) is the best of the four. For lending `T` in `Store<T>` plus
memory simultaneously, wasmtime provides store-context APIs that split the
borrow so the compiler sees host data and guest memory as disjoint.

### Wasmer

```rust
use wasmer::{Store, Module, Instance, TypedFunction, imports};

let mut store = Store::default();
let module = Module::from_file(&store, "effect.wasm")?;
let instance = Instance::new(&mut store, &module, &imports! {})?;

let render: TypedFunction<(i32, i32), ()> =
    instance.exports.get_typed_function(&store, "render")?;
let memory = instance.exports.get_memory("memory")?;

let canvas_ptr = /* ... */;
let canvas_len = 640 * 480 * 4;
render.call(&mut store, canvas_ptr, canvas_len)?;

let view = memory.view(&store);
let mut buf = vec![0u8; canvas_len as usize];
view.read(canvas_ptr as u64, &mut buf)?;
```

Ergonomics: very similar overall. `MemoryView` in modern Wasmer is a cursor,
not a raw slice, which is friendlier across compiler backends but blocks the
true zero-copy pattern wasmtime gives us. For canvas rendering this is a
regression unless you reach into `memory.data_unchecked_mut()` (marked
`unsafe`). Backend choice (Cranelift, LLVM, Singlepass, V8) is runtime
pluggable.

### Wasmi

```rust
use wasmi::{Engine, Module, Store, Linker, TypedFunc};

let engine = Engine::default();
let wasm = std::fs::read("effect.wasm")?;
let module = Module::new(&engine, &wasm[..])?;
let mut store = Store::new(&engine, ());
let linker = <Linker<()>>::new(&engine);
let instance = linker.instantiate(&mut store, &module)?.start(&mut store)?;

let render: TypedFunc<(i32, i32), ()> =
    instance.get_typed_func(&store, "render")?;
let memory = instance.get_memory(&store, "memory")
    .expect("effect module must export memory");

let canvas_ptr = /* ... */;
let canvas_len = 640 * 480 * 4;
render.call(&mut store, (canvas_ptr, canvas_len))?;

let data = memory.data(&store);
let canvas = &data[canvas_ptr as usize..(canvas_ptr + canvas_len) as usize];
```

Ergonomics: effectively identical to wasmtime's surface syntax. Wasmi
deliberately mirrors wasmtime's API shape. No Component Model, no async
beyond `call_resumable` for cooperative yielding.

### Extism

```rust
use extism::{Plugin, Manifest, Wasm};

let wasm = Wasm::file("effect.wasm");
let manifest = Manifest::new([wasm]);
let mut plugin = Plugin::new(&manifest, [], true)?;

let input: &[u8] = &[/* canvas metadata, seed, etc. */];
let output: &[u8] = plugin.call("render", input)?;
// output is a copy the kernel materialised out of plugin virtual memory
```

Ergonomics: by far the simplest surface. You pass bytes in, get bytes out.
No store, no memory, no typed exports. The cost: every byte of `input` and
`output` crosses the Extism kernel boundary through `load_u8` / `store_u8`
host functions. For a once-per-frame 1.2 megabyte canvas, this is the wrong
shape.

---

## 3. Ahead-of-Time Compilation

**Wasmtime** exposes AOT directly:

```rust
let compiled: Vec<u8> = engine.precompile_module(&wasm_bytes)?;
std::fs::write("effect.cwasm", &compiled)?;
// Later, potentially on a runtime-only wasmtime build:
let module = unsafe { Module::deserialize_file(&engine, "effect.cwasm") }?;
```

- Artifacts are **host-specific** (machine code + CPU feature flags + wasmtime
  config); the docs explicitly warn: "we cannot run Wasm programs pre-compiled
  for configurations that do not match our own."
- The runtime can be built without Cranelift and still deserialize. A minimal
  C API embedding is **2.1 megabytes** (vs 260 megabytes for the default
  build with logging).
- `.cwasm` is `mmap`-friendly: code pages are lazily paged in. Fast
  instantiation reduced from milliseconds to microseconds over the past
  years; pooling allocator plus copy-on-write heap images push it further.
- **Cold start** from source Wasm: one-time Cranelift compile cost, measured
  in tens to hundreds of milliseconds per module depending on size. **Warm
  start** from `.cwasm`: microseconds for deserialization, and near-instant
  instantiation with pooling.

**Wasmer** has the same story with richer backend choice:

- `Module::serialize()` / `Module::deserialize()` cache compiled artifacts.
- Wasmer 4.2 introduced zero-copy module deserialization, cutting load times
  by **up to 50 percent** on module load.
- Pluggable backends at runtime: **Cranelift** (balanced), **LLVM** (highest
  runtime performance, slowest compile), **Singlepass** (fastest compile,
  single-pass, weakest code). LLVM is "about 50 percent faster" than Cranelift
  per Wasmer's docs.

**Wasmi** does not AOT compile, it translates on the fly. Lazy translation
means "startup performance improved by several orders of magnitude" in v0.32.
There is no native code cache to persist because there is no native code. Good
for untrusted modules where you do not want a JIT surface; bad if you are
hoping for peak throughput.

**Extism** delegates to wasmtime. `CompiledPlugin` (since 1.9.0) gives you a
reusable compiled form that you can instantiate cheaply across pool slots.

---

## 4. WASM SIMD (v128)

As of **September 2025**, **WebAssembly 3.0** is ratified by W3C and includes
fixed-width 128-bit SIMD, relaxed SIMD, GC, tail calls, exception handling,
64-bit memory, and multi-memory.

**Wasmtime**: `v128` lowers to native SSE2 on x86-64 and NEON on aarch64 via
Cranelift. Relaxed SIMD is enabled by default (landed well before 2026, merged
in PR 7285 alongside threads and multi-memory). Relaxed SIMD exposes
operations that vary slightly by hardware (e.g. fused multiply-add rounding)
for higher throughput on programs that tolerate the wobble.

**Wasmer**: v7.1 added relaxed SIMD to both Cranelift and LLVM backends. LLVM
backend gives the best SIMD codegen in Wasmer's matrix, per their docs on
"WebAssembly and SIMD".

**Wasmi**: v0.32 introduced over 200 128-bit SIMD operators. v2.0 beta
isolated SIMD feature impact so opting into the `simd` crate feature no longer
taxes non-SIMD programs. Wasmi is still an **interpreter**, so SIMD lowers to
native SIMD intrinsics inside the interpreter loop but pays interpreter
dispatch per instruction. Not remotely comparable to JIT SIMD throughput.

**Extism**: whatever wasmtime exposes. Guest modules can freely use SIMD.

**Performance delta**: For canvas-style workloads (pixel blends, palette
mixing, HSV rotates), SIMD on a JIT runtime (wasmtime or wasmer) lands in the
**0.7 to 0.95x native** envelope, with compute-heavy inner loops near parity
to native. Historical data points: matrix multiplication with SIMD intrinsics
about **95 percent faster** than scalar JS (V8 blog); Frank Denis's 2023
benchmarks show Cranelift-based runtimes "virtually the same" as LLVM-based
ones for steady-state throughput.

References:

- <https://github.com/WebAssembly/spec/blob/wasm-3.0/proposals/relaxed-simd/Overview.md>
- <https://v8.dev/features/simd>
- <https://medium.com/wasmer/webassembly-and-simd-13badb9bf1a8>

---

## 5. Component Model, WIT, WASIp2 and p3

**Wasmtime** is the reference implementation.

- WASIp2 and the Component Model: **production** since v20.0 (2024) and
  solidly mature by v25.0. WIT-driven bindgen (`wasmtime::component::bindgen!`)
  is the preferred API surface for typed component calls.
- WASIp3 async: landed in 36.0 (August 2025) and continues to stabilize in
  v41 to v43 (2026). WASIp3 is **experimental** and off by default; wasmtime
  tracks the snapshot (currently `0.3.0-rc-2026-03-15`).
- Call overhead: "highly optimized, some overhead versus raw module interactions,
  especially for very frequent, small calls. Area of ongoing optimization." No
  concrete nanosecond figures published for 2026. In practice: meaningful
  overhead for tiny frequent calls, negligible for our case (one call per
  frame).

**Wasmer**: Aligning with the Component Model, async work in progress. Wasmer
positions WASIX as its preferred runtime ABI, which is a Wasmer-specific
extension, not a Component Model standard. For us, WASIX is irrelevant.

**Wasmi**: No Component Model support.

**Extism**: Uses its own ABI, not the Component Model. Extism's FAQ is
explicit that WIT / Component Model adoption is gated on ecosystem stability.

**Is WIT production-ready?** For core WASIp2 worlds (cli, http, filesystem,
sockets): yes, in wasmtime, since 2024. For p3 with async: no, 2026 is still
stabilization. For custom-authored WIT interfaces (our case, defining a
`hypercolor:effect` world): yes, wasmtime's bindgen is solid and used in
production by Fermyon, Cosmonic, and others.

**Our recommendation on WIT for Hypercolor effects**: Define a custom
`hypercolor:effect/renderer@0.1.0` world. Keep it tiny:

```wit
interface renderer {
  render: func(frame: frame-input) -> result<_, render-error>;
  resize: func(width: u32, height: u32);
  canvas-buffer: func() -> list<u8>;
}
```

Use wasmtime `bindgen!` on the host, `wit-bindgen` on the guest. This
tradeoff buys us typed cross-language plugin authoring (Rust, C, AssemblyScript,
Grain, Python via Componentize-Py) at the cost of a small call-overhead tax.

---

## 6. Hot Reload and Unload Semantics

This is the subtle one and it shaped our recommendation.

**Wasmtime has no module unload API.** Per wasmtime issue #2210 (open since
2020, still applicable in 2026): "no form of GC is implemented at this time,
so once an instance is created within a Store it will not be deallocated
until the Store itself is dropped." Module is `Arc`-refcounted, so dropping
the last `Module` reference does reclaim compiled code, but only if no Store
still holds an instance of it.

**The conventional pattern**: one `Engine` for the whole process, one
short-lived `Store` per plugin instance. To "hot reload" an effect:

1. Drop the old `Store` (reclaims the old instance's linear memory, tables,
   state).
2. Drop the old `Module` if no other Store holds it (reclaims compiled code
   and `.cwasm` mmap).
3. Load the new `.wasm` or `.cwasm`, create a new `Module`, create a new
   `Store`, instantiate.

With the **pooling allocator**, step 1 returns the instance's memory and
tables to a pre-allocated pool, and step 3's instantiation reuses that
pool slot. **Instance spin-up drops from milliseconds to microseconds** with
this setup. Copy-on-write heap images mean initial data is not re-materialized
if the plugin only reads it.

For Hypercolor, this is ideal: our render pipeline already owns an
`EffectEngine` behind a `Mutex`. Hot reload is a `Mutex::lock()` and swap of
the `Store` plus `Instance`. The rendering thread sees a brief stall, not a
process restart.

**Wasmer** has the same fundamental shape: Store-scoped, drop to reclaim.
Wasmer has done some work to move drop responsibility "to the module itself"
(commit 2f10460), but the semantic contract is unchanged.

**Wasmi**: interpreter, so no compiled code to manage, only bytecode and
instance state. Drop the `Store` and you reclaim everything. Cheapest hot
reload of the four, at the cost of not having native code in the first place.

**Extism**: explicit `Plugin::new` and `Plugin` drop, plus a `Pool` abstraction
for reuse. Extism 1.20 made `UserData` thread-safety mandatory for pool use,
which is an API break but aligned with our multi-thread daemon.

**Concurrent load while others execute**: all four support it, as long as
each plugin runs in its own Store (for wasmtime/wasmer/wasmi) or its own
Plugin instance (for Extism). The `Engine` is `Send + Sync` in wasmtime and
shareable across threads; `Store` is not.

---

## 7. Memory Model

Hypercolor's Canvas is effectively `Arc<Mutex<CanvasFrame>>` where
`CanvasFrame` carries a `Vec<u8>` of RGBA pixels at a configurable width and
height. Per-frame, we need to lend that buffer to the plugin for the duration
of its `render` call.

**Wasmtime**:

- `Memory::data(&store) -> &[u8]` and `Memory::data_mut(&mut store) -> &mut [u8]`
  expose the guest linear memory directly as Rust slices.
- The slice borrows the `Store` context for its lifetime. Good: the compiler
  enforces no concurrent mutation, no reenter into guest. Bad: you cannot
  hold it across a `func.call()`.
- Two patterns for our case:
  1. **Guest owns the canvas**: plugin exports `canvas_ptr() -> u32` and the
     host reads / writes through that pointer before and after `render`. This
     is the zero-copy path. Plugin allocates `W*H*4` bytes once at init.
  2. **Host writes a header**: plugin exports `init(width, height)` and
     `render(time_ms, audio_ptr, audio_len)`. Canvas offset is fetched post-init
     and cached host-side.
- For SharedMemory (threads proposal), base pointer is stable for the lifetime
  of the `SharedMemory`, enabling long-lived raw slices, but this requires
  the threads proposal enabled and introduces atomics; unneeded for us.
- Custom `LinearMemory` trait lets us back guest memory with host-managed
  allocation, useful if we want a slab allocator or memfd-backed canvas.
- Memory growth: guests can call `memory.grow`; host can call
  `Memory::grow(&mut store, delta)`. Limits enforced via `ResourceLimiter`.

**Wasmer**:

- `MemoryView` is a cursor, with `.read()` and `.write()` methods that copy.
- `.data_unchecked()` / `.data_unchecked_mut()` are `unsafe` raw-slice escapes,
  the workaround for our zero-copy need. We would have to justify `unsafe`
  in a workspace that forbids `unsafe_code`.
- This is the single biggest ergonomic gap versus wasmtime for our workload.

**Wasmi**:

- Same shape as wasmtime: `Memory::data` and `Memory::data_mut` lend slices.
- `no_std` friendly. Memory is a `Vec<u8>` internally.

**Extism**:

- Inaccessible. The Extism kernel is the canonical way to move data. For
  bulk transfers, it exposes block-at-a-time `load_u64` reads, which is the
  throughput story we measured below.

**Interop with borrow checker**: All three direct runtimes (wasmtime, wasmer,
wasmi) force you to think in "borrow store until done with slice." The
idiomatic pattern for us is:

```rust
// One-shot lend:
let mut store = /* per-frame or long-lived */;
// Before call:
{
    let mem = memory.data_mut(&mut store);
    mem[canvas_ptr..canvas_ptr + canvas_len]
        .copy_from_slice(&host_audio_samples);
} // mem borrow ends, store free
render.call(&mut store, ())?;
// After call:
{
    let mem = memory.data(&store);
    host_canvas.copy_from_slice(&mem[canvas_ptr..canvas_ptr + canvas_len]);
}
```

For true zero-copy (plugin writes directly into the host display buffer),
the cleanest path is a **custom `MemoryCreator` in wasmtime** that allocates
guest memory out of a shared `memfd` we mmap into the host. The plugin sees
a regular linear memory; the host sees the same bytes without copying.
Wasmtime's `MemoryCreator` trait is stable and documented.

---

## 8. Call Overhead Benchmarks

Per-call host to guest (direct export, no Component Model):

| Runtime  | Per-call overhead                                                              | Source                                    |
| -------- | ------------------------------------------------------------------------------ | ----------------------------------------- |
| Wasmtime | ~10 ns                                                                         | BA 2023 perf writeup, trampoline overhaul |
| Wasmer   | ~10 to 30 ns                                                                   | Same perf class; LLVM ties Cranelift      |
| Wasmi    | ~200 to 500 ns (interp)                                                        | Inferred; interpreter dispatch dominates  |
| Extism   | ~4.75 to 6.7 ns per kernel call; **plus** full call overhead per `plugin.call` | Dylibso 2024 writeup                      |

The Dylibso post is explicit: reading a 6.25 MiB payload via Extism's kernel
costs "approximately 3 million function calls," achieving about 1.57 GiB per
second one-way, dropping to 284.63 MiB per second for a four-way roundtrip.
Our canvas is 1.2 MiB at 640x480 RGBA; at 60 fps that is 72 MiB per second.
Nominally fine. But we would also be moving audio buffers, control state,
and metadata each frame, and we pay call overhead per kernel call, not per
byte. Extism's staging model is simply the wrong shape for streaming
uniform-size frame buffers.

**Component Model overhead vs raw exports** in wasmtime: official position
is "some overhead ... for very frequent, small calls." No published 2026
numbers. Anecdotally, 20 to 100 nanoseconds additional per call for simple
argument types, more for string or list types due to canonical ABI lift/lower.
For our once-per-frame `render` call this is invisible. For a hot-path
per-pixel host call it would be crippling; do not design that way.

**Marshalling cost for pointer + length** in wasmtime: near-zero. `(i32, i32)`
arguments lower to register moves on x86-64 / aarch64 in the trampoline. This
is the fastest possible shape.

---

## 9. Binary Size and Dependency Footprint

Rust binary size impact when embedded, approximate order of magnitude:

| Runtime  | Full build       | Minimal build                | Notes                                  |
| -------- | ---------------- | ---------------------------- | -------------------------------------- |
| Wasmtime | 30 to 80 MB      | 2.1 MB (runtime-only, C API) | Disable cranelift + winch + cache etc. |
| Wasmer   | 30 to 60 MB      | Unpublished minimal target   | LLVM backend adds tens of MB           |
| Wasmi    | ~2 MB            | sub-MB achievable            | Pure Rust, no_std, no LLVM/Cranelift   |
| Extism   | Wasmtime + ~5 MB | Cannot go below Wasmtime     | Framework layer on top                 |

For us, Hypercolor already ships **Servo** for HTML effect rendering, which
is hundreds of megabytes. Adding 30 to 80 MB of wasmtime is noise. The
binary-size argument favors wasmi on an embedded build target, but we are
targeting desktop Linux with Servo already in the dependency tree.

The **runtime-only / precompile-elsewhere** split is useful: we could ship
the daemon with a precompile-capable wasmtime (full size) for development
and a runtime-only wasmtime (2 MB) for release distribution, compiling
effects on a build server. This adds operational complexity; file under
"future optimization."

References:

- <https://docs.wasmtime.dev/examples-minimal.html>
- <https://lib.rs/crates/wasmtime/features>

---

## 10. Security Model and Resource Limits

| Control                       | Wasmtime                            | Wasmer                | Wasmi            | Extism (via wasmtime) |
| ----------------------------- | ----------------------------------- | --------------------- | ---------------- | --------------------- |
| Capability-based sandbox      | Yes, via WASI                       | Yes, WASI/WASIX       | Yes, WASI opt-in | Yes (wasmtime)        |
| Memory limits                 | `ResourceLimiter`                   | Config-driven         | ResourceLimiter  | Manifest              |
| CPU time / wall time          | **Epoch deadlines** (~10% overhead) | Metering API          | Fuel only        | Timer (via wasmtime)  |
| Deterministic instruction cap | **Fuel** (expensive)                | `Metering` middleware | **Fuel**         | Fuel (via wasmtime)   |
| Per-function fuel cost tuning | **Yes** (43.0, March 2026)          | Middleware weights    | Basic fuel       | Yes (via wasmtime)    |
| Stack depth limit             | Yes, config                         | Yes                   | Yes              | Yes                   |
| Table element cap             | ResourceLimiter                     | Config                | ResourceLimiter  | Manifest              |

**Wasmtime's epoch interruption** is the primitive we want for per-frame
budget enforcement:

- Host increments an atomic counter on a timer (say, every millisecond).
- Wasm code checks the counter at block boundaries (10 percent overhead).
- If it exceeds the deadline, execution traps with `Trap::Interrupt`.
- We catch the trap on the render thread and skip the frame.
- Non-deterministic (depends on wall clock), but that is fine: we are
  budget-capping, not replaying.

**Fuel** is the deterministic alternative: "same program, same fuel, same
interrupt location." Has a significant performance hit because every basic
block increments a counter. Useful if we want reproducible effects (seeding
a demoscene-style visual, for example) or running untrusted code with a
strict instruction count.

Wasmtime 43.0 (March 2026) added **per-operator fuel cost configuration**,
which lets us weight memory operations differently from arithmetic. Overkill
for our initial spec; valuable later.

**Our design**: epoch-based deadlines for production, fuel available as an
opt-in for deterministic effects. Both together is supported.

---

## 11. Comparison Matrix for Hypercolor Effects

Ranking: 1 (best for our use case) to 4.

| Criterion                            | Wasmtime             | Wasmer            | Wasmi      | Extism          |
| ------------------------------------ | -------------------- | ----------------- | ---------- | --------------- |
| Zero-copy `&mut [u8]` canvas lending | **1**                | 3 (unsafe)        | 2          | 4 (copy)        |
| Per-frame budget enforcement         | **1** (epoch)        | 2 (metering)      | 3 (fuel)   | 2 (via wt)      |
| Host to guest call overhead          | **1** (~10 ns)       | 1                 | 3 (interp) | 2               |
| SIMD throughput (v128)               | **1**                | 1                 | 4 (interp) | 1               |
| AOT disk cache                       | 1                    | **1** (pluggable) | 4 (n/a)    | 1               |
| Hot reload (drop Store)              | **1**                | 1                 | **1**      | 2 (Pool)        |
| Component Model / WIT readiness      | **1**                | 2                 | 4 (n/a)    | 3               |
| Pooling / fast re-instantiation      | **1** (COW)          | 2                 | 3          | 2               |
| Binary size added to Hypercolor      | 3                    | 3                 | **1**      | 3               |
| Security / resource limit maturity   | **1**                | 2                 | 2          | 1               |
| Crate ergonomics for our engine      | **1**                | 2 (MemoryView)    | 1          | 3 (opinionated) |
| Active 2026 release cadence          | **1** (monthly, LTS) | 2 (quarterly)     | 2          | 2               |
| **Aggregate for our use case**       | **1**                | 2                 | 3          | 4               |

Wasmi ranks high on hot reload, call ergonomics, and binary size, but the
interpreter tax on SIMD and steady-state throughput disqualifies it for a
60 fps canvas workload. Wasmer is close to wasmtime technically but loses on
`MemoryView` friction, looser security primitives, and a slower release
cadence. Extism solves a different problem (polyglot plugin ecosystem) and
its staging model double-copies our canvas.

---

## 12. Recommendation

**Use `wasmtime` directly, pinned to the current LTS (36.0 through August
2027), and migrate to 48.0 LTS when it ships in August 2026.**

Concrete stack:

- **Runtime**: `wasmtime = "36"` (LTS) with features
  `["async", "cranelift", "component-model", "pooling-allocator", "cache"]`.
- **Wire format**: Raw Core Wasm exports for v0.1 (simplest zero-copy path),
  migrate to WASIp2 Component Model with a custom
  `hypercolor:effect/renderer@0.1.0` WIT world in v0.2 once our plugin
  authoring story proves out.
- **Resource control**: `Config::epoch_interruption(true)` with a host
  timer advancing the engine epoch on a 1 millisecond tick. Each `render`
  call sets a per-frame deadline. Fuel metering off by default, opt-in via
  effect manifest for deterministic effects.
- **AOT**: `Engine::precompile_module` on first load, persist `.cwasm` under
  `$XDG_CACHE_HOME/hypercolor/effects/<hash>.cwasm`. Invalidate cache on
  wasmtime version change or CPU feature set change.
- **Memory**: Custom `MemoryCreator` backing guest linear memory with an
  `Arc<Mutex<Vec<u8>>>` the host shares with the canvas compositor. Zero-copy
  per frame.
- **Hot reload**: One `Engine` for the process, one `Store` per active
  effect. Swap on file change via `notify` crate. Pooling allocator makes
  the swap cost microseconds.
- **Sandboxing**: No WASI imports in v0.1. Effects are pure functions:
  `init`, `resize`, `render`. In v0.2, add `wasi:clocks/monotonic-clock` and
  `wasi:random/random` through a Linker. Never expose filesystem, network,
  or environment variables.
- **Threading**: Effects are single-threaded inside wasmtime. The render
  thread holds the `Store` mutex. If we want parallel effects (split-screen
  composition), give each effect its own `Store` on its own thread, share
  the `Engine`.

**Rejected alternatives, recorded for the spec:**

- **Extism**: adopt only if we later want a polyglot plugin marketplace
  (JavaScript, Python, Ruby effect authors). Defer until we have that need.
- **Wasmer**: technically competitive; the `MemoryView` copy cost and slower
  security advisory cadence tip it. Revisit if wasmtime governance or
  licensing ever becomes a concern.
- **Wasmi**: keep in our back pocket for a future "safety-critical deterministic
  effects" mode where no JIT is tolerable (kernel-level constrained environments,
  proof-of-concept for no_std embedded Hypercolor targets).

---

## Sources

Dated inline; collected here for a single-scroll reference.

- Wasmtime releases: <https://github.com/bytecodealliance/wasmtime/releases>
- Wasmtime LTS policy (2025-07-09): <https://bytecodealliance.org/articles/wasmtime-lts>
- Wasmtime 2023 perf writeup (call overhead, trampolines): <https://bytecodealliance.org/articles/wasmtime-and-cranelift-in-2023>
- Wasmtime 1.0 perf (instantiation, CoW): <https://bytecodealliance.org/articles/wasmtime-10-performance>
- Wasmtime 25.0 announcement: <https://bytecodealliance.org/articles/wasmtime-25.0>
- Wasmtime security advisories (2026-04-09): <https://bytecodealliance.org/articles/wasmtime-security-advisories>
- Wasmtime 43.0 coverage (2026-03-20): <https://www.prismnews.com/hobbies/rust-programming/wasmtime-4300-arrives-with-wasip3-support-and-expanded>
- Wasmtime pre-compilation docs: <https://docs.wasmtime.dev/examples-pre-compiling-wasm.html>
- Wasmtime fast instantiation: <https://docs.wasmtime.dev/examples-fast-instantiation.html>
- Wasmtime minimal embedding: <https://docs.wasmtime.dev/examples-minimal.html>
- Wasmtime interrupting execution (fuel, epoch): <https://docs.wasmtime.dev/examples-interrupting-wasm.html>
- Wasmtime deterministic execution: <https://docs.wasmtime.dev/examples-deterministic-wasm-execution.html>
- Wasmtime Memory API docs: <https://docs.wasmtime.dev/api/wasmtime/struct.Memory.html>
- Wasmtime PoolingAllocationConfig: <https://docs.wasmtime.dev/api/wasmtime/struct.PoolingAllocationConfig.html>
- Wasmtime module unloading issue #2210 (open, still relevant): <https://github.com/bytecodealliance/wasmtime/issues/2210>
- Wasmtime WASIp2 plugin pattern: <https://docs.wasmtime.dev/wasip2-plugins.html>
- Wasmer releases: <https://github.com/wasmerio/wasmer/releases>
- Wasmer 7.0 blog (2026-01-28): <https://wasmer.io/posts/wasmer-7>
- Wasmer SIMD writeup: <https://medium.com/wasmer/webassembly-and-simd-13badb9bf1a8>
- Wasmi releases: <https://github.com/wasmi-labs/wasmi/releases>
- Wasmi 1.0 announcement: <https://wasmi-labs.github.io/blog/posts/wasmi-v1.0/>
- Wasmi 0.32 (register IR, 5x speedup): <https://wasmi-labs.github.io/blog/posts/wasmi-v0.32/>
- Wasmi benchmarks repo: <https://github.com/wasmi-labs/wasmi-benchmarks>
- Extism releases: <https://github.com/extism/extism/releases>
- Extism Rust SDK docs: <https://docs.rs/extism/latest/extism/>
- Dylibso "How does Extism work" (call overhead numbers): <https://dylibso.com/blog/how-does-extism-work/>
- WebAssembly 3.0 announcement (2025-09-24): <https://www.x-cmd.com/blog/250924/>
- WebAssembly 3.0 change history (2026-04-09): <https://webassembly.github.io/spec/core/appendix/changes.html>
- WASI and Component Model current status (2025-02-16, 2026-referenced): <https://eunomia.dev/blog/2025/02/16/wasi-and-the-webassembly-component-model-current-status/>
- State of WebAssembly 2025-2026: <https://platform.uno/blog/the-state-of-webassembly-2025-2026/>
- Relaxed SIMD spec: <https://github.com/WebAssembly/spec/blob/wasm-3.0/proposals/relaxed-simd/Overview.md>
- V8 SIMD overview: <https://v8.dev/features/simd>
- Frank Denis 2023 benchmarks: <https://00f.net/2023/01/04/webassembly-benchmark-2023/>
