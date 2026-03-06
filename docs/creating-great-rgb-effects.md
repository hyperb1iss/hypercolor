# Creating Great RGB Effects

A practical guide to designing high-quality lighting effects for RGB LED hardware — distilled from analyzing 210 community SignalRGB effects, LED color science research, and the Hypercolor engine's rendering pipeline.

**Audience:** Effect authors building for Hypercolor or SignalRGB-compatible engines.

---

## The Golden Rule

**LEDs are not pixels.** Everything in this guide flows from one fact: RGB LEDs are discrete point light sources separated by physical space. There is no sub-pixel blending, no backlight diffusion, no gamma-corrected display pipeline doing work for you. Every design choice must account for this.

---

## 1. Color: What Looks Good on LEDs

### The Hue Tier List

Not all colors render equally on RGB LED hardware. Each LED contains three dies (red, green, blue) that mix additively.

| Tier | Colors | Hue Range | Why |
|------|--------|-----------|-----|
| **Tier 1** | Red, Green, Blue, Cyan, Magenta | 0, 120, 240, 180, 300 | Single die or clean two-die mix. Always vivid. |
| **Tier 2** | Orange, Purple, Rose, Azure, Spring Green, Amber | 20-35, 270, 330, 210, 150 | Two-die mixes that look excellent with tuning. |
| **Tier 3** | Yellow, Warm White, Pastels | 60, varies | Require all channels high — tend to wash out or read as green-tinted. |
| **Tier 4** | Brown, Gray | n/a | Physically impossible in isolation. Brown needs darker-than-surroundings context. |

**The safe vivid range is 180-330** (cyan through magenta). This blue-anchored region produces deep, saturated colors reliably.

**The danger zone is 30-90** (orange through yellow-green). Mixing red + green at similar intensities produces colors that appear washed-out, greenish, or disproportionately bright.

### Saturation: Go Hard or Go Home

Analysis of 210 community effects reveals a **binary saturation strategy**:
- **58.5%** of static HSL calls use S=100%
- **31.2%** use S=0% (pure white/gray)
- Only **5.9%** fall between 10-90%

This isn't laziness — it's practical wisdom. LEDs reward high saturation. At medium saturation (40-70%), RGB LEDs without a dedicated white channel produce muddy, indistinct results. The community learned this empirically.

**Guidelines:**
- **85-100% saturation** for vivid accent/primary colors
- **70-85%** when building multi-color palettes where colors need to coexist
- **Below 60%** — avoid unless you specifically want a washed-out look
- **0%** for intentional white/gray accents

### Lightness/Brightness: The Blowout Threshold

In HSL, L=50% is peak vividness — the pure hue with no white contamination. Above 60%, colors rapidly wash out to white on LEDs.

**The whiteness ratio test:** For any RGB color, compute `min(R,G,B) / max(R,G,B)`. Keep this below **0.3** for vivid LED colors.

| Whiteness Ratio | Result |
|-----------------|--------|
| 0.00 | Fully saturated — one channel is off |
| 0.25 | Slight wash, still reads as colored |
| 0.50 | Significant desaturation |
| 0.75+ | Effectively white |

**Practical rule:** Never run all three channels above 200/255 simultaneously unless you want white. For any vivid color, at least one channel should be at or near 0.

### Fixing Yellow and Warm Colors

Yellow (255, 255, 0) is the most common color complaint on LEDs. It draws double power, appears excessively bright, and often reads as greenish-white.

| Instead of... | Use... | Why |
|---------------|--------|-----|
| Yellow (255, 255, 0) | Gold (255, 190, 0) | Pulling green below red produces a warmer, more convincing tone |
| Pure yellow (60) | Amber (255, 140, 0) | Eye-friendly, reads as warm without the green contamination |
| Brown | Dim orange surrounded by brighter colors | Brown is a contextual perception, not an emissive color |

### Color Scheme Design

From stage lighting and the best community effects:

| Colors | Aesthetic | Examples |
|--------|-----------|---------|
| **1** | Elegant, professional | Monochromatic breathing, single-hue with brightness variation |
| **2** | High impact, clear hierarchy | Complementary: blue+orange, cyan+red, purple+gold |
| **3** | Vibrant but cohesive | Analogous: blue+purple+magenta; triadic: red+green+blue |
| **4-5** | Needs careful balance | Only for structured gradients or defined palettes |
| **6+** | Festive/party | Rainbow effects — fun but rarely "clean" |

**The 80/20 rule:** One color dominates, the other accents. The most admired setups use 1-2 coordinated colors.

---

## 2. Patterns That Work on LEDs

### Empirically Proven (from 210 community effects)

These patterns survive the low-resolution sampling from canvas to LED grid:

| Pattern | Why It Works | Community Prevalence |
|---------|-------------|---------------------|
| **Sine plasma** | Low spatial frequency, smooth gradients, cheap math | Very high (~40+ effects) |
| **Expanding rings/ripples** | Bold concentric gradients, naturally low-freq | High (~30 effects) |
| **Particle systems** | Discrete bright points + trails against dark | Very high (~50 effects) |
| **Noise fields** (simplex/Perlin) | Organic, smooth, no hard edges | High (~25 effects) |
| **Voronoi cells** | Large colored regions with visible boundaries | Medium (~15 effects) |
| **Metaballs** | Smooth implicit surfaces, natural glow-like merging | Medium (~10 effects) |
| **Linear/radial gradients** | Simplest possible, always clean | Very high (ubiquitous) |
| **Wave propagation** | Directional sweeps, color bands | High (~35 effects) |
| **Cellular automata** (Game of Life, etc.) | High-contrast on/off states | Medium (~8 effects) |

### Patterns That Fail on LEDs

| Pattern | Why It Fails |
|---------|-------------|
| **Bloom/glow post-processing** | No sub-pixel blending between physically separated LEDs — blur kernels have nothing to work with |
| **Ray marching / complex 3D** | Detail below LED resolution is wasted; expensive for no visible benefit |
| **Film grain / dithering** | Single-pixel noise is invisible at LED density |
| **Fine fractals** (Mandelbrot at high zoom) | Detail gets aliased to mush |
| **Thin lines / sharp geometry** | Below Nyquist limit — lines alias or vanish between LEDs |
| **Text rendering** | Illegible at keyboard-scale LED density |

### The Nyquist Rule

The maximum detail that survives sampling to an LED grid is limited by `1 / (2 * LED_spacing)`. On a keyboard with ~18mm key pitch, the finest visible feature spans about 2 keys. **Design with broad strokes, not fine detail.**

---

## 3. Animation Techniques

### The Trail/Fade Technique (The Community Standard)

The single most common animation technique across all 210 effects:

```javascript
// Each frame: overlay a semi-transparent black rectangle
ctx.fillStyle = 'rgba(0, 0, 0, 0.15)';
ctx.fillRect(0, 0, canvas.width, canvas.height);
// Then draw new elements on top
```

This creates motion trails without maintaining per-particle history. The alpha value controls trail length:
- **0.05-0.10** — long, smooth trails (comets, flowing effects)
- **0.10-0.20** — medium trails (most effects use this range)
- **0.20-0.40** — short, snappy trails (reactive effects)
- **1.0** — no trail (full clear each frame)

### Timing and Speed

From professional stage lighting and community best practices:

| Animation Type | Speed | Why |
|---------------|-------|-----|
| **Ambient/mood** | 1-3 second transitions | Feels intentional and calm |
| **Breathing/pulse** | 2-4 second full cycle | Matches natural respiration rhythm |
| **Wave sweep** | 0.5-2 seconds across full width | Visible motion without franticness |
| **Reactive (keypress, audio)** | 50-100ms onset, 300-500ms decay | Snappy trigger, graceful fade |
| **Color transition** | 200ms minimum | Below 200ms reads as flicker, not transition |

### Easing

**Sinusoidal easing** for organic effects (breathing, pulsing). Linear ramps look mechanical. The standard breathing formula:

```
brightness = (sin(time * speed) + 1) / 2
```

For reactive effects (key press, beat detection), use **fast attack / slow decay**: instant jump to peak, then exponential or sinusoidal fade back.

### Delta-Time Animation

Always base animation on elapsed time, not frame count:

```javascript
const now = performance.now();
const dt = (now - lastTime) / 1000; // seconds
lastTime = now;
position += velocity * dt;
```

This ensures consistent speed regardless of frame rate — critical since `requestAnimationFrame` is throttled by the engine's render loop.

---

## 4. The Rendering Model

### Canvas 2D at 320x200

Every community effect uses the same rendering approach:
- **Canvas 2D context** (not WebGL)
- **320x200 resolution** — the universal standard
- **`requestAnimationFrame`** for the render loop
- The engine samples canvas pixels at LED positions

This resolution is far higher than any LED grid, giving effects plenty of headroom for smooth gradients that survive downsampling.

### Why Canvas 2D Wins Over WebGL

Despite WebGL being "faster," community effects universally chose Canvas 2D because:
1. **Simpler mental model** — draw calls map directly to visual intent
2. **No shader compilation** — effects load instantly
3. **Adequate performance** — 320x200 is trivial for any GPU; the bottleneck is USB transfer to LED hardware, not rendering
4. **Better portability** — no driver/extension compatibility issues

### Compositing with `globalCompositeOperation`

Effects use compositing modes to create visual complexity:

| Mode | Effect | Use Case |
|------|--------|----------|
| `source-over` | Normal layering (default) | Everything |
| `lighter` | Additive blending | Overlapping light sources, glow simulation, energy effects |
| `screen` | Soft additive (never exceeds white) | More controlled than `lighter` |
| `multiply` | Darken overlaps | Shadow effects, masking |

**`lighter` (additive blending)** is the most important for LED effects — it naturally simulates how light from multiple sources combines.

---

## 5. Color Spaces: What to Use and When

### The Hierarchy

| Task | Best Space | Why |
|------|-----------|-----|
| **Hue cycling / rainbow** | HSV | Increment H, done. Clean and fast. |
| **Two-color gradient** | Oklab | No muddy/gray midpoints. Perceptually uniform. |
| **Multi-stop gradient** | Oklab | Predictable interpolation, no hue detours. |
| **Palette generation** | OKLCH | Hold L and C constant, rotate H — equal perceptual weight. |
| **Brightness dimming** | HSV | V maps directly to LED brightness (PWM duty cycle). |
| **Fire / heat** | HSV | S and V map to physical heat intuition. |
| **All internal math** | Linear RGB or Oklab | Correct blending. Never blend in sRGB. |

### Why HSL Dominates Community Effects (and Why You Should Avoid It)

72.4% of community effects use HSL — but this is a legacy artifact of web development, not a deliberate choice. HSL has two problems for LED work:

1. **Yellow at L=50% appears ~6x brighter than blue at L=50%.** Equal lightness values produce wildly different perceived brightness.
2. **Saturation means different things in HSL vs HSV.** In HSV, S=100% V=100% is the vivid hue. In HSL, S=100% L=50% is the same hue, but L>50% adds white. The mental model is confusing.

**Recommendation:** Use HSV for simple effects (hue cycling, breathing). Use Oklab for gradients and transitions. Use OKLCH for palette design.

### The RGB Gradient Trap

Never interpolate between distant colors in RGB. Classic failures:
- **Red to Blue in RGB:** passes through dim purple. Brightness dips at midpoint.
- **Yellow to Blue in RGB:** passes through literal gray.

Always interpolate in Oklab for perceptually smooth transitions, or use HSV hue rotation for rainbow sweeps.

---

## 6. Gamma Correction

### Why It's Mandatory

LEDs respond linearly to PWM. The human eye does not. Without gamma correction:
- Fades jump to "mostly bright" immediately, then crawl
- Dark values are indistinguishable
- Midtones appear washed out
- Color transitions look uneven

**This is the single highest-impact quality improvement for any LED effect.**

### The Math

```
corrected = 255 * (input / 255) ^ gamma
```

**Gamma 2.2** is the standard starting point. Use 2.8 for high-brightness environments, 1.8 for dim rooms.

### Key Values (Gamma 2.2)

| Input | Output | Perception |
|-------|--------|-----------|
| 0 | 0 | Off |
| 32 | 2 | Barely visible |
| 64 | 10 | Very dim |
| 128 | 55 | Perceptual midpoint |
| 192 | 137 | Moderately bright |
| 255 | 255 | Full brightness |

**Perceptual 50% brightness = PWM 55/255 (21.6%), not 128/255 (50%).** This is why uncorrected fades look wrong.

### Where to Apply It

Gamma correction is the **last step** in the pipeline, after all color math, blending, and interpolation. All internal math should happen in linear space. Gamma is output encoding only.

---

## 7. Composition and Visual Design

### Darkness as a Design Element

The best community effects use darkness strategically:
- **30-50% of LEDs off or very dim** often looks better than everything lit
- Off LEDs provide contrast that makes lit areas more impactful
- Trails that fade to **full black** (not dim) create cleaner motion
- Alternating lit and dark zones create visual rhythm

### The Hot Spot Technique

A single bright LED surrounded by dimmer ones creates a focal point. Use bright/hot spots intentionally:
- Particle cores at full brightness, fields at 30-60%
- Reactive keypresses as momentary hot spots with radial decay
- Audio-reactive peaks as traveling bright points

### Spatial Frequency Matters

Match your pattern's wavelength to hardware density:
- Color waves should span **10-20+ LEDs** for smooth appearance
- Single-LED color alternation creates moire/shimmer effects (intentional use only)
- Gradient bands narrower than 3-4 LEDs appear as hard steps

### Viewing Distance

Adjacent LEDs with different colors blend at distance (pointillism effect). Red-blue alternation up close becomes purple at 2 meters. Design for the intended viewing distance.

---

## 8. Audio Reactivity

### What the Engine Provides

The SignalRGB/Hypercolor engine exposes audio analysis:
- **Beat detection** — boolean pulse on bass hits
- **Frequency bands** — typically bass, mid, treble energy levels
- **Overall level** — RMS amplitude
- **Audio density** — how "full" the audio spectrum is

### Reactive Design Principles

| Principle | Implementation |
|-----------|---------------|
| **Fast onset, slow decay** | Jump to peak on beat, exponential fade over 300-500ms |
| **Map bass to brightness** | Low frequencies drive overall intensity |
| **Map treble to detail** | High frequencies modulate fine pattern elements |
| **Smooth the input** | Raw audio data is noisy — apply EMA smoothing (alpha 0.1-0.3) |
| **Threshold, don't scale** | A beat should trigger a clear visual event, not a proportional nudge |

---

## 9. Property System (User Controls)

Effects expose configurable properties via HTML meta tags:

```html
<meta property="speed" label="Speed" type="number" min="1" max="10" default="5">
<meta property="color" label="Color" type="color" default="#ff0000">
<meta property="mode" label="Mode" type="combobox" values="wave,pulse,chase" default="wave">
```

### Best Practices

- **Always provide sensible defaults** — the effect should look good with zero configuration
- **Speed, color count, and intensity** are the most commonly exposed controls
- Use `on[PropertyName]Changed()` callbacks for immediate response
- Range-limit numeric properties to prevent broken states
- Expose 3-5 meaningful controls, not 20 micro-adjustments

---

## 10. Common Community Palettes

15 named palettes appear repeatedly across ~25 effects, all defined in HSL:

| Palette | Character | Dominant Hues |
|---------|-----------|---------------|
| **Outrun** | Synthwave neon | Magenta, cyan, purple |
| **Vaporwave** | Retro pastel neon | Pink, cyan, purple, peach |
| **Beach** | Warm tropical | Teal, sand, coral |
| **Space** | Deep cosmic | Deep blue, purple, teal |
| **Retro** | 80s arcade | Red, orange, yellow, blue |
| **Rainbow** | Full spectrum | Full hue rotation |
| **Mondrian** | Primary bold | Red, blue, yellow, black/white |
| **Forest** | Natural greens | Green, emerald, brown, gold |
| **Ocean** | Cool depths | Navy, teal, cyan, white |
| **Neon** | Electric | Hot pink, electric blue, lime |
| **Sunset** | Warm gradient | Red, orange, gold, purple |
| **Arctic** | Cool minimal | Ice blue, white, pale cyan |
| **Volcano** | Hot intense | Red, orange, black |
| **Cyberpunk** | High-contrast neon | Magenta, yellow-green, cyan |
| **Pastel** | Soft gentle | Soft pink, lavender, mint |

**Observation:** The most popular palettes (Outrun, Vaporwave, Space, Cyberpunk) lean heavily into the 180-330 safe vivid range. This confirms the empirical finding that blue-anchored palettes produce the best LED results.

---

## 11. Hypercolor Engine Specifics

### Color Pipeline

Hypercolor's internal pipeline:

```
Effect Canvas (sRGB u8) → Spatial Sampling → polish_sampled_color() → fade_to_black → USB output (u8 RGB)
```

**Color types available:**
- `Rgba` / `Rgb` — u8 sRGB (input/output)
- `RgbaF32` — linear f32 (internal math)
- `Oklab` — perceptually uniform (gradients, blending)
- `Oklch` — polar perceptual (palette generation, chroma boost)

**Correct conversions:** The engine implements proper IEC 61966-2-1 sRGB transfer functions and Ottosson's reference Oklab/Oklch math.

### polish_sampled_color()

Applied to Matrix topology zones (keyboards, strips), this function:
1. Converts sRGB → linear → Oklch
2. Boosts chroma (saturation) and adjusts lightness
3. Converts back to sRGB

This compensates for the inherent dullness of sampled canvas colors on physical LEDs.

### Known Pipeline Issues

1. **Bilinear sampling blends in sRGB space** — `Rgba::to_f32()` divides by 255 without linearizing. Should decode sRGB → linear before blending, re-encode after.
2. **`fade_to_black` operates in sRGB space** — attenuation should happen in linear space for perceptually uniform dimming.
3. **`TemporalSmoother` EMA in sRGB space** — temporal blending should also operate in linear space.

These are minor issues that affect mid-tone accuracy. The overall pipeline is solid.

---

## 12. Quick Reference: Effect Author Checklist

### Before You Start
- [ ] Pick 1-3 colors from Tier 1/Tier 2 hues
- [ ] Keep saturation at 85-100% for primaries
- [ ] Keep HSL lightness at 40-55% (or HSV V at 80-100%, S at 85-100%)
- [ ] Choose a pattern with low spatial frequency (noise, waves, gradients, particles)

### While Building
- [ ] Use `rgba(0,0,0,alpha)` overlay for motion trails (alpha 0.05-0.30)
- [ ] Base animation on delta-time, not frame count
- [ ] Use `lighter` composite mode for overlapping light sources
- [ ] Interpolate gradients in Oklab, not RGB
- [ ] Use sinusoidal easing for organic motion
- [ ] Design darkness into the composition — not every LED needs to be lit

### Testing
- [ ] Does it look good with zero user configuration?
- [ ] Check the whiteness ratio: is `min(R,G,B)/max(R,G,B) < 0.3` for vivid areas?
- [ ] Do any colors fall in the 30-90 hue danger zone? Test on hardware.
- [ ] Are transitions slower than 200ms? (Below that reads as flicker)
- [ ] Does the effect work on both small (keyboard) and large (strip) layouts?

### Don't
- Don't use bloom/glow post-processing
- Don't render detail finer than ~3 LED widths
- Don't interpolate colors in RGB space across large hue distances
- Don't use HSL lightness above 60%
- Don't run all three channels above 200/255 unless you want white
- Don't use more than 3-4 simultaneous hues without a clear structural reason

---

## Sources

This guide synthesizes findings from:
- **210 community SignalRGB effects** (empirical analysis of `effects/community/*.html`)
- **5 builtin reference effects** (API surface and canonical patterns)
- **Hypercolor engine source** (`crates/hypercolor-types/src/canvas.rs`, `crates/hypercolor-core/src/spatial/sampler.rs`)
- Color science research documented in:
  - [docs/research/rgb-led-effect-design.md](research/rgb-led-effect-design.md) — theory, math patterns, rendering pipeline
  - [docs/color-science-led-guide.md](../docs/color-science-led-guide.md) — practical LED color science, hue tiers, gamma
- External sources: LearnOpenGL, NVIDIA GPU Gems, Hackaday, Adafruit, Bjorn Ottosson (Oklab), FastLED, Shadertoy, professional stage lighting references (full citations in the research documents above)
