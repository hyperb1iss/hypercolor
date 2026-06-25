+++
title = "Palettes"
description = "The Hypercolor palette registry: Oklab LUTs, paletteControl, and the one rule that decides whether you get a function or an index."
weight = 50
template = "page.html"
+++

A palette is a curated color ramp with an `id`, `name`, mood tags, hex `stops`, an `iq` cosine approximation for shaders, and `accent` / `background` swatches. The SDK interpolates between stops in Oklab space so gradients stay perceptually smooth, bakes a 256-entry lookup table per palette on first use, and caches that LUT for every sample afterward.

Twenty-eight palettes ship in the registry today, all tuned for LED hardware: high saturation, dark floors, and a wide berth around the orange-yellow washout zone. The registry lives in `sdk/shared/palettes.json` and is the single source of truth. Read it there rather than memorizing a list that will drift.

<!-- TODO: palette swatch grid image once /img/effects/palettes.webp exists -->

## The registry

Every palette is keyed by its `name` (not its `id`). Fetch the full list at runtime instead of hardcoding:

```typescript
import { paletteNames } from "@hypercolor/sdk";

console.log(paletteNames());
// ['SilkCircuit', 'Cyberpunk', 'Vaporwave', 'Synthwave', ...]
```

Inspect a single entry — stops, mood, accent, background — with `getPalette()`:

```typescript
import { getPalette } from "@hypercolor/sdk";

const p = getPalette("SilkCircuit");
console.log(p?.stops);      // ['#e135ff', '#80ffea', '#ff6ac1', '#f1fa8c', '#50fa7b']
console.log(p?.accent);     // '#e135ff'
console.log(p?.background); // '#0d0221'
```

`getPalette()` returns `undefined` for an unknown name, so guard with optional chaining or a fallback.

The `PaletteEntry` shape is exactly:

```typescript
interface PaletteEntry {
  readonly id: string;          // kebab-case, e.g. 'silk-circuit'
  readonly name: string;        // display + lookup key, e.g. 'SilkCircuit'
  readonly mood: readonly string[];
  readonly stops: readonly string[]; // hex colors, the gradient anchors
  readonly iq: { a: number[]; b: number[]; c: number[]; d: number[] };
  readonly accent: string;      // hero hex for UI chrome
  readonly background: string;  // dark floor hex
}
```

A sampling of what ships, to set the tonal range:

| Palette          | Mood                                  |
| ---------------- | ------------------------------------- |
| `SilkCircuit`    | electric, dark, vibrant (house)       |
| `Cyberpunk`      | dark, neon, futuristic                |
| `Vaporwave`      | retro, dreamy, pastel-neon            |
| `Synthwave`      | retro, warm, 80s                      |
| `Aurora`         | cool, ethereal, northern              |
| `Ocean`          | calm, deep, aquatic                   |
| `Fire`           | hot, intense, primal                  |
| `Lava`           | volcanic, extreme                     |
| `Ember`          | warm, cozy, glowing                   |
| `Viridis`        | perceptually-uniform, data            |
| `Inferno`        | perceptually-uniform, heat            |
| `Plasma`         | perceptually-uniform, vivid           |
| `Magma`          | perceptually-uniform, warm            |
| `Phosphor`       | retro, terminal, hacker               |

The scientific quartet (`Viridis`, `Inferno`, `Plasma`, `Magma`) carries the matplotlib stops verbatim, so data-driven effects stay perceptually uniform end to end. The full set of twenty-eight is in `sdk/shared/palettes.json`.

## The one rule that matters 🎯

There is exactly one thing to internalize about palettes, and it decides everything downstream: **the automatic palette behavior fires only for a control built with `paletteControl()`.** Internally that factory stamps `palette: true` on the control spec, and that flag is what the canvas and shader runtimes check (`isPaletteControl`).

A bare string array is inferred as a plain combobox, and `combo()` is a plain combobox too. Neither sets the flag, so neither gets the palette treatment:

```typescript
import { combo, paletteControl } from "@hypercolor/sdk";

// ✅ Palette-aware. Canvas → PaletteFn, shader → index uniform.
palette: paletteControl("Palette", ["SilkCircuit", "Aurora", "Fire"]);

// ❌ Just a dropdown of strings. You get the selected name, no LUT.
palette: ["SilkCircuit", "Aurora", "Fire"];
palette: combo("Palette", ["SilkCircuit", "Aurora", "Fire"]);
```

{% callout(type="warning") %}
The key name `palette` is **not** magic. Naming a control `palette` does nothing on its own. Only `paletteControl()` flips the flag. If your color sampling silently returns plain strings or your shader index never moves, this is almost always the cause: you reached for `combo()` or a raw array when you meant `paletteControl()`.
{% end %}

`paletteControl(label, values, opts?)` accepts the same options as `combo()` — `default`, `tooltip`, `group`, `uniform` — so you keep grouping and a non-first default without losing the behavior.

## Palettes in canvas effects

For a `canvas()` effect, a `paletteControl` resolves to a `PaletteFn`: a function that takes `t ∈ [0, 1]` and an optional alpha, and returns a ready-to-use CSS color string.

```typescript
import { canvas, paletteControl } from "@hypercolor/sdk";
import type { PaletteFn } from "@hypercolor/sdk";

export default canvas(
  "Ribbon",
  {
    palette: paletteControl("Palette", ["SilkCircuit", "Aurora", "Synthwave"]),
  },
  (ctx, time, controls) => {
    const pal = controls.palette as PaletteFn;
    ctx.fillStyle = pal(0.25);       // 'rgb(...)'
    ctx.fillStyle = pal(0.7, 0.4);   // 'rgba(...)' with alpha
  },
);
```

`pal(t)` returns `rgb(...)`; `pal(t, alpha)` returns `rgba(...)` with the given alpha when alpha is below 1. The runtime caches one `PaletteFn` per selected palette name, so switching the dropdown swaps functions without rebuilding the LUT it has already seen.

When you need a function bound to a fixed palette outside the control flow — a hardcoded background ramp, a secondary accent — build one with `createPaletteFn()`:

```typescript
import { createPaletteFn } from "@hypercolor/sdk";
import type { PaletteFn } from "@hypercolor/sdk";

const accent: PaletteFn = createPaletteFn("Ember");
ctx.strokeStyle = accent(0.85);
```

Calling `createPaletteFn` inside the draw loop is cheap: the underlying LUT is module-level cached, so only the very first call per palette name pays the build cost.

For raw numbers instead of CSS strings, sample directly:

```typescript
import { samplePalette, samplePaletteCSS } from "@hypercolor/sdk";

const [r, g, b] = samplePalette("SilkCircuit", 0.5);   // each in 0..1
const css = samplePaletteCSS("SilkCircuit", 0.5, 0.4); // 'rgba(...)'
```

An unknown palette name resolves to magenta (`[1, 0, 1]`) so a typo lights up loudly on the hardware instead of failing silently.

## Palettes in shader effects

GLSL can't hold a JavaScript closure, so a `paletteControl` inside a shader-backed `effect()` resolves differently: it becomes an **integer uniform** carrying the selected index, named with the standard `i`-prefix convention (`palette` → `iPalette`). The selected name maps to its position in the values array.

```typescript
import { effect, paletteControl } from "@hypercolor/sdk";

export default effect("My Shader", shader, {
  palette: paletteControl("Palette", ["Aurora", "Fire", "Ocean"]),
});
```

Switch on the index inside the fragment shader:

```glsl
uniform int iPalette;

vec3 paletteColor(float t, int mode) {
  if (mode == 1) return mix(vec3(0.95, 0.35, 0.18), vec3(1.00, 0.80, 0.25), t); // Fire
  if (mode == 2) return mix(vec3(0.05, 0.18, 0.42), vec3(0.25, 0.85, 0.95), t); // Ocean
  return mix(vec3(0.12, 0.58, 0.95), vec3(0.68, 0.22, 1.00), t);               // Aurora
}

void main() {
  vec3 c = paletteColor(t, iPalette);
  // ...
}
```

In practice most shaders skip the registry entirely and inline an Inigo Quilez cosine palette. It is cheap, smooth, and idiomatic GLSL:

```glsl
vec3 cosPal(float t, vec3 a, vec3 b, vec3 c, vec3 d) {
  return a + b * cos(6.28318 * (c * t + d));
}
```

Every registry entry already carries a cosine approximation in its `iq` field (`a`, `b`, `c`, `d`), so you can bake the matching coefficients into a shader to keep canvas and shader variants of the same effect in visual lockstep:

```typescript
const { a, b, c, d } = getPalette("Plasma")!.iq;
// feed a/b/c/d into your shader's cosPal uniforms
```

{% callout(type="info") %}
The cosine `iq` coefficients are an *approximation* of the stop-based ramp, not a pixel-exact match. The Oklab LUT (canvas path) and the cosine palette (shader path) will differ slightly. Treat `iq` as "close enough to read as the same palette," and tune by eye if the two paths sit side by side.
{% end %}

## How Oklab interpolation works

The LUT build is straightforward and runs once per palette name:

{% mermaid() %}
graph LR
  S[hex stops] --> O[sRGB to Oklab]
  O --> I[linear interp in Oklab]
  I --> B[Oklab to sRGB]
  B --> L[256-entry LUT]
  L --> C[cache by name]
{% end %}

Interpolating in Oklab instead of raw sRGB is what keeps mid-tones clean. A straight RGB lerp between two saturated hues dips through a muddy gray midpoint and wobbles in perceived brightness; Oklab keeps lightness and chroma even across the transition. That matters more on LEDs than on a screen, where a washed-out midpoint reads as a dead spot in the animation.

The cost is a few milliseconds on first use to convert stops and fill 256 entries. After that the LUT is cached forever and every sample is a clamp plus an array index — effectively free. For the deeper LED color reasoning behind these choices, see [Color science for RGB LEDs](@/effects/color-science.md).

## Designing your own palettes

Custom palettes live in `sdk/shared/palettes.json` (monorepo workspaces only for now; standalone-workspace custom palettes aren't wired up yet). A few rules earn their keep on real hardware:

- **Traverse saturated colors.** A ramp that passes through a low-saturation gray midpoint looks muddy on LEDs even with Oklab smoothing. Keep chroma up across the whole stop sequence.
- **Avoid the yellow-orange washout band.** Hues roughly in the 30–90° arc tend to read as bright white on LEDs. For warmth, route through red and orange like `Ember` or `Lava` rather than landing on pure yellow.
- **Keep a dark `background`.** Every entry declares one, and the built-ins are all near-black. Effects that paint a small bright region over a dark surround out-punch effects that flood the whole canvas at medium saturation.
- **Pick four to six stops.** Fewer can look abrupt; more tends to introduce wobble in the interpolated ramp.

A common idiom is the dark-floor pattern: sample a low position for the background and a high one for the foreground, so the entire effect stays inside one palette's mood.

```typescript
import { canvas, paletteControl } from "@hypercolor/sdk";
import type { PaletteFn } from "@hypercolor/sdk";

export default canvas(
  "Glow",
  { palette: paletteControl("Palette", ["SilkCircuit", "Aurora", "Ember"]) },
  (ctx, time, controls) => {
    const pal = controls.palette as PaletteFn;
    ctx.fillStyle = pal(0.05);            // dark end
    ctx.fillRect(0, 0, ctx.canvas.width, ctx.canvas.height);
    ctx.fillStyle = pal(0.8, 0.9);        // bright end, slight alpha
    drawForeground(ctx);
  },
);
```

Everything lives inside the chosen palette, so the effect inherits its mood automatically and stays coherent when the user swaps palettes from the [effects panel](@/studio/_index.md).

## Where palettes fit

Palettes are one piece of the control surface. For the full control vocabulary — shorthand inference, every factory, groups, the `speed` magic, and presets — see the [controls reference](@/effects/controls.md). For authoring canvas and shader effects end to end, see [TypeScript effects](@/effects/typescript-effects.md) and [GLSL effects](@/effects/glsl-effects.md).
