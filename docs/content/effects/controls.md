+++
title = "Controls"
description = "Sliders, dropdowns, colors, toggles, viewports. The full controls API"
weight = 6
template = "page.html"
+++

Controls are what users adjust on a live effect: sliders, dropdowns, color pickers, toggles, viewports. The SDK gives you a shorthand for quick declarations and explicit factories for anything with tooltips, groups, or custom defaults.

All control factories return a `ControlSpec` that both `canvas()` and `effect()` consume. The SDK generates the UI, bundles the declarations into HTML meta tags on build, and resolves values before handing them to your render function.

## Shorthand inference

For quick drafts, declare a control by value shape alone and let the SDK infer the type.

| Value | Control type | Example |
|---|---|---|
| `[min, max, default]` | Number slider | `speed: [1, 10, 5]` |
| `readonly string[]` | Combobox (first is default) | `palette: ['Fire', 'Ice']` |
| `'#rrggbb'` | Color picker | `tint: '#80ffea'` |
| `boolean` | Toggle | `mirror: false` |

Inferred controls derive their label from the key with camelCase split into words: `trailLength` becomes `"Trail Length"`.

Shorthand is great for throwaway effects and quick prototypes. Upgrade to factories the moment you need a tooltip, a group, or a non-first default.

## Factory functions

Import the factory from `@hypercolor/sdk`:

```typescript
import { color, combo, font, hue, num, rect, sensor, text, toggle } from '@hypercolor/sdk'
```

### `num(label, [min, max], default, options?)`

Number slider.

```typescript
density: num('Particle Density', [10, 1000], 200, {
    step: 10,
    tooltip: 'How many particles live in the field',
    group: 'Simulation',
})
```

Options:

- `step`: slider increment (default 1 for integers, continuous otherwise)
- `tooltip`: hover tooltip
- `group`: UI grouping label
- `normalize`: `'speed'`, `'percentage'`, or `'none'`. Applies an internal normalization before the value reaches your render function or the shader uniform.
- `uniform`: override the default `iSpeed`-style GLSL uniform name

### `combo(label, values, options?)`

Dropdown selector.

```typescript
palette: combo('Palette', ['SilkCircuit', 'Ice', 'Aurora'], {
    default: 'Ice',
    group: 'Color',
})
```

Options:

- `default`: override the default (otherwise the first value)
- `tooltip`, `group`, `uniform`

{% callout(type="warning", title="Palette combobox loses the auto-function") %}
The shorthand `palette: ['A', 'B', 'C']` gives canvas effects a palette function automatically. An explicit `combo('Palette', ['A', 'B', 'C'])` does not. The value stays a string. If you need the function, use `createPaletteFn` inside your draw function. See [Palettes](@/effects/palettes.md).
{% end %}

### `color(label, default, options?)`

Color picker, produces a hex string.

```typescript
tint: color('Tint', '#80ffea', { group: 'Color' })
```

The color picker returns `'#rrggbb'` strings to canvas effects and `vec3` uniforms (with 0-1 components) to shader effects.

### `toggle(label, default, options?)`

Boolean toggle.

```typescript
mirror: toggle('Mirror Mode', false, {
    tooltip: 'Reflect the effect horizontally',
})
```

In shaders, booleans become integer uniforms (`0` or `1`).

### `hue(label, [min, max], default, options?)`

Hue angle slider, typically `[0, 360]`. Semantically the same as `num`, but the UI uses a hue-gradient track.

```typescript
baseHue: hue('Base Hue', [0, 360], 270, { group: 'Color' })
```

### `text(label, default, options?)`

Single-line text input. Useful for faces, labels, and any effect that displays text.

```typescript
message: text('Display Text', 'HYPERCOLOR', { group: 'Content' })
```

### `rect(label, default, options?)`

Interactive viewport rectangle. The user drags a rectangle on top of a live preview; the value is `{ x, y, width, height }` in normalized `[0, 1]` coordinates.

```typescript
viewport: rect('Viewport', { x: 0.1, y: 0.1, width: 0.8, height: 0.8 }, {
    aspectLock: 16 / 9,
    preview: 'screen',
})
```

Options:

- `aspectLock`: lock the aspect ratio while dragging
- `preview`: `'screen'`, `'web'`, or `'canvas'` to pick the backdrop for the picker

### `sensor(label, default, options?)`

Sensor picker for face effects. The user chooses from available system sensors (CPU temperature, GPU load, network throughput, etc.). The runtime value is the sensor label string; pass it to the engine's sensor API to get a live reading.

```typescript
import { face, sensor } from '@hypercolor/sdk'

export default face('Temperature', {
    target: sensor('Sensor', 'cpu_temp'),
})
```

### `font(label, default, options?)`

Font family picker. Syntactic sugar over `combo()` with a curated list of LED-friendly font families. The face runtime loads the selected family before the first render.

```typescript
family: font('Family', 'JetBrains Mono', { group: 'Typography' })
```

Options:

- `families`: override the curated list with your own

## Groups

Grouping keeps the UI readable when a single effect has more than three or four controls. The `group` option on any factory places the control under a collapsible header:

```typescript
{
    bloom: num('Bloom', [0, 100], 62, { group: 'Color' }),
    nucleus: num('Nucleus', [0, 100], 55, { group: 'Color' }),
    palette: combo('Palette', [...], { group: 'Color' }),

    rotation: num('Rotation', [-10, 10], 4, { group: 'Motion' }),
    speed: num('Speed', [1, 10], 5, { group: 'Motion' }),

    petals: combo('Petals', [...], { group: 'Shape' }),
}
```

Group names are arbitrary. Pick them from the domain of the effect, not generic labels like "Settings".

## Reading values

Controls reach your render function as a `Record<string, unknown>`. You cast to the expected type:

```typescript
const speed = controls.speed as number
const palette = controls.palette as string
const mirror = controls.mirror as boolean
```

The SDK applies normalization before your function sees the value when the key matches a magic name:

- `speed`: normalized through `normalizeSpeed` so a slider value of `5` maps to a usable time-multiplier
- `palette` with shorthand declaration: becomes a `PaletteFn` in canvas effects, an integer index in shader effects

Every other control value arrives raw.

## Magic names

Two control keys have special behavior across the whole SDK:

| Key | Canvas behavior | Shader behavior |
|---|---|---|
| `speed` | Auto-normalized through `normalizeSpeed()` | Normalized, exposed as `uniform float iSpeed` |
| `palette` (shorthand only) | Replaced with `PaletteFn` | Replaced with selected index, exposed as `uniform int iPalette` |

If you'd rather keep your own naming, override it with the `uniform` option or use a different key (`tintPalette`, `speedMult`) and handle resolution yourself.

## Presets

Presets live in the options object, not the controls map:

```typescript
export default canvas(
    'Prism Choir',
    controls,
    draw,
    {
        presets: [
            {
                name: 'Rose Window',
                description: 'Twelve petals, slow rotation, generous bloom',
                controls: {
                    bloom: 70,
                    nucleus: 55,
                    palette: 'SilkCircuit',
                    petals: '12',
                    rotation: 3,
                },
            },
            {
                name: 'Hex Choir',
                description: 'Six-petal mode with a steady hexagonal pulse',
                controls: {
                    bloom: 55,
                    nucleus: 60,
                    palette: 'Aurora',
                    petals: '6',
                    rotation: 2,
                },
            },
        ],
    },
)
```

Every preset should set every control. Partial presets are allowed but discouraged because users expect a preset to fully reset the effect. Preset names show up as buttons in the studio and in the daemon UI.

## Build-time metadata

On build, the SDK serializes every control declaration into HTML meta tags that the daemon reads directly. You never write these by hand, but seeing the output helps when debugging metadata issues:

```html
<meta property="speed" label="Speed" type="number" min="1" max="10" default="5" group="Motion" />
<meta property="palette" label="Palette" type="combobox" values="Aurora,Fire,Ocean" default="Aurora" group="Color" />
<meta preset="Default" preset-description="Balanced" preset-controls='{"speed":5,"palette":"Aurora"}' />
```

The validator (`bun run validate`) parses these and flags missing fields, malformed JSON, and unknown types before the daemon ever sees the artifact.
