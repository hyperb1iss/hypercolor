+++
title = "Creating Effects"
description = "Write custom RGB effects using the Hypercolor TypeScript SDK"
weight = 1
template = "page.html"
+++

Effects are self-contained visual programs that render to a canvas. The `@hypercolor/sdk` provides a concise API for defining effects with controls, metadata, and render logic. Effects compile to single-file HTML that the daemon loads and renders headlessly.

## Effect Lifecycle

Every effect follows this flow:

1. **Init** — The SDK bootstraps a canvas, parses control definitions, and sets up the render context (Canvas 2D or WebGL)
2. **Render loop** — Your render function is called every frame (~60fps) with the current time, control values, and audio data
3. **Control updates** — When a user adjusts a slider or picks a color, the new value is injected and available on the next frame
4. **Cleanup** — When the effect is deactivated, resources are released

## Setting Up the SDK

The SDK lives in the `sdk/` directory at the project root:

```bash
just sdk-install    # Install dependencies (uses Bun)
just sdk-dev        # Start dev server with HMR
```

The dev server opens a preview environment where you can see your effect running live, adjust controls, and feed in audio.

## Your First Effect: Color Pulse

Let's write a simple effect that cycles through colors with a pulsing brightness.

Create a new file at `sdk/src/effects/color-pulse/main.ts`:

```typescript
import { canvas } from '@hypercolor/sdk'

export default canvas('Color Pulse', {
    speed: [1, 10, 5],
    saturation: [50, 100, 90],
}, (ctx, time, controls) => {
    const w = ctx.canvas.width
    const h = ctx.canvas.height
    const speed = controls.speed as number
    const sat = controls.saturation as number

    // Cycle hue over time
    const hue = (time * speed * 36) % 360

    // Pulse brightness with a sine wave
    const brightness = 50 + 30 * Math.sin(time * speed * 2)

    ctx.fillStyle = `hsl(${hue}, ${sat}%, ${brightness}%)`
    ctx.fillRect(0, 0, w, h)
}, {
    description: 'Gentle color cycling with brightness pulse',
    author: 'You',
    tags: ['ambient', 'simple'],
})
```

## Adding Controls

Controls are declared as a plain object. The SDK infers the control type from the value shape:

| Value Shape | Control Type | Example |
|---|---|---|
| `[min, max, default]` | Number slider | `speed: [1, 10, 5]` |
| `string[]` | Dropdown | `palette: ['Fire', 'Ice', 'Aurora']` |
| `'#rrggbb'` | Color picker | `color: '#ff0066'` |
| `boolean` | Toggle | `mirror: false` |

The key name auto-derives the UI label: `warpStrength` becomes **"Warp Strength"** in the control panel.

For more control over labels, tooltips, and defaults, use the explicit factory functions:

```typescript
import { canvas, num, combo, color, toggle } from '@hypercolor/sdk'

export default canvas('My Effect', {
    density: num('Particle Density', [10, 1000], 200, {
        step: 10,
        tooltip: 'Number of particles in the field',
    }),
    palette: combo('Color Theme', ['SilkCircuit', 'Ice', 'Aurora'], {
        default: 'Ice',
    }),
    bgColor: color('Background', '#0d0221'),
    mirror: toggle('Mirror Mode', false),
}, renderFunction)
```

## Shader Effects (WebGL)

For GPU-accelerated effects, write a GLSL fragment shader and use the `effect` function:

```typescript
import { effect } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect('Borealis', shader, {
    speed:     [1, 10, 5],
    intensity: [0, 100, 82],
    palette:   ['Northern Lights', 'SilkCircuit', 'Cyberpunk'],
}, {
    description: 'Aurora borealis with domain-warped fBm noise',
})
```

Numeric controls are automatically mapped to GLSL uniforms with the same name. The SDK also provides built-in uniforms:

```glsl
uniform float iTime;           // Elapsed seconds
uniform vec2 iResolution;      // Canvas size (320, 200)
uniform float iAudioLevel;     // Overall audio level (0-1)
uniform float iAudioBass;      // Bass energy (0-1)
uniform float iAudioMid;       // Mid energy (0-1)
uniform float iAudioTreble;    // Treble energy (0-1)
```

## Stateful Canvas Effects

For effects that need persistent state across frames (particle systems, physics simulations), use `canvas.stateful`:

```typescript
import { canvas } from '@hypercolor/sdk'

export default canvas.stateful('Bubble Garden', {
    speed: [0, 100, 10],
    size:  [1, 10, 5],
    color: '#ff0066',
}, () => {
    // Init: create state that persists across frames
    const bubbles = Array.from({ length: 50 }, () => ({
        x: Math.random() * 320,
        y: Math.random() * 200,
        vx: (Math.random() - 0.5) * 2,
        vy: (Math.random() - 0.5) * 2,
        r: Math.random() * 10 + 5,
    }))

    // Return the render function (called every frame)
    return (ctx, time, controls) => {
        ctx.fillStyle = '#000'
        ctx.fillRect(0, 0, 320, 200)

        for (const b of bubbles) {
            b.x += b.vx * (controls.speed as number) / 10
            b.y += b.vy * (controls.speed as number) / 10
            // Wrap around edges
            if (b.x < 0) b.x += 320
            if (b.x > 320) b.x -= 320
            if (b.y < 0) b.y += 200
            if (b.y > 200) b.y -= 200

            ctx.fillStyle = controls.color as string
            ctx.beginPath()
            ctx.arc(b.x, b.y, (controls.size as number) * b.r / 5, 0, Math.PI * 2)
            ctx.fill()
        }
    }
})
```

## Metadata

Effect metadata helps with discovery and categorization:

```typescript
export default effect('My Effect', shader, controls, {
    description: 'A brief description of what this effect does',
    author: 'Your Name',
    tags: ['audio-reactive', 'ambient', 'particles'],
    audioReactive: true,
})
```

## Building Effects

Build a single effect:

```bash
just effect-build color-pulse
```

Build all effects:

```bash
just effects-build
```

Built effects are output to `effects/hypercolor/` as single-file HTML. The daemon discovers and loads effects from this directory automatically.

{% callout(type="warning", title="Generated output") %}
The `effects/hypercolor/` directory is generated build output. Never hand-edit files there — make changes in `sdk/src/effects/` and rebuild.
{% end %}

## Testing in Preview

With `just sdk-dev` running, your effect is available in the dev preview at `http://localhost:5173`. The preview provides:

- Live rendering of your effect
- Control panel for adjusting parameters in real time
- Audio visualization when audio input is active
- Hot module replacement — save your file and see changes instantly
