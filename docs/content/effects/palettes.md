+++
title = "Palettes"
description = "The palette registry, Oklab interpolation, and the one gotcha worth knowing"
weight = 8
template = "page.html"
+++

Palettes are curated color ramps with an id, name, stops, and a cosine-palette approximation for shader use. The SDK interpolates between stops in Oklab space so gradients are perceptually smooth, builds a 256-entry LUT per palette on first use, and caches the LUT for every subsequent sample.

## Built-in palettes

These are the palettes that ship with every workspace. Each one is curated for LED hardware, which means high saturation, dark floors, and avoidance of the orange-yellow washout zone.

| Palette          | Mood                                        |
| ---------------- | ------------------------------------------- |
| `SilkCircuit`    | Electric, dark, vibrant (the house palette) |
| `Cyberpunk`      | Dark, neon, futuristic                      |
| `Vaporwave`      | Retro, dreamy, pastel-neon                  |
| `Synthwave`      | Retro, warm, 80s                            |
| `Neon Flux`      | Energetic, vibrant                          |
| `Matrix`         | Dark, digital, monochrome green             |
| `Aurora`         | Cool, ethereal, northern                    |
| `Ocean`          | Calm, deep, aquatic                         |
| `Forest`         | Earthy, organic, lush                       |
| `Sunset`         | Warm, romantic, dusk                        |
| `Cherry Blossom` | Gentle, spring, delicate                    |
| `Fire`           | Hot, aggressive                             |
| `Lava`           | Molten, deep orange/red                     |
| `Ember`          | Smoldering, orange/amber                    |
| `Solar`          | Bright, yellow/white heat                   |
| `Ice`            | Cool, crystalline                           |
| `Deep Sea`       | Dark blue, bioluminescent                   |
| `Midnight`       | Dark purple/blue                            |
| `Frost`          | Cool, soft                                  |
| `Viridis`        | Scientific, green-to-purple                 |
| `Inferno`        | Scientific, black-to-orange-to-yellow       |
| `Plasma`         | Scientific, purple-to-pink-to-yellow        |
| `Magma`          | Scientific, black-to-red-to-white           |
| `Candy`          | Pastel, playful                             |
| `Pastel`         | Soft, low-saturation                        |
| `Cotton Candy`   | Pink/blue pastel                            |
| `Mono`           | Grayscale                                   |
| `Phosphor`       | Amber CRT                                   |

Fetch the full list at runtime with `paletteNames()`:

```typescript
import { paletteNames } from "@hypercolor/sdk";
console.log(paletteNames());
```

Inspect a single palette's metadata (mood, stops, accent, background) with `getPalette()`:

```typescript
import { getPalette } from "@hypercolor/sdk";
const p = getPalette("SilkCircuit");
console.log(p?.stops); // ['#e135ff', '#80ffea', '#ff6ac1', '#f1fa8c', '#50fa7b']
console.log(p?.background); // '#0d0221'
```

## Using palettes in canvas effects

Canvas effects get palettes as a function that returns a CSS color string. Two shapes for declaring one:

### Shorthand: `palette: ['A', 'B', 'C']`

Use the key `palette` with a string array and the SDK automatically replaces the resolved value with a `PaletteFn`:

```typescript
export default canvas(
  "Ribbon",
  { palette: ["SilkCircuit", "Aurora", "Synthwave"] },
  (ctx, time, controls) => {
    const pal = controls.palette as (t: number, alpha?: number) => string;
    ctx.fillStyle = pal(0.25);
    ctx.fillStyle = pal(0.7, 0.4); // with alpha
  },
);
```

`pal(t)` takes `t ∈ [0, 1]` and returns `rgb(...)`. `pal(t, alpha)` returns `rgba(...)` with the given alpha.

### Explicit: `combo('Palette', [...])`

When you need a tooltip, a group, or a non-first default, use `combo()`, but you lose the auto-function. The resolved value stays a string.

```typescript
import { canvas, combo, createPaletteFn } from "@hypercolor/sdk";
import type { PaletteFn } from "@hypercolor/sdk";

export default canvas.stateful(
  "Ribbon",
  {
    palette: combo("Palette", ["SilkCircuit", "Aurora", "Synthwave"], {
      group: "Color",
    }),
  },
  () => {
    let name = "";
    let pal: PaletteFn = createPaletteFn("SilkCircuit");
    return (ctx, time, controls) => {
      const next = controls.palette as string;
      if (next !== name) {
        name = next;
        pal = createPaletteFn(name);
      }
      ctx.fillStyle = pal(0.25);
    };
  },
);
```

`createPaletteFn(name)` returns a `PaletteFn` bound to a specific palette. Building it inside the draw function every frame is cheap because the underlying LUT is module-level cached; rebuilding only happens on the first call per palette name.

{% callout(type="warning", title="The one gotcha") %}
The automatic palette function only fires for the **shorthand** declaration with the exact key `palette`. Both of these lose the magic:

- `combo('Palette', ['A', 'B'])` (explicit factory)
- `colors: ['A', 'B']` (different key name)

Use `createPaletteFn` inside the draw when you need one of those.
{% end %}

## Using palettes in shader effects

Shaders don't receive the palette function directly because GLSL can't hold a JavaScript closure. Instead, the `palette` shorthand becomes an integer uniform whose value is the selected index:

```typescript
export default effect("My Shader", shader, {
  palette: ["Aurora", "Fire", "Ocean"],
});
```

```glsl
uniform int iPalette;

vec3 paletteColor(float t, int mode) {
    if (mode == 1) return mix(vec3(0.95, 0.35, 0.18), vec3(1.00, 0.80, 0.25), t);
    if (mode == 2) return mix(vec3(0.05, 0.18, 0.42), vec3(0.25, 0.85, 0.95), t);
    return mix(vec3(0.12, 0.58, 0.95), vec3(0.68, 0.22, 1.00), t);
}

void main() {
    vec3 c = paletteColor(t, iPalette);
    // ...
}
```

Shaders typically use inline cosine palettes instead of the registry. The `cosPal` pattern is cheap, smooth, and fits GLSL idioms:

```glsl
vec3 cosPal(float t, vec3 a, vec3 b, vec3 c, vec3 d) {
    return a + b * cos(6.28318 * (c * t + d));
}
```

Each registered palette has a cosine approximation in its `iq` field that you can bake into the shader if you want to preserve parity between canvas and shader variants of the same effect.

## Oklab interpolation

The palette LUT is built by converting each stop from sRGB to Oklab, linearly interpolating in Oklab, and converting back. The result is a perceptually smooth gradient: no muddy mid-tones when transitioning between hues, no uneven perceived brightness.

The trade-off is cost on first use: converting stops and building a 256-entry LUT takes a few milliseconds. That cost is paid once per palette name and then cached forever. Sampling the LUT is effectively free.

For direct access without a function wrapper:

```typescript
import { samplePalette, samplePaletteCSS } from "@hypercolor/sdk";

const [r, g, b] = samplePalette("SilkCircuit", 0.5); // each in 0..1
const css = samplePaletteCSS("SilkCircuit", 0.5, 0.4); // 'rgba(...)'
```

## Palette design

If you're adding your own palettes (monorepo only for now; custom user palettes aren't supported in standalone workspaces yet), keep these rules in mind:

- **Stops should traverse saturated colors.** A palette that passes through a low-saturation grayish color in the middle will look muddy on LEDs even though Oklab interpolation smooths it.
- **Avoid the yellow-orange washout zone for stops.** 30-90° in hue tends to read as bright-white on LEDs. If you need warmth, reach for `Ember` or `Lava` which route through red and orange without pure yellow.
- **Keep a dark background.** Every palette declares a `background` field; the built-in ones are all very dark. Effects that fill a small area at high saturation with a dark surround outperform effects that fill the whole canvas with medium saturation.
- **Pick four to six stops.** Fewer stops can look abrupt; more tends to introduce unintentional wobble in the interpolated ramp.

## Dark floor pattern

A common idiom is to sample a dark palette position for the background and a bright one for the foreground:

```typescript
export default canvas.stateful(
  "Glow",
  { palette: ["SilkCircuit", "Aurora", "Ember"] },
  () => {
    return (ctx, time, controls) => {
      const pal = controls.palette as (t: number, alpha?: number) => string;
      ctx.fillStyle = pal(0.05); // dark end of the palette
      ctx.fillRect(0, 0, ctx.canvas.width, ctx.canvas.height);
      ctx.fillStyle = pal(0.8, 0.9); // bright end with alpha
      drawForeground(ctx);
    };
  },
);
```

This keeps the whole effect visually coherent: everything lives inside the chosen palette and your effect inherits its mood automatically.
