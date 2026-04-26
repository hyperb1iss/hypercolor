# Prior-Art WebAssembly Plugin Ecosystems

_Research date: 2026-04-18. Target: WASM effect loader for the Hypercolor daemon._

The Hypercolor render loop needs third-party effects that can run on a 16.6 ms frame budget at 60 fps, recover gracefully from misbehaving guests, and be distributable to non-Rust authors. This document surveys shipped WebAssembly plugin ecosystems, then distills their techniques into a patterns inventory we can pull from when designing the loader.

Each section cites public engineering writeups, spec repos, or product docs, with dates where the source makes them explicit.

## 1. Zed extensions (wasmtime + WIT)

Zed's extension system is the closest cultural fit to what we want: an editor that boots fast, runs on a tight per-frame budget, and still exposes a polyglot-friendly plugin surface. Zed settled on WebAssembly with the Component Model as of the v0.131 extensions release, and the design has aged well.

The host-guest contract is a WIT world defined in `crates/extension_api/wit/since_v0.2.0/`. Extensions implement a Rust trait (`zed::Extension`) and the `zed_extension_api` crate uses `wit_bindgen::generate!` at compile time to turn the WIT into C-ABI-compatible Rust. On the host side, Zed uses `wasmtime::component::bindgen!` with `async: true` so that extension calls into the editor feel synchronous to the guest while the host thread can continue doing real work. The two sides are generated from the same WIT, which makes type drift structurally impossible. See ["Life of a Zed Extension: Rust, WIT, Wasm"](https://zed.dev/blog/zed-decoded-extensions) for the full walkthrough.

The extension capabilities cover language servers (LSP), slash commands for the AI assistant, context servers (MCP), Tree-sitter grammar bundles, themes, snippets, and icon themes. The `wasm_api_version` is embedded in every manifest (currently `0.0.6`), which lets Zed reject extensions built against incompatible hosts without loading them. This is the shape of "structural version gating" we should copy.

Distribution is lightweight and git-native. Authors push to GitHub, then open a PR against [`zed-industries/extensions`](https://github.com/zed-industries/extensions). CI on merge compiles the extension server-side via the `extensions_cli` tool, produces a `.tar.gz` containing the `extension.wasm`, Tree-sitter grammars, and Scheme query files, and uploads to S3. Zed's in-app browser fetches metadata from the zed.dev API. As of October 2025, the registry requires a valid open-source license on the extension repo or CI rejects the PR.

Developer onboarding: `zed: install dev extension` points Zed at a local directory, and you iterate without publishing. There's no true hot reload in the editor-process sense; you re-install the dev extension after rebuilding. For an editor this is fine. For us it is probably not, because effect authors will want to see color changes live.

Sources: [Zed blog post on extensions](https://zed.dev/blog/zed-decoded-extensions), [Zed extensions repo](https://github.com/zed-industries/extensions), [Developing Extensions docs](https://zed.dev/docs/extensions/developing-extensions), [`zed_extension_api` on docs.rs](https://docs.rs/zed_extension_api).

## 2. Figma plugins and widgets

Figma's story is not WASM-first, but its sandbox architecture is worth stealing from because Figma is the reference example of "millions of users safely running untrusted third-party code in a creative tool." Their plugin VM is [QuickJS compiled to WebAssembly](https://www.figma.com/blog/how-we-built-the-figma-plugin-system/), which is a clever inversion: the plugin code is JavaScript, but the JS engine itself is the WASM sandbox. This lets Figma expose a high-level JS API to plugin authors while getting the sandbox guarantees of WASM.

The architecture splits plugins into two processes. The "main" script runs inside the QuickJS/WASM sandbox with access to the Figma document API but no browser APIs. A separate UI iframe handles anything that needs real DOM, `fetch`, or `localStorage`, and the two halves communicate via message passing through `figma.showUI()`. Browser APIs like `XMLHttpRequest`, `fetch`, `setTimeout`, and DOM are deliberately absent from the main sandbox. The ["How plugins run"](https://developers.figma.com/docs/plugins/how-plugins-run/) docs describe this clearly.

Distribution is a curated community store with a human review step. Figma says the review typically completes in [5-10 business days](https://help.figma.com/hc/en-us/articles/360042293394-Publish-plugins-to-the-Figma-Community) and once approved, authors can publish updates immediately without re-review. Private distribution exists for enterprise orgs and skips review entirely. There's also an optional security disclosure form that Figma reviews separately.

The real takeaway for us: **the dual-process model (sandboxed compute + capability-gated UI)** maps cleanly onto our architecture of "effect code running on the render thread + optional UI running in the daemon's preview/tui layer."

Sources: ["How we built the Figma plugin system"](https://www.figma.com/blog/how-we-built-the-figma-plugin-system/), [Figma developer docs](https://developers.figma.com/docs/plugins/how-plugins-run/), [Plugin review guidelines](https://help.figma.com/hc/en-us/articles/360039958914-Plugin-and-widget-review-guidelines).

## 3. Fermyon Spin and wasmCloud

Both products are serverless-WASM platforms built on the Component Model. Our daemon is not a scheduler, but the way they compose components and declare capabilities in a manifest is directly applicable to how an effect declares what it needs from the host.

Spin 2.0 (November 2023) and Spin 3.0 (November 2024) shipped with a `spin.toml` manifest that lists components, their triggers, and their allowed capabilities. The big addition in 2.0 was [component composition](https://www.fermyon.com/blog/composing-components-with-spin-2): a single Spin app can be built by linking multiple isolated components via upstream Component Model tooling, so platform teams can swap out a shared "http-client" or "key-value" component without touching the developer's code. The 3.0 release added [selective deployments](https://www.infoworld.com/article/2335330/spin-20-shines-on-wasm-component-composition-portability.html), which repackage components into different microservice shapes.

wasmCloud (CNCF incubating) takes a different tack: components are "actors" and capabilities ("HTTP server", "key-value", etc.) are served by separate "providers" over [NATS-based lattices](https://wasmcloud.com/docs/v1/concepts/lattice/). Their [Q3 2025 roadmap](https://wasmcloud.com/blog/globally-distributed-webassembly-applications-with-wasmcloud-and-nats/) transitioned providers to a wRPC server model, where WIT interfaces are served over TCP, NATS, QUIC, or UDP transports. The wRPC serialization gives us a reference for how to turn a WIT interface into on-wire bytes if we ever want to let effects live out-of-process.

The relevant lesson for us is less about serverless and more about **the manifest-declares-capabilities pattern**. An effect should declare, in a `hypercolor-effect.toml` or WIT world, "I need audio FFT of size N, keyboard pressure, and the GPU canvas API at version X." The loader should be able to refuse to instantiate an effect whose declared capabilities don't match the host's current state (no audio device connected, wrong API version).

Sources: ["Composing Components with Spin 2.0"](https://www.fermyon.com/blog/composing-components-with-spin-2), ["Introducing Spin 2.0"](https://www.fermyon.com/blog/introducing-spin-v2), [wasmCloud lattice concepts](https://wasmcloud.com/docs/v1/concepts/lattice/).

## 4. Shopify Functions

Shopify runs merchant-authored WASM in the hot path of checkout. Their budget model is the closest existing analogue to "a plugin that must return within a frame."

The [WebAssembly for Functions docs](https://shopify.dev/docs/apps/build/functions/programming-languages/webassembly-for-functions) lay out hard per-invocation limits: **256 KB WASM binary cap, 11M instruction limit per invocation, 30-point query complexity ceiling**. The 11M instruction limit is enforced by Wasmtime's fuel accounting in Shopify's host (see their engineering blog ["How Shopify uses WebAssembly outside the browser"](https://shopify.engineering/shopify-webassembly)). This is interesting because fuel is deterministic, but slower than epoch interruption; Shopify traded speed for audit determinism, which makes sense when every call touches money.

Distribution is via the Shopify App Store. Functions deploy as part of an app version (a snapshot of code + extensions). Each extension has a UID set in `shopify.extension.toml` that maps code to the record on Shopify's side. CI/CD deploys are a normal `shopify app deploy` invocation that creates an immutable app version and releases it atomically. For public distribution, apps go through human review against a published rubric; for custom (single-merchant) distribution, you bypass review and ship via a private link. See [distribution docs](https://shopify.dev/docs/apps/launch/distribution).

The [shopify-function-wasm-api](https://github.com/Shopify/shopify-function-wasm-api) repo shows their ABI: guest functions receive JSON (or cbor, depending on the extension point) as a buffer in linear memory, return a buffer with the result. Simple and language-agnostic.

Takeaways: **strict per-invocation instruction budgets enforced by the runtime, immutable versioned deploys, curated + private distribution channels side by side.**

Sources: [Shopify WASM docs](https://shopify.dev/docs/apps/build/functions/programming-languages/webassembly-for-functions), [Shopify engineering blog](https://shopify.engineering/shopify-webassembly), [shopify-function-wasm-api repo](https://github.com/Shopify/shopify-function-wasm-api), [app distribution](https://shopify.dev/docs/apps/launch/distribution).

## 5. Envoy proxy WASM filters (proxy-wasm)

Envoy runs WASM on the packet path in production, which is the exact profile we face. The [proxy-wasm spec](https://github.com/proxy-wasm/spec) defines an ABI version (0.2.1 is the current recommended) for plugin interactions.

Execution model: plugins run inline on Envoy's worker threads and are invoked via stream callbacks defined by the ABI. Each worker thread has its own WASM instances and their own linear memory; instances are not shared across workers. This is a clean way to avoid cross-thread locking and matches our render thread's desire to own its plugin state exclusively. See the [Envoy Wasm architecture overview](https://www.envoyproxy.io/docs/envoy/latest/intro/arch_overview/advanced/wasm).

The ABI handles the "pass a buffer across the host-guest boundary" problem by copying from Envoy memory into WASM linear memory and back. Every HTTP filter has two context types: a root context (ID 0, config-only) and a per-request context that the host creates fresh for each request. Root context persists for the lifetime of the filter; per-request context is short-lived and gets per-request state.

Asynchronous and blocking operations (DNS, HTTP fan-out, gRPC) are delegated to Envoy; the plugin calls a host function that starts the operation and gets a completion callback invoked on the same worker when the result comes back. The plugin itself is always synchronous.

For budget enforcement, the proxy-wasm spec doesn't mandate a specific mechanism, but implementations typically use wasmtime's epoch-based interruption. An Envoy-internal timer bumps the global epoch every N milliseconds; any plugin that doesn't yield gets interrupted. This is exactly the pattern we need.

Sources: [proxy-wasm spec](https://github.com/proxy-wasm/spec), [Envoy Wasm docs](https://www.envoyproxy.io/docs/envoy/latest/intro/arch_overview/advanced/wasm), [Tetrate's breakdown](https://tetrate.io/blog/wasm-modules-and-envoy-extensibility-explained-part-1), [Kong's proxy-wasm post](https://konghq.com/blog/engineering/proxy-wasm).

## 6. Fastly Compute@Edge

Fastly is the production reference for "WASM with sub-millisecond cold start at scale." The [Compute platform](https://docs.fastly.com/products/compute) runs a fresh WASM instance per request and destroys it when the response is sent.

The cold-start magic comes from three techniques:

- **AOT compilation**: modules are compiled to native machine code once (via Cranelift) and the native code is persisted. No JIT at request time.
- **The pooling allocator**: Wasmtime pre-reserves virtual memory slots up-front. Creating a new instance grabs a slot, not a fresh `mmap`.
- **Copy-on-write linear memory init**: the initial linear memory image is a [memfd that's mmapped CoW](https://github.com/bytecodealliance/wasmtime/pull/3697) into each slot; the instance pays for memory only when it writes.

The net is that [Lucet (Fastly's original runtime) instantiated in under 50 microseconds](https://www.fastly.com/blog/how-lucet-wasmtime-make-stronger-compiler-together), and Wasmtime has matched or exceeded that after the Fastly team ported the techniques over. Fastly migrated from Lucet to Wasmtime in mid-2020, consolidating on wasmtime + Cranelift.

For us, the relevant bits are: pre-compile WASM to `.cwasm` files and ship the cached artifact alongside the source module, always use the pooling allocator for the render thread, and rely on CoW init for cheap reset between instances.

Sources: [Fastly Compute docs](https://docs.fastly.com/products/compute), [Lucet + Wasmtime merger](https://www.fastly.com/blog/how-lucet-wasmtime-make-stronger-compiler-together), [wasmtime PoolingAllocationConfig](https://docs.wasmtime.dev/api/wasmtime/struct.PoolingAllocationConfig.html), [CoW allocator PR](https://github.com/bytecodealliance/wasmtime/pull/3697).

## 7. Web Audio Modules (WAM) v2

WAM 2 is the closest academic/industrial analogue to our problem: real-time compute on streaming buffers, polyglot plugin authoring, and a single standard usable across unrelated hosts. It's documented in a [2022 ACM paper](https://dl.acm.org/doi/fullHtml/10.1145/3487553.3524225) by Michel Buffa et al., and the [website](https://www.webaudiomodules.com/docs/intro/) still tracks the canonical implementation.

The architecture has three parts:

- **WAM processor**: a `WAM` instance that runs in the high-priority AudioWorklet thread as a WASM module. Extends `AudioWorkletProcessor`. This is the real-time DSP core.
- **WAM controller**: runs in the main thread, handles loading, preset management, and UI. Talks to the processor via message passing plus SharedArrayBuffer ring buffers.
- **WAM host**: the DAW or page embedding the plugin. Loads WAMs by URI.

The cleverest part is the MIDI and parameter transport. Message ports (the default AudioWorklet IPC) have allocation and latency overhead, so WAM 2 uses a [SharedArrayBuffer ring buffer](https://developer.chrome.com/blog/audio-worklet-design-pattern/) for param changes and MIDI. The processor drains the ring on each block, without crossing the audio thread barrier. This is the same trick we'd want for keyboard pressure, audio FFT frames, and any other high-frequency input: **give the plugin an SHM view into a ring that the host writes**, never round-trip through function calls.

What WAM 2 does right: uniform plugin ABI, cross-site discoverability via URI, CPU isolation via the AudioWorklet thread. What it struggles with: it's still "beta" five years in; there's no centralized registry (each plugin is a URL), and debugging WASM from the audio thread is painful. Over 40 plugins ship as of early 2024.

Sources: [WAM 2 intro](https://www.webaudiomodules.com/docs/intro/), [ACM paper](https://dl.acm.org/doi/fullHtml/10.1145/3487553.3524225), [AudioWorklet + WASM pattern](https://developer.chrome.com/blog/audio-worklet-design-pattern/).

## 8. Extism

Extism is the general-purpose polyglot plugin framework, by Dylibso (seed funded by Felicis in 2023 at $6.6M). Their thesis: "make all software programmable." The [PDK model](https://extism.org/docs/concepts/pdk/) gives you Rust, JS/TS, Go, Haskell, AssemblyScript, C, Zig, and .NET guest SDKs, plus host SDKs for about 16 languages.

ABI: plugin functions take a buffer of bytes and return a buffer of bytes. All serialization is the author's problem, which is a deliberate simplification. Input/output is stored in linear memory; the host passes `(offset, length)` pointers. Host functions are declared in the guest as `extern` and the PDK marshals calls through Extism-managed shared memory. See [Memory concepts](https://extism.org/docs/concepts/memory/) and [Host Functions](https://extism.org/docs/concepts/host-functions/).

Extism also ships built-in runtime limiters and timers, plus [HTTP without WASI](https://extism.org/docs/concepts/host-functions/) for network access. Their XTP platform is a commercial extension registry + signing system built on Extism, so authors can publish plugins and operators can pin/audit versions. Steve Manuel (Dylibso CEO) and Matt Butcher (Fermyon CEO) jointly pitch WASM as the universal plugin runtime; Dylibso sits below the Component Model in the stack and is pragmatic about not blocking on Component Model finalization.

Strengths: polyglot PDKs are real and shipped, not hypothetical. The bytes-in-bytes-out model is boring and works. Limitations: no Component Model integration (they predate stable CM), so type safety across the boundary is the author's responsibility. For us, the PDK model is the right DX aspiration: **one `hypercolor-effect-pdk` per language, generated from our WIT, so Python/JS/C++/Rust authors get an idiomatic local API.**

Sources: [Extism PDK docs](https://extism.org/docs/concepts/pdk/), [Extism GitHub](https://github.com/extism/extism), [Dylibso's "Why Extism?"](https://dylibso.com/blog/why-extism/), [TechCrunch on Dylibso funding](https://techcrunch.com/2023/03/24/dylibso-raises-6-6m-to-help-developers-take-webassembly-to-production/).

## 9. Hyperlight (Microsoft)

Hyperlight is Microsoft's micro-VM runtime for WASM, open-sourced November 2024 and accepted into the CNCF Sandbox in February 2025. The unique claim: [0.0009-second (900 microsecond) per-function execution](https://opensource.microsoft.com/blog/2025/02/11/hyperlight-creating-a-0-0009-second-micro-vm-execution-time/) with hardware-level isolation via KVM (Linux) or Windows Hypervisor Platform (Windows). Hyperlight-Wasm is the higher-level crate that runs Component Model WASM inside those VMs; see [hyperlight-wasm repo](https://github.com/hyperlight-dev/hyperlight-wasm).

VM creation: 1-2 ms today, targeting sub-millisecond. The VM has no OS kernel inside it. It's a slice of memory with a guest binary and a small "micro-guest" shim that handles host calls via a custom ABI. For WASM, the micro-guest is a Wasmtime runtime running inside the micro-VM.

Why it matters for us: Hyperlight gives you **hardware-enforced isolation** on top of the WASM sandbox. For Hypercolor, the WASM sandbox plus Rust's memory safety is probably enough; we're not running adversarial code on a shared tenant, we're running community-contributed effects on the user's own machine. But Hyperlight is the fallback if we ever needed "this plugin may not touch any memory except its own, enforced by the CPU." It's also a useful reference for sub-millisecond per-call overhead targets.

Microsoft's Azure Front Door Edge Actions service uses Hyperlight in production (private preview as of March 2025, per the [announcement post](https://opensource.microsoft.com/blog/2025/03/26/hyperlight-wasm-fast-secure-and-os-free/)).

Sources: [Hyperlight Wasm announcement](https://opensource.microsoft.com/blog/2025/03/26/hyperlight-wasm-fast-secure-and-os-free/), [0.0009s execution benchmark](https://opensource.microsoft.com/blog/2025/02/11/hyperlight-creating-a-0-0009-second-micro-vm-execution-time/), [hyperlight-wasm repo](https://github.com/hyperlight-dev/hyperlight-wasm).

## 10. Lunatic and wasmCloud actor model

Lunatic is an Erlang-inspired WASM runtime ([GitHub](https://github.com/lunatic-solutions/lunatic), YC W21). Each actor is its own WASM instance with its own linear memory and stack; actors communicate only by message passing through mailboxes. One actor crashing cannot corrupt another; supervisors restart crashed actors. This is great for backend services and awful for our use case because:

- Messages must be copied between actors' linear memories, which we'd pay on every frame.
- The message-passing model wants coarse-grained tasks, not a 16 ms deadline.
- Actor supervision overhead is designed for "long-running service with rare crashes," not "600 effects per second, each with a hard deadline."

wasmCloud's actor model is similar but distributed across a NATS lattice. See section 3.

What we can steal anyway: **supervisor trees for crash recovery.** If an effect panics, we don't want to take down the render thread. Spawning the effect in its own wasmtime `Store` with a "kill and respawn" policy on fuel exhaustion or panic matches the Erlang "let it crash" philosophy without paying for full actor IPC.

Sources: [Lunatic GitHub](https://github.com/lunatic-solutions/lunatic), [Launch HN](https://news.ycombinator.com/item?id=26367029).

## 11. Real-time creative apps without WASM (Resonite, VRChat, Roblox)

These are the most successful UGC-with-user-scripting platforms in production. None use WASM, but the design lessons matter.

**Resonite** uses [ProtoFlux](https://wiki.resonite.com/ProtoFlux), a node-based visual scripting language that compiles to something like FrooxEngine bytecode. For more serious extensibility, Resonite has a [plugin system](https://wiki.resonite.com/Plugins) that loads compiled C# DLLs at startup with a launch argument. Community plugins are UGC and the wiki explicitly warns they're not held to the same scrutiny as official nodes. As of the 2025 wiki, official custom node support is still being tracked in GitHub issues, not shipped.

**VRChat** uses [Udon](https://creators.vrchat.com/worlds/udon/), a custom bytecode VM interpreted by the VRChat client. Udon Graph is the visual interface, UdonSharp transpiles C# to Udon bytecode. The VM exposes only a whitelist of approved Unity and C# stdlib APIs; everything else is blocked. Shader property names must be prefixed `_Udon` or be the literal `_AudioTexture` to be set via `VRCShader.SetGlobal`, which is a string-level capability check on shader globals. VRChat still patches sandbox escapes periodically (see ["Breaking out of VRChat using a Unity bug"](https://khang06.github.io/vrcescape/) and the 2024.3.1p4 patch).

**Roblox** uses [Luau](https://luau.org/sandbox/), their Lua fork with gradual typing. Luau has VM-level support for a global interrupt that the host sets, and any Luau code is guaranteed to hit the interrupt handler eventually at function calls or loop iterations. This is [epoch-based interruption, Lua flavor](https://github.com/Roblox/luau/blob/master/SECURITY.md), and it's the feature that makes Roblox's UGC game engine workable. Each script also gets its own global table via `__index` into a builtin, so scripts can't pollute each other. Roblox's [plugin marketplace](https://devforum.roblox.com/t/introducing-plugin-marketplace/400582) is curated, requires ID verification to raise per-user publish limits above 2, and supports paid distribution.

What to learn:

- **Visual scripting layer on top of the real plugin runtime.** ProtoFlux and Udon Graph lower the barrier to entry for non-programmers. Worth considering as a follow-on to a "pro" WASM-native path.
- **API whitelists, not blacklists.** VRChat learned this the hard way. The set of exposed host capabilities should be explicit and small.
- **Interrupt-on-budget is a first-class VM feature.** Luau and wasmtime both have it; use it.
- **Curated marketplace with ID verification gates abuse.** Roblox's 2-plugin limit for unverified authors is a nice friction mechanism.

Sources: [Resonite ProtoFlux wiki](https://wiki.resonite.com/ProtoFlux), [Resonite Plugins wiki](https://wiki.resonite.com/Plugins), [VRChat Udon docs](https://creators.vrchat.com/worlds/udon/), [Udon VM and assembly](https://creators.vrchat.com/worlds/udon/vm-and-assembly/), [Luau sandbox](https://luau.org/sandbox/), [Luau SECURITY.md](https://github.com/Roblox/luau/blob/master/SECURITY.md), [Roblox Plugin Marketplace announcement](https://devforum.roblox.com/t/introducing-plugin-marketplace/400582).

## 12. Patterns inventory for Hypercolor

Twelve concrete techniques to pull into the WASM effect loader design. Each cites the ecosystem it comes from and explains how it maps to our frame-budget render loop.

**P1. WIT-first host-guest contract, generated on both sides.**
_Source: Zed, wasmCloud, Spin._
Define the host-plugin ABI in WIT (canvas dimensions, frame input, audio FFT, keyboard pressure, host logging). Use `wit_bindgen::generate!` on the guest side and `wasmtime::component::bindgen!` on the host side. Drift becomes structurally impossible and we get polyglot guest support for free as the Component Model toolchain matures.

**P2. Epoch-based interruption for per-frame budget enforcement.**
_Source: Envoy proxy-wasm, Roblox Luau's global interrupt._
A timer on the daemon's control thread bumps the wasmtime epoch once per frame (or once every N frames for slack). Effects that don't return within budget get trapped, we log the offending effect, and the render loop substitutes a safe fallback for that frame. Fuel is the alternative, but epoch is 2-3x cheaper at runtime per the wasmtime docs and we don't need deterministic auditability.

**P3. Pre-compile to `.cwasm` and use the pooling instance allocator.**
_Source: Fastly Compute@Edge, wasmtime pooling allocator._
At install time, compile each effect's `.wasm` to a `.cwasm` once and cache it. At runtime, instantiate via `PoolingAllocationConfig` with affine slots, so an effect that re-instantiates (on reload, on param change, on render restart) reuses the same memory slot and benefits from CoW init. This gets us sub-millisecond instance creation.

**P4. SHM ring buffer for high-frequency inputs, not function calls.**
_Source: Web Audio Modules 2._
Audio FFT frames, keyboard pressure, MIDI, and any other per-frame or per-packet streams live in a shared linear-memory region that the host writes and the guest reads. Avoids paying function-call overhead for every sample. The guest gets a pointer + length once at init and just reads from it each tick.

**P5. Declarative capability manifest per effect.**
_Source: Spin `spin.toml`, WASI capabilities, Shopify `shopify.extension.toml`._
Each effect ships a `hypercolor-effect.toml` or equivalent that declares: API version, required inputs (audio, screen, keyboard), canvas dimensions it supports, whether it uses host logging, whether it's CPU or GPU pathway. The loader rejects effects whose manifest doesn't match the current host. This turns "plugin crashes because audio device is missing" into "plugin never loads because manifest says it needs audio."

**P6. Immutable versioned bundles with API-version gating.**
_Source: Zed `wasm_api_version`, Shopify app versions._
Every published effect is an immutable `.tar.gz` containing the `.wasm`, `.cwasm` cache, manifest, and preview asset. The manifest embeds the `hypercolor_api_version` it was built against. The loader refuses to instantiate effects built against an incompatible host. This makes upgrades predictable.

**P7. Registry-as-git-repo with CI-built artifacts.**
_Source: Zed's `zed-industries/extensions` repo._
Authors submit PRs to a `hypercolor/effects` repo. CI builds the effect, runs it against a headless test host (render 60 frames, check no panic, check canvas checksum), produces a signed artifact, uploads to our CDN. The daemon fetches metadata from our API and downloads on demand. No central review team required for the first 100 effects; CI is the review.

**P8. Polyglot PDKs generated from the WIT.**
_Source: Extism PDKs._
Ship `hypercolor-effect-pdk` for at least Rust, JS/TS (via AssemblyScript or JCO), and Zig. Each PDK is a thin idiomatic wrapper over the generated WIT bindings so authors don't need to understand the Component Model to ship an effect. Rust and TS are the priorities; the others are nice-to-have.

**P9. Host capability whitelist, not blacklist.**
_Source: VRChat Udon, Luau's explicit builtin table._
The WIT world defines the exact set of host functions an effect can call. There is no escape hatch, no "just expose `std::fs` for debugging." If an effect needs a capability we haven't exposed, we add it to the WIT with intent and publish a new API version. VRChat has patched sandbox escapes repeatedly because they started with a blacklist; we don't.

**P10. Supervisor + fallback effect on crash.**
_Source: Lunatic/Erlang "let it crash."_
Each effect runs in its own `wasmtime::Store`. On panic or budget miss, the loader kills the store, logs to the event bus, and swaps a safe built-in effect (solid color, or the previous known-good effect) into the render pipeline for that slot. On repeated crashes (e.g., 3 times in 30 seconds), the effect is marked disabled until manually re-enabled. The render loop never stops.

**P11. Local dev-extension install with file-watcher hot reload.**
_Source: Zed `install dev extension`, plus our own SDK-dev pattern._
`hyper effect dev /path/to/effect-src` watches the source tree, rebuilds the `.wasm` on change, and swaps it into the running daemon at the next frame boundary. We already have the infra for this via `just effect-build`; the WASM loader just needs to accept a "replace this effect's module" command on a frame boundary. Zed doesn't do true hot reload; we can, because we control the render tick.

**P12. Dual distribution: public registry + private/local.**
_Source: Figma private plugins, Shopify custom distribution, Roblox's verified-author publish limits._
Public registry goes through CI-gated review. Private distribution is "drop a `.tar.gz` in `~/.local/share/hypercolor/effects/` and it loads." This serves enterprise/internal use cases and demo pipelines without blocking on our review queue.

## Top 5 ideas to copy

If we have to pick the highest-leverage items first, these are the ones.

1. **WIT-defined contract with wasmtime Component Model on the host.** This is load-bearing: it's the foundation everything else builds on. Copy Zed's generate-from-the-same-WIT-on-both-sides discipline. Worst case we start with core Wasm + wit-bindgen and upgrade to full Component Model when WASI 0.3 lands later in 2026.

2. **Epoch-based interruption tied to the frame tick.** Bump the epoch once per frame from the render loop's control thread. Any effect that doesn't return within budget traps and gets substituted. This is the single most important safety mechanism; without it, a bad effect kills the render loop.

3. **Pooling allocator + pre-compiled `.cwasm` per effect.** Pay compile cost once at install/CI time, pay instance cost in microseconds at runtime. This turns "WASM effects are dangerous for a real-time loop" into "WASM effects are free to swap in and out mid-frame."

4. **Shared linear-memory ring buffer for high-frequency inputs.** Audio FFT, keyboard pressure, interaction state: all flow through an SHM region the guest reads each tick. No per-sample function calls. This is the WAM 2 trick and it's the right call for anything at render-tick frequency.

5. **Git-native registry with CI-built signed bundles.** Authors PR to `hypercolor/effects`. CI validates the manifest, compiles `.wasm` → `.cwasm`, runs a headless render smoke test, signs the bundle, uploads. No human gatekeeper for v1. Clone Zed's workflow; it's proven at scale and it gets us a real plugin ecosystem without building a store backend.

The combination of 1 + 2 + 3 + 4 is the minimum viable real-time WASM plugin runtime. Pattern 5 is what makes it a real ecosystem rather than a party trick. Everything else in the inventory is follow-on work.

## Sources

- [Life of a Zed Extension: Rust, WIT, Wasm](https://zed.dev/blog/zed-decoded-extensions) — Zed Blog
- [Zed Extensions Repo](https://github.com/zed-industries/extensions) — GitHub
- [Developing Zed Extensions](https://zed.dev/docs/extensions/developing-extensions) — Zed Docs
- [`zed_extension_api` crate](https://docs.rs/zed_extension_api) — docs.rs
- [How Figma Built Its Plugin System](https://www.figma.com/blog/how-we-built-the-figma-plugin-system/) — Figma Blog
- [Figma: How Plugins Run](https://developers.figma.com/docs/plugins/how-plugins-run/) — Figma Developer Docs
- [Figma Plugin Review Guidelines](https://help.figma.com/hc/en-us/articles/360039958914-Plugin-and-widget-review-guidelines) — Figma Help Center
- [Composing Components with Spin 2.0](https://www.fermyon.com/blog/composing-components-with-spin-2) — Fermyon
- [Introducing Spin 2.0](https://www.fermyon.com/blog/introducing-spin-v2) — Fermyon
- [wasmCloud Lattice](https://wasmcloud.com/docs/v1/concepts/lattice/) — wasmCloud Docs
- [wasmCloud Globally Distributed WebAssembly](https://wasmcloud.com/blog/globally-distributed-webassembly-applications-with-wasmcloud-and-nats/) — wasmCloud Blog
- [Shopify WebAssembly for Functions](https://shopify.dev/docs/apps/build/functions/programming-languages/webassembly-for-functions) — Shopify Dev
- [How Shopify Uses WebAssembly Outside the Browser](https://shopify.engineering/shopify-webassembly) — Shopify Engineering
- [shopify-function-wasm-api](https://github.com/Shopify/shopify-function-wasm-api) — GitHub
- [Shopify App Distribution](https://shopify.dev/docs/apps/launch/distribution) — Shopify Dev
- [Proxy-Wasm Spec](https://github.com/proxy-wasm/spec) — GitHub
- [Envoy Wasm Architecture](https://www.envoyproxy.io/docs/envoy/latest/intro/arch_overview/advanced/wasm) — Envoy Docs
- [Wasm Modules and Envoy Extensibility](https://tetrate.io/blog/wasm-modules-and-envoy-extensibility-explained-part-1) — Tetrate Blog
- [What is Proxy-Wasm](https://konghq.com/blog/engineering/proxy-wasm) — Kong Blog
- [Fastly Compute](https://docs.fastly.com/products/compute) — Fastly Docs
- [Lucet and Wasmtime Merger](https://www.fastly.com/blog/how-lucet-wasmtime-make-stronger-compiler-together) — Fastly Blog
- [Wasmtime PoolingAllocationConfig](https://docs.wasmtime.dev/api/wasmtime/struct.PoolingAllocationConfig.html) — Wasmtime Docs
- [memfd/madvise CoW pooling allocator PR](https://github.com/bytecodealliance/wasmtime/pull/3697) — Wasmtime GitHub
- [Wasmtime Interrupting Execution](https://docs.wasmtime.dev/examples-interrupting-wasm.html) — Wasmtime Docs
- [Wasmtime Fast Instantiation](https://docs.wasmtime.dev/examples-fast-instantiation.html) — Wasmtime Docs
- [Epoch-based Interruption PR](https://github.com/bytecodealliance/wasmtime/pull/3699) — Wasmtime GitHub
- [Web Audio Modules 2 Introduction](https://www.webaudiomodules.com/docs/intro/) — WAM Docs
- [WAM 2 ACM Paper (2022)](https://dl.acm.org/doi/fullHtml/10.1145/3487553.3524225) — ACM
- [AudioWorklet + WASM Design Pattern](https://developer.chrome.com/blog/audio-worklet-design-pattern/) — Chrome for Developers
- [Extism PDK Docs](https://extism.org/docs/concepts/pdk/) — Extism
- [Extism Memory Concepts](https://extism.org/docs/concepts/memory/) — Extism
- [Extism Host Functions](https://extism.org/docs/concepts/host-functions/) — Extism
- [Extism GitHub](https://github.com/extism/extism) — GitHub
- [Why Extism?](https://dylibso.com/blog/why-extism/) — Dylibso Blog
- [Dylibso Series Seed](https://techcrunch.com/2023/03/24/dylibso-raises-6-6m-to-help-developers-take-webassembly-to-production/) — TechCrunch
- [Hyperlight Wasm: Fast, Secure, OS-free (March 2025)](https://opensource.microsoft.com/blog/2025/03/26/hyperlight-wasm-fast-secure-and-os-free/) — Microsoft Open Source Blog
- [Hyperlight 0.0009s Execution Time (Feb 2025)](https://opensource.microsoft.com/blog/2025/02/11/hyperlight-creating-a-0-0009-second-micro-vm-execution-time/) — Microsoft Open Source Blog
- [hyperlight-wasm repo](https://github.com/hyperlight-dev/hyperlight-wasm) — GitHub
- [Lunatic GitHub](https://github.com/lunatic-solutions/lunatic) — GitHub
- [Lunatic Launch HN (2021)](https://news.ycombinator.com/item?id=26367029) — Hacker News
- [Resonite ProtoFlux](https://wiki.resonite.com/ProtoFlux) — Resonite Wiki
- [Resonite Plugins](https://wiki.resonite.com/Plugins) — Resonite Wiki
- [VRChat Udon](https://creators.vrchat.com/worlds/udon/) — VRChat Creation
- [Udon VM and Assembly](https://creators.vrchat.com/worlds/udon/vm-and-assembly/) — VRChat Creation
- [Breaking Out of VRChat Using a Unity Bug](https://khang06.github.io/vrcescape/) — khang06
- [Luau Sandbox](https://luau.org/sandbox/) — Luau
- [Luau SECURITY.md](https://github.com/Roblox/luau/blob/master/SECURITY.md) — GitHub
- [Roblox Plugin Marketplace](https://devforum.roblox.com/t/introducing-plugin-marketplace/400582) — Roblox DevForum
- [WIT Reference](https://component-model.bytecodealliance.org/design/wit.html) — Component Model Docs
- [wit-bindgen](https://github.com/bytecodealliance/wit-bindgen) — GitHub
- [WASI Roadmap](https://wasi.dev/roadmap) — WASI.dev
- [Looking Ahead to WASIp3](https://www.fermyon.com/blog/looking-ahead-to-wasip3) — Fermyon
