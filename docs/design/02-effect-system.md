# Hypercolor Effect System Design

> The effect system is the soul of Hypercolor. Everything else -- the daemon, the drivers, the spatial engine -- exists to serve the effects. This document defines how effects are categorized, authored, discovered, composed, shared, and performed.

---

## Table of Contents

1. [Effect Taxonomy](#1-effect-taxonomy)
2. [Effect Authoring Experience](#2-effect-authoring-experience)
3. [Effect Discovery & Browsing](#3-effect-discovery--browsing)
4. [Effect Marketplace / Repository](#4-effect-marketplace--repository)
5. [Effect Composition & Layering](#5-effect-composition--layering)
6. [Effect Parameters & Presets](#6-effect-parameters--presets)
7. [Audio-Reactive Effects Deep Dive](#7-audio-reactive-effects-deep-dive)
8. [Effect Performance & Resource Management](#8-effect-performance--resource-management)
9. [Persona Scenarios](#9-persona-scenarios)
10. [Recommended Built-in Effect Library](#10-recommended-built-in-effect-library)

---

## 1. Effect Taxonomy

Every effect rendered on the 320x200 canvas falls into one or more of these categories. Categories are not mutually exclusive -- an effect can be both **Ambient** and **Audio-Reactive**, or both **Generative** and **Interactive**. The taxonomy serves two purposes: organizing the effect library for users, and guiding authoring decisions for creators.

### 1.1 Ambient

Slow, meditative effects that provide atmosphere without demanding attention. The visual equivalent of background music.

| Effect Pattern | Description | Technique |
|---|---|---|
| **Aurora** | Northern lights curtains drifting vertically | Gradient streaks with Perlin noise displacement, slow hue rotation |
| **Lava Lamp** | Blobby organic forms rising and falling | Metaball algorithm or SVG-filtered circles with buoyancy physics |
| **Ocean** | Deep water caustics and gentle wave motion | Layered sine waves with Voronoi caustic overlay |
| **Nebula** | Cosmic gas clouds with slow drift | Fractal Brownian Motion (fBM) noise, multi-octave layering |
| **Breathing** | Single or multi-color sinusoidal pulse | `sin(time)` brightness modulation, ease-in-out curve |
| **Candle** | Warm flickering light | Low-frequency noise mapped to brightness + slight hue variation |
| **Fireflies** | Sparse glowing particles with organic movement | Particle system with attraction/repulsion, fade-in/fade-out lifecycle |
| **Clouds** | Slow-moving volumetric cloud layers | Multi-layer Perlin noise with parallax scrolling |

**Shared characteristics:** Low CPU/GPU cost. Frame rates can drop to 30fps without visual degradation. Smooth interpolation is critical -- any jitter breaks the illusion of calm. Color temperature should skew warm or cool depending on mood, never harsh.

### 1.2 Reactive (Audio-Driven)

Effects that respond to music, voice, or ambient sound. The category users care about most for gaming and music setups.

| Effect Pattern | Description | Audio Mapping |
|---|---|---|
| **Spectrum Analyzer** | Classic frequency bar visualization | FFT bins -> bar heights, mel-scaled for perceptual accuracy |
| **Beat Pulse** | Full-canvas flash on beat detection | `beat` -> brightness spike with exponential decay |
| **Bass Drop** | Radial shockwave from center on heavy bass | `bass` energy -> ring expansion velocity and opacity |
| **Waveform** | Oscilloscope-style audio waveform | Raw PCM samples -> vertical displacement of horizontal line |
| **Chromagram Palette** | Colors shift based on detected musical key | 12-bin chromagram -> hue mapping (C=red, D=orange, ...) |
| **Mood Gradient** | Warm/cool gradient driven by harmonic analysis | `chordMood` (-1 minor, +1 major) -> blue-to-orange gradient |
| **Frequency Fire** | Flames whose height tracks frequency bands | Bass->flame base, mid->flame body, treble->flame tips |
| **Spectral Flux Strobe** | Flash intensity proportional to spectral change | `spectralFlux` -> brightness, high flux = dramatic visual change |

**Shared characteristics:** Latency-critical. Audio-to-visual delay must stay under 16ms (one frame at 60fps). Smoothing is essential to prevent seizure-inducing flicker -- exponential moving averages with configurable decay. Effects should degrade gracefully to a static ambient state when audio is silent.

### 1.3 Interactive (Input-Driven)

Effects that respond to keyboard, mouse, or other HID input. Primarily for per-key keyboard lighting but applicable to any device.

| Effect Pattern | Description | Input Mapping |
|---|---|---|
| **Typing Ripples** | Color wave radiating from pressed keys | Key position -> ripple origin, ring expansion with fade |
| **Key Trail** | Recently pressed keys glow and fade | Key position -> brightness with time-based decay |
| **WASD Highlight** | Gaming keys illuminate on press | Key state -> zone brightness, configurable key groups |
| **Heatmap** | Frequently used keys glow hotter | Cumulative key press count -> color temperature ramp |
| **Rain** | Color drops fall from pressed key positions | Key position -> particle emitter at top of column |
| **Reactive Zones** | Different keyboard regions have different base effects | Zone map -> independent effect instances per region |

**Shared characteristics:** Requires the keyboard input source (`evdev` on Linux). Effects need a spatial model of the keyboard -- which physical position maps to which key. The spatial layout engine already handles this via `DeviceZone` topology, but interactive effects need *reverse* mapping: from key event back to canvas coordinate.

**Input event flow:**
```
evdev key event -> InputSource::sample() -> InteractiveEffectState
  -> effect reads key states per frame
  -> renders to canvas with key positions as spatial anchors
```

### 1.4 Informational

Effects that visualize system telemetry, notifications, or time. Functional beauty.

| Effect Pattern | Description | Data Source |
|---|---|---|
| **CPU Temperature** | Color gradient from cool blue to hot red | `sysfs` thermal zone -> 30-100C mapped to hue 240-0 |
| **GPU Load** | Bar or radial fill showing GPU utilization | NVIDIA/AMD driver APIs -> 0-100% mapped to fill level |
| **Network Activity** | Sparkle/pulse on upload/download | `/proc/net/dev` delta -> particle burst intensity |
| **Clock** | Time displayed as color patterns | Hour -> hue, minute -> position, second -> animation phase |
| **Notification Pulse** | Brief flash on desktop notification | D-Bus notification signal -> color burst with app-specific hue |
| **Disk I/O** | Activity indicator for storage operations | `iostat` data -> brightness/speed modulation |
| **RAM Pressure** | Fill level showing memory usage | `/proc/meminfo` -> gradient fill from green to red |

**Shared characteristics:** Data refresh rates are much slower than visual frame rates (1-10 Hz for most telemetry). Effects should interpolate between data points rather than stepping. The informational layer is best used as a *modifier* on an ambient base effect -- CPU temp tinting an aurora, for example.

**Data source architecture:**
```rust
pub trait TelemetrySource: Send + Sync {
    fn name(&self) -> &str;
    fn poll_interval(&self) -> Duration;
    fn sample(&mut self) -> Result<TelemetryData>;
}

pub enum TelemetryData {
    Temperature(f32),         // Celsius
    Percentage(f32),          // 0.0 - 1.0
    BytesPerSecond(u64),      // Network/disk throughput
    Count(u64),               // Event counter
}
```

### 1.5 Screen-Reactive

Effects that mirror or respond to on-screen content. The bridge between display and ambient lighting.

| Effect Pattern | Description | Technique |
|---|---|---|
| **Screen Ambience** | Mirror display colors to surrounding LEDs | PipeWire screen capture -> downscale -> canvas fill |
| **Dominant Color** | Extract dominant screen color, flood all LEDs | K-means clustering on captured frame -> single color |
| **Edge Glow** | Sample only screen edges for bias lighting | Border sampling with configurable depth |
| **Game-Specific** | Custom profiles for specific game UIs | Screen region -> health bar red, minimap colors, etc. |

**Shared characteristics:** Screen capture latency is the bottleneck. PipeWire DMA-BUF with zero-copy is essential on Wayland. The Screen Ambience built-in effect (already in the repo) demonstrates the pattern: `engine.zone` provides pre-sampled HSL data in a 28x20 grid, which the effect renders to the canvas. Hypercolor's screen input source must replicate this interface.

**Latency budget:** Capture (2-5ms) + Downscale (1ms) + Effect render (1ms) + Spatial sample + Device push (2-5ms) = ~10ms total. Achievable at 60fps.

### 1.6 Generative

Algorithmically-generated visuals with emergent complexity. These effects are why the 320x200 canvas exists -- they create visual richness that simple color cycling cannot.

| Effect Pattern | Description | Algorithm |
|---|---|---|
| **Particle Systems** | Hundreds of colored particles with physics | Position + velocity + acceleration, with spawn/die lifecycle |
| **Cellular Automata** | Conway's Game of Life variants | Grid state with configurable rule sets (B3/S23, etc.) |
| **Fractals** | Mandelbrot, Julia, or L-system renderings | Iterative computation, zoom animation |
| **Voronoi** | Cell-based color regions with drifting seeds | Fortune's algorithm or brute-force at 320x200 (tractable) |
| **Reaction-Diffusion** | Gray-Scott or Belousov-Zhabotinsky patterns | Two-chemical simulation on pixel grid |
| **Flow Fields** | Particles following Perlin noise vector field | Noise-based velocity field, particle trails with fade |
| **Plasma** | Classic demoscene plasma with sine interference | `sin(x + t) + sin(y + t) + sin(x + y + t)` -> palette lookup |

**Shared characteristics:** Computationally expensive relative to simple effects. The wgpu path is ideal for these -- WGSL compute shaders can run cellular automata or reaction-diffusion on the GPU in microseconds. For the Servo path, WebGL shaders handle the heavy lifting. Canvas 2D implementations should be avoided for complex generative effects due to CPU overhead.

### 1.7 Artistic

Curated color palettes and mood boards. Less about animation, more about specific aesthetic choices.

| Effect Pattern | Description | Palette Source |
|---|---|---|
| **SilkCircuit** | The Hypercolor design system palette | Electric Purple, Neon Cyan, Coral, Electric Yellow |
| **Synthwave** | Neon pink, electric blue, deep purple | Retrowave aesthetic, optional grid line animation |
| **Nature** | Earth tones, forest greens, sky blues | Extracted from nature photography |
| **Seasonal** | Holiday-specific color schemes | Christmas (red/green/gold), Halloween (orange/purple/black) |
| **Film Palettes** | Colors from iconic cinematography | Blade Runner amber, Matrix green, Tron cyan |
| **Custom Palette** | User-defined color collection | Color picker with 2-8 color stops |

**Shared characteristics:** These are fundamentally *gradient definitions* applied to a *motion pattern*. The palette defines the colors; the motion (wave, breathe, shift, static) defines how those colors move across the canvas. This separation is key to the effect authoring experience -- users should be able to apply any palette to any motion pattern.

### 1.8 Utility

The basics. Every lighting system needs these, and they must be flawless.

| Effect Pattern | Description | Implementation |
|---|---|---|
| **Solid Color** | Single color across all LEDs | Canvas filled with one color. Must support hex, HSL, color picker |
| **Gradient** | Linear or radial gradient | 2-8 color stops, configurable angle and position |
| **Color Cycle** | Smooth transition through a color sequence | Palette interpolation with configurable speed and easing |
| **Rainbow** | HSL hue sweep across the canvas | Hue = `(position + time * speed) % 360` |
| **Breathing** | Pulsing brightness | Sinusoidal brightness envelope, configurable period |
| **Strobe** | On/off flashing (use responsibly) | Binary state toggle with configurable frequency. **Capped at 10Hz max** for photosensitivity safety |
| **Off** | All LEDs dark | The most important effect. Must be instant and reliable |

**Shared characteristics:** Zero audio dependency. Zero network dependency. These must work on every device, every time, with no configuration. They are the fallback when anything goes wrong.

### Category Tagging

Effects declare their categories via metadata. Multiple tags are encouraged:

```html
<!-- HTML meta tag format -->
<meta property="categories" content="reactive,generative" />
```

```rust
// Rust-native format
pub struct EffectMetadata {
    pub categories: Vec<EffectCategory>,
    // ...
}

pub enum EffectCategory {
    Ambient,
    Reactive,
    Interactive,
    Informational,
    ScreenReactive,
    Generative,
    Artistic,
    Utility,
}
```

---

## 2. Effect Authoring Experience

The authoring experience determines whether Hypercolor grows a creator community or stays a personal project. Three authoring paths serve three audiences: web developers (HTML/Canvas), shader artists (WGSL), and visual thinkers (node editor). All three paths converge on the same output: pixels on the 320x200 canvas.

### 2.1 HTML/Canvas Path (The Familiar Path)

This is the path of least resistance. Anyone who can write a web page can write a Hypercolor effect. The 230+ existing community HTML effects prove the model works.

**File structure:**
```
my-effect/
  my-effect.html      # The effect (required)
  my-effect.png       # Preview thumbnail (recommended, 320x200)
  README.md           # Description, credits (optional)
```

**Minimal effect:**
```html
<head>
  <title>My First Effect</title>
  <meta description="A simple color wave" />
  <meta publisher="username" />
  <meta property="speed" label="Speed" type="number"
        min="1" max="100" default="50" />
</head>
<body style="margin:0; background:#000">
  <canvas id="exCanvas" width="320" height="200"></canvas>
</body>
<script>
  const ctx = document.getElementById('exCanvas').getContext('2d');
  let t = 0;

  function update() {
    for (let x = 0; x < 320; x++) {
      const hue = (x + t * speed / 10) % 360;
      ctx.fillStyle = `hsl(${hue}, 100%, 50%)`;
      ctx.fillRect(x, 0, 1, 200);
    }
    t++;
    requestAnimationFrame(update);
  }
  requestAnimationFrame(update);
</script>
```

**The Lightscript TypeScript path** extends this with proper structure:

```typescript
import { CanvasEffect, NumberControl } from '@hypercolor/lightscript';

@NumberControl('speed', { label: 'Speed', min: 1, max: 100, default: 50 })
export class ColorWave extends CanvasEffect {
  private t = 0;

  render(ctx: CanvasRenderingContext2D) {
    for (let x = 0; x < this.width; x++) {
      const hue = (x + this.t * this.controls.speed / 10) % 360;
      ctx.fillStyle = `hsl(${hue}, 100%, 50%)`;
      ctx.fillRect(x, 0, 1, this.height);
    }
    this.t++;
  }
}
```

**WebGL path** for GPU-accelerated effects:

```typescript
import { WebGLEffect, ComboboxControl } from '@hypercolor/lightscript';

@ComboboxControl('palette', {
  label: 'Palette',
  values: ['Aurora', 'Lava', 'Ice'],
  default: 'Aurora'
})
export class PlasmaShader extends WebGLEffect {
  fragmentShader = `
    uniform float iTime;
    uniform vec2 iResolution;
    uniform float iAudioBass;

    void main() {
      vec2 uv = gl_FragCoord.xy / iResolution;
      float v = sin(uv.x * 10.0 + iTime) + sin(uv.y * 8.0 + iTime * 0.7);
      v += sin((uv.x + uv.y) * 6.0 + iTime * 0.5) + iAudioBass;
      gl_FragColor = vec4(v * 0.5 + 0.5, uv.y, 1.0 - v * 0.3, 1.0);
    }
  `;
}
```

### 2.2 WGSL Shader Path (The Performance Path)

For effects that need maximum throughput or want to leverage GPU compute. These run on the wgpu renderer -- no web engine overhead.

**File structure:**
```
my-shader/
  my-shader.wgsl       # The shader (required)
  my-shader.toml       # Metadata + control definitions (required)
  my-shader.png        # Preview thumbnail (recommended)
```

**Shader file (`aurora.wgsl`):**
```wgsl
struct Uniforms {
    time: f32,
    resolution: vec2<f32>,
    audio_level: f32,
    audio_bass: f32,
    audio_mid: f32,
    audio_treble: f32,
    // User controls mapped by name
    speed: f32,
    intensity: f32,
    color_shift: f32,
}

@group(0) @binding(0) var<uniform> u: Uniforms;

// Simplex noise helpers...

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = pos.xy / u.resolution;
    let noise = fbm(uv * 3.0 + u.time * u.speed * 0.01);
    let hue = noise * 0.3 + u.color_shift;
    return hsv_to_rgb(hue, 0.8, noise * u.intensity);
}
```

**Metadata file (`aurora.toml`):**
```toml
[effect]
name = "Aurora Native"
description = "GPU-native northern lights with fBM noise"
author = "hyperb1iss"
categories = ["ambient", "generative"]
audio_reactive = true

[[controls]]
id = "speed"
label = "Speed"
type = "number"
min = 0.0
max = 100.0
default = 30.0

[[controls]]
id = "intensity"
label = "Intensity"
type = "number"
min = 0.0
max = 100.0
default = 70.0

[[controls]]
id = "color_shift"
label = "Color Shift"
type = "number"
min = 0.0
max = 1.0
default = 0.3
step = 0.01
```

**Uniform injection:** The daemon reads the TOML, constructs a uniform buffer matching the struct layout, and updates it each frame with current time, audio data, and user control values. The shader author declares the struct; the engine fills it.

### 2.3 Visual Effect Builder (The Creative Path)

A node-based editor in the web UI for users who think visually rather than textually. Not a v1 feature, but the architecture should not preclude it.

**Concept:**
```
[Noise Generator] --> [Color Ramp] --> [Blend] --> [Output]
                                          ^
[Audio Bass] -----> [Smooth] ------------|
```

**Node types:**
- **Generators:** Noise (Perlin, Simplex, Worley), Gradient, Solid Color, Pattern (stripe, checker, dots)
- **Modifiers:** Color Ramp, Hue Shift, Brightness, Blur, Distort, Mirror, Tile
- **Inputs:** Audio (bass, mid, treble, beat, spectrum), Time, Mouse, Keyboard
- **Math:** Add, Multiply, Remap, Smoothstep, Sin/Cos, Threshold
- **Blend:** Layer (screen, multiply, overlay, add), Mask, Mix

**Implementation strategy:** The node graph compiles to a WGSL compute shader. Each node is a function; edges are variable bindings. The compiler walks the graph topologically and emits a single shader. This means node-built effects run at native wgpu speed -- no interpretation overhead.

**Phase:** v3 or later. The node editor UI is significant work, but the underlying shader compiler can be built incrementally.

### 2.4 Templates & Starter Kits

New effect authors need scaffolding. Ship these templates:

| Template | Description | Path |
|---|---|---|
| `canvas-basic` | Minimal Canvas 2D effect with controls | HTML |
| `canvas-audio` | Audio-reactive Canvas 2D with spectrum access | HTML |
| `webgl-shader` | Three.js fragment shader with audio uniforms | HTML/WebGL |
| `webgl-particles` | Three.js particle system with audio reactivity | HTML/WebGL |
| `wgsl-basic` | Minimal WGSL shader with time and resolution | WGSL |
| `wgsl-audio` | WGSL shader with full audio uniform block | WGSL |
| `wgsl-compute` | WGSL compute shader for cellular automata/sim | WGSL |

**CLI scaffolding:**
```bash
hypercolor effect new my-aurora --template canvas-audio
# Creates effects/custom/my-aurora/my-aurora.html with boilerplate
```

### 2.5 Testing Tools

Effect authors need to verify their work against different device configurations without owning every device.

**Layout simulator:** The effect dev server provides a virtual device panel. Authors select from preset layouts:
- Single LED strip (60 LEDs)
- Keyboard (full-size, TKL, 60%)
- Fan ring (16 LEDs) x4
- Case interior (mixed strips + fans)
- Full desktop setup (keyboard + mouse + strip + fans + Strimers)

The simulator samples the effect canvas at the simulated LED positions and renders the result, showing exactly how the effect will look on real hardware.

**Audio test signals:** When no audio input is available, the dev server can inject:
- Sine sweep (frequency rises over time)
- Beat pattern (4/4 kicks at configurable BPM)
- Pink noise (even energy distribution)
- Music file playback (local MP3/FLAC)

### 2.6 Effect Dev Server with HMR

The development experience must be instant. The effect dev server watches the filesystem and hot-reloads effects on change.

```
hypercolor dev [--effect effects/custom/my-effect.html] [--port 3420]
```

**Architecture:**
```
┌─────────────────────────────────┐
│  Effect Dev Server (port 3420)   │
│                                  │
│  ┌──────────────┐  ┌──────────┐ │
│  │ File Watcher  │  │ WebSocket│ │
│  │  (notify)     │──│  Server  │ │
│  └──────┬───────┘  └─────┬────┘ │
│         │                │       │
│  ┌──────▼───────┐  ┌─────▼────┐ │
│  │ Effect Engine │  │ Browser  │ │
│  │ (Servo/wgpu)  │  │ Preview  │ │
│  └──────┬───────┘  └──────────┘ │
│         │                        │
│  ┌──────▼───────┐               │
│  │ Virtual LEDs  │               │
│  │ (simulated    │               │
│  │  device panel)│               │
│  └──────────────┘               │
└─────────────────────────────────┘
```

**HMR behavior:**
1. File change detected via `notify` crate
2. For HTML effects: Servo reloads the page. State is lost (by design -- effects should be stateless between reloads)
3. For WGSL shaders: Pipeline is recompiled. If compilation fails, error is displayed in the dev UI overlay and the previous working shader continues running
4. WebSocket pushes the updated preview to all connected browser clients
5. Control values are preserved across reloads (stored in the dev server, re-injected after reload)

**Dev UI overlay** (rendered in the browser preview panel):
- Live FPS counter
- Frame time histogram (detect jank)
- Audio data inspector (see the FFT bins, beat state, mel bands)
- Control panel (auto-generated from meta tags)
- Canvas zoom (the 320x200 canvas is tiny -- zoom to see pixel detail)
- Error console (JS errors, shader compilation errors)

---

## 3. Effect Discovery & Browsing

How users find effects is as important as the effects themselves. A library of 230+ effects is useless if users can't find the one they want.

### 3.1 The Effect Browser

The web UI's effect browser is the primary discovery interface. It must feel like browsing a curated gallery, not scrolling a file list.

**Layout:**
```
┌────────────────────────────────────────────────────────────────┐
│  [Search...🔍]  [Category ▼]  [Audio ▼]  [Sort ▼]  [+ New]   │
├────────────────────────────────────────────────────────────────┤
│                                                                │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐      │
│  │ ~~~~~~~~ │  │ ▓▓▓▓▓▓▓▓ │  │ ◌◌◌◌◌◌◌◌ │  │ ≋≋≋≋≋≋≋≋ │      │
│  │ ~~~~~~~~ │  │ ▓▓▓▓▓▓▓▓ │  │ ◌◌◌◌◌◌◌◌ │  │ ≋≋≋≋≋≋≋≋ │      │
│  │  Aurora   │  │  Matrix   │  │ Particles │  │  Ocean    │      │
│  │  ★ 4.8   │  │  ★ 4.5   │  │  ★ 4.7   │  │  ★ 4.3   │      │
│  └──────────┘  └──────────┘  └──────────┘  └──────────┘      │
│                                                                │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐      │
│  │ ▒▒▒▒▒▒▒▒ │  │ ░░░░░░░░ │  │ ████████ │  │ ╬╬╬╬╬╬╬╬ │      │
│  │ ▒▒▒▒▒▒▒▒ │  │ ░░░░░░░░ │  │ ████████ │  │ ╬╬╬╬╬╬╬╬ │      │
│  │ Fire Viz  │  │ Lava Lamp │  │ Spectrum  │  │ Cyberpunk │      │
│  │  ★ 4.6   │  │  ★ 4.4   │  │  ★ 4.9   │  │  ★ 4.2   │      │
│  └──────────┘  └──────────┘  └──────────┘  └──────────┘      │
│                                                                │
│  ◄ 1  2  3  4  5  ... 24 ►                                    │
└────────────────────────────────────────────────────────────────┘
```

### 3.2 Preview System

Static thumbnails are insufficient. Effects are animated -- the preview must convey motion.

**Preview tiers:**

| Tier | Format | When Used | Storage |
|---|---|---|---|
| **Thumbnail** | 320x200 PNG | Grid view, search results | Bundled with effect |
| **Animated Preview** | 320x200 WebP animation (3-5 sec loop) | Hover state, expanded card | Generated on install or first browse |
| **Live Preview** | Real-time canvas render | Detail view, before applying | Rendered by the effect engine on demand |

**Animated preview generation:** When a new effect is installed, the daemon renders 180 frames (3 seconds at 60fps) with default parameters and encodes them as an animated WebP. For audio-reactive effects, a standard test signal (pink noise + 120 BPM beat) is used. Previews are cached in `~/.local/share/hypercolor/previews/`.

**Live preview:** When a user clicks an effect card, the detail panel renders the effect in real-time using the actual effect engine (Servo or wgpu). This is the "try before you apply" experience. The live preview runs at reduced priority -- if the main render loop is busy, the preview drops frames gracefully.

### 3.3 Search & Filtering

**Text search** matches against:
- Effect name (weighted highest)
- Description text
- Author name
- Category tags
- Control parameter names (searching "bass" finds effects with a "bass boost" control)

**Filter dimensions:**

| Filter | Options |
|---|---|
| **Category** | Ambient, Reactive, Interactive, Informational, Screen, Generative, Artistic, Utility |
| **Audio** | Audio-reactive, Non-audio, Audio-optional |
| **Renderer** | Servo (HTML), wgpu (native), Both |
| **Source** | Built-in, Community, Custom (user-created) |
| **Performance** | Light (<1ms), Medium (1-5ms), Heavy (>5ms) |
| **Compatibility** | All devices, Strip-optimized, Keyboard-optimized, Matrix-optimized |

**Sort options:** Popular (install count), Rating, Newest, Name (A-Z), Recently Used, Trending (high install rate over last 7 days).

### 3.4 Recommendations

**"Similar effects"** uses a simple feature vector:
- Category tags (one-hot encoded)
- Audio reactivity (boolean)
- Dominant color palette (extracted from preview thumbnail via k-means)
- Control parameter count (complexity proxy)
- Frame time (performance profile)

Cosine similarity between feature vectors produces "Similar to Aurora" suggestions. No ML needed -- the feature space is small enough for exact nearest-neighbor search.

**"If you like X, try Y"** -- Curated editorial lists for launch. Community-driven recommendations (based on install correlation) later.

### 3.5 Sections & Curation

The effect browser home page shows curated sections:

| Section | Logic |
|---|---|
| **Featured** | Manually curated by maintainers. Rotates weekly |
| **Popular** | Highest install count, all time |
| **Trending** | Highest install rate, last 7 days |
| **New Arrivals** | Most recently published |
| **Audio Essentials** | Best-rated audio-reactive effects |
| **Chill Vibes** | Top ambient effects |
| **Recently Used** | User's own history |
| **By This Author** | Other effects from the same creator (shown in detail view) |

---

## 4. Effect Marketplace / Repository

### 4.1 Architecture: Git-Native Repository

Effects live in a git repository. This is the simplest possible infrastructure that supports versioning, forking, pull requests, and community contribution.

**Repository structure:**
```
hypercolor-effects/                     # GitHub repository
├── registry.toml                       # Effect index (name, version, hash, metadata)
├── effects/
│   ├── aurora/
│   │   ├── aurora.html
│   │   ├── aurora.png
│   │   └── effect.toml                 # Metadata, version, dependencies
│   ├── matrix/
│   │   ├── matrix.html
│   │   ├── matrix.png
│   │   └── effect.toml
│   └── ...
└── packs/
    ├── gaming-essentials/
    │   └── pack.toml                   # Lists included effects
    ├── chill-ambient/
    │   └── pack.toml
    └── ...
```

**`effect.toml` manifest:**
```toml
[effect]
name = "Aurora"
version = "2.1.0"
description = "Northern lights illuminating your devices"
author = "Hypercolor"
license = "MIT"
categories = ["ambient", "generative"]
audio_reactive = false
renderer = "servo"                      # "servo" | "wgpu" | "both"
min_hypercolor = "0.2.0"               # Minimum compatible engine version

[preview]
thumbnail = "aurora.png"
# animated preview auto-generated on install

[compatibility]
# Optional: declare what layouts this effect is optimized for
optimized_for = ["strip", "matrix"]
```

### 4.2 Distribution: hypercolor-effects Registry

The Hypercolor daemon ships with a default registry URL pointing to the official `hypercolor-effects` GitHub repository. The registry is a single `registry.toml` file that the client fetches to know what's available.

**Install flow:**
```bash
# CLI
hypercolor effect install aurora
hypercolor effect install gaming-essentials   # Install a pack
hypercolor effect update                      # Update all installed effects
hypercolor effect search "fire"               # Search the registry

# Or via web UI: one-click install from the effect browser
```

**Under the hood:**
1. Fetch `registry.toml` from the registry URL (cached, refreshed every 4 hours or on manual refresh)
2. Download the effect directory as a tarball (GitHub archive API) or sparse checkout
3. Verify SHA-256 hash against registry entry
4. Extract to `~/.local/share/hypercolor/effects/community/<name>/`
5. Generate animated preview
6. Register in local effect database

**Why git, not npm/crates.io?**
- Effects are HTML files and images, not compiled packages. Git handles this naturally
- GitHub provides free hosting, CI, issues, pull requests
- Forking an effect to customize it is a native git operation
- No registry server to maintain. The `registry.toml` file *is* the registry
- Version history is implicit in git commits

### 4.3 Versioning & Updates

Effects follow semver:
- **Patch** (2.1.0 -> 2.1.1): Bug fix, no visible change
- **Minor** (2.1.0 -> 2.2.0): New controls, visual improvements, backward-compatible
- **Major** (2.1.0 -> 3.0.0): Breaking changes to controls (renamed/removed parameters)

**Update behavior:** By default, minor and patch updates are auto-applied. Major updates require user confirmation (their presets might break).

### 4.4 Rating & Reviews

Keep it simple. GitHub Discussions or a lightweight JSON-based rating system stored in the repository:

```toml
# ratings/aurora.toml (auto-generated from user submissions)
[[rating]]
user = "anon_sha256_abc123"
stars = 5
comment = "Best aurora I've seen"
version = "2.1.0"
date = "2026-03-15"
```

Users submit ratings through the Hypercolor web UI. Ratings are posted as GitHub Discussions comments (via the GitHub API) or stored locally for aggregation. A bot periodically compiles ratings into the `ratings/` directory.

**Moderation:** Community ratings are public. Offensive content is reported via GitHub's built-in mechanism. Effect maintainers can respond to reviews.

### 4.5 Effect Bundles / Packs

Packs are curated collections shipped as a single installable unit.

```toml
# packs/gaming-essentials/pack.toml
[pack]
name = "Gaming Essentials"
description = "10 effects perfect for gaming setups"
author = "hyperb1iss"
version = "1.0.0"

[[effects]]
name = "rainbow"
version = "^1.0"

[[effects]]
name = "fire-visualizer"
version = "^2.0"

[[effects]]
name = "spectrum-analyzer"
version = "^1.5"

# ... 7 more
```

**Shipped packs:**
- **Essentials** -- The effects every user needs (solid, gradient, rainbow, breathing, spectrum)
- **Gaming** -- Audio-reactive + interactive keyboard effects
- **Chill** -- Ambient effects for background ambiance
- **Audiophile** -- Deep audio visualization (chromagram, waveform, spectral flux)
- **Showcase** -- The most visually impressive effects for showing off

### 4.6 Revenue Sharing & Licensing

**No revenue sharing. Free-only.** Hypercolor is open source. Effects are open source. The community wins when everything is free and forkable.

Effects in the official registry must use an OSI-approved license (MIT, Apache-2.0, GPL-3.0, etc.). The default template uses MIT. Authors can publish proprietary effects on their own -- Hypercolor will load any HTML file -- but the official registry is free-only.

### 4.7 Content Moderation

**What could go wrong:**
- **Malicious effects:** JavaScript that attempts to exfiltrate data, phone home, or abuse system resources. Mitigated by Servo sandboxing (no network access, no filesystem access, no clipboard) and the wgpu shader validator (WGSL is not Turing-complete in a dangerous way)
- **Offensive content:** Effects with hate symbols, explicit imagery. Moderated via GitHub PR review process -- all effects enter the registry through pull requests
- **Copyright infringement:** Brand logos, copyrighted artwork. Same PR review process
- **Seizure triggers:** Rapid flashing effects. Automated check: reject effects that produce brightness changes > 3Hz across > 25% of the canvas. Flag for manual review if detected

**Automated checks (CI on PR):**
1. Effect loads without JavaScript errors
2. Effect renders at least 30fps on the CI runner
3. No external network requests (CSP violation check)
4. Photosensitivity scan (flash frequency analysis on test render)
5. Preview thumbnail exists and matches the effect name
6. `effect.toml` is valid and complete

---

## 5. Effect Composition & Layering

Single effects are great. Layered effects are magic. The composition system lets users combine multiple effects into a single visual output.

### 5.1 Layer Stack

The composition engine maintains an ordered stack of effect layers, each with its own blend mode and opacity.

```
┌─────────────────────────────┐
│  Layer 3: Beat Pulse        │  Blend: Add, Opacity: 60%
│  (audio-reactive flash)     │
├─────────────────────────────┤
│  Layer 2: Spectrum Bars     │  Blend: Screen, Opacity: 80%
│  (audio-reactive bars)      │
├─────────────────────────────┤
│  Layer 1: Aurora             │  Blend: Normal, Opacity: 100%
│  (ambient base)             │
└─────────────────────────────┘
         │
         ▼
    Composited 320x200 canvas
```

**Data model:**
```rust
pub struct EffectComposition {
    pub layers: Vec<EffectLayer>,
    pub transitions: Vec<TransitionState>,
}

pub struct EffectLayer {
    pub effect_id: String,
    pub opacity: f32,                    // 0.0 - 1.0
    pub blend_mode: BlendMode,
    pub mask: Option<LayerMask>,
    pub zone_filter: Option<Vec<String>>, // Apply only to specific zones
    pub enabled: bool,
}

pub enum BlendMode {
    Normal,      // Source over destination
    Add,         // Additive (great for glow/flash effects)
    Screen,      // Lighter blend (brightens without blowing out)
    Multiply,    // Darker blend (tinting)
    Overlay,     // Contrast enhancement
    SoftLight,   // Subtle tinting
    Difference,  // Psychedelic color inversion
}
```

**Compositing implementation:** Each layer renders to its own 320x200 RGBA buffer. The compositor blends them bottom-to-top using the specified blend mode. At 320x200, the per-pixel blend math is trivially fast -- 64,000 pixels x 8 blend operations = microseconds on any modern CPU. For the wgpu path, compositing runs as a compute shader for zero CPU cost.

### 5.2 Per-Zone Effect Assignment

Different device zones can run different effects simultaneously. This is essential for complex setups where you want the keyboard running one effect while case fans run another.

```
Zone Mapping:
  ┌─────────────────────────────────┐
  │        Effect Canvas             │
  │  ┌───────────────────────────┐  │
  │  │ Zone: Keyboard             │  │  → Effect: Typing Ripples
  │  └───────────────────────────┘  │
  │  ┌────────┐  ┌────────┐        │
  │  │ Zone:  │  │ Zone:  │        │  → Effect: Aurora
  │  │ Fan 1  │  │ Fan 2  │        │
  │  └────────┘  └────────┘        │
  │  ┌───────────────────────────┐  │
  │  │ Zone: LED Strip            │  │  → Effect: Spectrum Analyzer
  │  └───────────────────────────┘  │
  └─────────────────────────────────┘
```

**Implementation options:**

**Option A: Multiple canvases.** Each zone group gets its own 320x200 canvas and effect engine instance. Zones sample from their assigned canvas. Simple but expensive -- multiple Servo instances or shader pipelines.

**Option B: Single canvas, zone masking.** One effect renders to the full canvas. Zone-specific effects render to separate smaller buffers and are composited into the main canvas at the zone's position. More efficient, slightly more complex.

**Recommendation: Option B.** The spatial layout engine already maps zones to canvas regions. Zone-specific effects render to a buffer sized to the zone's canvas footprint, then blit into the main canvas. The main canvas runs the "background" effect; zone overrides layer on top.

### 5.3 Effect Masks & Regions

Masks restrict an effect's visibility to specific canvas regions. Useful for vignettes, split effects, and creative compositions.

**Mask types:**
```rust
pub enum LayerMask {
    /// Rectangular region (normalized 0-1 coordinates)
    Rect { x: f32, y: f32, width: f32, height: f32 },

    /// Circular/elliptical region
    Ellipse { cx: f32, cy: f32, rx: f32, ry: f32 },

    /// Arbitrary grayscale image (8-bit, same resolution as canvas)
    Image(Vec<u8>),

    /// Gradient mask (linear or radial)
    Gradient { gradient_type: GradientType, angle: f32, stops: Vec<(f32, f32)> },

    /// Audio-driven mask (e.g., bass level controls mask boundary)
    AudioDriven { source: AudioMaskSource, min: f32, max: f32 },
}
```

### 5.4 Transitions

When switching effects, don't just cut -- crossfade.

```rust
pub enum Transition {
    /// Instant switch (default for utility effects)
    Cut,

    /// Linear opacity crossfade
    Crossfade { duration: Duration },

    /// Horizontal/vertical wipe
    Wipe { direction: WipeDirection, duration: Duration },

    /// Dissolve via noise pattern
    Dissolve { duration: Duration },

    /// Fade to black, then fade in new effect
    FadeThrough { fade_out: Duration, hold: Duration, fade_in: Duration },
}
```

**Transition implementation:** During a transition, both the outgoing and incoming effects render simultaneously. The compositor blends them according to the transition's progress curve. After the transition completes, the outgoing effect is stopped and its resources freed.

### 5.5 Effect Chains / Pipelines

For advanced users: effects can feed into each other as a processing pipeline.

```
[Noise Generator] → [Color Ramp] → [Audio Modulate Brightness] → [Output]
```

This is the same concept as the node editor (Section 2.3) but expressed as a linear chain. The first effect renders to a buffer; the second effect reads that buffer as an input texture and transforms it.

**Not a v1 feature.** The architecture supports it (effects already render to pixel buffers), but the UI and authoring complexity is significant. Composition layers with blend modes cover 90% of the use cases.

---

## 6. Effect Parameters & Presets

### 6.1 The Control System

Effects declare parameters. The engine generates UI. This is the contract:

**Supported control types:**

| Type | HTML Meta | Rust | UI Widget | Value |
|---|---|---|---|---|
| `number` | `type="number" min="0" max="100" default="50"` | `ControlType::Number { min, max, step }` | Slider + input | `f32` |
| `boolean` | `type="boolean" default="0"` | `ControlType::Boolean` | Toggle switch | `bool` |
| `combobox` | `type="combobox" values="A,B,C" default="A"` | `ControlType::Combobox { values }` | Dropdown | `String` |
| `color` | `type="color" default="#ff0000"` | `ControlType::Color` | Color picker | `String` (hex) |
| `hue` | `type="hue" min="0" max="360" default="180"` | `ControlType::Hue { min, max }` | Hue wheel | `f32` |
| `text` | `type="textfield" default=""` | `ControlType::TextField` | Text input | `String` |
| `sensor` | `type="sensor" default="CPU Load"` | `ControlType::Sensor` | Sensor picker | `String` |

**Control injection flow (Servo path):**
1. Parse `<meta>` tags on effect load to discover controls
2. Render the control panel UI in the web frontend
3. When user changes a value: REST/WebSocket -> daemon -> `ServoRenderer::inject_control(name, value)`
4. Servo evaluates `window['controlName'] = value; window.update?.();`
5. Effect reads the global variable on next frame

**Control injection flow (wgpu path):**
1. Parse `effect.toml` on effect load
2. Build uniform buffer layout matching the shader's struct
3. User changes a value -> daemon updates the uniform buffer
4. Shader reads the new value on next frame via `u.control_name`

### 6.2 Preset System

Presets are named snapshots of all control values for a given effect.

```toml
# ~/.config/hypercolor/presets/aurora/chill-evening.toml
[preset]
name = "Chill Evening"
effect = "aurora"
created = "2026-03-01T20:30:00Z"

[values]
effectSpeed = 25
amount = 40
frontColor = "#8800ff"
backColor = "#110033"
colorCycle = false
cycleSpeed = 50
```

**Operations:**
- **Save preset:** Snapshot current control values with a name
- **Load preset:** Apply all values from a preset
- **Delete preset:** Remove a saved preset
- **Export preset:** Copy to clipboard or file (for sharing)
- **Import preset:** Load from file or URL
- **Default preset:** Every effect has an implicit "Default" preset from its control defaults

**Preset discovery:** Presets can be shared alongside effects in the marketplace. An effect can ship with multiple presets in its directory:

```
aurora/
  aurora.html
  aurora.png
  effect.toml
  presets/
    northern-lights.toml
    fire-aurora.toml
    minimal.toml
```

### 6.3 Parameter Animation

Automate control values over time. Turn a static effect into a dynamic one without writing code.

```rust
pub struct ParameterAnimation {
    pub control_id: String,
    pub curve: AnimationCurve,
    pub duration: Duration,
    pub repeat: RepeatMode,
}

pub enum AnimationCurve {
    /// Sinusoidal oscillation between min and max
    Sine { min: f32, max: f32 },

    /// Linear ramp from start to end
    Linear { start: f32, end: f32 },

    /// Random walk with configurable step size
    RandomWalk { step: f32, min: f32, max: f32 },

    /// Keyframe sequence with interpolation
    Keyframes(Vec<(f32, f32)>),  // (time_normalized, value)
}

pub enum RepeatMode {
    Once,
    Loop,
    PingPong,
}
```

**Use case:** Slowly cycle the Aurora's `frontColor` hue over 5 minutes while keeping all other parameters static. The animation modulates the control value independently of user interaction -- if the user manually changes the value, the animation is overridden (or paused, user preference).

### 6.4 Parameter Linking

Bind a control's value to an audio or system data source.

```rust
pub struct ParameterLink {
    pub control_id: String,
    pub source: LinkSource,
    pub mapping: LinkMapping,
}

pub enum LinkSource {
    AudioLevel,
    AudioBass,
    AudioMid,
    AudioTreble,
    AudioBeat,
    CpuTemp,
    GpuLoad,
    Time { component: TimeComponent },  // Hour, Minute, Second
}

pub struct LinkMapping {
    pub source_range: (f32, f32),       // Input range
    pub target_range: (f32, f32),       // Output range (maps to control min/max)
    pub smoothing: f32,                  // 0.0 (instant) to 0.99 (very smooth)
    pub invert: bool,
}
```

**Use case:** Link the Aurora's `effectSpeed` to `AudioBass`. When the bass hits, the aurora speeds up. When it's quiet, it drifts slowly. The smoothing parameter prevents jittery behavior.

### 6.5 Randomization & Variation

A "Surprise Me" button that randomizes parameters within their declared ranges. Useful for exploration and discovering unexpected combinations.

**Smart randomization rules:**
- Color controls: Random hue, but keep saturation > 50% and lightness between 30-70% (avoid ugly colors)
- Speed controls: Bias toward the middle of the range (extremes are often unusable)
- Boolean controls: 50/50 chance
- Combobox controls: Uniform random selection
- Controls with `step` defined: Snap to valid steps

**Variation mode:** Instead of full randomization, apply a small random offset to all current values. "I like this but want something slightly different." Offset magnitude is configurable (subtle: +/-10%, moderate: +/-25%, wild: +/-50%).

---

## 7. Audio-Reactive Effects Deep Dive

Audio reactivity is the flagship feature. It's what makes RGB lighting feel alive rather than decorative. The pipeline from microphone to LED must be fast, accurate, and musically meaningful.

### 7.1 The Full Audio Data Pipeline

```
Audio Source (PulseAudio/PipeWire)
    │
    ▼
┌──────────────────────────────────────────────────────────────────┐
│  Audio Capture (cpal)                                             │
│  • Sample rate: 44100 Hz                                          │
│  • Buffer size: 1024 samples (~23ms)                              │
│  • Format: f32 mono (stereo downmixed)                            │
└───────────────────┬──────────────────────────────────────────────┘
                    │ Raw PCM samples
                    ▼
┌──────────────────────────────────────────────────────────────────┐
│  Windowing + FFT                                                  │
│  • Hann window applied to reduce spectral leakage                 │
│  • 2048-point FFT (1024 unique bins)                              │
│  • Magnitude spectrum: |FFT|^2                                    │
│  • → 200-bin log-scaled frequency array (20Hz - 20kHz)            │
└───────────────────┬──────────────────────────────────────────────┘
                    │
        ┌───────────┼───────────────────────────────────────┐
        │           │                                       │
        ▼           ▼                                       ▼
┌────────────┐ ┌─────────────┐                    ┌────────────────┐
│ Band Energy │ │ Mel Bands   │                    │ Raw Spectrum   │
│             │ │             │                    │                │
│ bass:  0-1  │ │ 24 mel-     │                    │ freq[200]:     │
│ mid:   0-1  │ │ scaled      │                    │ log-frequency  │
│ treble:0-1  │ │ bands       │                    │ magnitude      │
│ level: 0-1  │ │ (perceptual)│                    │ array          │
└──────┬─────┘ └──────┬──────┘                    └───────┬────────┘
       │              │                                    │
       │              ▼                                    │
       │     ┌────────────────┐                           │
       │     │ Chromagram     │                           │
       │     │                │                           │
       │     │ 12 pitch       │                           │
       │     │ classes        │                           │
       │     │ (C,C#,...,B)   │                           │
       │     │ dominantPitch  │                           │
       │     │ harmonicHue    │                           │
       │     │ chordMood      │                           │
       │     └───────┬────────┘                           │
       │             │                                    │
       ▼             ▼                                    ▼
┌──────────────────────────────────────────────────────────────────┐
│  Beat Detection & Onset                                           │
│                                                                   │
│  Spectral flux (difference between consecutive FFT frames)        │
│  → Adaptive threshold → onset detection                           │
│  → Inter-onset interval tracking → BPM estimation                 │
│  → Phase-locked beat prediction → beatPhase (0-1)                 │
│  → beatConfidence, beatAnticipation                               │
│                                                                   │
│  Separate spectral flux per band:                                 │
│    spectralFluxBands[3] = [bass_flux, mid_flux, treble_flux]     │
└───────────────────┬──────────────────────────────────────────────┘
                    │
                    ▼
┌──────────────────────────────────────────────────────────────────┐
│  Spectral Features                                                │
│                                                                   │
│  brightness: spectral centroid (perception of "brightness")       │
│  spread: spectral bandwidth                                       │
│  rolloff: frequency below which 85% of energy is contained        │
│  density: number of active frequency bins above noise floor        │
└───────────────────┬──────────────────────────────────────────────┘
                    │
                    ▼
            ┌────────────────┐
            │ AudioData      │
            │ (published     │
            │  every frame   │
            │  via event bus)│
            └────────────────┘
```

### 7.2 The AudioData Struct

```rust
/// Complete audio analysis data, published every frame (60Hz)
pub struct AudioData {
    // === Basic (every effect needs these) ===
    pub level: f32,                     // Overall RMS level, 0.0-1.0
    pub bass: f32,                      // Low band energy (20-250 Hz), 0.0-1.0
    pub mid: f32,                       // Mid band energy (250-4000 Hz), 0.0-1.0
    pub treble: f32,                    // High band energy (4000-20000 Hz), 0.0-1.0
    pub freq: [f32; 200],              // Log-scaled frequency magnitudes

    // === Beat (for pulse/flash effects) ===
    pub beat: bool,                     // True on beat onset frame
    pub beat_pulse: f32,               // Decaying pulse: 1.0 on beat, exponential decay
    pub beat_phase: f32,               // 0.0-1.0, phase within current beat period
    pub beat_confidence: f32,          // How confident the detector is in the BPM
    pub beat_anticipation: f32,        // Ramps up before expected beat
    pub onset: bool,                    // True on any onset (not just beats)
    pub onset_pulse: f32,              // Decaying onset pulse

    // === Mel Scale (perceptually accurate) ===
    pub mel_bands: [f32; 24],          // 24 mel-scaled frequency bands
    pub mel_bands_normalized: [f32; 24], // Normalized to 0.0-1.0

    // === Chromagram (musical pitch) ===
    pub chromagram: [f32; 12],         // Energy per pitch class (C, C#, ..., B)
    pub dominant_pitch: u8,            // Index 0-11 of strongest pitch class
    pub dominant_pitch_confidence: f32,

    // === Spectral Features ===
    pub spectral_flux: f32,            // Rate of spectral change
    pub spectral_flux_bands: [f32; 3], // Per-band spectral flux
    pub brightness: f32,               // Spectral centroid (perception of brightness)
    pub spread: f32,                   // Spectral bandwidth
    pub rolloff: f32,                  // 85% energy frequency

    // === Harmonic Analysis ===
    pub harmonic_hue: f32,             // 0.0-1.0 hue mapped from dominant pitch
    pub chord_mood: f32,               // -1.0 (minor/sad) to 1.0 (major/happy)

    // === Derived (convenience) ===
    pub density: f32,                  // Fraction of active frequency bins
    pub width: f32,                    // Stereo width (if stereo input available)
}
```

### 7.3 How Effect Authors Access Audio Data

**Servo path (JavaScript):**
```javascript
// Injected by the runtime into window.engine.audio every frame
const audio = window.engine.audio;

// Basic
audio.level          // -100 to 0 (dB scale, LightScript compat) or 0-1 (Hypercolor mode)
audio.bass           // 0.0 - 1.0
audio.mid            // 0.0 - 1.0
audio.treble         // 0.0 - 1.0
audio.freq           // Uint8Array(200) — 0-255 scaled magnitudes

// Beat
audio.beat           // boolean
audio.beatPulse      // 0.0 - 1.0, decaying
audio.beatPhase      // 0.0 - 1.0
audio.onset          // boolean
audio.onsetPulse     // 0.0 - 1.0, decaying

// Mel
audio.melBands            // Float32Array(24)
audio.melBandsNormalized  // Float32Array(24) — 0.0-1.0

// Chromagram
audio.chromagram               // Float32Array(12)
audio.dominantPitch            // 0-11
audio.dominantPitchConfidence  // 0.0-1.0

// Spectral
audio.spectralFlux             // 0.0+
audio.spectralFluxBands        // Float32Array(3)
audio.brightness               // 0.0-1.0
audio.spread                   // 0.0-1.0
audio.rolloff                  // Hz

// Harmonic
audio.harmonicHue              // 0.0-1.0
audio.chordMood                // -1.0 to 1.0

// Derived
audio.density                  // 0.0-1.0
audio.width                    // 0.0-1.0
```

**wgpu path (WGSL):**
```wgsl
struct AudioUniforms {
    level: f32,
    bass: f32,
    mid: f32,
    treble: f32,
    beat: f32,          // 1.0 on beat frame, 0.0 otherwise
    beat_pulse: f32,
    beat_phase: f32,
    spectral_flux: f32,
    harmonic_hue: f32,
    chord_mood: f32,
    density: f32,
    brightness: f32,
}

@group(0) @binding(1) var<uniform> audio: AudioUniforms;
@group(0) @binding(2) var audio_spectrum: texture_1d<f32>;  // 200-bin FFT as texture
@group(0) @binding(3) var audio_mel: texture_1d<f32>;       // 24-bin mel as texture
@group(0) @binding(4) var audio_chromagram: texture_1d<f32>; // 12-bin chromagram
```

### 7.4 Common Audio-Reactive Patterns

A cookbook of proven audio-to-visual mappings:

| Pattern | Audio Source | Visual Target | Why It Works |
|---|---|---|---|
| **Bass = Brightness** | `audio.bass` | Global brightness multiplier | Bass frequencies are felt physically; brightness mirrors that impact |
| **Treble = Speed** | `audio.treble` | Animation speed / particle velocity | High frequencies feel fast; speed matches that perception |
| **Beat = Flash** | `audio.beatPulse` | Additive white flash with decay | The most instinctive mapping: drum hit = visual impact |
| **Level = Scale** | `audio.level` | Object size or effect radius | Louder = bigger. Universal metaphor |
| **Chromagram = Hue** | `audio.harmonicHue` | Base hue rotation | Maps musical key to color wheel. C=red, F#=cyan. Synesthesia |
| **Mood = Temperature** | `audio.chordMood` | Warm-cool color shift | Minor chords feel cold (blue); major chords feel warm (orange) |
| **Spectral Flux = Chaos** | `audio.spectralFlux` | Particle spawn rate / distortion | Musical transitions (drops, builds) create visual complexity |
| **Mel Bands = Multi-bar** | `audio.melBands[i]` | Individual bar/ring/zone brightness | Each mel band drives a separate visual element |
| **Onset = Particle Burst** | `audio.onset` | Spawn N particles at random positions | Any percussive event creates a visual burst |
| **Beat Phase = Progress** | `audio.beatPhase` | Circular progress indicator / sweep | Anticipation builds between beats; resolution on beat |

**Anti-patterns to warn authors about:**
- **Raw FFT to brightness without smoothing:** Creates seizure-inducing flicker. Always smooth with exponential moving average
- **Beat detection without confidence check:** Low-confidence beats fire on noise. Gate with `beatConfidence > 0.5`
- **Linear frequency mapping:** Human hearing is logarithmic. Use mel bands, not raw FFT bins, for perceptually even visualization
- **No silence handling:** Effects should degrade to a pleasant static state when audio is absent, not flicker at the noise floor

### 7.5 Latency Considerations

The audio-visual synchronization budget:

| Stage | Latency | Notes |
|---|---|---|
| Audio capture buffer | ~23ms | 1024 samples at 44.1kHz. Can reduce to 512 samples (~11ms) at the cost of FFT resolution |
| FFT computation | <1ms | 2048-point FFT is trivial |
| Beat detection | ~0ms | Runs inline with FFT |
| Event bus publish | <1ms | tokio watch channel, essentially free |
| Effect render | 1-10ms | Depends on effect complexity |
| Spatial sampling | <1ms | 320x200 bilinear sampling |
| Device push | 1-5ms | USB HID or UDP |
| **Total** | **~28-40ms** | 1.5-2.5 frames at 60fps |

**Perceived latency:** Humans tolerate audio-visual desync up to ~45ms before it feels "off." The pipeline fits within this budget at default settings. For latency-critical effects (drum pad visualization), reducing the capture buffer to 512 samples brings the total under 25ms.

**PipeWire advantage:** PipeWire's graph-based audio routing can provide lower-latency capture than PulseAudio, especially with the pro-audio configuration (buffer size 64 or 128 samples).

---

## 8. Effect Performance & Resource Management

Effects run on a daemon that must be rock-solid. A misbehaving effect cannot crash the daemon, starve other effects, or consume unbounded resources.

### 8.1 Effect Complexity Budgets

Each effect gets a per-frame time budget:

| Budget Tier | Frame Time | Target FPS | Use Case |
|---|---|---|---|
| **Light** | <2ms | 60fps (easy) | Solid color, gradient, simple breathing |
| **Medium** | 2-8ms | 60fps (comfortable) | Aurora, matrix, most Canvas 2D effects |
| **Heavy** | 8-14ms | 60fps (tight) | WebGL particle systems, complex shaders |
| **Extreme** | >14ms | Drops below 60fps | Full-scene Three.js, reaction-diffusion |

**Budget enforcement:**
```rust
pub struct FrameBudget {
    pub target_frame_time: Duration,  // 16.67ms for 60fps
    pub effect_budget: Duration,      // Target: 12ms (leave 4.67ms for sampling + device push)
    pub warning_threshold: Duration,  // 10ms — log a warning
    pub kill_threshold: Duration,     // 100ms — effect is hung, force-terminate
    pub consecutive_overruns: u32,    // Track how many frames exceed budget
    pub max_consecutive_overruns: u32, // 30 — reduce to 30fps after this many
}
```

If an effect consistently exceeds its budget (30+ consecutive overruns), the daemon automatically halves the target FPS for that effect (60 -> 30 -> 15). The user sees a "Performance Warning" badge on the effect in the UI.

### 8.2 GPU vs CPU Rendering Decisions

| Scenario | Renderer | Reason |
|---|---|---|
| New WGSL/GLSL shader effect | wgpu (GPU) | Native GPU execution, microsecond frame times |
| Simple Canvas 2D effect (< 50 draw calls) | Servo (CPU, software rendering) | Servo's software renderer is fast enough for simple effects |
| Complex Canvas 2D (> 200 draw calls) | Servo (GPU-accelerated if available) | Servo can use GPU compositing for Canvas 2D |
| WebGL effect | Servo (GPU) | WebGL is inherently GPU-accelerated |
| Three.js scene | Servo (GPU) | Three.js requires WebGL context |
| Composition (multi-layer) | wgpu compute shader | Blend operations are trivially parallel |

**Fallback chain:** If the GPU is unavailable (headless server, broken driver), all rendering falls back to Servo's `SoftwareRenderingContext`. At 320x200, software rendering is fast enough for most effects (not WebGL, which requires a GPU by definition).

### 8.3 Frame Dropping Behavior

When the system is under load:

```
Frame budget exceeded?
  │
  ├─ Occasional (< 5% of frames): No action. Jitter is invisible.
  │
  ├─ Frequent (5-20% of frames):
  │     Log performance warning
  │     Show yellow "!" badge on effect card in UI
  │
  ├─ Consistent (> 20% of frames for 30+ frames):
  │     Reduce target FPS: 60 → 30
  │     Show orange warning in UI: "Effect running at reduced frame rate"
  │
  └─ Extreme (frame time > 100ms):
        Force-terminate effect render for this frame
        Show red warning in UI: "Effect too heavy for this system"
        Suggest switching to a lighter alternative
```

**Device push priority:** If the frame budget is exceeded, the spatial sampling and device push still happen with the last successfully rendered frame. LEDs never go dark due to a slow effect -- they just show the previous frame.

### 8.4 Memory Limits

| Resource | Limit | Enforcement |
|---|---|---|
| Canvas buffer | 256KB (320x200x4) | Fixed allocation, cannot grow |
| Per-effect JS heap (Servo) | 64MB | Servo's SpiderMonkey GC enforced |
| Per-effect GPU memory (wgpu) | 32MB | Texture/buffer allocation cap |
| Preview cache (all effects) | 256MB | LRU eviction of animated previews |
| Total effect engine memory | 512MB | Hard cap, daemon refuses to load more effects |

### 8.5 Sandboxing

**Servo path (HTML effects):**
- **Network:** Disabled. Effects cannot make HTTP requests, WebSocket connections, or any network I/O. CSP: `connect-src 'none'`
- **Filesystem:** No access. `<input type="file">` disabled. No IndexedDB. No LocalStorage (or limited to 1MB)
- **Clipboard:** Disabled
- **Camera/Microphone:** Disabled (audio comes from the engine, not the browser)
- **Navigation:** Disabled. Effects cannot navigate away from their own page
- **eval():** Allowed (some effects use it). Acceptable because there's no dangerous API surface to abuse
- **Process isolation:** Servo runs in the daemon process but with constrained API surface. A misbehaving effect can cause Servo overhead but cannot escape the engine sandbox

**wgpu path (WGSL shaders):**
- **WGSL validation:** The wgpu shader compiler validates all shaders before execution. Invalid shaders are rejected at compile time
- **No unbounded loops:** WGSL does not allow infinite loops (all loops must have a provable bound or use `break`)
- **Fixed resources:** Shaders can only access the uniform buffers and textures explicitly bound by the engine
- **GPU timeout:** The OS GPU scheduler enforces execution time limits (typically 2 seconds). A shader that takes too long is killed by the driver

**Can a malicious effect crash the daemon?**
- **Servo path:** A JavaScript infinite loop will block the Servo event loop. The daemon's frame budget watchdog detects this (frame time > 100ms) and forces a page reload or effect switch. The daemon itself continues running because the render loop has a timeout
- **wgpu path:** A shader that produces invalid output (NaN, infinity) will render garbage pixels but cannot crash the GPU or daemon. The wgpu validation layer catches out-of-bounds access at compile time
- **Memory exhaustion:** Both paths have memory caps. If an effect tries to allocate beyond its limit, the allocation fails and the effect errors out

---

## 9. Persona Scenarios

### 9.1 Bliss writes a chromagram-driven WebGL shader

**Context:** Bliss wants to create a new effect where musical pitch classes drive color palette selection. The effect uses Three.js for WebGL rendering with a custom fragment shader.

**Step 1: Scaffold**
```bash
hypercolor effect new chromatic-iris --template webgl-shader
```

This creates `effects/custom/chromatic-iris/` with a TypeScript file, HTML wrapper, and `effect.toml`.

**Step 2: Author**

Bliss writes a Lightscript `WebGLEffect` class with a fragment shader that reads from the chromagram:

```typescript
@ComboboxControl('palette', {
  label: 'Palette',
  values: ['SilkCircuit', 'Warm', 'Cool', 'Neon'],
  default: 'SilkCircuit'
})
@NumberControl('chromaSmooth', {
  label: 'Chroma Smoothing',
  min: 0, max: 95, default: 70
})
export class ChromaticIris extends WebGLEffect {
  // Fragment shader reads iAudioSpectrum texture
  // and chromagram uniform to blend palette colors
  // based on which pitch classes are active
}
```

**Step 3: Develop with HMR**
```bash
hypercolor dev --effect effects/custom/chromatic-iris/
```

The dev server opens at `localhost:3420`. Bliss plays music through the system audio, and the effect responds in real-time. She tweaks the shader, saves, and the effect hot-reloads. The audio inspector shows the 12 chromagram bins updating live.

**Step 4: Test layouts**

In the dev UI, Bliss switches between layout presets: a full keyboard shows how the effect maps across 100+ keys. A 4-fan setup shows how the circular sampling looks. A single LED strip reveals that the horizontal gradient needs more contrast.

**Step 5: Share**

Bliss saves two presets ("SilkCircuit Pulse" and "Cosmic Synth"), generates a preview thumbnail, and publishes to the community repository via PR.

### 9.2 Jake installs the Gaming Pack

**Context:** Jake is a gamer who just installed Hypercolor. He wants effects that react to his gameplay audio.

**Step 1: First launch**

Jake opens the Hypercolor web UI at `localhost:9420`. The effect browser shows the built-in effects. He browses but wants more.

**Step 2: Discover**

He clicks the "Marketplace" tab and sees curated sections: "Featured," "Trending," "Gaming Essentials." The Gaming Essentials pack catches his eye -- it has 10 effects with an animated preview mosaic.

**Step 3: Install**

Jake clicks "Install Pack." The daemon downloads all 10 effects, generates previews, and registers them. A notification appears: "Gaming Essentials installed -- 10 effects ready."

**Step 4: Apply**

He selects "Fire Visualizer" from the newly installed pack. The control panel auto-generates sliders for flame sensitivity, height, and color. He cranks up sensitivity, picks a blue flame color, and starts a game. The flames dance with the game audio.

**Step 5: Save profile**

Jake saves his setup as a profile: "Gaming Night" -- Fire Visualizer on the keyboard, Spectrum Analyzer on the LED strip, Breathing on the case fans. He can switch to this profile with one click or a CLI command:
```bash
hypercolor profile "Gaming Night"
```

### 9.3 Luna creates a "Stream Starting" transition effect

**Context:** Luna is a streamer who wants her RGB to transition from her brand colors to an audio-reactive effect when she starts streaming.

**Step 1: Create the base**

Luna creates a custom effect for her brand gradient (coral pink to electric blue) using the Solid Color effect with a custom gradient preset.

**Step 2: Layer composition**

In the web UI's composition editor, she sets up a two-layer stack:
- Layer 1: Her brand gradient (Normal blend, 100% opacity)
- Layer 2: Spectrum Analyzer (Add blend, 0% opacity initially)

**Step 3: Transition trigger**

She configures a scheduled transition: when she clicks "Go Live" (or triggers via CLI/D-Bus), the composition crossfades Layer 2's opacity from 0% to 80% over 5 seconds. Her brand colors remain visible underneath the spectrum bars.

**Step 4: Automate**

Luna adds a D-Bus trigger from her streaming software (OBS). When OBS starts streaming, it fires a D-Bus signal that Hypercolor catches:
```bash
# OBS script calls:
dbus-send --dest=tech.hyperbliss.hypercolor1 \
  /tech/hyperbliss/hypercolor1 \
  tech.hyperbliss.hypercolor1.SetEffect \
  string:"luna-stream-starting"
```

**Step 5: The result**

When Luna starts her stream, her setup smoothly transitions from brand colors to reactive audio lighting. Her chat sees the shift and knows she's live.

### 9.4 Dev builds a CPU temperature effect

**Context:** A developer wants their case LEDs to indicate CPU temperature at a glance -- cool blue when idle, hot red under load.

**Step 1: Check the template**

The `canvas-basic` template is sufficient. No audio needed.

**Step 2: Author the effect**

```html
<head>
  <title>CPU Heatmap</title>
  <meta description="CPU temperature as a color gradient" />
  <meta property="coolColor" label="Cool Color" type="color" default="#0066ff" />
  <meta property="hotColor" label="Hot Color" type="color" default="#ff2200" />
  <meta property="warningTemp" label="Warning Temp (C)" type="number"
        min="50" max="100" default="80" />
</head>
```

**Step 3: Connect to telemetry**

The developer uses parameter linking to bind the effect's internal `temperature` value to the `CpuTemp` telemetry source. In the web UI's parameter panel, they right-click the color interpolation parameter and select "Link to... > CPU Temperature."

The link mapping is: source range `[30, 100]` -> target range `[0.0, 1.0]`, where 0.0 maps to `coolColor` and 1.0 maps to `hotColor`.

**Step 4: Enhance with zones**

The developer assigns the CPU Heatmap to their case fan zones only. The keyboard continues running the Matrix effect, and the LED strip runs Aurora. Three effects, three zones, one canvas.

**Step 5: Add a warning pulse**

When temperature exceeds `warningTemp`, the effect overlays a pulsing glow. This is implemented in the effect's JavaScript -- the telemetry data is accessible as a control value that the parameter link continuously updates.

---

## 10. Recommended Built-in Effect Library

These effects ship with Hypercolor and must work flawlessly on day one. They represent every category, demonstrate the engine's capabilities, and give users an immediate sense of what Hypercolor can do.

### Utility (must-ship)

| # | Effect | Category | Audio | Description |
|---|---|---|---|---|
| 1 | **Solid Color** | Utility | No | Single color fill with color picker |
| 2 | **Gradient** | Utility | No | Linear/radial gradient, 2-8 stops, configurable angle |
| 3 | **Rainbow** | Utility | No | Classic HSL sweep with speed and direction |
| 4 | **Breathing** | Utility | No | Sinusoidal brightness pulse, configurable period and color |
| 5 | **Color Cycle** | Utility | No | Smooth transitions through a customizable palette |
| 6 | **Off** | Utility | No | All LEDs dark |

### Ambient

| # | Effect | Category | Audio | Description |
|---|---|---|---|---|
| 7 | **Aurora** | Ambient | Optional | Northern lights with drift, color customization, optional audio speed modulation |
| 8 | **Lava Lamp** | Ambient, Generative | No | Metaball organic blobs with configurable color palette |
| 9 | **Ocean Depths** | Ambient | No | Deep water caustics with gentle wave motion |
| 10 | **Nebula** | Ambient, Generative | No | fBM noise cosmic clouds with slow drift |
| 11 | **Fireflies** | Ambient, Generative | No | Sparse glowing particles with organic movement |
| 12 | **Neon Shift** | Ambient, Artistic | No | Smooth cycling through neon color palette |

### Audio-Reactive

| # | Effect | Category | Audio | Description |
|---|---|---|---|---|
| 13 | **Spectrum** | Reactive | Required | Classic frequency bar visualizer with mel-scaled bands |
| 14 | **Beat Pulse** | Reactive | Required | Full-canvas flash on beat with color and decay controls |
| 15 | **Bass Wave** | Reactive | Required | Radial ripple from center driven by bass energy |
| 16 | **Fire Visualizer** | Reactive | Required | Flames driven by frequency data with fan/bar dashboard |
| 17 | **Chromatic** | Reactive | Required | Colors shift based on detected musical pitch (chromagram) |
| 18 | **VU Meter** | Reactive | Required | Classic analog VU meter with peak hold |

### Generative

| # | Effect | Category | Audio | Description |
|---|---|---|---|---|
| 19 | **Plasma** | Generative | Optional | Classic demoscene sine interference pattern |
| 20 | **Matrix** | Generative | No | Falling characters with customizable sets and color modes |
| 21 | **Voronoi Flow** | Generative | Optional | Drifting Voronoi cells with audio-reactive seed velocity |
| 22 | **Particle Storm** | Generative, Reactive | Optional | Particle system with configurable physics and audio spawning |

### Interactive

| # | Effect | Category | Audio | Description |
|---|---|---|---|---|
| 23 | **Typing Ripples** | Interactive | No | Color waves radiate from pressed keys |
| 24 | **Key Trail** | Interactive | No | Pressed keys glow and fade with configurable trail length |

### Screen-Reactive

| # | Effect | Category | Audio | Description |
|---|---|---|---|---|
| 25 | **Screen Ambience** | Screen | No | Mirrors display colors to surrounding LEDs with picture modes |

### Informational

| # | Effect | Category | Audio | Description |
|---|---|---|---|---|
| 26 | **System Monitor** | Informational | No | CPU/GPU/RAM as color temperature gradient zones |

### Artistic

| # | Effect | Category | Audio | Description |
|---|---|---|---|---|
| 27 | **SilkCircuit** | Artistic | Optional | The Hypercolor design palette: Electric Purple, Neon Cyan, Coral, Electric Yellow in motion |
| 28 | **Synthwave** | Artistic | Optional | Retrowave neon grid with optional audio-reactive intensity |
| 29 | **Cyberpunk** | Artistic, Reactive | Optional | Hot magenta and cool cyan with glitch textures |
| 30 | **Pastel Dream** | Artistic | No | Soft pastel gradient with gentle drift |

### Implementation Priority

**Phase 0 (ship with first build):** Solid Color, Gradient, Rainbow, Breathing, Off (items 1-3, 4, 6)
**Phase 1 (with audio):** Spectrum, Beat Pulse, Color Cycle (items 5, 13, 14)
**Phase 2 (with Servo):** Aurora, Matrix, Fire Visualizer, Screen Ambience, all artistic effects (items 7, 12, 16, 20, 25, 27-30)
**Phase 3 (with keyboard input):** Typing Ripples, Key Trail (items 23-24)
**Phase 4 (with telemetry):** System Monitor (item 26)

The remaining effects (Lava Lamp, Ocean Depths, Nebula, Fireflies, Bass Wave, Chromatic, VU Meter, Plasma, Voronoi Flow, Particle Storm) are filled in across phases as the engine matures. All 30 effects should be shipping by the end of Phase 3.

---

## Appendix A: Effect File Format Reference

### HTML Effect (Servo Path)

```
Required:
  <title>           Effect name
  <meta description> Effect description
  <canvas>          320x200 render target (id="exCanvas")
  <script>          Effect logic with requestAnimationFrame loop

Optional:
  <meta publisher>  Author name
  <meta property>   Control definitions (repeatable)
  <meta property="categories"> Comma-separated category tags
  <style>           CSS for the effect page

Globals injected by engine:
  window[controlId]           Current control values
  window.update()             Called when controls change
  window.engine.audio         AudioData object (updated every frame)
  window.engine.zone          Zone data (for screen capture effects)
  window.onEngineReady        Called when engine is initialized
```

### WGSL Effect (wgpu Path)

```
Required:
  effect.toml                  Metadata and control definitions
  effect.wgsl                  Fragment shader (or compute shader)

Optional:
  effect.png                   320x200 preview thumbnail
  presets/*.toml               Named parameter presets

Uniform bindings:
  @group(0) @binding(0)       Uniforms struct (time, resolution, controls)
  @group(0) @binding(1)       AudioUniforms struct
  @group(0) @binding(2)       audio_spectrum texture_1d (200 bins)
  @group(0) @binding(3)       audio_mel texture_1d (24 bins)
  @group(0) @binding(4)       audio_chromagram texture_1d (12 bins)
```

---

## Appendix B: Glossary

| Term | Definition |
|---|---|
| **Canvas** | The 320x200 RGBA pixel buffer that effects render to |
| **Spatial sampling** | Extracting LED colors from the canvas at physical LED positions |
| **Zone** | A group of LEDs belonging to one device channel (e.g., "Fan 1", "Strimer ATX") |
| **Lightscript** | TypeScript framework for LightScript-compatible effect authoring |
| **FFT** | Fast Fourier Transform -- converts audio time-domain signal to frequency domain |
| **Mel scale** | Perceptual frequency scale matching human hearing sensitivity |
| **Chromagram** | 12-bin pitch class distribution (one bin per semitone, octave-folded) |
| **Spectral flux** | Frame-to-frame difference in FFT magnitude, used for onset detection |
| **Beat phase** | Position within the current beat period (0.0 = beat just hit, 1.0 = next beat imminent) |
| **Blend mode** | Algorithm for combining two pixel layers (Normal, Add, Screen, Multiply, etc.) |
| **HMR** | Hot Module Replacement -- live-reloading effect code without restarting the engine |
| **Preset** | Named snapshot of all control parameter values for an effect |
| **Pack** | Curated collection of effects installable as a single unit |
