# Effect Design Theory Reference

Detailed reference covering mathematical patterns, rendering pipeline, animation techniques, shader porting, and audio reactivity for RGB LED effects. Consult SKILL.md for quick rules.

---

## Mathematical Patterns

### Noise Functions

#### Simplex Noise (Preferred)

Uses a simplicial grid (triangles in 2D) instead of squares. Benefits for LED effects:

- Fewer directional artifacts than Perlin (no grid-aligned bias)
- O(n^2) complexity vs O(2^n) for classic Perlin
- Better isotropic (direction-independent) noise
- Ideal for organic/flowing effects

For LED grids, **2-3 octaves** of fractal noise is sufficient. More octaves add detail below LED resolution that gets aliased away.

#### Perlin Noise

Good for 1D and 2D but shows grid-aligned artifacts (horizontal/vertical bias) at low resolution. Prefer simplex for 2D effects.

#### Multi-Octave (Fractal) Noise

Layer noise at different frequencies and amplitudes:

```
value = noise(x) * 1.0          // base
      + noise(x*2) * 0.5        // detail
      + noise(x*4) * 0.25       // fine detail
```

Each octave at 2x frequency, 0.5x amplitude. 2-3 octaves for LEDs.

### Voronoi (Worley Noise)

Partition space into cells around random seed points. Each pixel colored by nearest seed distance. Produces:

- Organic cell patterns (like soap bubbles or cracked mud)
- Clean boundaries between color regions
- Works well at low LED density because cells are naturally large

**Variants:** Color by nearest seed, color by second-nearest, color by distance difference (cracks).

### Metaballs

Implicit surface technique where each "blob" has an influence field:

```
field(x,y) = sum(radius_i^2 / distance(x,y, center_i)^2)
```

When `field > threshold`, the pixel is "inside." Creates smooth, organic merging shapes. Naturally glow-like without post-processing.

**LED advantage:** Metaballs are inherently low-frequency. The smooth falloff fields survive downsampling to LED grids.

### Sine Plasma

The simplest successful pattern. Layer sinusoidal functions:

```
value = sin(x * freq1 + time)
      + sin(y * freq2 + time * 0.7)
      + sin((x + y) * freq3 + time * 1.3)
      + sin(sqrt(x*x + y*y) * freq4 + time * 0.5)
```

Map the summed value to a color palette. Produces smooth, flowing, psychedelic patterns that look great on LEDs.

### Expanding Rings / Ripples

Concentric circles emanating from a point:

```
distance = sqrt((x - cx)^2 + (y - cy)^2)
value = sin(distance * frequency - time * speed)
```

Naturally low spatial frequency. Multiple overlapping ring sources create interference patterns.

### Particle Systems

Discrete bright points moving through space with trails. The standard community approach:

1. Array of particles with position, velocity, color, lifetime
2. Each frame: update positions, draw particles, apply trail fade
3. Trail via semi-transparent black overlay (see Animation section)

Particle effects work on LEDs because the bright particles are point sources — exactly what LEDs are.

---

## Patterns That Fail on LEDs

### Bloom / Glow Post-Processing

Traditional bloom convolves bright regions with a Gaussian blur. Fails because:

1. LEDs are too sparse for blur kernels to produce visible softness
2. No optical blending between physically separated LEDs
3. Keycap bezels isolate each LED perceptually

**What works instead:**

- Radial falloff functions: `1.0 / (1.0 + distance^2)` per glow source
- Additive blending of multiple falloff sources
- Brightness boost at source (saturated white core, colored surround)
- Temporal glow (pulse adjacent LEDs with delayed, attenuated color)

### Ray Marching / Complex 3D

Detail below LED resolution is wasted compute. A 100-LED keyboard cannot represent the detail a ray marcher produces.

### Film Grain / Dithering

Single-pixel noise is invisible at LED density.

### Fine Fractals

Mandelbrot at high zoom has detail that aliases to mush when sampled to LED positions.

### Thin Lines / Sharp Geometry

Below the Nyquist limit for LED grids. Lines vanish or alias between LEDs.

---

## The Nyquist Rule

Maximum representable spatial frequency = `1 / (2 * LED_pitch)`. On a keyboard with ~18mm key pitch, the finest visible feature spans ~2 keys. Pre-filter (low-pass) the canvas before sampling to avoid aliasing.

**Practical rule:** Design features that span at least 3-4 LEDs. Below that, patterns break down.

---

## Rendering Pipeline

### Canvas 2D at the daemon's configured resolution

The universal format across all 210 community effects:

- Canvas 2D context (not WebGL)
- Resolution is **whatever the daemon is configured for** — 640x480 by default
- `requestAnimationFrame` for the render loop
- Engine samples canvas pixels at LED positions
- Effects MUST read `ctx.canvas.width` / `ctx.canvas.height` every frame — never hardcode
- Effects ported from the historical 320x200 SDK grid use
  `scaleContext(ctx.canvas, { width: 320, height: 200 })` to translate design coords to live pixels

Why Canvas 2D over WebGL:

1. Simpler mental model — draw calls map to visual intent
2. No shader compilation — instant effect loading
3. Adequate performance — even 640x480 is trivial; USB transfer is the bottleneck
4. Better portability — no driver issues

### Compositing Modes

| Mode          | Effect                              | Use Case                           |
| ------------- | ----------------------------------- | ---------------------------------- |
| `source-over` | Normal layering                     | Default                            |
| `lighter`     | Additive blending                   | Overlapping lights, energy effects |
| `screen`      | Soft additive (never exceeds white) | Controlled glow                    |
| `multiply`    | Darken overlaps                     | Shadows, masking                   |

**`lighter` is the key mode for LED effects** — simulates how real light combines.

---

## Temporal Patterns and Animation

### Trail/Fade Technique

Each frame: overlay semi-transparent black, then draw new elements:

```javascript
ctx.fillStyle = "rgba(0, 0, 0, alpha)";
ctx.fillRect(0, 0, canvas.width, canvas.height);
```

| Alpha     | Trail Length | Use                     |
| --------- | ------------ | ----------------------- |
| 0.02-0.05 | Very long    | Slow comets, aurora     |
| 0.05-0.10 | Long         | Flowing effects         |
| 0.10-0.20 | Medium       | Standard (most effects) |
| 0.20-0.40 | Short        | Reactive, snappy        |
| 0.50-1.0  | None/minimal | Full redraw each frame  |

### Delta-Time Animation

Always base motion on elapsed time:

```javascript
const now = performance.now();
const dt = (now - lastTime) / 1000;
lastTime = now;
position += velocity * dt;
```

Never use frame count — `requestAnimationFrame` rate varies.

### Easing Functions

**Sinusoidal** for organic motion (breathing, pulsing):

```javascript
brightness = (Math.sin(time * speed) + 1) / 2;
```

**Fast attack / slow decay** for reactive effects:

- Instant jump to peak on trigger
- Exponential decay: `value *= 0.95` each frame
- Or `value = peak * Math.exp(-decay * elapsed)`

### Breathing Effect

```javascript
const phase = (Math.sin((time * 2 * Math.PI) / period) + 1) / 2;
const brightness = minBright + phase * (maxBright - minBright);
```

Period of 2-4 seconds. Use HSV V for brightness control.

---

## Audio Reactivity

### Available Engine Data

- **Beat detection:** Boolean pulse on bass hits
- **Frequency bands:** Bass, mid, treble energy levels
- **Overall level:** RMS amplitude
- **Audio density:** How "full" the spectrum is

### Design Principles

1. **Fast onset, slow decay:** Jump to peak on beat, exponential fade over 300-500ms
2. **Map bass to brightness:** Low frequencies drive overall intensity
3. **Map treble to detail:** High frequencies modulate fine pattern elements
4. **Smooth the input:** Apply EMA smoothing (alpha 0.1-0.3) to raw audio data
5. **Threshold, don't scale:** Beats should trigger clear visual events, not proportional nudges

---

## Property System

Effects expose user controls via HTML meta tags:

```html
<meta
  property="speed"
  label="Speed"
  type="number"
  min="1"
  max="10"
  default="5"
/>
<meta property="color" label="Color" type="color" default="#ff0000" />
<meta
  property="mode"
  label="Mode"
  type="combobox"
  values="wave,pulse,chase"
  default="wave"
/>
```

Available types: `number`, `color`, `boolean`, `combobox`, `string`.

Callbacks: `on[PropertyName]Changed()` for immediate response.

**Best practice:** 3-5 meaningful controls with sensible defaults. The effect should look great at zero configuration.

---

## Shader Porting Guide

When adapting Shadertoy/GLSL shaders to LED Canvas 2D effects:

### What Translates Well

- **UV-based math** — normalize pixel coordinates to 0-1, same logic applies
- **Distance fields** — compute distance from shapes, map to color
- **Noise functions** — reimplement in JS (simplex noise libraries available)
- **Color palettes** — `palette(t) = a + b * cos(2*pi * (c*t + d))` (Inigo Quilez technique)
- **Time-based animation** — `iTime` maps to `performance.now() / 1000`

### What Doesn't Translate

- **Per-pixel parallelism** — Canvas 2D is CPU-sequential; use imageData for batch pixel writes
- **Multi-pass rendering** — No framebuffer ping-pong; use multiple canvas layers
- **3D ray marching** — Too much detail for LED density; simplify to 2D projections
- **Post-processing chains** — Bloom, DOF, motion blur — skip these entirely for LEDs

### Porting Checklist

1. Replace `fragCoord/iResolution` with `x/width, y/height`
2. Replace `iTime` with `performance.now() / 1000`
3. Remove any post-processing passes
4. Reduce spatial frequency (increase scale factors)
5. Convert color output to canvas RGB
6. Test at effective LED resolution (not full canvas)

---

## Hypercolor Engine Details

### Color Types

```rust
Rgba    // u8 sRGB — input/output format
Rgb     // u8 sRGB — no alpha
RgbaF32 // linear f32 — internal math
Oklab   // perceptually uniform — gradients, blending
Oklch   // polar perceptual — palette generation, chroma boost
```

### Sampling Pipeline

```
Canvas (sRGB u8) -> zone_local_to_canvas -> sample -> polish_sampled_color -> fade_to_black -> [u8;3]
```

**`polish_sampled_color()`** (Matrix topology zones only):

1. sRGB -> linear -> Oklch
2. Boost chroma and adjust lightness
3. Oklch -> linear -> sRGB

This compensates for the inherent dullness of sampled canvas colors on physical LEDs.

### Known Pipeline Issues

1. Bilinear sampling blends in sRGB space (should linearize first)
2. `fade_to_black` attenuation in sRGB space (should be linear)
3. `TemporalSmoother` EMA in sRGB space (should be linear)
4. `parse_hex_color()` skips sRGB -> linear decode for control colors

These affect mid-tone accuracy but the overall pipeline is solid.
