+++
title = "Creating Effects"
description = "Build Hypercolor effects in a Bun workspace and ship them to the daemon"
weight = 1
template = "page.html"
+++

Hypercolor effects are self-contained visual programs that render into a canvas, get sampled by the spatial engine, and land on real LEDs. The fastest path is a standalone Bun workspace powered by `@hypercolor/sdk` and the `hypercolor` authoring CLI.

## Quick Start

Create a fresh workspace:

```bash
bunx create-hypercolor-effect aurora-lab
cd aurora-lab
```

That gives you:

```text
aurora-lab/
  effects/
    aurora/
      main.ts
  dist/
  package.json
  bunfig.toml
  tsconfig.json
```

Start the preview studio:

```bash
bun run dev
```

The studio opens at `http://localhost:4200` and includes:

- Live iframe preview of the selected effect
- Generated controls and preset buttons
- Canvas size presets for daemon, strip, matrix, and ring layouts
- Audio simulation controls with manual beat triggering
- LED sampling preview for strip, matrix, and ring hardware shapes

## Your First Effect

Each effect lives at `effects/<id>/main.ts` and exports one default value.

```typescript
import { canvas, color, combo, num } from '@hypercolor/sdk'

export default canvas(
    'Aurora',
    {
        glow: num('Glow', [0, 100], 74, { group: 'Atmosphere' }),
        palette: combo('Palette', ['Aurora', 'Fire', 'Ocean'], {
            group: 'Color',
        }),
        tint: color('Tint', '#80ffea'),
    },
    (ctx, time, controls) => {
        const width = ctx.canvas.width
        const height = ctx.canvas.height
        const glow = (controls.glow as number) / 100

        const gradient = ctx.createLinearGradient(0, 0, width, height)
        gradient.addColorStop(0, controls.tint as string)
        gradient.addColorStop(1, '#0a1020')

        ctx.fillStyle = gradient
        ctx.fillRect(0, 0, width, height)

        ctx.globalAlpha = 0.16 + glow * 0.24
        ctx.fillStyle = '#e135ff'
        ctx.beginPath()
        ctx.arc(
            width * (0.5 + Math.sin(time * 0.5) * 0.18),
            height * (0.45 + Math.cos(time * 0.3) * 0.1),
            Math.min(width, height) * (0.2 + glow * 0.14),
            0,
            Math.PI * 2
        )
        ctx.fill()
        ctx.globalAlpha = 1
    },
    {
        author: 'You',
        description: 'A starter canvas effect',
        presets: [
            {
                name: 'Default',
                controls: {
                    glow: 74,
                    palette: 'Aurora',
                    tint: '#80ffea',
                },
            },
            {
                name: 'Fire',
                controls: {
                    glow: 92,
                    palette: 'Fire',
                    tint: '#ff6a3d',
                },
            },
        ],
    }
)
```

Always read `ctx.canvas.width` and `ctx.canvas.height` every frame. The daemon canvas is configurable, and the studio presets intentionally bounce between aspect ratios so resolution-dependent code breaks early.

## Controls and Metadata

The SDK supports both shorthand control declarations and explicit helpers.

| Value shape | Control type | Example |
|---|---|---|
| `[min, max, default]` | Number slider | `speed: [1, 10, 5]` |
| `string[]` | Dropdown | `palette: ['Aurora', 'Fire', 'Ocean']` |
| `'#rrggbb'` | Color picker | `tint: '#80ffea'` |
| `boolean` | Toggle | `mirror: false` |

Explicit helpers add labels, groups, defaults, and tooltips:

```typescript
import { canvas, color, combo, num, toggle } from '@hypercolor/sdk'
```

Metadata matters because it becomes HTML artifact metadata and catalog data inside the daemon:

```typescript
{
    author: 'You',
    description: 'Luminous curtains of color',
    audio: true,
    presets: [...],
}
```

## Shader Effects

Fragment shader effects use the same workspace flow:

```typescript
import { effect, combo, num } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect(
    'Borealis',
    shader,
    {
        intensity: num('Intensity', [0, 100], 82),
        palette: combo('Palette', ['Aurora', 'SilkCircuit', 'Frost']),
    },
    {
        description: 'Aurora curtains with layered shader motion',
    }
)
```

The workspace `bunfig.toml` already declares `.glsl` as `text`, so `import shader from './fragment.glsl'` just works in both `bun run dev` and `bun run build`.

## Studio Workflow

The scaffolded scripts map straight to the authoring CLI:

```bash
bun run dev
bun run build
bun run validate
bun run ship
bun run ship:daemon
```

The underlying commands are:

```bash
bunx hypercolor dev
bunx hypercolor build --all
bunx hypercolor validate dist/aurora.html
bunx hypercolor install dist/aurora.html
bunx hypercolor install dist/aurora.html --daemon
```

## Build, Validate, Install

Build every effect in the workspace:

```bash
bun run build
```

Validate the generated artifacts:

```bash
bun run validate
```

Install locally with a filesystem copy:

```bash
bun run ship
```

Upload through the daemon API:

```bash
bun run ship:daemon
```

`ship` writes into the Hypercolor user effects directory. `ship:daemon` validates first, uploads via `POST /api/v1/effects/install`, and the daemon registers the effect immediately.

## Adding More Effects

Inside an existing workspace, scaffold another effect:

```bash
bunx hypercolor add ember --template canvas
bunx hypercolor add skyline --template shader --audio
```

That creates a new `effects/<id>/` directory without touching your existing effects.

## Monorepo Dogfooding

Inside the Hypercolor monorepo itself, the old `just` shortcuts still work:

```bash
just sdk-dev
just effects-build
just effect-build borealis
```

Those commands now dogfood the same Bun authoring core that standalone workspaces use.
