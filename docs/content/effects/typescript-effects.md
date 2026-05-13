+++
title = "TypeScript Effects"
description = "Write canvas effects with @hypercolor/sdk. The primary authoring path"
weight = 3
template = "page.html"
+++

TypeScript effects are the default path. You write a draw function, declare controls, and the SDK wires up the render loop, control UI, audio pipeline, and palette sampling around you.

An effect is a single module that exports a single default value produced by `canvas()`:

```typescript
import { canvas, num } from "@hypercolor/sdk";

export default canvas(
  "Gradient Sweep",
  {
    speed: num("Speed", [1, 10], 3),
  },
  (ctx, time, controls) => {
    const { width, height } = ctx.canvas;
    const speed = controls.speed as number;
    const offset = (time * speed * 0.1) % 1;

    const gradient = ctx.createLinearGradient(0, 0, width, 0);
    gradient.addColorStop(offset, "#e135ff");
    gradient.addColorStop((offset + 0.5) % 1, "#80ffea");
    ctx.fillStyle = gradient;
    ctx.fillRect(0, 0, width, height);
  },
  { description: "A drifting magenta-to-cyan gradient" },
);
```

That's it. No boilerplate, no lifecycle methods, no manifest file. The SDK reads the declaration, generates the controls UI, hooks up the render loop, and bundles the whole thing into a standalone HTML artifact when you build.

## The `canvas()` signature

```typescript
canvas(name, controls, draw, options?)
canvas.stateful(name, controls, factory, options?)
```

| Parameter  | Type                    | Purpose                                                        |
| ---------- | ----------------------- | -------------------------------------------------------------- |
| `name`     | `string`                | Display name shown in the daemon catalog and UI                |
| `controls` | `ControlMap`            | Declared control slots (see [Controls](@/effects/controls.md)) |
| `draw`     | `DrawFn` or `FactoryFn` | Per-frame render function, or a factory that returns one       |
| `options`  | `CanvasFnOptions`       | Metadata (author, description, presets, audio, designBasis)    |

`DrawFn` has the signature:

```typescript
type DrawFn = (
  ctx: CanvasRenderingContext2D,
  time: number,
  controls: Record<string, unknown>,
) => void;
```

- `ctx` is a standard Canvas2D context. Its dimensions are whatever the daemon is currently rendering at.
- `time` is elapsed seconds (not milliseconds) since the effect started.
- `controls` is a resolved map of the current control values. The SDK handles normalization, palette resolution, and combobox index mapping before your function sees it.

## Stateless vs stateful

The SDK distinguishes stateless and stateful effects purely by the render function's arity.

**Stateless** draws from the arguments alone. Pure, no closure variables, no persistent buffers. Arity is 1 or more.

```typescript
canvas("Pulse", { speed: [1, 10, 5] }, (ctx, time, controls) => {
  // draws entirely from ctx + time + controls
});
```

**Stateful** uses a factory that returns the draw function. The factory runs once when the effect starts, so anything you set up in the closure persists across frames. Arity is 0.

```typescript
canvas("Fireflies", { count: [10, 500, 120] }, () => {
  const flies = Array.from({ length: 120 }, makeFly);
  return (ctx, time, controls) => {
    for (const fly of flies) {
      updateAndDraw(ctx, fly, time, controls);
    }
  };
});
```

The SDK detects which one you wrote from the function's `.length`. If that heuristic ever fights you, use the explicit form:

```typescript
canvas.stateful("Fireflies", { count: [10, 500, 120] }, () => {
  const flies = Array.from({ length: 120 }, makeFly);
  return (ctx, time, controls) => {
    /* ... */
  };
});
```

Use stateful when you need particles, history buffers, trail accumulators, or any data that needs to live longer than one frame.

## Resolution independence

Always read `ctx.canvas.width` and `ctx.canvas.height` every frame. The daemon canvas is user-tunable in Settings → Rendering, and build/install loops should be checked against the real runtime rather than hardcoded dimensions.

```typescript
canvas("Orbit", controls, (ctx, time, controls) => {
  const { width, height } = ctx.canvas;
  const minDim = Math.min(width, height);
  const cx = width * 0.5;
  const cy = height * 0.5;
  // everything below is sized in units of minDim
});
```

For effects authored against a fixed grid (most commonly legacy 320 by 200, the legacy LightScript default), pass a `designBasis` and use `scaleContext`:

```typescript
import { canvas, scaleContext } from "@hypercolor/sdk";

export default canvas(
  "Legacy",
  controls,
  (ctx, time, controls) => {
    const s = scaleContext(ctx.canvas, { width: 320, height: 200 });
    ctx.fillRect(s.dx(20), s.dy(30), s.dw(60), s.dh(40));
  },
  { designBasis: { width: 320, height: 200 } },
);
```

## Palettes

Declare a palette control with the shorthand `palette: ['A', 'B', 'C']` and the SDK gives you a palette function automatically. Call it with `t ∈ [0, 1]` to get a CSS color string; Oklab interpolation keeps gradients perceptually smooth.

```typescript
export default canvas(
  "Ribbon",
  {
    palette: ["SilkCircuit", "Aurora", "Synthwave"],
  },
  (ctx, time, controls) => {
    const pal = controls.palette as (t: number, alpha?: number) => string;
    ctx.fillStyle = pal(0.25, 0.6);
    // ...
  },
);
```

If you need an explicit combobox with tooltips or groups, use `createPaletteFn` directly:

```typescript
import { canvas, combo, createPaletteFn } from "@hypercolor/sdk";

export default canvas.stateful(
  "Ribbon",
  {
    palette: combo("Palette", ["SilkCircuit", "Aurora", "Synthwave"], {
      group: "Color",
    }),
  },
  () => {
    let name = "";
    let pal = createPaletteFn("SilkCircuit");
    return (ctx, time, controls) => {
      const next = controls.palette as string;
      if (next !== name) {
        name = next;
        pal = createPaletteFn(name);
      }
      ctx.fillStyle = pal(0.25, 0.6);
      // ...
    };
  },
);
```

See [Palettes](@/effects/palettes.md) for the full registry and the Oklab internals.

## Audio

Canvas effects pull audio with `audio()`. Call it inside your draw function to get a fresh `AudioData` snapshot.

```typescript
import { audio, canvas } from "@hypercolor/sdk";

export default canvas(
  "Pulse",
  { speed: [1, 10, 5] },
  (ctx, time, controls) => {
    const a = audio();
    const intensity = Math.max(a.beatPulse, a.bassEnv * 0.6);
    // ...
  },
  { audio: true },
);
```

The `audio: true` flag is metadata for the daemon; it doesn't change what `audio()` returns. Set it so the catalog marks the effect as audio-reactive and the daemon wires up the audio pipeline when the effect is active.

Under no audio, every field is zero or a sensible idle value. Don't gate behavior on strict equality with zero; clamp to a floor so the effect reads in a quiet room:

```typescript
const level = Math.max(a.levelShort, 0.04);
```

The richest audio surface is the harmonic stack: `chromagram`, `harmonicHue`, `chordMood`, `dominantPitch`, `dominantPitchConfidence`, `onsetPulse`. Most effects lean on `bass` and `beatPulse` alone and miss that the SDK already does music-theory-aware analysis for free. See [Audio](@/effects/audio.md) for the full surface and idioms.

## Composite modes and trails

Canvas effects do not auto-clear between frames. That's deliberate: trail effects and history accumulators depend on it. For a clean-each-frame look, call `ctx.fillRect` over the whole canvas with an opaque background:

```typescript
ctx.fillStyle = "#04050a";
ctx.fillRect(0, 0, ctx.canvas.width, ctx.canvas.height);
```

For persistence trails, use a semi-transparent fill:

```typescript
ctx.fillStyle = "rgba(4, 5, 10, 0.22)";
ctx.fillRect(0, 0, ctx.canvas.width, ctx.canvas.height);
```

For additive glow on top of an existing canvas, switch composite modes:

```typescript
ctx.save();
ctx.globalCompositeOperation = "lighter";
drawGlowyThings(ctx);
ctx.restore();
```

Pair `'lighter'` with palette sampling for a choir-of-voices look where overlapping petals bloom cleanly rather than muddying to white.

## Presets

Presets are named control snapshots that ship with the effect. Users pick them from the catalog and the daemon applies every control in one click.

```typescript
{
    presets: [
        {
            name: 'Default',
            description: 'Balanced settings',
            controls: { speed: 5, palette: 'Aurora', bloom: 60 },
        },
        {
            name: 'Slow Burn',
            description: 'Minimal motion for ambient sets',
            controls: { speed: 1, palette: 'Ember', bloom: 30 },
        },
    ],
}
```

Every preset must set every control; partial presets are allowed but discouraged because users expect presets to fully reset the effect. Presets show up in the daemon UI with the rest of the effect metadata.

## Designing for LED hardware

Canvas effects look great on a monitor and look terrible on LEDs for predictable reasons. The two biggest mistakes:

- large bright white fills that wash out to a blinding amoeba-shaped blob
- medium-saturation colors that read as vivid on screen and as muddy soup on strips

Rules that pay off every time:

- keep at least one RGB channel at or near zero for any vivid color
- default to HSV saturation 85-100%; reserve lower saturation for deliberate desaturated looks
- use a dark floor so idle states don't wash out a room
- treat treble as sparkle, bass as structure
- draw small, bright, specific things against near-black, not big washes

[Color Science for RGB LEDs](@/effects/color-science.md) has the deeper dive.

## A worked example

The canonical small-and-creative example is a harmonic petal effect. Twelve radial beams, each bound to a chromatic pitch class, palette sampling around the Circle of Fifths, chord mood bending the petals outward on major and inward on minor.

```typescript
import {
  audio,
  canvas,
  combo,
  createPaletteFn,
  num,
  toggle,
} from "@hypercolor/sdk";
import type { PaletteFn } from "@hypercolor/sdk";

type Ring = { born: number; strength: number };

export default canvas(
  "Prism Choir",
  {
    bloom: num("Bloom", [0, 100], 62, { group: "Color" }),
    palette: combo("Palette", ["SilkCircuit", "Aurora", "Synthwave"], {
      group: "Color",
    }),
    petals: combo("Petals", ["6", "8", "12"], {
      default: "12",
      group: "Shape",
    }),
    rotation: num("Rotation", [-10, 10], 4, { group: "Motion" }),
    trails: toggle("Trails", true, { group: "Motion" }),
  },
  () => {
    let paletteName = "";
    let paletteFn: PaletteFn = createPaletteFn("SilkCircuit");
    const rings: Ring[] = [];
    let lastRing = -Infinity;

    return (ctx, time, controls) => {
      const { width, height } = ctx.canvas;
      const minDim = Math.min(width, height);
      const a = audio();

      const next = controls.palette as string;
      if (next !== paletteName) {
        paletteName = next;
        paletteFn = createPaletteFn(next);
      }
      const pal = paletteFn;

      const petalCount = Number.parseInt(controls.petals as string, 10) || 12;
      const rotation = controls.rotation as number;
      const bloom = (controls.bloom as number) / 100;
      const trails = controls.trails as boolean;

      if (trails) {
        ctx.fillStyle = "rgba(4, 2, 14, 0.22)";
      } else {
        ctx.fillStyle = "#040212";
      }
      ctx.fillRect(0, 0, width, height);

      if (a.onsetPulse > 0.55 && time - lastRing > 0.12) {
        rings.push({ born: time, strength: Math.min(a.onsetPulse, 1) });
        lastRing = time;
        if (rings.length > 6) rings.shift();
      }

      ctx.save();
      ctx.translate(width * 0.5, height * 0.5);
      ctx.rotate(time * rotation * 0.15);
      ctx.globalCompositeOperation = "lighter";
      ctx.lineCap = "round";

      const stride = 12 / petalCount;
      const hueOffset = a.harmonicHue / 360;

      for (let i = 0; i < petalCount; i++) {
        const chroma = a.chromagram[Math.floor(i * stride) % 12] ?? 0;
        const angle =
          (i / petalCount) * Math.PI * 2 +
          a.chordMood * 0.3 * Math.sin(i * 0.7);
        const length = minDim * (0.18 + chroma * 0.42 + a.beatPulse * 0.05);
        const endX = Math.cos(angle) * length;
        const endY = Math.sin(angle) * length;
        const tipT = ((i / petalCount) * 0.82 + hueOffset + time * 0.03) % 1;

        const stroke = ctx.createLinearGradient(0, 0, endX, endY);
        stroke.addColorStop(0, pal(tipT, 0));
        stroke.addColorStop(0.8, pal((tipT + 0.12) % 1, 0.78));
        stroke.addColorStop(1, pal((tipT + 0.22) % 1, 0));

        ctx.strokeStyle = stroke;
        ctx.lineWidth = 2 + chroma * 14 + a.beatPulse * 8 + bloom * 6;
        ctx.beginPath();
        ctx.moveTo(0, 0);
        ctx.lineTo(endX, endY);
        ctx.stroke();
      }
      ctx.restore();
    };
  },
  {
    audio: true,
    author: "You",
    description:
      "Twelve petals of light, each bound to a chromatic pitch class",
  },
);
```

Audio-reactive, harmonic, palette-smooth, resolution-independent, LED-safe. Ship.
