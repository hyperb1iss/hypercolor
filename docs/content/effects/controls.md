+++
title = "Controls reference"
description = "The full Hypercolor SDK controls API: shorthand inference, every factory, groups, the speed magic, palettes, presets, and build-time meta."
weight = 40
template = "page.html"
+++

Controls are what a user adjusts on a live effect: sliders, dropdowns, color pickers, toggles, font menus, sensor pickers, viewport rectangles. The SDK gives you a shorthand for quick declarations and explicit factories for anything that needs a tooltip, a group, a custom default, or a non-numeric type.

Every factory returns a `ControlSpec` that both `canvas()` and `effect()` consume. The SDK builds the UI, serializes the declarations into HTML `<meta>` tags at build time, and resolves the live values before handing them to your render function. You declare controls once; the studio panel, the daemon UI, the validator, and the runtime all read from that single declaration.

For the authoring path these controls plug into, see [TypeScript canvas effects](@/effects/typescript-effects.md). For where they show up to a user, see [Effects and controls in the studio](@/studio/effects-and-controls.md).

## Control types at a glance

There are nine control types. The discriminated union lives in `controls/specs.ts`; this table is its public face. Pick the factory in the right-hand column, or reach for the inferred shorthand where one exists.

| Type | Factory | Runtime value | Inferred shorthand |
| --- | --- | --- | --- |
| Number slider | `num` | `number` | `[min, max, default]` |
| Combobox | `combo`, `paletteControl`, `font` | `string` (or `PaletteFn` when tagged) | `readonly string[]` |
| Toggle | `toggle` | `boolean` | `boolean` |
| Color | `color` | `'#rrggbb'` | `'#rrggbb'` |
| Hue | `hue` | `number` (degrees) | none |
| Text | `text` | `string` | non-`#` string |
| Sensor | `sensor` | sensor label `string` | none |
| Rectangle | `rect` | `{ x, y, width, height }` | none |
| Asset | `asset` | media reference `string` | none |

Hue, sensor, rect, and asset have no literal shape, so they exist only as factories. Everything else has both a shorthand and a factory.

## Shorthand inference

For quick drafts, declare a control by value shape alone and let the SDK infer the type. Inference is shape-based: the SDK looks at the literal you wrote and picks a control type.

| Value shape | Inferred control | Example |
| --- | --- | --- |
| `[min, max, default]` | Number slider | `speed: [1, 10, 5]` |
| `[min, max, default, step]` | Number slider with a step | `density: [0, 1000, 200, 10]` |
| `readonly string[]` | Combobox (first is default) | `palette: ['Fire', 'Ice']` |
| `boolean` | Toggle | `mirror: false` |
| `'#rrggbb'` | Color picker | `tint: '#80ffea'` |
| `'plain string'` | Text field | `label: 'HELLO'` |
| `number` (bare) | Number slider, range 0-100 | `opacity: 80` |

Two details from the inference rules are worth pinning down. A bare number such as `opacity: 80` becomes a slider with range `0-100` whose default is the value you wrote. A string is a color only when it starts with `#`; every other string becomes a text field.

Inferred controls derive their label from the key by splitting camelCase into words: `trailLength` becomes `"Trail Length"`, `edgeGlow` becomes `"Edge Glow"`. Anything the inference table can't classify (an empty array, a `null`, an object) throws a build error naming the offending key, so a bad shorthand fails loud instead of rendering a broken control:

```text
Cannot infer control type for "weird". Expected [min, max, default],
string[], boolean, '#hex', string, or number. Got: {}
```

Shorthand is great for throwaway effects and prototypes. Upgrade to factories the moment you need a tooltip, a group, a non-first default, or any type that has no literal shape: hue, sensor, asset, rect.

## Factory functions

Import the factories you need from `@hypercolor/sdk`:

```typescript
import {
  asset,
  color,
  combo,
  font,
  hue,
  num,
  paletteControl,
  rect,
  sensor,
  text,
  toggle,
} from "@hypercolor/sdk";
```

### `num(label, [min, max], default, options?)`

Number slider.

```typescript
density: num("Particle Density", [10, 1000], 200, {
  step: 10,
  tooltip: "How many particles live in the field",
  group: "Simulation",
});
```

Options:

- `step` — slider increment
- `tooltip` — hover tooltip text
- `group` — UI grouping label
- `normalize` — `'speed'`, `'percentage'`, or `'none'`. Applies an internal normalization before the value reaches your render function or the shader uniform.
- `uniform` — override the auto-derived `iSpeed`-style GLSL uniform name

### `combo(label, values, options?)`

Dropdown selector. The runtime value is the selected string.

```typescript
shape: combo("Shape", ["Circle", "Square", "Hexagon"], {
  default: "Hexagon",
  group: "Geometry",
});
```

Options: `default` (otherwise the first value wins), `tooltip`, `group`, `uniform`.

{% callout(type="info") %}
**The palette function comes from `paletteControl`, not the key name.** A plain `combo('Palette', ['A', 'B', 'C'])`, or the shorthand `palette: ['A', 'B', 'C']`, stays a raw string at runtime. The automatic `PaletteFn` (canvas) and integer index (shaders) come from the dedicated `paletteControl()` factory, which tags the spec with `meta.palette = true`. The key name `palette` is not magic. If you have a plain combobox and want the function, call `createPaletteFn(name)` inside your draw. See [Palettes](@/effects/palettes.md).
{% end %}

### `paletteControl(label, values, options?)`

Palette picker. Returns a `combobox` spec tagged with `meta.palette = true`. That flag is what drives both the studio's palette-picker UI and the palette-function conversion at runtime, described under [Palette controls](#palette-controls). Same option set as `combo()`: `default`, `tooltip`, `group`, `uniform`.

```typescript
palette: paletteControl("Palette", ["Aurora", "Fire", "Ocean"], {
  group: "Color",
});
```

### `color(label, default, options?)`

Color picker, produces a hex string.

```typescript
tint: color("Tint", "#80ffea", { group: "Color" });
```

Canvas effects receive `'#rrggbb'` strings. Shader effects receive a `vec3` uniform with components in the `0.0–1.0` range.

### `toggle(label, default, options?)`

Boolean toggle.

```typescript
mirror: toggle("Mirror Mode", false, {
  tooltip: "Reflect the effect horizontally",
});
```

In shaders, booleans become integer uniforms (`0` or `1`).

### `hue(label, [min, max], default, options?)`

Hue-angle slider, typically `[0, 360]`. Semantically a `num`, but the UI draws a hue-gradient track so the value reads as a position on the color wheel.

```typescript
baseHue: hue("Base Hue", [0, 360], 270, { group: "Color" });
```

### `text(label, default, options?)`

Single-line text input. Useful for [display faces](@/effects/display-faces.md), labels, and any effect that renders typed content.

```typescript
message: text("Display Text", "HYPERCOLOR", { group: "Content" });
```

### `font(label, defaultFamily, options?)`

Font family picker. This is syntactic sugar over `combo()` — it produces a `combobox` spec whose values are font family names. The face runtime loads the selected family before the first render, so the glyphs are ready by frame one.

```typescript
family: font("Family", "JetBrains Mono", { group: "Typography" });
```

If you omit `families`, the SDK uses a curated, LED-legible default list: `JetBrains Mono`, `Inter`, `Orbitron`, `Audiowide`, `Bebas Neue`, `DM Sans`, `Exo 2`, `Roboto Condensed`, `Rajdhani`, `Space Mono`, `Space Grotesk`, and `Sora`. When you pass your own `families` list and the `defaultFamily` is not already in it, the factory prepends the default automatically so the menu always opens on a valid selection.

Options:

- `families` — override the curated list with your own families
- `tooltip`, `group`

### `sensor(label, default, options?)`

Sensor picker. The user chooses from the live system sensors the daemon exposes (CPU temperature, GPU load, network throughput, and so on). The runtime value is the **sensor label string**, not a reading — pass it to the engine's sensor API to get the current value each frame.

```typescript
import { face, sensor } from "@hypercolor/sdk";

export default face("Temperature", {
  target: sensor("Sensor", "cpu_temp"),
});
```

Inside the render function, resolve the label to a live reading. `getSensorValue` hands back a `{ value, min, max, unit }` object (or `null` when the sensor is unavailable); the live number is `reading.value`:

```typescript
const label = controls.target as string;
const reading = engine.getSensorValue(label);
const current = reading?.value ?? 0;
```

Sensor controls are the backbone of data-driven [display faces](@/effects/display-faces.md): a gauge that tracks `gpu_load`, a bar that breathes with `cpu_temp`. Options: `tooltip`, `group`.

### `asset(label, mediaKind?, options?)`

User media picker. The user supplies an image, video, or Lottie animation, and the control hands your effect a reference to it. The `mediaKind` argument scopes what the picker accepts.

```typescript
backdrop: asset("Backdrop", "image", { group: "Content" });
```

`mediaKind` is one of `'any'` (the default), `'image'`, `'video'`, or `'lottie'`. Options: `default` (a starting asset reference, an empty string when omitted), `tooltip`, `group`. Asset controls are the one control type exempt from the shader uniform-binding check, since user media has no scalar uniform form.

### `rect(label, default, options?)`

Interactive viewport rectangle. The user drags a rectangle over a live preview, and the value is `{ x, y, width, height }` in normalized `[0, 1]` coordinates — resolution-independent, like every spatial value in Hypercolor.

```typescript
viewport: rect(
  "Viewport",
  { x: 0.1, y: 0.1, width: 0.8, height: 0.8 },
  {
    aspectLock: 16 / 9,
    preview: "screen",
  },
);
```

Options:

- `aspectLock` — lock the aspect ratio while the user drags
- `preview` — `'screen'`, `'web'`, or `'canvas'`, picking the backdrop the picker draws the rectangle over
- `tooltip`, `group`

## Groups

Grouping keeps the panel readable once an effect carries more than three or four controls. The `group` option on any factory files the control under a collapsible header:

```typescript
{
  bloom: num('Bloom', [0, 100], 62, { group: 'Color' }),
  nucleus: num('Nucleus', [0, 100], 55, { group: 'Color' }),
  palette: paletteControl('Palette', ['Aurora', 'Fire', 'Ocean'], { group: 'Color' }),

  rotation: num('Rotation', [-10, 10], 4, { group: 'Motion' }),
  speed: num('Speed', [1, 10], 5, { group: 'Motion' }),

  petals: combo('Petals', ['6', '8', '12'], { group: 'Shape' }),
}
```

Group names are arbitrary strings. Pick them from the domain of the effect (Color, Motion, Shape) rather than generic labels like "Settings".

## Reading values

Controls reach your render function as a `Record<string, unknown>`. Cast each to its expected type:

```typescript
const speed = controls.speed as number;
const palette = controls.palette as string;
const mirror = controls.mirror as boolean;
const viewport = controls.viewport as {
  x: number;
  y: number;
  width: number;
  height: number;
};
```

The runtime re-polls control values during playback (every 0.1s, or immediately when the panel marks them dirty), so a slider drag updates the next frame without a reload. Most values arrive raw; the exceptions are a control keyed `speed` and any `paletteControl`, described next.

## Auto-normalized speed

One control name triggers automatic normalization. The SDK's `MAGIC_NAMES` table holds exactly one entry: `speed`. When a control is keyed `speed` and you haven't set an explicit `normalize` option, its value runs through `normalizeSpeed()` — `max(0.2, (speed / 5) ** 1.5)`, a multiplier in the `0.2–3.0` range — before your function or the shader sees it, so a slider value of `5` maps to a `1.0` time multiplier. In shaders it surfaces as `uniform float iSpeed`. No other key is normalized automatically.

To opt out, either rename the key (for example `speedMult`) and normalize the value yourself, or set an explicit `normalize: 'none'` on the factory. The `normalize` option on `num` also lets you apply `'speed'` or `'percentage'` normalization to a differently-named control on purpose. Normalization keys off the resolved hint, which is your explicit option first and the `MAGIC_NAMES` lookup only as a fallback.

{% callout(type="tip") %}
`'percentage'` normalization maps a `0–200` slider to a `0–2` multiplier via `normalizePercentage()` (`max(0.01, value / 100)`), so `100` reads as `1.0`. It is the right hint for "intensity" or "scale" controls you want centered on unity.
{% end %}

## Palette controls

Palette conversion is a separate, opt-in mechanism that has nothing to do with the control's key name. A combobox carries the palette behavior only when its spec is tagged `meta.palette = true`, which is what the `paletteControl()` factory does. For a tagged palette control, the runtime replaces the selected string with a `PaletteFn` in canvas effects and an integer index (`uniform int iPalette`) in shaders.

| Mechanism | Canvas behavior | Shader behavior |
| --- | --- | --- |
| `speed` key (no explicit normalize) | Normalized through `normalizeSpeed()` | Normalized, exposed as `uniform float iSpeed` |
| `paletteControl()` (`meta.palette`) | Replaced with a `PaletteFn` | Replaced with index, exposed as `uniform int iPalette` |

A plain `combo()`, the shorthand `palette: ['A', 'B', 'C']`, or any other untagged combobox leaves the value as a plain string. Recover a palette function with `createPaletteFn(name)` in your draw. See [Palettes](@/effects/palettes.md) for the registry and the Oklab sampling internals.

Uniform names for shader effects derive from the control key as `i` + PascalCase: `trailLength` becomes `iTrailLength`, `edgeGlow` becomes `iEdgeGlow`. Override with the `uniform` option. See [GLSL shader effects](@/effects/glsl-effects.md) for the full uniform contract, including the build-time check that every non-asset control has a matching `i<Key>` uniform.

## Presets

Presets live in the options object passed to `canvas()` / `effect()` / `face()`, not in the controls map. Each preset is a named bundle of control values:

```typescript
export default canvas("Prism Choir", controls, draw, {
  presets: [
    {
      name: "Rose Window",
      description: "Twelve petals, slow rotation, generous bloom",
      controls: {
        bloom: 70,
        nucleus: 55,
        palette: "SilkCircuit",
        petals: "12",
        rotation: 3,
      },
    },
    {
      name: "Hex Choir",
      description: "Six-petal mode with a steady hexagonal pulse",
      controls: {
        bloom: 55,
        nucleus: 60,
        palette: "Aurora",
        petals: "6",
        rotation: 2,
      },
    },
  ],
});
```

Aim to set every control in every preset. Partial presets are allowed, but users expect a preset to fully reset the effect, so leaving controls unset leads to surprising carry-over from whatever was selected before. Preset names render as buttons in the studio and the daemon UI.

## Build-time metadata

On build, the SDK serializes every control declaration into HTML `<meta>` tags that the daemon parses directly. You never hand-write these, but reading the output helps when a control isn't showing up the way you expect:

```html
<meta property="speed" label="Speed" type="number" min="1" max="10" default="5" group="Motion" />
<meta property="palette" label="Palette" type="combobox" values="Aurora,Fire,Ocean" default="Aurora" group="Color" />
<meta property="backdrop" label="Backdrop" type="asset" default="" group="Content" media-kind="image" />
<meta preset="Rose Window" preset-description="Twelve petals" preset-controls='{"bloom":70,"palette":"SilkCircuit"}' />
```

Each control becomes one `<meta property=… type=… />` tag carrying whatever attributes it declared: `min`/`max`/`step` for numbers, `values` for comboboxes, `aspectLock`/`preview` for rectangles, `media-kind` for assets, plus the shared `label`, `default`, `tooltip`, and `group`. A `rect` default serializes as the comma-joined `"x,y,width,height"` string. Each preset becomes one `<meta preset=… preset-controls='{json}' />` tag.

On the daemon side these tags deserialize into the effect's `controls` array and `presets`, which is what the REST API hands back. `GET /api/v1/effects/{id}` returns those declarations, and the live control values for the active effect are exposed through the effects domain alongside a controls-version token you can echo for optimistic updates. See the [REST reference](@/api/rest.md) for the full effect surface.

Run the validator on the built artifact before installing. It parses these tags and flags missing fields, duplicate control ids, invalid ranges, malformed preset JSON, unknown control types, and unknown media kinds, before the daemon ever loads the artifact. (The shader uniform-binding check is a separate, earlier gate: the build step itself fails if a non-asset control has no matching `i<Key>` uniform, so a built artifact has already cleared that one.)

```bash
bunx hypercolor validate dist/*.html
```

See [Dev workflow](@/effects/dev-workflow.md) for the build, validate, and install loop and every CLI flag.
