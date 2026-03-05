# Writing Hypercolor Effects

> The `@hypercolor/sdk` gives you one function call between your idea and a running effect.

## Quick Start

### Shader Effect (WebGL)

```typescript
import { effect } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect('Borealis', shader, {
    speed:     [1, 10, 5],
    intensity: [0, 100, 82],
    palette:   ['Northern Lights', 'SilkCircuit', 'Cyberpunk', 'Fire'],
}, {
    description: 'Aurora borealis with domain-warped fBm noise',
})
```

### Canvas Effect (Canvas2D)

```typescript
import { canvas } from '@hypercolor/sdk'

export default canvas.stateful('Bubble Garden', {
    speed: [0, 100, 10],
    size:  [1, 10, 5],
    color: '#ff0066',
}, () => {
    const bubbles = createBubbles(50)
    return (ctx, time, controls) => {
        ctx.fillStyle = '#000'
        ctx.fillRect(0, 0, ctx.canvas.width, ctx.canvas.height)
        for (const b of bubbles) {
            b.x += b.vx * (controls.speed as number) / 10
            ctx.fillStyle = controls.color as string
            ctx.beginPath()
            ctx.arc(b.x, b.y, (controls.size as number) * 4, 0, Math.PI * 2)
            ctx.fill()
        }
    }
})
```

---

## Two Effect Types

| | Shader (`effect`) | Canvas (`canvas`) |
|---|---|---|
| **Renderer** | WebGL2 fragment shader | Canvas2D JavaScript |
| **Best for** | Noise, math, GPU-heavy visuals | Particles, physics, pixel-level logic |
| **Controls become** | GLSL uniforms (auto-mapped) | `controls` object properties |
| **Import** | `import { effect } from '@hypercolor/sdk'` | `import { canvas } from '@hypercolor/sdk'` |

---

## Controls

Controls are the sliders, dropdowns, color pickers, and toggles that users adjust in the UI. You declare them as a plain object — the SDK infers the type from the value shape.

### Shape-Based Inference

| Value | Control Type | Example |
|---|---|---|
| `[min, max, default]` | Number slider | `speed: [1, 10, 5]` |
| `string[]` | Combobox (dropdown) | `palette: ['Fire', 'Ice']` |
| `'#rrggbb'` | Color picker | `color: '#ff0066'` |
| `boolean` | Toggle | `invert: false` |

The first item in a `string[]` is the default. The key name auto-derives the UI label: `warpStrength` becomes **"Warp Strength"**.

### Explicit Factory Functions

When you need custom labels, tooltips, or a non-first default, use factory functions:

```typescript
import { effect, num, combo, toggle, color, hue, text } from '@hypercolor/sdk'

export default effect('My Effect', shader, {
    // Shorthand — inferred
    speed: [1, 10, 5],

    // Explicit — full control
    density: num('Particle Density', [10, 1000], 200, {
        step: 10,
        tooltip: 'Number of particles in the field',
    }),

    palette: combo('Color Theme', ['SilkCircuit', 'Ice', 'Aurora', 'Fire'], {
        default: 'Ice',  // not the first item
    }),

    mirror: toggle('Mirror Mode', false, {
        tooltip: 'Reflect the effect horizontally',
    }),

    bgColor: color('Background', '#0d0221'),

    hueShift: hue('Hue Offset', [0, 360], 0),

    title: text('Label', 'Hello'),
})
```

### All Factory Functions

| Function | Signature | Produces |
|---|---|---|
| `num()` | `num(label, [min, max], default, opts?)` | Number slider |
| `combo()` | `combo(label, values, opts?)` | Combobox dropdown |
| `toggle()` | `toggle(label, default, opts?)` | Boolean toggle |
| `color()` | `color(label, '#hex', opts?)` | Color picker |
| `hue()` | `hue(label, [min, max], default, opts?)` | Hue wheel |
| `text()` | `text(label, default, opts?)` | Text input |

**Options common to all factories:**
- `tooltip?: string` — help text shown on hover
- `uniform?: string` — override the auto-derived GLSL uniform name

---

## Shader Effects in Detail

### File Structure

```
src/effects/my-effect/
  main.ts        # Effect declaration
  fragment.glsl  # Fragment shader
```

### How Controls Map to Uniforms

Each control key auto-maps to a GLSL uniform named `i` + PascalCase:

| Control Key | Uniform Name | GLSL Type |
|---|---|---|
| `speed` | `iSpeed` | `float` |
| `warpStrength` | `iWarpStrength` | `float` |
| `palette` | `iPalette` | `int` |
| `color` | `iColor` | `vec3` |
| `invert` | `iInvert` | `int` |

Your GLSL shader uses these uniforms alongside the built-in ones:

```glsl
#version 300 es
precision highp float;

uniform float iTime;
uniform vec2  iResolution;
uniform float iSpeed;        // auto-mapped from `speed: [1, 10, 5]`
uniform int   iPalette;      // auto-mapped from `palette: [...]`
uniform float iIntensity;    // auto-mapped from `intensity: [0, 100, 82]`

out vec4 fragColor;

void main() {
    vec2 uv = gl_FragCoord.xy / iResolution;
    float t = iTime * iSpeed;
    // ... your effect logic
    fragColor = vec4(color, 1.0);
}
```

### Built-in Uniforms

These are always available in your shader — you don't declare them:

| Uniform | Type | Description |
|---|---|---|
| `iTime` | `float` | Elapsed time in seconds |
| `iResolution` | `vec2` | Canvas size in pixels |
| `iFrame` | `int` | Frame counter |

### Magic Names

Some control key names trigger automatic behavior:

| Key Name | Magic Behavior |
|---|---|
| `speed` | Value normalized from `[min, max]` to `[0.0, 2.0]` for consistent animation speed |
| `palette` | Combobox string converted to `int` index (for GLSL palette lookup) |

### Audio-Reactive Effects

Add `audio: true` to enable audio data:

```typescript
export default effect('Spectral Fire', shader, {
    speed:     [1, 10, 6],
    intensity: [20, 100, 84],
    palette:   ['Bonfire', 'Forge', 'Spellfire'],
}, {
    description: 'Fire tongues mapped to audio frequency bands',
    audio: true,
})
```

This provides additional uniforms in your shader:

| Uniform | Type | Description |
|---|---|---|
| `iAudioBass` | `float` | Low frequency energy (0-1) |
| `iAudioMid` | `float` | Mid frequency energy (0-1) |
| `iAudioHigh` | `float` | High frequency energy (0-1) |
| `iAudioLevel` | `float` | Overall volume level (0-1) |

### Advanced: Setup and Frame Hooks

For effects that need custom uniform logic beyond auto-mapping:

```typescript
export default effect('Complex Effect', shader, {
    speed: [1, 10, 5],
}, {
    setup(ctx) {
        // Called once after WebGL program is linked
        ctx.registerUniform('uCustomMatrix', [1, 0, 0, 1])
    },
    frame(ctx, time) {
        // Called every frame — set dynamic uniforms
        const angle = time * 0.01
        ctx.setUniform('uCustomMatrix', [
            Math.cos(angle), -Math.sin(angle),
            Math.sin(angle),  Math.cos(angle),
        ])
    },
})
```

The `ShaderContext` provides:
- `ctx.controls` — current control values
- `ctx.audio` — audio data (if `audio: true`)
- `ctx.gl` — the `WebGL2RenderingContext`
- `ctx.program` — the compiled `WebGLProgram`
- `ctx.width` / `ctx.height` — canvas dimensions
- `ctx.registerUniform(name, value)` — register a new uniform
- `ctx.setUniform(name, value)` — update an existing uniform

---

## Canvas Effects in Detail

### Stateless vs Stateful

**Stateless** — draw function called directly each frame (no persistent state):

```typescript
export default canvas('Gradient', {
    hueShift: [0, 360, 0],
}, (ctx, time, controls) => {
    const w = ctx.canvas.width
    const h = ctx.canvas.height
    const hue = (time * 0.05 + (controls.hueShift as number)) % 360
    ctx.fillStyle = `hsl(${hue}, 80%, 50%)`
    ctx.fillRect(0, 0, w, h)
})
```

**Stateful** — factory returns draw function (persistent state in closure):

```typescript
export default canvas.stateful('Particles', {
    count: [10, 200, 50],
    speed: [0, 100, 30],
}, () => {
    // This runs once — initialize state here
    const particles = Array.from({ length: 50 }, () => ({
        x: Math.random() * 320,
        y: Math.random() * 200,
        vx: (Math.random() - 0.5) * 2,
        vy: (Math.random() - 0.5) * 2,
    }))

    // This runs every frame
    return (ctx, time, controls) => {
        ctx.fillStyle = 'rgba(0, 0, 0, 0.1)'  // trail effect
        ctx.fillRect(0, 0, ctx.canvas.width, ctx.canvas.height)

        const speed = (controls.speed as number) / 50
        for (const p of particles) {
            p.x += p.vx * speed
            p.y += p.vy * speed
            ctx.fillStyle = '#80ffea'
            ctx.beginPath()
            ctx.arc(p.x, p.y, 3, 0, Math.PI * 2)
            ctx.fill()
        }
    }
})
```

Use `canvas.stateful()` when your effect needs to remember things between frames (particle positions, previous values, buffers).

### Draw Function Signature

```typescript
(ctx: CanvasRenderingContext2D, time: number, controls: Record<string, unknown>) => void
```

- `ctx` — standard Canvas2D context, already sized to the canvas
- `time` — elapsed time in milliseconds (use for animation)
- `controls` — current values of all declared controls

### DeltaTime

If you need frame-independent animation, compute deltaTime in your factory:

```typescript
export default canvas.stateful('Smooth', { speed: [1, 10, 5] }, () => {
    let lastTime = 0

    return (ctx, time, controls) => {
        const dt = lastTime ? (time - lastTime) / 1000 : 0.016
        lastTime = time

        // Use dt for frame-independent movement
        position += velocity * dt * (controls.speed as number)
    }
})
```

### Palette as a Function (Canvas)

When you name a control `palette` with shared palette names, the SDK gives you a **function** in the controls object instead of a string:

```typescript
export default canvas.stateful('Nebula', {
    palette: ['SilkCircuit', 'Cyberpunk', 'Aurora', 'Fire'],
}, () => {
    return (ctx, time, controls) => {
        const palette = controls.palette as (t: number, alpha?: number) => string

        // Sample the palette at any point [0, 1]
        ctx.fillStyle = palette(0.0)            // first color
        ctx.fillStyle = palette(0.5)            // middle color
        ctx.fillStyle = palette(1.0)            // last color
        ctx.fillStyle = palette(0.5, 0.6)       // with alpha

        // Animate through the palette
        const t = (Math.sin(time * 0.001) + 1) / 2
        ctx.fillStyle = palette(t)
    }
})
```

The palette function uses Oklab color space interpolation for perceptually smooth gradients. Values wrap: `palette(1.2)` is equivalent to `palette(0.2)`.

**Important:** If your dropdown options are NOT shared palette names (e.g. custom scene names), use a different key name like `colorMode` or `scene` to avoid the palette magic.

---

## Options

Both `effect()` and `canvas()` accept an options object as the last argument:

### Shader Effect Options

```typescript
interface EffectFnOptions {
    description?: string    // Shown in effect browser
    author?: string         // Default: 'Hypercolor'
    audio?: boolean         // Enable audio uniforms
    vertexShader?: string   // Custom vertex shader
    setup?: (ctx) => void   // Called once after GL init
    frame?: (ctx, t) => void // Called every frame
}
```

### Canvas Effect Options

```typescript
interface CanvasFnOptions {
    description?: string    // Shown in effect browser
    author?: string         // Default: 'Hypercolor'
    width?: number          // Canvas width (default: 320)
    height?: number         // Canvas height (default: 200)
}
```

---

## Building Effects

Effects compile from TypeScript to standalone HTML files that the Hypercolor runtime (Servo) renders.

```bash
# Build a single effect
bun scripts/build-effect.ts src/effects/borealis/main.ts

# Build all effects
bun scripts/build-effect.ts --all

# Custom output directory
bun scripts/build-effect.ts --out ./my-builds src/effects/borealis/main.ts
```

The build pipeline:
1. Extracts metadata (name, description, controls) without executing the effect
2. Generates `<meta>` tags for each control
3. Bundles TypeScript + GLSL into a single IIFE
4. Wraps everything in an HTML file with canvas element

Output goes to `effects/evolved/` by default.

### HTML Output Format

```html
<head>
  <title>Borealis</title>
  <meta description="Aurora borealis with domain-warped fBm noise"/>
  <meta publisher="Hypercolor"/>
  <meta property="speed" label="Speed" type="number" min="1" max="10" default="5"/>
  <meta property="palette" label="Palette" type="combobox"
        default="Northern Lights" values="Northern Lights,SilkCircuit,..."/>
</head>
<body>
  <canvas id="exCanvas" width="320" height="200"></canvas>
  <script>/* bundled JS */</script>
</body>
```

This is the contract between effects and the Hypercolor runtime. You can also write effects as plain HTML without the SDK — the runtime only cares about this format.

---

## Real Examples

### Minimal Shader (11 lines)

```typescript
import { effect } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect('Nebula Drift', shader, {
    speed:        [1, 10, 5],
    cloudDensity: [10, 100, 60],
    warpStrength: [0, 100, 50],
    starField:    [0, 100, 40],
    palette:      ['SilkCircuit', 'Cyberpunk', 'Aurora', 'Fire', 'Vaporwave'],
}, {
    description: 'Animated domain-warped fBm nebula clouds with multi-layer star field',
})
```

### Shader with Non-First Defaults

```typescript
import { effect, combo } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect('Synth Horizon', shader, {
    scene:      ['Roller Grid', 'Arcade Carpet', 'Laser Lanes'],
    speed:      [1, 10, 5],
    gridDensity: [10, 100, 62],
    glow:       [10, 100, 72],
    palette:    combo('Palette', ['SilkCircuit', 'Rink Pop', 'Arcade Heat', 'Ice Neon', 'Midnight'], { default: 'Rink Pop' }),
    colorMode:  combo('Color Mode', ['Static', 'Color Cycle', 'Mono Neon'], { default: 'Color Cycle' }),
    cycleSpeed: [0, 100, 44],
}, {
    description: 'Retro roller-rink geometry with neon horizon scenes',
})
```

### Audio-Reactive Shader

```typescript
import { effect } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect('Spectral Fire', shader, {
    speed:       [1, 10, 6],
    flameHeight: [20, 100, 78],
    turbulence:  [0, 100, 62],
    intensity:   [20, 100, 84],
    palette:     ['Bonfire', 'Forge', 'Spellfire', 'Sulfur', 'Ashfall'],
    emberAmount: [0, 100, 60],
    scene:       ['Classic', 'Inferno', 'Torch', 'Wildfire'],
}, {
    description: 'Layered fire tongues with embers and optional audio lift',
    audio: true,
})
```

### Shader with Color Controls

```typescript
import { effect } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect('Plasma Engine', shader, {
    bgColor: '#03020c',
    color1:  '#94ff4f',
    color2:  '#2cc8ff',
    color3:  '#ff4fd8',
    speed:   [1, 10, 5],
    bloom:   [0, 100, 68],
    spread:  [0, 100, 54],
    density: [10, 100, 60],
}, {
    description: 'Dual-flow particle field with additive sparks',
})
```

---

## Migration from Decorator API

The old decorator/class pattern still works — both APIs coexist. But the new API is dramatically simpler.

### Before (old decorator pattern — 86+ lines)

```typescript
import { Effect, NumberControl, ComboboxControl, WebGLEffect } from '@hypercolor/sdk'
import shader from './fragment.glsl'

interface Controls {
    speed: number
    intensity: number
    palette: string
}

@Effect({ name: 'Borealis', description: '...' })
class Borealis extends WebGLEffect<Controls> {
    @NumberControl({ label: 'Speed', min: 1, max: 10, default: 5 })
    speed!: number

    @NumberControl({ label: 'Intensity', min: 0, max: 100, default: 82 })
    intensity!: number

    @ComboboxControl({ label: 'Palette', values: [...], default: 'Northern Lights' })
    palette!: string

    protected createUniforms(): Record<string, number | number[]> {
        return {
            iSpeed: normalizeSpeed(this.speed),
            iIntensity: this.intensity,
            iPalette: comboboxValueToIndex(this.palette, [...]),
        }
    }

    protected updateUniforms(): Record<string, number | number[]> {
        return this.createUniforms()
    }

    // ... getFragmentShader, getVertexShader, etc.
}
```

### After (new declarative API — 13 lines)

```typescript
import { effect } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect('Borealis', shader, {
    speed:     [1, 10, 5],
    intensity: [0, 100, 82],
    palette:   ['Northern Lights', 'SilkCircuit', 'Cyberpunk', 'Fire'],
}, {
    description: 'Aurora borealis with domain-warped fBm noise',
})
```

Everything the old pattern did manually — uniform mapping, control reading, normalization, metadata extraction — is now automatic.

---

## API Reference

### `effect(name, shader, controls, options?)`

Creates a WebGL shader effect.

| Parameter | Type | Description |
|---|---|---|
| `name` | `string` | Display name |
| `shader` | `string` | GLSL fragment shader source |
| `controls` | `ControlMap` | Controls object (shorthand or factory) |
| `options?` | `EffectFnOptions` | Description, audio, hooks |

### `canvas(name, controls, drawFn, options?)`

Creates a stateless canvas effect (draw function called every frame).

| Parameter | Type | Description |
|---|---|---|
| `name` | `string` | Display name |
| `controls` | `ControlMap` | Controls object |
| `drawFn` | `DrawFn` | `(ctx, time, controls) => void` |
| `options?` | `CanvasFnOptions` | Description, dimensions |

### `canvas.stateful(name, controls, factory, options?)`

Creates a stateful canvas effect (factory returns draw function).

| Parameter | Type | Description |
|---|---|---|
| `name` | `string` | Display name |
| `controls` | `ControlMap` | Controls object |
| `factory` | `FactoryFn` | `() => DrawFn` |
| `options?` | `CanvasFnOptions` | Description, dimensions |

### Control Factories

| Function | Parameters | Description |
|---|---|---|
| `num(label, [min, max], default, opts?)` | `step?, tooltip?, uniform?` | Number slider |
| `combo(label, values, opts?)` | `default?, tooltip?, uniform?` | Dropdown |
| `toggle(label, default, opts?)` | `tooltip?, uniform?` | On/off toggle |
| `color(label, '#hex', opts?)` | `tooltip?, uniform?` | Color picker |
| `hue(label, [min, max], default, opts?)` | `tooltip?, uniform?` | Hue wheel |
| `text(label, default, opts?)` | `tooltip?, uniform?` | Text input |
