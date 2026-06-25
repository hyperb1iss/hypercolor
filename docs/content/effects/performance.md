+++
title = "Effect performance"
description = "Keep your effect inside the render budget: adaptive FPS tiers, canvas tuning, Servo memory, and time-based motion that survives every tier."
weight = 160
template = "page.html"
+++

Write your effect so it looks right at 10 FPS and at 60 FPS, and the engine handles the rest. Hypercolor renders on a dedicated thread with an adaptive frame-rate controller that drops tiers fast under load and climbs back slowly when there is headroom. Your job is to stay inside the frame budget and animate off elapsed time, not frame counts. This page is the developer-side performance reference for effect authors: how the budget works, what eats it, and the knobs that actually move the needle.

If you are chasing low FPS as a *user* rather than tuning an effect you are writing, read [Low FPS and stuttering](@/troubleshooting/performance.md) instead. For the full render-loop architecture, see [Render pipeline](@/architecture/render-pipeline.md).

![Effect gallery in the web UI](/img/ui/effects.webp)

## The frame budget is a moving target

The daemon never renders at a fixed frame rate. `FpsController` (`crates/hypercolor-core/src/engine/fps.rs`) runs five tiers, and each tier sets the wall-clock budget for one full frame: input sampling, your effect's `render_into`, spatial sampling, and the device write all have to finish inside it.

| Tier | FPS | Frame budget |
|---|---|---|
| `Minimal` | 10 | 100 ms |
| `Low` | 20 | 50 ms |
| `Medium` | 30 | ~33.3 ms |
| `High` | 45 | ~22.2 ms |
| `Full` | 60 | ~16.6 ms |

That budget covers the *whole* frame, not just your effect. A TypeScript canvas or GLSL effect renders inside a Servo session and lands its surface on the compositor; the SparkleFlinger compositor latches the newest surface per producer and blends one canonical RGBA canvas. The spatial sampler then maps that canvas to LED positions and the backend manager queues the device writes. Your effect shares the budget with all of it, so "I have 16 ms at 60 FPS" is the ceiling, not the allowance.

{% callout(type="info") %}
The controller is a pure timing state machine. It owns no threads and does no I/O. The render loop drives it with `begin_frame` and `end_frame` each iteration, and it reports back through `FrameStats` (frame time, headroom, whether the budget was exceeded, the EWMA-smoothed frame time, and the consecutive-miss count).
{% end %}

## How tiers shift

The asymmetry is the whole design: **downshift is aggressive, upshift is patient.** This keeps a momentary spike from stranding you at a low tier, while preventing oscillation when you are riding the edge of a budget.

Downshift fires after **2 consecutive budget misses** (`downshift_miss_threshold`). One slow frame is tolerated; two in a row drops you a tier immediately.

Upshift requires **5 seconds of sustained headroom** (`upshift_sustain_secs`), and only while the EWMA-smoothed frame time stays under **70% of the current tier's budget** (`upshift_headroom_ratio`). The smoothing uses a slow EWMA (`ewma_alpha = 0.05`), so history dominates and one fast frame never triggers a climb. Any budget miss resets the upshift eligibility clock to zero.

{% mermaid() %}
graph LR
  M[Minimal 10] -->|sustained headroom| L[Low 20]
  L -->|sustained headroom| Me[Medium 30]
  Me -->|sustained headroom| H[High 45]
  H -->|sustained headroom| F[Full 60]
  F -->|2 misses| H
  H -->|2 misses| Me
  Me -->|2 misses| L
  L -->|2 misses| M
{% end %}

The practical consequence for an author: a single heavy effect that overruns the budget twice will pull the *entire rig* down a tier, not just itself. Profile against the budget of the tier you expect to run at, with headroom to spare, so you are not the reason everything downshifts.

{% callout(type="warning") %}
Performance baselines are product contracts. The tier ceiling, canvas resolution, and FPS caps are intended performance, not suggestions. Fix a slow effect by making the per-frame work cheaper, never by lowering a baseline or capping the tier to make telemetry look calm. If an effect genuinely cannot hold a tier, that is a profiling problem to solve, not a ceiling to drop.
{% end %}

## Animate off time, never frame counts

`FrameInput` (`crates/hypercolor-core/src/effect/traits.rs`) hands you `time_secs`, `delta_secs`, and `frame_number`. Because the tier changes underneath you, `frame_number` advances at a rate that depends on system load. An effect that moves a wavefront "two pixels per frame" travels at half speed the instant the rig downshifts from 60 to 30 FPS.

Drive every animation from `delta_secs` (the wall-clock time since the previous frame) or `time_secs` (elapsed seconds since activation). Both stay correct across all five tiers.

```typescript
// Wrong — speed is tied to the tier.
position += 2;

// Right — speed is the same at 10, 30, or 60 FPS.
position += pixelsPerSecond * ctx.delta;
```

In a GLSL effect the same rule applies to the `iTime` uniform, and in a native Rust renderer you read `input.time_secs` / `input.delta_secs`. `frame_number` is fine for "do this once on frame 0" or deterministic seeding, never for motion.

## Tuning the canvas

The render canvas defaults to **640x480** (`DEFAULT_CANVAS_WIDTH` / `DEFAULT_CANVAS_HEIGHT` in `crates/hypercolor-types/src/canvas.rs`) and flows from `daemon.canvas_width` / `daemon.canvas_height`. Per-pixel work scales with the area, so the canvas size is the single biggest lever on render cost.

The key insight is that LEDs do not resolve fine canvas detail. The spatial sampler maps each LED to a normalized `[0.0, 1.0]` position and samples the canvas there, so a thin one-pixel line or a tight fractal aliases to mush on hardware no matter how crisp it is on the canvas. Design broad strokes (see [Color science for LEDs](@/effects/color-science.md)), and a smaller canvas often loses nothing visible while buying back real budget.

Practical canvas guidance:

- **Read the size every frame.** Both TypeScript and native effects must treat `ctx.canvas.width`/`height` (or `input.canvas_width`/`canvas_height`) as live values. The daemon can resize the canvas at a frame boundary; an effect that caches the dimensions at init renders garbage after a resize.
- **Cost scales with area, not width.** Halving both dimensions quarters the per-pixel work. If your effect is per-pixel heavy (large blur kernels, multi-octave noise, many overdraws), the canvas size matters more than any micro-optimization inside the loop.
- **Avoid per-frame allocation.** Allocating buffers, gradients, or typed arrays every frame thrashes the allocator inside the budget. Build them once and reuse. In a native Rust renderer this is the difference between a stateless draw fn and a stateful one that owns its scratch buffers.

{% callout(type="tip") %}
The canvas never auto-clears in the TypeScript SDK. `clearCanvas()` is intentionally a no-op, so the draw function owns clearing. Use an opaque `fillRect` for clean frames and a semi-transparent `fillRect` for trails. Trail-by-fade (a per-frame `rgba(0,0,0,alpha)` overlay) is cheaper than tracking and redrawing every historical position, and it reads better on LEDs.
{% end %}

## Servo memory and the HTML path

TypeScript canvas, GLSL, and display-face effects all render through Servo. Servo runs in-process and carries real memory weight (the daemon's resident set includes the in-process Servo runtime), so a few habits keep an HTML effect honest:

- **The artifact is bundled into one file.** The build inlines the JS bundle (IIFE), shader text, palette tables, and metadata into one HTML file. Display-face font controls can load selected Google Fonts at runtime unless capture mode disables remote fonts, so keep font choices intentional and dependencies lean.
- **Lean on built-in helpers instead of heavy libraries.** The SDK ships math, motion, layout, and palette helpers (see the [SDK API reference](@/effects/sdk-api-reference.md)). Reaching for a large external animation or color library to do what a built-in already does pays for it in bundle size and per-frame cost.
- **GLSL moves per-pixel work to the GPU, but it is still WebGL2 in Servo**, not a native pipeline. A fragment shader is the right tool for dense per-pixel math; it is not free, and an expensive shader still has to land its surface inside the frame budget. See [GLSL shader effects](@/effects/glsl-effects.md).

{% callout(type="warning") %}
There is no runnable native GPU effect lane today. The renderer factory has no working shader path: `EffectSource::Shader` returns `shader effect '...' is not runnable yet`, requesting `gpu` acceleration errors outright, and `auto` falls back to CPU with `gpu effect renderer acceleration is not available yet`. Every effect runs either as a Servo HTML surface or as a compiled-in native Rust renderer. The wgpu lane is future work; do not design an effect that assumes it. See [Renderer internals](@/architecture/renderer-internals.md).
{% end %}

## Audio without the strobe tax

Audio is sampled once per frame and handed to the effect; reading it is cheap. The performance trap is visual, not computational: mapping a binary beat flag straight to brightness produces a harsh strobe that also wastes the smooth motion the adaptive tiers are protecting.

- Use the decaying pulse fields (`beatPulse` in TypeScript, `beat_pulse` in native Rust) rather than the raw beat flag, and redirect beat energy into *motion* rather than brightness.
- Gate reactivity by `beatConfidence` so non-rhythmic audio does not flail.
- The full surface and the Rust-vs-TypeScript field-name split live in [Audio API](@/effects/audio.md). Shaders receive a strict subset of the audio uniforms; pitch-class data (chromagram, mel bands) is canvas-only.

## Native Rust effects

Compiled-in native effects (`crates/hypercolor-core/src/effect/builtin/`) skip Servo entirely and produce a `Canvas` directly in Rust, which makes them the cheapest path per frame. The performance contract is the same budget, with a few Rust-specific notes:

- **Implement `render_into`, not `tick`.** `render_into(&mut self, input, target: &mut Canvas)` writes into caller-owned storage and avoids a per-frame allocation. `tick` is a legacy convenience wrapper that allocates a fresh canvas each call; reach for it only outside the hot loop.
- **Hold scratch state on the renderer.** The renderer is `&mut self`, so per-effect buffers, lookup tables, and precomputed gradients belong as fields you build once in `init` and reuse every frame.
- **Stay in the right color space.** The `Canvas` is sRGB `u8`. Color controls arrive as linear RGBA in `[0.0, 1.0]` and must be converted to sRGB before you write pixels. Doing color math in the wrong space is a correctness bug, not a perf one, but the conversion belongs outside the inner loop where you can.

The trait surface, lifecycle, and registration are covered in [Native Rust effects](@/effects/native-rust-effects.md).

## Reading the numbers

The daemon publishes timing on the event bus and exposes it over the API. The diagnose tooling surfaces the active tier, the EWMA-smoothed frame time, headroom, and the consecutive-miss count straight from `FrameStats`, which is the fastest way to confirm whether your effect is the one forcing a downshift.

{% api_endpoint(method="GET", path="/api/v1/system") %}
Returns daemon status including the active render tier and frame timing. Watch the tier while your effect runs: if applying it pulls the tier down and it stays down, your effect is overrunning the budget. Compare against a known-cheap built-in like `solid_color` to isolate the cost.
{% end %}

The WebSocket metrics channel streams the same timing live, which is what the web UI dashboard reads. For the full REST surface and the metrics envelope, see the [REST API reference](@/api/rest.md); for the binary frame and metrics channels, see the [WebSocket protocol](@/api/websocket.md).

## Checklist

Before you ship an effect, confirm it:

- Animates entirely off `delta_secs` / `time_secs` (or `iTime`), never `frame_number`.
- Reads the canvas size every frame and survives a resize.
- Allocates buffers, gradients, and lookup tables once, not per frame.
- Holds the tier you expect with headroom to spare, verified against the diagnose metrics.
- Uses decaying beat pulses and confidence gating for audio reactivity.
- Draws broad strokes that read on LEDs rather than fine detail the sampler discards.

Tune the per-frame work, keep the baseline, and let the adaptive controller do the rest.
