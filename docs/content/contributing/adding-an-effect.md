+++
title = "Adding an effect"
description = "Authoring, review criteria, and submission for built-in and core effects."
weight = 20
+++

Hypercolor ships two categories of effects: 11 **native built-ins** compiled directly into `hypercolor-core`, and a large library of **HTML effects** built from the TypeScript SDK. This page covers both contribution paths: when to choose each, how to implement it, and what the review checklist looks for.

If you want to write an effect for personal use or a community release, start with the [TypeScript SDK authoring guide](@/effects/creating-effects.md) instead. This page is for contributors targeting `main`.

---

## Which path is right?

| Path | Where it lives | Toolchain | When to use it |
|---|---|---|---|
| **Native Rust** | `crates/hypercolor-core/src/effect/builtin/` | Rust | Always-available fallbacks, diagnostics, utility patterns, audio-reactive primitives that need tight timing |
| **HTML / TypeScript SDK** | `sdk/src/effects/<name>/` | Bun + TypeScript | Visual effects, generative art, WebGL shaders, anything that leans on the browser canvas API |

The 11 native built-ins (`solid_color`, `gradient`, `rainbow`, `breathing`, `audio_pulse`, `color_wave`, `color_zones`, `screen_cast`, `media_player`, `calibration`, and `web_viewport`, which is Servo-only) exist because they must work even when Servo is not compiled in. New native effects should meet that same bar. Everything else belongs in the SDK.

{% callout(type="info") %}
There is no runnable wgpu/GPU shader lane in Hypercolor. `EffectSource::Shader` is reserved. GLSL effects run as WebGL2 inside Servo's renderer, not as native GPU compute. Frame wgpu shaders as future work and never suggest authors target that path.
{% end %}

---

## Native Rust effects

### File layout

Each native built-in lives in its own submodule under `crates/hypercolor-core/src/effect/builtin/`:

```
crates/hypercolor-core/src/effect/builtin/
├── mod.rs           # registration, create_builtin_renderer()
├── common.rs        # shared control + preset constructors
├── breathing.rs     # example: a complete built-in
└── your_effect.rs   # your new effect goes here
```

### Implement the EffectRenderer trait

Every native effect is a struct that implements `EffectRenderer` from `crates/hypercolor-core/src/effect/traits.rs`. The trait is `Send` but not `Sync`, so keep that in mind if the effect holds non-`Sync` state.

The two methods you must implement are `init` (called once on activation) and `render_into` (called once per frame):

```rust
use hypercolor_types::canvas::{Canvas, RgbaF32};
use hypercolor_types::effect::{
    ControlDefinition, ControlValue, EffectCategory, EffectMetadata, EffectSource,
    PresetTemplate,
};
use crate::effect::traits::{EffectRenderer, FrameInput, prepare_target_canvas};
use super::common::{builtin_effect_id, slider_control};

pub struct YourEffectRenderer {
    speed: f32,
    brightness: f32,
}

impl YourEffectRenderer {
    #[must_use]
    pub fn new() -> Self {
        Self { speed: 1.0, brightness: 1.0 }
    }
}

impl Default for YourEffectRenderer {
    fn default() -> Self { Self::new() }
}

impl EffectRenderer for YourEffectRenderer {
    fn init(&mut self, _metadata: &EffectMetadata) -> anyhow::Result<()> {
        Ok(())
    }

    fn render_into(&mut self, input: &FrameInput<'_>, canvas: &mut Canvas) -> anyhow::Result<()> {
        prepare_target_canvas(canvas, input.canvas_width, input.canvas_height);
        // input.time_secs and input.delta_secs carry timing;
        // input.audio carries audio analysis when audio_reactive = true.
        // Fill canvas pixels here.
        Ok(())
    }

    fn set_control(&mut self, name: &str, value: &ControlValue) {
        match name {
            "speed" => {
                if let Some(v) = value.as_f32() { self.speed = v.max(0.1); }
            }
            "brightness" => {
                if let Some(v) = value.as_f32() { self.brightness = v.clamp(0.0, 1.0); }
            }
            _ => {}
        }
    }

    fn destroy(&mut self) {}
}
```

Key constraints:

- `render_into` returns `anyhow::Result`, so use `?`, never `unwrap()`.
- Always call `prepare_target_canvas` at the top of `render_into` so the canvas resizes correctly when the daemon config changes. Never hardcode dimensions.
- Math goes in linear RGB or Oklab; final pixel values go to the canvas in sRGB. Use `RgbaF32::to_srgba()` for the conversion.
- `unsafe_code` is forbidden across the workspace. No unsafe blocks.

### Write a metadata constructor

The registry learns about an effect through its `metadata()` function. Use the helpers in `common.rs` for controls and presets. `slider_control` takes `(id, name, default, min, max, step, group, tooltip)`:

```rust
use std::path::PathBuf;
use hypercolor_types::effect::{EffectCategory, EffectMetadata, EffectSource};
use super::common::{builtin_effect_id, slider_control};

pub(super) fn metadata() -> EffectMetadata {
    EffectMetadata {
        id: builtin_effect_id("your_effect"),
        name: "Your Effect".into(),
        author: "Your Name".into(),
        version: "0.1.0".into(),
        description: "One-sentence description of what it does.".into(),
        category: EffectCategory::Ambient,
        tags: vec!["your-effect".into()],
        controls: vec![
            slider_control("speed", "Speed", 1.0, 0.1, 10.0, 0.1, "Motion", "Animation rate."),
            slider_control("brightness", "Brightness", 1.0, 0.0, 1.0, 0.01, "Output", "Output brightness."),
        ],
        presets: vec![],
        audio_reactive: false,
        screen_reactive: false,
        source: EffectSource::Native {
            path: PathBuf::from("builtin/your_effect"),
        },
        license: Some("Apache-2.0".into()),
    }
}
```

### Register the effect

Edit `crates/hypercolor-core/src/effect/builtin/mod.rs` in three places:

1. Declare the module:

```rust
mod your_effect;
```

2. Re-export the renderer:

```rust
pub use self::your_effect::YourEffectRenderer;
```

3. Add to `builtin_metadata()` and `create_builtin_renderer()`:

```rust
// in builtin_metadata():
your_effect::metadata(),

// in create_builtin_renderer():
"your_effect" => Some(Box::new(YourEffectRenderer::new())),
```

### Write tests

Tests go in a `tests/` directory, not inline `#[cfg(test)]` blocks. The whole workspace follows this rule. Name the file `your_effect_tests.rs` and cover at minimum:

- Canvas is non-empty after `render_into()`.
- Every declared control ID is handled in `set_control()` without panicking.
- `render_into()` returns `Ok` for a zero `delta_secs` frame.

---

## HTML / TypeScript effects

SDK effects live under `sdk/src/effects/<name>/`. The build pipeline compiles them to self-contained HTML files that the daemon renders via Servo's WebGL2 context.

{% callout(type="warning") %}
`@hypercolor/sdk` is pre-release and not yet on npm. The monorepo wires it via a `file:` dependency. The `bun create @hypercolor/effect` scaffold flow described in [spec 31](@/effects/creating-effects.md) is the post-publish target, not yet available externally.
{% end %}

### Build an effect in the monorepo

```bash
# Build all SDK effects -> effects/hypercolor/*.html
just effects-build

# Build a single effect by name
just effect-build <name>

# SDK dev mode: package watchers with HMR (no preview server)
just sdk-dev
```

`effects/hypercolor/` is generated build output and is gitignored. Never hand-edit those files; make changes in `sdk/src/effects/` and regenerate.

### File layout

```
sdk/src/effects/
└── your-effect/
    ├── main.ts          # canvas() or effect() declaration
    └── fragment.glsl    # GLSL-only effects; omit for canvas effects
```

Display faces (`face()` declarations for HUDs, clocks, and sensor readouts) live in their own tree under `sdk/src/faces/<name>/` and build via `just faces-build`.

### Canvas effect starter

```typescript
import { canvas, num, combo } from "@hypercolor/sdk";

export default canvas(
  "Your Effect",
  {
    speed: num("Speed", [1, 10], 5, { group: "Motion" }),
    palette: combo("Palette", ["Aurora", "Fire", "Ocean"], { group: "Color" }),
    brightness: num("Brightness", [0, 100], 80, { group: "Color" }),
  },
  (ctx, time, controls) => {
    const { width, height } = ctx.canvas; // read every frame, never hardcode
    const speed = controls.speed ?? 5;
    const brightness = (controls.brightness ?? 80) / 100;
    // draw to ctx here
  },
  {
    description: "One-sentence description.",
    author: "Your Name",
    presets: [
      {
        name: "Default",
        controls: { speed: 5, palette: "Aurora", brightness: 80 },
      },
    ],
  },
);
```

### GLSL shader effect starter

GLSL effects run inside Servo's WebGL2 renderer. Import the shader as a string via the `.glsl` text loader (configured in `bunfig.toml`):

```typescript
import { effect, num } from "@hypercolor/sdk";
import shader from "./fragment.glsl";

export default effect(
  "Your Shader Effect",
  shader,
  {
    speed: num("Speed", [1, 10], 5, { group: "Motion" }),
  },
  { description: "Description." },
);
```

```glsl
#version 300 es
precision highp float;

out vec4 fragColor;
uniform float iTime;
uniform vec2 iResolution;
uniform float iSpeed;   // maps to "speed" control

void main() {
    vec2 uv = gl_FragCoord.xy / iResolution.xy;
    fragColor = vec4(uv, sin(iTime * iSpeed) * 0.5 + 0.5, 1.0);
}
```

Uniform naming: `iTime`, `iResolution`, `iMouse`, `iAudioBass`, `iAudioMid`, `iAudioTreble`, and `iAudioBeatPulse` are injected automatically. Per-control uniforms are the control ID prefixed with `i` and converted to camelCase (`speed` → `iSpeed`). See the [GLSL effects guide](@/effects/glsl-effects.md) for the full uniform catalog.

---

## Review checklist

The effect-reviewer agent (`.agents/agents/effect-reviewer/`) runs this checklist. Familiarize yourself before submitting, because the issues caught here are the ones that look fine on a monitor but fail on hardware.

### Color quality

- **Saturation.** Vivid areas should be 85-100% saturation. Hues in the 20-70% range look muddy on LEDs.
- **Blowout.** For vivid colors, `min(R,G,B) / max(R,G,B)` must be below 0.3. White channels wash out LED color.
- **Yellow and orange.** Pure yellow `(255,255,0)` reads as white on most LEDs. Use gold `(255,190,0)` or amber `(255,140,0)` instead. Flag any hue in the 30-90 range for hardware testing.
- **HSL lightness cap.** Above 60% lightness washes to white. Stay below it.
- **Gradient interpolation.** Interpolate in Oklab or Oklch, never raw sRGB. The SDK's `samplePalette()` does this for you. Raw sRGB midpoints desaturate visibly on hardware.

### Animation quality

- **Delta-time.** Animations must use `deltaTime` or `performance.now()` diffs, never fixed increments. Fixed increments produce hardware-dependent speeds.
- **Trail technique.** Background alpha clears between 0.05 and 0.40 for motion trails. Below 0.05 ghosts; above 0.40 flickers.
- **Minimum transition duration.** No visible transition under 200ms. Below that threshold it reads as flicker on LEDs.
- **Easing.** Organic motion uses sinusoidal easing, not linear.

### Audio reactivity (if applicable)

- **Beat flash anti-pattern.** Beat energy should drive movement (zoom, rotation, acceleration), not raw brightness spikes.
- **Exponential decay.** Use exponential decay (multiplier ~0.85 per frame) for audio levels, not instant on/off.
- **RMS over peak.** Use `rms_level` for overall loudness. Raw peak is too spiky on LED hardware.
- **Band grouping.** Group frequency content into bass/mid/treble or mel bands. Raw FFT bins alias into noise.

### Composition

- **Darkness is part of the design.** 30-50% of LEDs should be off or dim. Fully lit rigs lose contrast and look flat.
- **Color count.** 1-3 coordinated colors. Rainbow-everything reads as noise on hardware even when it looks fine on a monitor.
- **Spatial frequency.** Patterns should span at least 10 LEDs. Below that threshold they alias to noise on most strip/matrix hardware.

### Technical: HTML effects

- **Canvas resolution.** Effects must read `ctx.canvas.width` and `ctx.canvas.height` every frame. Effects ported from the legacy 320x200 SDK grid must use `scaleContext(ctx.canvas, { width: 320, height: 200 })` from `@hypercolor/sdk`. The daemon renders at 640x480 by default.
- **Control meta tags.** Every control must have `id`, `label`, `type`, `default`, and appropriate `min`/`max`.
- **Preset control IDs.** JSON in `preset-controls` attributes must match actual declared control IDs.
- **No blocking.** No synchronous operations or heavy allocations in the draw loop.
- **`audio-reactive` declared.** Set `<meta audio-reactive="true" />` explicitly rather than relying on content heuristics.

### Technical: native Rust effects

- **Linear color math.** Math in linear RGB or Oklab; output converted to sRGB for the canvas.
- **Control dispatch.** `set_control()` must handle every control ID declared in `metadata()`.
- **Canvas sizing.** Use `Canvas::new(input.canvas_width, input.canvas_height)` for any scratch canvas; never hardcode dimensions. Call `prepare_target_canvas` at the top of `render_into`.
- **Error handling.** `render_into` returns `anyhow::Result`. No `unwrap()` anywhere.

---

## Submission

### For native effects (Rust)

1. Run `just verify`. This covers fmt, clippy (`-D warnings`), and all workspace tests. The workspace convention forbids inline `#[cfg(test)]` blocks; use `tests/<name>_tests.rs`.
2. Run `just test-crate hypercolor-core` to confirm your tests pass in isolation.
3. Open a pull request against `main`. Prefix the title: `feat(builtin): <effect name>`. Include a short description of the visual output and why it belongs as a compiled-in effect rather than an SDK HTML effect.

### For HTML / TypeScript effects

1. Build: `just effect-build your-effect`. Confirm the file appears under `effects/hypercolor/your-effect.html`.
2. Validate: `bunx hypercolor validate effects/hypercolor/your-effect.html`. Fix any errors; address warnings before submitting.
3. Install locally and activate for a visual check: `bunx hypercolor install effects/hypercolor/your-effect.html --daemon`, then `hypercolor effects activate your-effect`.
4. Open a pull request against `main`. Prefix the title: `feat(effects): <effect name>`. Include a GIF or screenshot of the effect running. If you used audio reactivity, note what input source you tested against.

### What reviewers look for

- The effect passes the full review checklist above.
- It demonstrates something not already covered by the existing catalog. Browse the [effects section](@/effects/_index.md) and the shipped SDK effects under `sdk/src/effects/` before submitting to avoid near-duplicates.
- Controls are meaningful and the defaults produce a visually coherent result at first launch.
- Presets are named descriptively, not just "Default 1/2/3".
- The effect name is title-cased and distinct from existing effects.
- Native effects include tests; HTML effects include at least one preset.

{% callout(type="tip") %}
Run the effect-reviewer agent against your work before opening a PR. It catches the color science and animation issues that are hardest to see on a monitor but obvious on hardware.
{% end %}
