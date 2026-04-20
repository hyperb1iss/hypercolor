+++
title = "Effects"
description = "Author lighting effects for Hypercolor with TypeScript, GLSL, or raw HTML"
sort_by = "weight"
template = "section.html"
+++

Effects are web pages. Your desk is the canvas.

Every Hypercolor effect renders into a canvas, and the daemon samples that canvas at each LED's physical position before pushing colors to hardware. The default render surface is 640x480 at up to 60 FPS, adaptively retuned live. Effects stay resolution-independent by reading `ctx.canvas.width` / `ctx.canvas.height` every frame; spatial coordinates are normalized `[0, 1]`, so the same effect lights a 60-LED strip, a 600-LED matrix, and a 24-LED ring.

## Three authoring paths

Pick one based on what you're making.

**[TypeScript canvas effects](@/effects/typescript-effects.md).** The primary path. Write a draw function, declare controls, ship. The `@hypercolor/sdk` gives you palette sampling, audio analysis, typed controls, and a clean build/install loop. Choose this for particle systems, procedural animation, state-driven motion, or anything that feels like "code I would write in a demoscene party."

**[GLSL shader effects](@/effects/glsl-effects.md).** A fragment shader runs on the GPU for every canvas pixel. Controls turn into uniforms automatically (`speed` becomes `iSpeed`), audio bands become uniforms (`iAudioBass`, `iAudioBeatPulse`), and the SDK wraps it all into a WebGL2 pipeline. Choose this for noise fields, domain-warped fractals, kaleidoscopes, and anything where you'd rather describe "what every pixel is" than "what to draw next."

**[Raw LightScript HTML](@/effects/raw-html.md).** A standalone HTML file with one canvas, one script tag, and a few meta tags. No SDK, no build step, no TypeScript. Hypercolor reads LightScript's wire format directly, so these effects work with zero tooling. This is the least-encouraged path for new work because you give up palettes, typed audio data, and the stronger authoring ergonomics, but it's the right call for porting, for one-file oddities, and for effects that must travel without a workspace.

## Before you write anything

Even the most ambitious effect starts with a workspace and a real build/install loop.

- [Setup](@/effects/setup.md) covers installing Bun, scaffolding a workspace, pointing at `@hypercolor/sdk`, and the standalone-vs-monorepo split.
- [Dev Workflow](@/effects/dev-workflow.md) walks through building, validating, installing, and checking the result in the running daemon.

## Reference

Once you've picked a path, pair it with the reference pages for anything deeper.

- [Controls](@/effects/controls.md). Sliders, dropdowns, colors, toggles, viewports. Shorthand inference and explicit factories. Presets.
- [Audio](@/effects/audio.md). The full `AudioData` surface, which fields matter for what mood, and shader uniform mappings.
- [Palettes](@/effects/palettes.md). The palette registry, Oklab-interpolated sampling, canvas vs shader integration, and the one gotcha about `palette` vs `combo('Palette', ...)`.
- [Color Science for RGB LEDs](@/effects/color-science.md). Why LED lighting is different from screens, and how to make colors look great.
- [AI Prompt Template](@/effects/ai-prompt-template.md). A drop-in prompt for asking Claude, GPT, or another model to write an effect that actually fits the SDK and the hardware.
