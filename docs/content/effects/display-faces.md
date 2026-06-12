+++
title = "Display Faces"
description = "Build cinematic faces for LCD displays — pump screens, Push 2, simulators"
weight = 7
+++

Faces are full-screen HTML pages for the LCDs on your rig: AIO pump
screens, the Ableton Push 2 strip, anything the daemon drives as a
display. Where an effect paints a canvas that gets sampled onto LEDs, a
face owns every pixel of a real screen — so layout, typography, and
motion all matter.

This guide builds **System Pulse** (a clock, animated metric cards, live
network throughput, and your rig's colors) from scratch. It is the face
the SDK ships as the reference implementation; the finished source lives
at `sdk/src/faces/system-pulse/main.ts`.

## The face contract

```typescript
import { face, color, sensor, palette } from '@hypercolor/sdk'

export default face('My Face', {
    accent: color('Accent', palette.neonCyan),
    cpuSensor: sensor('CPU Sensor', 'cpu_temp'),
}, {
    description: 'What it does',
    designBasis: { width: 480, height: 480 },
    net: true,          // opt into engine.net
    lighting: true,     // opt into engine.lighting
}, (ctx) => {
    // Setup: build DOM under ctx.container, draw on ctx.ctx (canvas overlay)
    return (time, controls, sensors, audio, data) => {
        // Update: runs every frame at the display's target fps
    }
})
```

The setup function runs once and returns the per-frame update function.
`ctx` carries the display truth; the update arguments carry the data.

## Display truth: ctx.display

The daemon injects a descriptor describing the physical surface before
your code runs. `ctx.display` resolves it (falling back to viewport
measurements when you preview in a normal browser):

| Field | Meaning |
|---|---|
| `shape` | `round`, `square`, `wide`, or `tall` (2:1 aspect threshold) |
| `class` | `pump-lcd`, `panel`, or `strip` — device-family layout hint |
| `circular` | physical corner clipping — the SDK masks the container for you |
| `safeArea` | largest unclipped rect; on a 480x480 round LCD that is 339x339 centered |
| `aspect` | width over height |

**Design inside the safe area on round displays.** Anything outside it is
physically cut off by the bezel.

## Layout variants

A face that is gorgeous on a 480x480 pump screen is a postage stamp on a
960x160 strip. Declare per-shape setups; the SDK picks by resolved shape:

```typescript
{
    variants: {
        wide: (ctx) => buildFace(ctx, true),
    },
},
(ctx) => buildFace(ctx, false)   // base covers round/square/tall
```

Strip composition rules that survived our two-display gate:

- Go **edge to edge** — anchor the hero element left, let atmosphere and
  rails run the full width like a letterbox frame. Never center a small
  cluster in a sea of black.
- Key type sizes off `ctx.height`, not the design basis, so a strip keeps
  readable proportions.

## Data sources

Sensors are always available. Everything else is opt-in via `face()`
options, which emit metadata the daemon uses to gate injection — faces
that don't opt in pay nothing.

| Option | Update argument | Payload |
|---|---|---|
| (always) | `sensors` | `read/normalized/formatted(label)` over live system telemetry |
| `audio: true` | `audio` | mel bands, beat, level — the full analysis frame |
| `media: true` | `data.media` | track, artist, album art data URL, eased `positionMs()` |
| `net: true` | `data.net` | `rxBps/txBps/iface` at 1 Hz |
| `lighting: true` | `data.lighting` | scene name, effect names, dominant LED colors |

Every accessor is safe-defaulted: no player means
`data.media.available()` is `false`, not a crash. **Always design the
absent state** — an idle card, a breathing glyph — never a blank screen.

## Motion that works at 15–30 fps

Faces render through Servo at a capped frame rate (default 30). All SDK
motion primitives are time-based, so they are correct at any fps:

- `Smoothed(initial, halflife)` — eased tracking for live values.
- `Transition` / `transitionOnChange` — glide on step changes (controls,
  presets) with mid-flight retargeting.
- `Spring` — organic overshoot, fixed-substep stable.
- The shared atmosphere kit (`sdk/src/faces/shared/atmosphere.ts`) —
  nebula fields, rising motes, comet rings/rails, and `entrance()` for
  staggered boot choreography.

Design slow and eased. Fast travel strobes at 15 fps; a hard-blinking
separator reads as a glitch where a breathing one reads as alive.

## Servo CSS support

Faces render in Servo, not Chrome. The pixel-proven matrix (asserted by
`servo_css_probe_tests` in hypercolor-core):

| Works | Silently ignored |
|---|---|
| flexbox (row/column/gap), transforms, `clip-path: circle()` | CSS grid layout, aspect-ratio media queries |

**Never structure a layout with CSS grid** — children render stacked
full-width with no error. Flexbox everything. Canvas gradients throw on
malformed colors; pass hex or `rgba()` strings (the SDK's `withAlpha` is
hex-only — handle `hsl()` yourself if you generate it).

## The dev loop

```bash
just face-dev system-pulse
```

This builds the face, installs it into the running daemon, creates two
simulator displays (480x480 round + 960x160 strip), assigns the face to
both, opens the Displays page, and rebuilds on every save. Save the file,
watch both previews update.

**The quality gate:** a face ships when it looks intentional on *both*
canonical displays — round and strip — with live data and in its idle
state. Screenshot both, every time.

## Controls and presets

Controls follow the effect SDK exactly (`color`, `num`, `toggle`,
`combo`, `sensor`, `font`). Two face-specific conventions:

- Ship presets. A face with 4–8 presets (including a minimal "naked"
  variant) feels designed; a face with bare controls feels like homework.
- Keep control ids stable across redesigns — assignments persist control
  values by id, so renames silently orphan user tweaks.

## Installing and sharing

A built face is one self-contained HTML file with its manifest in meta
tags — controls, presets, data-source opt-ins, the format version.

```bash
just effect-build system-pulse          # build to effects/hypercolor/
# install into a daemon (validates manifest, size, render surface):
curl -F "file=@system-pulse.html" http://localhost:9420/api/v1/effects/install
```

Re-uploading a file with the same name updates it in place — the effect
id is path-derived, so existing display assignments follow the update.
