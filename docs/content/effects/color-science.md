+++
title = "Color Science for RGB LEDs"
description = "Why LED lighting is different from screens, and how to make colors look great"
weight = 2
template = "page.html"
+++

Designing colors for physical RGB LEDs is fundamentally different from designing for screens. LEDs are point light sources viewed directly; the human eye sees the raw emitted spectrum, not reflected light. Understanding these differences is the gap between effects that look stunning and effects that look like a washed-out mess.

## LEDs Are Not Screens

RGB LEDs create color through **additive mixing**: three discrete diodes (red, green, blue) combine their light output. More channels active means brighter and *less saturated*. All three at full power produces white, not a richer color.

Key constraints unique to emissive light:

- **You cannot make dark colors.** There is no "brown" or "black" in emissive space. These require subtractive mixing or surrounding context.
- **Each channel has wildly different perceived brightness.** The human eye's luminous efficiency peaks at ~555nm (green). Green appears roughly 6x brighter than blue at the same power level.

| Channel | Perceived Brightness (sRGB) |
|---------|----------------------------|
| Red     | 21.3%                      |
| Green   | 71.5%                      |
| Blue    | 7.2%                       |

This asymmetry is the root cause of most LED color design problems.

## The Hue Perception Tiers

Not all hues are created equal on RGB LEDs. They fall into distinct tiers based on how well the hardware can reproduce them:

**Tier 1 — Pure single-channel hues (best reproduction):**
- Red (0deg), Green (120deg), Blue (240deg)
- These use a single LED at full power. Maximum saturation, zero channel blending.

**Tier 2 — Two-channel hues (excellent):**
- Cyan (180deg), Magenta (300deg)
- Two channels at balanced power. Still very vivid.

**Tier 3 — Unequal blends (good, with caveats):**
- Orange (~30deg), Yellow (~60deg), Spring Green (~150deg), Azure (~210deg), Violet (~270deg), Rose (~330deg)
- These require unequal channel ratios. The dominant channel tends to overpower, making the color read as "tinted bright" rather than a distinct hue.

{% callout(type="tip", title="The yellow problem") %}
Yellow (R=255, G=255, B=0) is the most deceptive hue on LEDs. On screen it looks rich. On hardware, red and green at full power produces an extremely bright, slightly greenish wash that barely reads as yellow. Reduce green to ~60-70% of red for a warmer, more recognizable yellow.
{% end %}

## Saturation: LEDs Reward Boldness

Unlike screens where medium saturation produces pleasant pastels, **physical LEDs look their best at high saturation** (75-100%). Low-saturation colors that look fine on a monitor appear washed-out and indistinct on LED hardware.

| Saturation Range | Result on LEDs |
|:---|:---|
| **90-100%** | Maximum vividness. Primary and secondary hues pop. Can be harsh at high brightness. |
| **70-90%** | Rich and vivid without being aggressive. The sweet spot for most effects. |
| **40-70%** | Noticeably softer. Can read as "washed out" on RGB-only hardware. |
| **Below 40%** | Effectively dim white with a color tint. Not useful for color effects. |

**Recommendation:** Default to HSV saturation of 85-100% for vivid effects. Drop to 70-85% only when building multi-color palettes where you need colors to coexist without competing.

## Brightness and the White Blowout Trap

The most common LED color mistake: pushing lightness too high causes colors to **blow out to white**. High lightness in additive color means "add more of all channels," which converges on white.

In HSL terms:
- **L=50%** is peak color vividness (the pure hue)
- **L=60-70%** starts washing out
- **L=75%+** is pastel territory — mostly white with a hint of color
- **L=90%+** is effectively white on LEDs

In HSV terms:
- **V=100%, S=100%** is peak vividness
- Reducing S while keeping V high adds white (washout)
- Reducing V while keeping S high makes colors darker/richer without washing out

{% callout(type="warning", title="The HSL trap") %}
HSL is intuitive for design tools but dangerous for LED work. An HSL lightness of 70% looks fine on screen but produces a washed-out mess on hardware. Prefer HSV or OKLCH for LED color calculations.
{% end %}

## Gamma Correction

LED hardware does not have the same gamma curve as monitors. Most addressable LEDs have a roughly linear brightness response to PWM duty cycle, while human brightness perception is roughly logarithmic. Without gamma correction, the bottom 50% of brightness values appear nearly identical (dim), and the top 50% change too rapidly.

Apply gamma correction in your effects:

```typescript
// Apply gamma 2.2 correction for perceptually uniform brightness
function gammaCorrect(value: number, gamma = 2.2): number {
    return Math.pow(value / 255, gamma) * 255
}
```

Or use OKLCH, which has perceptually uniform lightness built in — changes in the L component produce visually proportional brightness changes on hardware.

## Gradient Transitions

When transitioning between colors, the path through color space matters:

- **RGB interpolation** — The shortest math path. Produces muddy intermediate colors (e.g., red-to-cyan passes through gray).
- **HSL/HSV interpolation** — Better hue transitions but uneven perceived brightness (green appears brighter than blue at the same S/L).
- **OKLCH interpolation** — Perceptually uniform. Transitions appear smooth and natural. The best choice for LED gradients.

```typescript
// Interpolate in OKLCH for perceptually smooth transitions
// (using the coloraide library or equivalent)
function lerpOklch(c1: Oklch, c2: Oklch, t: number): Oklch {
    return {
        L: c1.L + (c2.L - c1.L) * t,
        C: c1.C + (c2.C - c1.C) * t,
        h: lerpHue(c1.h, c2.h, t),  // Handle hue wrapping
    }
}
```

## Practical Tips

1. **Test on hardware, not just screen.** What looks good in the preview may look different on your actual LEDs. The preview is a guide, not ground truth.

2. **Use fewer colors at higher saturation** rather than many colors at medium saturation. Three vivid colors beat seven muddy ones.

3. **Keep brightness moderate for ambient effects.** Full brightness (V=100%) is great for reactive accents but fatiguing for background lighting. V=60-80% is the sweet spot for ambient.

4. **Embrace the darkness.** LEDs off (black) is a valid and powerful design element. Negative space makes lit areas pop.

5. **Avoid pure white backgrounds.** White (R=G=B=255) draws maximum power, produces maximum heat, and washes out any colored accents. Use dark backgrounds with colored highlights.
