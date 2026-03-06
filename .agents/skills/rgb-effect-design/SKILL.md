---
name: RGB Effect Design
version: 1.0.0
description: >-
  This skill should be used when creating, modifying, or debugging RGB lighting
  effects for Hypercolor or SignalRGB-compatible engines. Triggers on "create an
  effect", "write a lighting effect", "design LED colors", "fix washed out
  colors", "port a shader to LEDs", "why does this look bad on LEDs",
  "color palette for RGB", "effect looks white", "colors too bright",
  "effect flickering", "design a palette for keyboard", "LED animation", or any
  work involving HTML canvas effects, LED color science, gamma correction, or
  the Hypercolor SDK effect pipeline.
---

# RGB Effect Design for LED Hardware

Practical guidance for creating high-quality lighting effects on physical RGB LEDs — keyboards, strips, fans, and other addressable hardware. Derived from analysis of 210+ community effects and LED color science research.

## Core Principle

LEDs are discrete point light sources separated by physical space. There is no sub-pixel blending, no backlight diffusion. Design with bold strokes, high saturation, and low spatial frequency.

## Color Rules

### Saturation: High or Nothing

LED hardware rewards binary saturation. Use **85-100%** for vivid colors, **0%** for intentional white. Avoid the 20-70% range — it produces muddy, indistinct results on RGB LEDs without a dedicated white channel.

### Blowout Prevention

Keep the **whiteness ratio** below 0.3:

```
whiteness_ratio = min(R, G, B) / max(R, G, B)
```

- Never run all three channels above 200/255 unless white is intended
- HSL lightness above 60% washes out to white on LEDs
- For vivid colors, at least one RGB channel should be near 0

### Hue Quality

| Tier | Hues | Notes |
|------|------|-------|
| **1 (best)** | Red(0), Green(120), Blue(240), Cyan(180), Magenta(300) | Single die or clean mix |
| **2** | Orange(25), Purple(270), Rose(330), Azure(210), Amber(35) | Excellent with tuning |
| **3** | Yellow(60), Warm White, Pastels | Tend to wash out |
| **4** | Brown, Gray | Impossible in isolation |

**Safe vivid range: 180-330** (cyan through magenta). **Danger zone: 30-90** (orange through yellow-green).

### Fixing Yellow

Never use pure yellow (255,255,0) — shift to gold (255,190,0) or amber (255,140,0) by pulling green below red.

## Color Spaces

| Task | Use | Why |
|------|-----|-----|
| Hue cycling / rainbow | HSV | Increment H; fast and clean |
| Gradients between colors | Oklab | No muddy midpoints |
| Palette generation | OKLCH | Equal perceptual weight across hues |
| Brightness control | HSV (V channel) | Maps to LED PWM |
| Internal blending | Linear RGB or Oklab | Never blend in sRGB |

Do not use HSL for LED work. Its lightness model causes yellow to appear 6x brighter than blue at equal L values.

## Patterns That Work on LEDs

**High success:** Sine plasma, expanding rings, particle systems, noise fields (simplex/Perlin), linear/radial gradients, wave sweeps, Voronoi cells, metaballs.

**Fail on LEDs:** Bloom/glow post-processing, ray marching, film grain, fine fractals, thin lines, text rendering. Detail below ~3 LED widths is wasted.

## Animation Techniques

### Trail/Fade (The Universal Technique)

```javascript
ctx.fillStyle = 'rgba(0, 0, 0, 0.15)'; // alpha controls trail length
ctx.fillRect(0, 0, canvas.width, canvas.height);
```

- 0.05-0.10: long trails (comets)
- 0.10-0.20: standard (most effects)
- 0.20-0.40: snappy (reactive effects)

### Timing

- Ambient: 1-3s transitions
- Breathing: 2-4s cycle, sinusoidal easing
- Reactive: 50-100ms onset, 300-500ms decay
- Minimum transition: 200ms (below = flicker)

### Always Use Delta-Time

```javascript
const dt = (performance.now() - lastTime) / 1000;
position += velocity * dt;
```

## Composition

- **Use darkness:** 30-50% of LEDs off or very dim often looks better than everything lit
- **Color count:** 1-2 coordinated colors > rainbow everything. Max 3-4 for tasteful results
- **Hot spots:** Single bright LED surrounded by dim ones creates intentional focal points
- **Spatial frequency:** Waves should span 10-20+ LEDs minimum

## Gamma Correction

Mandatory for quality LED output. Apply as the **last step** after all color math.

```
corrected = 255 * (input / 255) ^ 2.2
```

Perceptual 50% brightness = PWM 55/255 (21.6%), not 128/255.

## Rendering Model

All community effects use Canvas 2D at **320x200** with `requestAnimationFrame`. This resolution provides ample headroom for smooth gradients that survive downsampling to LED positions.

Use `globalCompositeOperation = 'lighter'` for additive blending of overlapping light sources.

## Hypercolor Engine Pipeline

```
Effect Canvas (sRGB u8) -> Spatial Sampling -> polish_sampled_color() -> fade_to_black -> USB output
```

`polish_sampled_color()` boosts chroma in Oklch space to compensate for sampling dullness on physical LEDs. Available color types: `Rgba`/`Rgb` (u8 sRGB), `RgbaF32` (linear f32), `Oklab`, `Oklch`. The engine implements correct sRGB transfer functions and Oklab/Oklch math.

## Quick Checklist

Before starting:
- Pick 1-3 colors from Tier 1/2 hues
- Saturation 85-100%, HSL lightness 40-55%
- Choose a low-spatial-frequency pattern

While building:
- Trail/fade overlay for motion
- Delta-time animation
- Oklab interpolation for gradients
- Sinusoidal easing for organic motion
- Design darkness into the composition

Testing:
- Whiteness ratio < 0.3 for vivid areas?
- Any hues in 30-90 danger zone? Test on hardware
- Transitions > 200ms?
- Works on both small (keyboard) and large (strip) layouts?

## Detailed References

For deeper information, consult:
- **`references/color-science.md`** — Full LED color science: saturation ranges, hue tiers, gamma correction, blowout prevention, yellow/brown problem, gradient transitions, per-channel calibration
- **`references/effect-design.md`** — Complete effect design theory: noise functions, Voronoi, metaballs, temporal patterns, palette design, shader porting, rendering pipeline details
- **Project root: `docs/creating-great-rgb-effects.md`** — Synthesis guide with community palette catalog, audio reactivity, property system, composition rules
