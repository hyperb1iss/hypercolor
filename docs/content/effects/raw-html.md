+++
title = "Raw HTML Effects"
description = "The LightScript-compatible HTML wire format. The least-encouraged path"
weight = 5
template = "page.html"
+++

Hypercolor's HTML effect format is a straight superset of LightScript's. A self-contained HTML file with one canvas, one script tag, and a few meta tags drops straight into the daemon with zero tooling. The SDK reads this format directly, the daemon loads it without any build step, and every LightScript effect you've ever seen runs as-is.

This path is the least-encouraged for new work. You give up typed controls, palette sampling, the `AudioData` struct, and most of the SDK authoring ergonomics. Choose it for porting, for one-file oddities, and for effects that must travel without a workspace. For greenfield authoring, [TypeScript effects](@/effects/typescript-effects.md) are almost always the better call.

## The minimum viable effect

```html
<!DOCTYPE html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <meta name="hypercolor-version" content="1" />
    <title>Drifting Gradient</title>
  </head>
  <body style="margin:0;overflow:hidden;background:#000;">
    <div id="exStage" style="position:relative;overflow:hidden;background:#000;width:100vw;height:100vh;">
      <canvas id="exCanvas" style="display:block;width:100%;height:100%;"></canvas>
    </div>
    <script>
      const canvas = document.getElementById('exCanvas')
      const ctx = canvas.getContext('2d')

      function resize() {
        canvas.width = window.innerWidth
        canvas.height = window.innerHeight
      }

      function frame(time) {
        const t = time * 0.001
        const hue = (t * 40) % 360
        const gradient = ctx.createLinearGradient(0, 0, canvas.width, canvas.height)
        gradient.addColorStop(0, `hsl(${hue}, 100%, 55%)`)
        gradient.addColorStop(1, `hsl(${(hue + 70) % 360}, 100%, 35%)`)
        ctx.fillStyle = gradient
        ctx.fillRect(0, 0, canvas.width, canvas.height)

        requestAnimationFrame(frame)
      }

      resize()
      window.addEventListener('resize', resize)
      requestAnimationFrame(frame)
    </script>
  </body>
</html>
```

That's a complete, installable effect. Scaffold a workspace with `--template html` and you get exactly this.

## What the daemon requires

The daemon extracts effect metadata from HTML when it loads an artifact. The validator enforces the same rules before install. Three things are strictly required:

1. A `<meta name="hypercolor-version" content="1">` tag. This identifies the file as a Hypercolor effect and pins the wire format.
2. A render surface. For canvas effects, that's `<canvas id="exCanvas">`. For face effects, it's `<div id="faceContainer">`. The id is how the spatial sampler finds the pixels.
3. At least one `<script>` tag. The daemon loads the HTML and runs the script in a rendering context; without a script, there's nothing to animate.

Everything else is optional.

## Metadata meta tags

The daemon reads these from `<head>`:

| Tag | Purpose |
|---|---|
| `<meta name="hypercolor-version" content="1" />` | Required. Identifies the format version. |
| `<title>Effect Name</title>` | Display name in the catalog. |
| `<meta description="..." />` | One-line description. |
| `<meta publisher="..." />` | Author or publisher name. Also accepts `<meta name="author" content="...">`. |
| `<meta audio-reactive="true" />` | Marks the effect as audio-reactive so the daemon wires up the audio pipeline. |
| `<meta screen-reactive="true" />` | Marks the effect as screen-capture reactive. |
| `<meta category="Ambient" />` | Optional catalog category. |
| `<meta renderer="webgl" />` | Hint that the effect uses WebGL instead of Canvas2D. |

Attribute order doesn't matter, attribute quoting can be single or double. The parser is lenient with attributes that don't fit the standard name/content pair.

## Controls as meta tags

Controls are declared with `<meta property="...">` tags. Each property becomes a UI control that the daemon exposes and the script reads from `window` or a similar runtime shim.

```html
<meta property="speed" label="Speed" type="number" min="1" max="10" default="5" group="Motion" />
<meta property="palette" label="Palette" type="combobox" values="Aurora,Fire,Ocean" default="Aurora" group="Color" />
<meta property="tint" label="Tint" type="color" default="#80ffea" group="Color" />
<meta property="trails" label="Trails" type="boolean" default="true" group="Motion" />
```

The attribute schema:

| Attribute | Used by | Description |
|---|---|---|
| `property` | required | The runtime variable name. Becomes a global the script can read. |
| `label` | optional | Human-readable label shown in the UI. Defaults to `property`. |
| `type` | required | `number`, `combobox`, `color`, `boolean`, `hue`, `textfield`, `rect`, `sensor`. |
| `min` / `max` | `number`, `hue` | Slider bounds. |
| `step` | `number` | Slider increment. |
| `default` | all | Starting value. For `rect`, a `"x,y,w,h"` string. |
| `values` | `combobox` | Comma-separated list of options. |
| `group` | optional | UI grouping label. |
| `tooltip` | optional | Hover tooltip. |
| `aspectLock` | `rect` | Locks aspect ratio on the viewport picker. |
| `preview` | `rect` | `screen`, `web`, or `canvas` viewport preview mode. |

The script reads control values by name, either from a host-provided shim or from the global scope. In a stock LightScript-style effect that looks like:

```html
<script>
  const speed = window.speed ?? 5
  const palette = window.palette ?? 'Aurora'
  const tint = window.tint ?? '#80ffea'
  const trails = window.trails ?? true
</script>
```

The daemon injects these globals before your script runs when controls are declared. For a well-behaved effect, always provide a fallback default in case the injection hasn't landed yet.

## Presets as meta tags

Presets are named control snapshots. Each preset is a single meta tag whose `preset-controls` attribute contains a JSON object:

```html
<meta preset="Default"
      preset-description="Balanced settings"
      preset-controls='{"speed":5,"palette":"Aurora","tint":"#80ffea","trails":true}' />
<meta preset="Slow Burn"
      preset-description="Minimal motion for ambient sets"
      preset-controls='{"speed":1,"palette":"Ember","tint":"#ff6a3d","trails":false}' />
```

The outer quotes must be single quotes if the JSON value contains double quotes, or use HTML entities. The daemon falls back to an empty object if the JSON is unparseable and emits a warning.

## Audio access in raw HTML

Hypercolor injects a global `engine.audio` object into HTML effects that declare `audio-reactive="true"`. The shape is a subset of the TypeScript `AudioData` interface:

```javascript
const audio = window.engine?.audio
if (audio) {
  const level = audio.level        // 0-1 RMS
  const bass = audio.bass          // 0-1
  const beat = audio.beat          // 0 or 1 this frame
  const beatPulse = audio.beatPulse
  const freq = audio.frequency     // Float32Array(200)
}
```

This pull model works but it's less ergonomic than the TypeScript `audio()` function, and the injected object may lag the full TypeScript surface. For serious audio-reactive work, scaffold a TypeScript workspace instead.

## Designing for hardware

All the rules from [Color Science for RGB LEDs](@/effects/color-science.md) apply identically. The HTML path makes it easier to reach for `fillRect` over the whole canvas with a white gradient and call it done. That's the wrong call on LEDs. Keep at least one RGB channel near zero for any vivid color, keep saturation above 85% when you want a color to read as a color, and leave a dark floor so idle states don't wash out the room.

## Validating raw HTML

Even without a workspace, the SDK validator works on a standalone file if you have Bun and `@hypercolor/sdk` installed globally:

```bash
bunx @hypercolor/sdk validate my-effect.html
```

or from inside any workspace:

```bash
bunx hypercolor validate ../path/to/my-effect.html
```

The validator checks the required meta tag, the render surface, and the script tag, and reports metadata warnings for missing author, description, or unparseable preset JSON.

## Installing raw HTML

The install flow is the same as for TypeScript effects:

```bash
bunx hypercolor install my-effect.html             # local filesystem copy
bunx hypercolor install my-effect.html --daemon    # POST to running daemon
```

You can also drop the file directly into the user effects directory:

```bash
cp my-effect.html ~/.local/share/hypercolor/effects/user/
hypercolor effects rescan
```

The daemon picks it up on the next rescan or on startup.

## When to reach for this path

- Porting an existing LightScript effect. Paste it in, add the `hypercolor-version` meta tag, done.
- A one-off visual that you don't need to iterate on and don't want to carry a workspace for.
- Constrained environments where TypeScript isn't available.
- Testing the daemon's HTML loader or validator in isolation.

For anything else, the [TypeScript effects](@/effects/typescript-effects.md) path pays for itself in the first iteration.
