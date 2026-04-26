+++
title = "Color Science for RGB LEDs"
description = "Why LED lighting is different from screens, and how to make colors look great"
weight = 9
template = "page.html"
+++

Designing colors for physical RGB LEDs is not the same as designing for screens. LEDs are point light sources viewed directly; the eye sees the raw emitted spectrum, not reflected light. Getting this wrong is the difference between effects that look stunning and effects that look like a washed-out mess.

This page is the practical guide. Everything here is distilled from LED color science research, analysis of 210 community HTML effects, and the Hypercolor engine's rendering pipeline.

## LEDs are not screens

RGB LEDs create color through **additive mixing**: three discrete dies (red, green, blue) combine their light output. More channels active means brighter and less saturated. All three at full power produces white, not a richer color.

Key constraints unique to emissive light:

- You cannot make dark colors. There is no "brown" or "black" in emissive space. These require subtractive mixing or surrounding context.
- Each channel has wildly different perceived brightness. The human eye's luminous efficiency peaks at ~555nm (green). Green appears roughly 6x brighter than blue at the same power level.

| Channel | Perceived brightness (sRGB) |
| ------- | --------------------------- |
| Red     | 21.3%                       |
| Green   | 71.5%                       |
| Blue    | 7.2%                        |

This asymmetry is the root cause of most LED color problems.

## The hue tier list

Not all hues are created equal on RGB LEDs. They fall into tiers based on how well the hardware can reproduce them.

| Tier  | Hues                                             | Angles                    | Why                                                                    |
| ----- | ------------------------------------------------ | ------------------------- | ---------------------------------------------------------------------- |
| **1** | Red, Green, Blue, Cyan, Magenta                  | 0, 120, 240, 180, 300     | Single die or clean two-die mix. Always vivid.                         |
| **2** | Orange, Purple, Rose, Azure, Spring Green, Amber | 20-35, 270, 330, 210, 150 | Two-die mixes that look excellent with tuning.                         |
| **3** | Yellow, Warm White, Pastels                      | 60, varies                | Require all channels high. Wash out or read green-tinted.              |
| **4** | Brown, Gray                                      | n/a                       | Impossible in isolation. Brown needs darker-than-surroundings context. |

The safe vivid range is **180-330°** (cyan through magenta). This blue-anchored region produces deep, saturated colors reliably.

The danger zone is **30-90°** (orange through yellow-green). Mixing red and green at similar intensities produces colors that read as washed-out, greenish, or disproportionately bright.

{% callout(type="tip", title="The yellow problem") %}
Yellow (R=255, G=255, B=0) is the most deceptive hue on LEDs. On screen it looks rich. On hardware, red and green at full power produces an extremely bright, slightly greenish wash that barely reads as yellow. Reduce green to about 60-70% of red for a warmer, more convincing yellow.
{% end %}

## Saturation: go hard or go home

Analysis of 210 community effects shows a binary saturation strategy:

- 58.5% of static HSL calls use S=100%
- 31.2% use S=0% (pure white or gray)
- Only 5.9% fall between 10-90%

That isn't laziness. It's practical wisdom. LEDs reward high saturation; at medium saturation, RGB LEDs without a dedicated white channel produce muddy, indistinct results. The community learned this empirically.

| Saturation    | Result on LEDs                                                                        |
| ------------- | ------------------------------------------------------------------------------------- |
| **90-100%**   | Maximum vividness. Primary and secondary hues pop. Can feel harsh at high brightness. |
| **70-90%**    | Rich and vivid without being aggressive. The sweet spot for most effects.             |
| **40-70%**    | Noticeably softer. Can read as washed out on RGB-only hardware.                       |
| **Below 40%** | Effectively dim white with a tint. Not useful for color effects.                      |

Default to HSV saturation 85-100% for vivid effects. Drop to 70-85% only when building multi-color palettes where colors need to coexist without competing.

## Brightness and the whiteness trap

The most common LED color mistake is pushing lightness too high and causing colors to blow out to white. High lightness in additive color means "add more of all channels," which converges on white.

In HSL:

- L=50% is peak color vividness (the pure hue)
- L=60-70% starts washing out
- L=75%+ is pastel territory, mostly white with a hint of color
- L=90%+ is effectively white on LEDs

In HSV:

- V=100%, S=100% is peak vividness
- Reducing S while keeping V high adds white (washout)
- Reducing V while keeping S high makes colors darker and richer without washing out

### The whiteness ratio test

For any RGB color, compute `min(R,G,B) / max(R,G,B)`. Keep this below **0.3** for vivid LED colors.

| Whiteness ratio | Result                               |
| --------------- | ------------------------------------ |
| 0.00            | Fully saturated. One channel is off. |
| 0.25            | Slight wash, still reads as colored. |
| 0.50            | Significant desaturation.            |
| 0.75+           | Effectively white.                   |

**Never run all three channels above 200/255 simultaneously unless you want white.** For any vivid color, at least one channel should be at or near 0.

{% callout(type="warning", title="The HSL trap") %}
HSL is intuitive for design tools but dangerous for LED work. An HSL lightness of 70% looks fine on screen but produces a washed-out mess on hardware. Prefer HSV or OKLCH for LED color calculations.
{% end %}

## Gamma correction

LED hardware does not share the gamma curve of monitors. Most addressable LEDs have a roughly linear brightness response to PWM duty cycle, while human brightness perception is roughly logarithmic. Without gamma correction, the bottom 50% of brightness values appears nearly identical (dim) and the top 50% changes too rapidly.

The math:

```
corrected = 255 * (input / 255) ^ gamma
```

**Gamma 2.2** is the standard starting point. Use 2.8 for high-brightness environments, 1.8 for dim rooms.

Key values at gamma 2.2:

| Input | Output | Perception          |
| ----- | ------ | ------------------- |
| 0     | 0      | Off                 |
| 32    | 2      | Barely visible      |
| 64    | 10     | Very dim            |
| 128   | 55     | Perceptual midpoint |
| 192   | 137    | Moderately bright   |
| 255   | 255    | Full brightness     |

Perceptual 50% brightness is PWM 55/255, not 128/255. This is why uncorrected fades look wrong.

TypeScript implementation:

```typescript
function gammaCorrect(value: number, gamma = 2.2): number {
  return Math.pow(value / 255, gamma) * 255;
}
```

Or use OKLCH, which has perceptually uniform lightness built in. Changes in the L component produce visually proportional brightness changes on hardware.

## Color space hierarchy

Pick the right space for the job.

| Task                  | Best space          | Why                                                          |
| --------------------- | ------------------- | ------------------------------------------------------------ |
| Hue cycling / rainbow | HSV                 | Increment H; clean and fast.                                 |
| Two-color gradient    | Oklab               | No muddy midpoints. Perceptually uniform.                    |
| Multi-stop gradient   | Oklab               | Predictable interpolation, no hue detours.                   |
| Palette generation    | OKLCH               | Hold L and C constant, rotate H for equal perceptual weight. |
| Brightness dimming    | HSV                 | V maps directly to PWM duty cycle.                           |
| Fire / heat           | HSV                 | S and V map to physical heat intuition.                      |
| Internal math         | Linear RGB or Oklab | Correct blending. Never blend in sRGB.                       |

**Never interpolate between distant colors in RGB.** Red to blue in RGB passes through dim purple with a brightness dip at midpoint; yellow to blue passes through literal gray. Interpolate in Oklab for perceptually smooth transitions, or use HSV hue rotation for rainbow sweeps.

The built-in palette system uses Oklab interpolation automatically. See [Palettes](@/effects/palettes.md) for the details.

## Patterns that work on LEDs

From 210 community effects, these survive the low-resolution sampling from canvas to LED grid:

| Pattern                       | Why it works                                        | Prevalence |
| ----------------------------- | --------------------------------------------------- | ---------- |
| Sine plasma                   | Low spatial frequency, smooth gradients             | Very high  |
| Expanding rings/ripples       | Bold concentric gradients, naturally low-freq       | High       |
| Particle systems              | Discrete bright points and trails against dark      | Very high  |
| Noise fields (simplex/Perlin) | Organic, smooth, no hard edges                      | High       |
| Voronoi cells                 | Large colored regions with visible boundaries       | Medium     |
| Metaballs                     | Smooth implicit surfaces, natural glow-like merging | Medium     |
| Linear/radial gradients       | Simplest possible, always clean                     | Very high  |
| Wave propagation              | Directional sweeps, color bands                     | High       |

These patterns fail because the detail lives below what LEDs can resolve:

- Bloom/glow post-processing (no sub-pixel blending between separated LEDs)
- Ray marching / complex 3D (detail below LED resolution is wasted)
- Film grain / single-pixel noise (invisible at LED density)
- Fine fractals (alias to mush)
- Thin lines / sharp geometry (below Nyquist, lines alias or vanish)
- Text rendering (illegible at keyboard-scale LED density)

### The Nyquist rule

The maximum detail that survives sampling to an LED grid is limited by `1 / (2 * LED_spacing)`. On a keyboard with ~18mm key pitch, the finest visible feature spans about two keys. **Design with broad strokes, not fine detail.**

## Animation timing

From stage lighting and community practice:

| Animation                  | Speed                           | Why                                           |
| -------------------------- | ------------------------------- | --------------------------------------------- |
| Ambient / mood             | 1-3s transitions                | Feels intentional and calm.                   |
| Breathing / pulse          | 2-4s full cycle                 | Matches natural respiration.                  |
| Wave sweep                 | 0.5-2s across full width        | Visible motion without franticness.           |
| Reactive (keypress, audio) | 50-100ms onset, 300-500ms decay | Snappy trigger, graceful fade.                |
| Color transition           | 200ms minimum                   | Below 200ms reads as flicker, not transition. |

Always base animation on elapsed time (`time` in seconds), not frame count. The render loop retunes FPS adaptively; frame-counted animation speeds up and slows down unpredictably.

## The trail/fade technique

The single most common animation technique across community effects: overlay a semi-transparent dark rectangle each frame before drawing new elements.

```typescript
ctx.fillStyle = "rgba(0, 0, 0, 0.15)";
ctx.fillRect(0, 0, ctx.canvas.width, ctx.canvas.height);
// then draw new foreground
```

The alpha value controls trail length:

| Alpha     | Trail character                        |
| --------- | -------------------------------------- |
| 0.05-0.10 | Long, smooth (comets, flowing effects) |
| 0.10-0.20 | Medium (most effects land here)        |
| 0.20-0.40 | Short, snappy (reactive effects)       |
| 1.0       | No trail (full clear each frame)       |

Pair this with `globalCompositeOperation = 'lighter'` when drawing multiple overlapping lights for natural-looking additive glow.

## Composite modes

| Mode          | Effect                              | When to use                                     |
| ------------- | ----------------------------------- | ----------------------------------------------- |
| `source-over` | Normal layering                     | Default for everything                          |
| `lighter`     | Additive blending                   | Overlapping light sources, glow, energy effects |
| `screen`      | Soft additive (never exceeds white) | Controlled additive                             |
| `multiply`    | Darken overlaps                     | Shadow effects, masking                         |

`'lighter'` is the most important for LED work: it simulates how light from multiple emissive sources actually combines.

## Practical tips

1. **Test on hardware, not just the preview.** The studio is a guide, not ground truth. What looks good in the browser may look different on your LEDs.
2. **Use fewer colors at higher saturation** rather than many colors at medium saturation. Three vivid colors beat seven muddy ones.
3. **Keep brightness moderate for ambient effects.** Full brightness is great for reactive accents but fatiguing for background lighting. V=60-80% is the sweet spot for ambient.
4. **Embrace darkness.** LEDs off is a valid and powerful design element. Negative space makes lit areas pop.
5. **Avoid pure white backgrounds.** White draws maximum power, produces maximum heat, and washes out any colored accents. Dark backgrounds with colored highlights always win.

## Color scheme hierarchy

| Colors  | Character                    | Examples                                                      |
| ------- | ---------------------------- | ------------------------------------------------------------- |
| **1**   | Elegant, professional        | Monochromatic breathing, single hue with brightness variation |
| **2**   | High impact, clear hierarchy | Complementary: blue+orange, cyan+red, purple+gold             |
| **3**   | Vibrant but cohesive         | Analogous: blue+purple+magenta; triadic: red+green+blue       |
| **4-5** | Careful balance needed       | Structured gradients or defined palettes                      |
| **6+**  | Festive / party              | Rainbow effects fun but rarely "clean"                        |

The 80/20 rule: one color dominates, the other accents. The most admired setups use one or two coordinated colors.
