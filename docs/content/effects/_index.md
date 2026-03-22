+++
title = "Effects"
description = "Understanding and creating visual effects for RGB lighting"
sort_by = "weight"
template = "section.html"
+++

Effects are the visual core of Hypercolor, and they're wild. Each effect renders to a 320x200 pixel canvas at up to 60fps. The spatial engine samples that canvas at each LED's physical position, translating pixels into colors that get pushed to real hardware. With 30+ built-in effects and a TypeScript SDK that makes authoring new ones a breeze, the creative possibilities are basically limitless.

Hypercolor supports two rendering paths:

- **HTML/Canvas/WebGL effects** built with TypeScript using the `@hypercolor/sdk`, compiled to single-file HTML. This is the primary authoring path. Effects can use Canvas 2D for particle systems and procedural animation, or WebGL fragment shaders for GPU-accelerated noise, fractals, and mathematical visualizations.

- **Native effects** rendered server-side via wgpu for maximum performance. These bypass the browser engine entirely and run as WGSL compute/render pipelines.

Both paths produce the same output: an RGBA pixel buffer that feeds into the spatial sampler. Effects can consume real-time audio data (FFT bins, beat detection, mel bands, spectral analysis) to create audio-reactive lighting.

In this section:

- **[Creating Effects](@/effects/creating-effects.md)** — Write your first effect with the TypeScript SDK
- **[Color Science](@/effects/color-science.md)** — Why RGB LEDs are different from screens, and how to make colors look great
- **[SDK Reference](@/effects/sdk.md)** — Full API documentation for `@hypercolor/sdk`
