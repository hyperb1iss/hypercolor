# LED Color Science Reference

Detailed LED color science covering saturation, hue quality, gamma correction, blowout prevention, and gradient transitions. Consult SKILL.md for quick rules.

---

## Additive Color Physics

RGB LEDs contain three dies. Colors are created through additive mixing — more channels active = brighter and less saturated. Mixing all three at full power produces white.

**Channel luminance weights (sRGB/WCAG):**
- Red: 21.3%
- Green: 71.5%
- Blue: 7.2%

Green appears ~6x brighter than blue at the same PWM. This asymmetry is the root cause of most LED color problems.

**Power draw:** Pure primary = ~20mA. Two-channel (yellow) = ~40mA. White = ~60mA. On a 60-LED strip, full white draws 3.6A. Plan for 1/3 of calculated maximum in practice.

---

## Saturation Deep Dive

| Range (HSV/HSL) | Result on LEDs | Use Case |
|-----------------|----------------|----------|
| 90-100% | Maximum vividness. Can look harsh in dark rooms at high brightness. | Accents, single-color washes, reactive effects |
| 70-90% | Rich without being aggressive. The sweet spot for multi-color palettes. | Gradients, ambient effects |
| 40-70% | Noticeably softer. Reads as "washed out" on RGB-only hardware. | Pastels (better with RGBW) |
| 10-40% | Dim white/gray with slight color tint on RGB LEDs. | Subtle mood lighting only |

**Community empirical data (210 effects):** 58.5% of HSL calls at S=100%, 31.2% at S=0%, only 5.9% between. Binary saturation is the learned best practice.

---

## Brightness and Blowout

### HSL Lightness Map

| L Value | Result |
|---------|--------|
| 40-50% | Peak vividness |
| 50-60% | Still vivid, slightly lighter |
| 60-70% | Washing out — color diluted |
| 75%+ | Pastel territory — mostly white |
| 90%+ | Effectively white |

### HSV Value Map

| V Value | Result |
|---------|--------|
| 100% (S=100%) | Maximum vividness |
| 80% | Rich and deep — often best for ambient |
| 60% | Moody, dark but saturated |
| 40% | Dim but colored — useful for breathing lows |

### How to Keep Colors Vivid

1. Reduce brightness via HSV V (preserves saturation) rather than increasing HSL L (destroys it)
2. Never run all three channels above 200/255 unless white is intended
3. For vivid colors, at least one channel should be at or near 0
4. Keep `min(R,G,B) / max(R,G,B)` below 0.3

---

## Hue Quality by Region

```
  0- 30  RED -> ORANGE      Warm, intense, low eye strain
 30- 60  ORANGE -> YELLOW   Tricky zone. Yellow is problematic.
 60-120  YELLOW -> GREEN    120 pure green is excellent
120-180  GREEN -> CYAN      Beautiful gradient region
180-240  CYAN -> BLUE       Cool and striking
240-300  BLUE -> MAGENTA    Deep and dramatic
300-360  MAGENTA -> RED     Vivid and electric
```

**Safest vivid range: 180-330** — blue-anchored, uses channels that produce deep, saturated output.

**Most challenging: 30-90** — relies on R+G mixing, produces greenish, bright, or washed colors.

### Recommended Warm Colors (Tuned)

| Name | RGB | HSV | Notes |
|------|-----|-----|-------|
| Warm Red | 255, 30, 0 | 7, 100%, 100% | Deep warm red |
| Orange | 255, 100, 0 | 24, 100%, 100% | Classic vivid |
| Amber | 255, 140, 0 | 33, 100%, 100% | Eye-friendly |
| Gold | 255, 190, 0 | 45, 100%, 100% | Richer than yellow |
| Tuned Yellow | 255, 200, 10 | 47, 96%, 100% | Much better than 255,255,0 |

---

## The Yellow/Brown Problem

**Yellow (255, 255, 0):**
- Double power draw (~40mA)
- No true yellow wavelength — brain interprets separate R+G as yellow
- Often reads as greenish-white on hardware
- Fix: Shift to amber/gold (255, 140-190, 0). Never use equal R and G.

**Brown:**
- Perceptually "dark orange" but LEDs cannot make dark colors in isolation
- RGB(128, 64, 0) looks like dim orange, not brown
- Only works when surrounding LEDs are significantly brighter (relative context)

---

## LED vs Screen Perception

| Property | Screen | Physical LED |
|----------|--------|-------------|
| Viewing | Reflected/filtered through glass | Direct point-source emission |
| Context | Surrounded by other lit pixels | Often dark environment |
| Gamma | Display applies 2.2 curve | No built-in correction; PWM is linear |
| Diffusion | Sub-pixel blending behind diffuser | Point sources separated by physical gaps |
| Saturation | Medium saturation looks fine | Medium saturation looks washed out |

**Critical:** Colors designed on a monitor will not look the same on LEDs. Always test on hardware.

---

## Gradient Transitions

### The RGB Interpolation Trap

- Red (255,0,0) -> Blue (0,0,255) in RGB: passes through dim purple. Brightness dip at midpoint.
- Yellow (255,255,0) -> Blue (0,0,255) in RGB: passes through literal gray.

### Quality Ranking for LED Gradients

1. **Oklab** — Best. Smooth, no muddy midpoints, consistent brightness.
2. **OKLCH** — Same quality but allows hue-angle control. Watch for "long way around" hue circle.
3. **HSV hue rotation** — Good for rainbow sweeps. Brightness varies across hues.
4. **CIE LAB/LCH** — Good but hue shift issues in blue region (270-330).
5. **HSL** — Lightness peaks at yellow causing brightness shifts.
6. **RGB linear** — Only for transitions between very similar colors (< 30 hue difference).

### Smooth Transition Pipeline

```
1. Convert start/end colors to Oklab
2. Linearly interpolate L, a, b components
3. Convert result to linear RGB
4. Apply gamma correction
5. Send to LED hardware
```

Performance: Optimized Oklab interpolation (LMS shortcut) adds only 1.3-1.4x overhead vs RGB — negligible at LED refresh rates.

---

## Gamma Correction

### Why Mandatory

LEDs respond linearly to PWM. Human eyes perceive brightness non-linearly (~power curve). Without gamma:
- Fades jump to bright immediately, then crawl
- Dark values are indistinguishable
- Midtones appear washed out

**This is the single highest-impact quality improvement.**

### The Math

```
corrected = 255 * (input / 255) ^ gamma
```

| Gamma | Use Case |
|-------|----------|
| 1.8 | Mild correction, dim rooms |
| 2.2 | Standard — good general-purpose |
| 2.8 | Aggressive, high-brightness environments |

### Key LUT Values (Gamma 2.2)

| Input | Output | Perception |
|-------|--------|-----------|
| 0 | 0 | Off |
| 32 | 2 | Barely visible |
| 64 | 10 | Very dim |
| 128 | 55 | Perceptual midpoint |
| 192 | 137 | Moderately bright |
| 255 | 255 | Full brightness |

**Perceptual 50% = PWM 21.6%, not 50%.**

### Per-Channel Tuning

Different LED dies have different brightness curves:
- Red: gamma ~2.0-2.2
- Green: gamma ~2.2-2.4 (perceived brighter, may need more correction)
- Blue: gamma ~2.2-2.6 (perceived dimmer)

Single gamma of 2.2 for all channels is a solid default.

### Pipeline Position

Gamma correction is the **last step** — after all color math, blending, interpolation. All internal operations in linear space. Gamma is output encoding only.

---

## Color Scheme Design

### Palette Size Rules

| Colors | Aesthetic | Best For |
|--------|-----------|---------|
| 1 | Elegant, professional | Ambient, workstation |
| 2 | High impact, clear hierarchy | Most effects (80/20 rule) |
| 3 | Vibrant but cohesive | Maximum for "tasteful" |
| 4-5 | Needs careful balance | Structured gradients only |
| 6+ | Festive/party | Rainbow effects |

### Quick Palette Picks

**Complementary (high drama):**
- Blue (240) + Orange (25)
- Cyan (180) + Red (0)
- Purple (270) + Gold (45)

**Analogous (harmony):**
- Blue (240) + Purple (270) + Magenta (300)
- Cyan (180) + Green (120) + Spring Green (150)
- Red (0) + Orange (25) + Amber (35)

### Professional Design Principles (from stage lighting)

1. Start monochromatic, add contrast only when needed
2. Complementary pairs: 80/20 split — one dominates
3. Analogous colors (30-60 hue apart) for calm cohesion
4. Slow transitions (1-3s) beat fast ones — below 200ms reads as flicker
5. Sinusoidal easing for organic motion
6. Darkness is a design element — off LEDs provide contrast
7. Match wave wavelength to hardware density (10-20+ LEDs minimum)

### Community Palettes (15 recurring)

The most popular palettes lean into 180-330 (cyan through magenta):

- **Outrun:** Magenta, cyan, purple
- **Vaporwave:** Pink, cyan, purple, peach
- **Space:** Deep blue, purple, teal
- **Cyberpunk:** Magenta, yellow-green, cyan
- **Neon:** Hot pink, electric blue, lime
- **Ocean:** Navy, teal, cyan, white
- **Sunset:** Red, orange, gold, purple
- **Arctic:** Ice blue, white, pale cyan
- **Volcano:** Red, orange, black
- **Forest:** Green, emerald, brown, gold
- **Beach:** Teal, sand, coral
- **Retro:** Red, orange, yellow, blue
- **Rainbow:** Full hue rotation
- **Mondrian:** Red, blue, yellow, black/white
- **Pastel:** Soft pink, lavender, mint
