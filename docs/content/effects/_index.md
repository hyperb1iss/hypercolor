+++
title = "Effects overview"
description = "Build effects by reading. The authoring paths — TypeScript canvas, GLSL, native Rust, display faces — and when to pick each."
sort_by = "weight"
weight = 0
template = "section.html"
+++

Effects are web pages. Your desk is the canvas.

Every Hypercolor effect renders into a single RGBA canvas, and the daemon samples that canvas at each LED's physical position before pushing colors to hardware. The default render surface is 640x480 at up to 60 FPS, retuned live across five adaptive tiers. Both dimensions and the target FPS flow from `daemon.canvas_width`, `daemon.canvas_height`, and the active render tier, so treat them as live values, never constants.

![The Hypercolor effects browser](/img/ui/effects.webp)

## Resolution independence is the whole game

The same effect lights a 60-LED strip, a 600-LED matrix, and a 24-LED ring without edits. That works because spatial coordinates are normalized to `[0.0, 1.0]` and the canvas is decoupled from the LED count. Two rules keep an effect portable:

- Read `ctx.canvas.width` and `ctx.canvas.height` every frame. The daemon resizes the canvas live, and a hardcoded size breaks the moment someone retunes it.
- Animate off elapsed time, not frame counts. FPS shifts adaptively, so `time` and delta seconds stay correct where `frameNumber` drifts.

Design for broad strokes. The maximum detail a strip can resolve is roughly `1 / (2 × LED_spacing)`, so thin lines, small text, and fine fractal filigree alias to mush on real hardware. The [color science](@/effects/color-science.md) page covers why LEDs are not screens and how to make colors actually sing.

## Pick an authoring path 🎯

Four paths, one canvas contract. The first two are the SDK's TypeScript surface, the third compiles into the daemon as Rust, and the fourth targets LCD display surfaces. There is a fifth escape hatch below for one-file ports.

{% callout(type="info") %}
The TypeScript SDK (`@hypercolor/sdk`) is pre-release and not published to npm yet. Workspaces depend on a local checkout through a `file:` spec until it ships, so the spec instructions in [setup](@/effects/setup.md) stay local-checkout-first until the package goes public.
{% end %}

### TypeScript canvas effects

The primary path, and where most new work starts. Write a `canvas()` draw function, declare your controls inline, and ship. The SDK hands you palette sampling, audio analysis, typed controls, and math and motion helpers, then bundles each effect into one self-contained HTML artifact. Choose this for particle systems, procedural animation, state-driven motion, or anything that feels like a demoscene party in code.

Start at [TypeScript effects](@/effects/typescript-effects.md). Reach for [controls](@/effects/controls.md), [palettes](@/effects/palettes.md), and [audio](@/effects/audio.md) as you go.

### GLSL shader effects

A fragment shader runs per canvas pixel. Controls become uniforms automatically (`speed` becomes `iSpeed`), audio bands arrive as uniforms, and the SDK wraps the whole thing in a WebGL2 pipeline through `effect()`. Choose this for noise fields, domain-warped fractals, kaleidoscopes, and anything where describing "what every pixel is" beats "what to draw next."

These run as WebGL2 inside Servo, shipped as a self-contained HTML artifact. They are not a native GPU path.

{% callout(type="warning") %}
There is no runnable wgpu or SPIR-V shader lane today. `EffectSource::Shader` bails with a "not runnable yet" error, and requesting GPU effect-renderer acceleration falls back to CPU (`RenderAccelerationMode::Gpu` is rejected outright). Treat a native GPU compute path as future work. Every shader effect you ship runs as WebGL2 in Servo.
{% end %}

Read [GLSL effects](@/effects/glsl-effects.md) for the uniform contract and LED-specific shader patterns.

### Native Rust effects

A compiled-in `EffectRenderer` written in Rust, living in `crates/hypercolor-core/src/effect/builtin/`. This is the CPU canvas path behind `EffectSource::Native`: the effect's source-file stem is the lookup key, matched in `create_builtin_renderer`, and you register it by editing `builtin/mod.rs`. Choose this for the always-available baseline effects that ship inside the daemon, where you want the render loop's full `FrameInput` (timing, audio, interaction, screen, sensors) without a Servo session.

This is the only documentation for the native Rust authoring path. Read [native Rust effects](@/effects/native-rust-effects.md) for the trait lifecycle, the Canvas API, control dispatch, and the `builtin/mod.rs` registration step.

### Display faces

Full-fidelity HTML faces for LCD surfaces such as AIO pump screens and the Ableton Push 2 strip. Author with `face()` in the SDK: a setup function runs once and returns a per-frame update closure with access to sensors, audio, now-playing media, network, and scene lighting. Faces render through Servo, lay out with flexbox (CSS grid is not reliable under Servo), and adapt to the device shape the daemon injects through the display descriptor.

Choose this when you're filling a real screen, not a strip. Read [display faces](@/effects/display-faces.md) for the contract, the canonical displays a face must satisfy, and the Servo CSS matrix.

### Raw HTML escape hatch

Below all four: a standalone LightScript-compatible HTML file with one canvas, one script tag, and a few meta tags. No SDK, no build step, no TypeScript. Hypercolor reads the wire format directly. It's the least-encouraged path for new work because you give up palettes, typed audio data, and authoring ergonomics, but it's right for porting, one-file oddities, and effects that must travel without a workspace. See [raw HTML](@/effects/raw-html.md).

## How a frame becomes light

Every path produces the same thing: a canvas the spatial sampler reads. The renderer is polymorphic, so wgpu, Servo, and native Rust all satisfy one trait and slot into the same loop.

{% mermaid() %}
graph TD
  A[FrameInput: timing, audio, interaction, screen, sensors] --> B[EffectRenderer.render_into]
  B --> C[RGBA Canvas]
  C --> D[SpatialEngine samples canvas at each LED position]
  D --> E[ZoneColors written to devices]
{% end %}

`FrameInput` carries `time_secs`, `delta_secs`, `frame_number`, the audio snapshot, interaction and screen data, sensors, and the target canvas dimensions. Control values arrive separately through `set_control`, so a slider change applies on the next frame without restarting the effect.

## Before you write anything

Even the most ambitious effect starts with a workspace and a real build loop.

- [Setup](@/effects/setup.md): install Bun, scaffold a workspace, wire the local `file:` SDK spec, and pick a template (`canvas`, `shader`, `face`, or `html`).
- [Creating effects](@/effects/creating-effects.md): scaffold to first effect to build, with the canvas and shader shapes side by side.
- [Dev workflow](@/effects/dev-workflow.md): the build, validate, install, and verify-in-the-running-daemon loop, plus the authoring CLI versus the system CLI.

## Reference

Once you've picked a path, pair it with the deeper pages.

- [Controls](@/effects/controls.md): sliders, dropdowns, colors, toggles, fonts, sensors, viewports. Shorthand inference, explicit factories, presets.
- [Palettes](@/effects/palettes.md): the registry, Oklab-interpolated sampling, the shorthand-only `palette` gotcha, and the shader index path.
- [Audio](@/effects/audio.md): the full audio surface, which field matters for which mood, and the shader uniform subset.
- [Color science](@/effects/color-science.md): why LED lighting differs from screens, and how to make colors look great.
- [SDK API reference](@/effects/sdk-api-reference.md): every export from `@hypercolor/sdk`, grouped by module.
- [SDK CLI reference](@/effects/sdk-cli-reference.md): the `bunx hypercolor` authoring CLI, its flags, and environment variables.
- [Performance](@/effects/performance.md) and [troubleshooting](@/effects/troubleshooting.md): keeping inside the frame budget and the build-time failure modes.
- [AI prompt template](@/effects/ai-prompt-template.md): a drop-in prompt for asking a model to write an effect that fits the SDK and the hardware.

## Browse what already ships

Hypercolor ships a stack of native built-in effects compiled into the daemon (the `builtin/` set: `solid_color`, `gradient`, `rainbow`, `breathing`, `audio_pulse`, `color_wave`, `color_zones`, `screen_cast`, `media_player`, `calibration`, `web_viewport`, and friends) plus a large library of SDK HTML effects. Rather than memorize a count that moves every release, open the [catalog](@/effects/catalog.md) for the gallery, or hit `GET /api/v1/effects` on a running daemon to list exactly what's loaded.

{% api_endpoint(method="GET", path="/api/v1/effects") %}
List every effect the daemon knows about — native built-ins and installed HTML effects alike — wrapped in the standard `{ data, meta }` envelope. This is the source of truth for what you can apply right now. See the [REST reference](@/api/rest.md) for the full effects domain.
{% end %}
