---
name: native-effect-authoring
version: 1.0.0
description: >-
  This skill should be used when writing Rust-native effects for Hypercolor's
  native rendering path. Triggers on "native effect", "builtin effect", "Rust
  effect", "EffectRenderer", "FrameInput", "tick function", "audio reactive
  Rust", "canvas fill", "effect renderer trait", "write a new effect", "builtin
  audio pulse", "breathing effect", "color wave", or any work in
  crates/hypercolor-core/src/effect/builtin/.
---

# Native Effect Authoring

Native effects are Rust implementations of `EffectRenderer` in
`crates/hypercolor-core/src/effect/builtin/`. They render directly to Canvas
without Servo, the fastest path at about 1ms per frame.

## EffectRenderer Trait Contract

```rust
pub trait EffectRenderer: Send {
    fn init(&mut self, metadata: &EffectMetadata) -> anyhow::Result<()>;
    fn render_into(&mut self, input: &FrameInput<'_>, target: &mut Canvas)
        -> anyhow::Result<()>;
    fn initialize_controls(
        &mut self,
        revision: SetRevision,
        controls: &ControlSet,
    ) -> Result<(), ControlError>;
    fn apply_controls(&mut self, batch: &ControlDeltaBatch<'_>)
        -> Result<(), ControlError>;
    fn destroy(&mut self);
}
```

- `init`: called once after construction, receives full metadata
- `render_into`: called every frame with a reusable target canvas
- `initialize_controls`: receives the authoritative snapshot before the first frame
- `apply_controls`: receives one ordered atomic batch between frames
- `destroy`: cleanup (rarely needed for native effects)

## FrameInput: What's Available Per Frame

```rust
pub struct FrameInput<'a> {
    pub time_secs: f32,          // elapsed since activation
    pub delta_secs: f32,         // frame delta (use for animation!)
    pub frame_number: u64,       // monotonic counter
    pub audio: &'a AudioData,    // full audio analysis snapshot
    pub interaction: &'a InteractionData,
    pub canvas_width: u32,       // default 320 (configurable)
    pub canvas_height: u32,      // default 200 (configurable)
}
```

**Always use `delta_secs` for animation.** Frame rate is adaptive (10-60 FPS),
so fixed increments produce stuttery motion at lower tiers.

## AudioData Fields Catalog

Available every frame when audio input is active:

| Field               | Type       | Range      | Use For                                           |
| ------------------- | ---------- | ---------- | ------------------------------------------------- |
| `rms_level`         | f32        | 0.0-1.0    | Overall loudness                                  |
| `peak_level`        | f32        | 0.0-1.0    | Transient detection                               |
| `beat_detected`     | bool       | n/a        | Impulse on beat onset                             |
| `beat_confidence`   | f32        | 0.0-1.0    | Beat reliability                                  |
| `beat_phase`        | f32        | 0.0-1.0    | Position in beat cycle                            |
| `beat_pulse`        | f32        | 0.0-1.0    | Decaying impulse (1.0 on beat, exponential decay) |
| `bpm`               | f32        | n/a        | Estimated BPM                                     |
| `spectrum`          | Vec\<f32\> | 200 bins   | Logarithmic 20Hz-20kHz                            |
| `mel_bands`         | Vec\<f32\> | 24 bands   | Perceptual frequency bands                        |
| `chromagram`        | Vec\<f32\> | 12 classes | Pitch class energy (C, C#, D, ...)                |
| `spectral_centroid` | f32        | n/a        | Brightness (high = treble-heavy)                  |
| `spectral_flux`     | f32        | n/a        | Rate of spectral change                           |
| `onset_detected`    | bool       | n/a        | Onset (broader than beat)                         |
| `onset_pulse`       | f32        | 0.0-1.0    | Decaying onset impulse                            |

## Control Dispatch Pattern

Apply the complete batch before returning. Native renderers normally cannot
fail once values have passed control admission, so updating derived fields in
one method call is atomic from the render loop's perspective.

```rust
fn apply_controls(&mut self, batch: &ControlDeltaBatch<'_>) -> Result<(), ControlError> {
    for (control_id, value) in batch.changes {
        match control_id.as_str() {
            "base_color" => if let ControlValue::ColorLinear(c) = value {
                self.base_color = [c.r, c.g, c.b, c.a];
            },
            "sensitivity" => if let Some(v) = value.as_effect_f32() {
                self.sensitivity = v;
            },
            "palette" => if let ControlValue::Enum(s) = value {
                self.palette.clone_from(s);
            },
            _ => {}
        }
    }
    Ok(())
}
```

`ControlValue::ColorLinear` carries linear RGBA. Convert to encoded sRGB only
when writing the canvas.

## Canvas Output

`Canvas` is `Rgba` pixels in **sRGB gamma space** (u8 per channel). Default
dimensions are 640x480 (`DEFAULT_CANVAS_WIDTH/HEIGHT` constants), but always
use `input.canvas_width/height` from `FrameInput` because they are configurable.
Available operations:

- `Canvas::new(width, height)`: opaque black canvas
- `canvas.fill(rgba)`: solid fill
- `canvas.set_pixel(x, y, rgba)`: individual pixel write
- `canvas.get_pixel(x, y)`: read a pixel (returns `Rgba::BLACK` for out-of-bounds)
- `canvas.pixels()`: iterator of `[u8; 4]` chunks (read-only)
- `canvas.as_rgba_bytes()`: raw `&[u8]` slice (read-only)
- `canvas.as_rgba_bytes_mut()`: raw `&mut [u8]` slice (mutable, for bulk pixel manipulation)
- `canvas.clear()`: fill with opaque black
- `canvas.width()` / `canvas.height()`: dimensions

## Available Color Types

| Type           | Space            | Use                             |
| -------------- | ---------------- | ------------------------------- |
| `Rgba` / `Rgb` | sRGB u8          | Canvas pixels, final output     |
| `LinearRgba`   | Linear f32       | Math, blending, lerp            |
| `Oklab`        | Perceptual       | Smooth gradients                |
| `Oklch`        | Perceptual polar | Hue cycling, palette generation |

The engine provides correct sRGB transfer functions and Oklab/Oklch conversions between all types.

## Beat Flash Anti-Pattern

**Do not** map `beat_detected` directly to brightness spikes. The result is
harsh strobing that's unpleasant on LEDs. Instead, redirect beat energy to
**movement**:

- Zoom/scale pulses on beat
- Rotation speed boosts
- Wave acceleration
- Particle emission bursts

Use `beat_pulse` (decaying exponential) for smooth energy, not the binary `beat_detected`.

## Effect Lifecycle States

```
Loading → Initializing → Running → Paused → Destroying
```

`Paused` exists for crossfade transitions. The effect is alive but not actively rendering.

## Registration

New builtin renderers register in `src/effect/builtin/mod.rs` via
`create_builtin_renderer()`. Add a match arm mapping your effect's name string
to its renderer constructor. The factory in `src/effect/factory.rs` dispatches
`EffectSource::Native` effects to `create_builtin_renderer` automatically. You
only need to touch `builtin/mod.rs`.

Metadata for native effects uses `EffectSource::Native { path }`. Control
definitions go in the `EffectMetadata.controls` vec.

## Existing Builtins as Templates

| Effect       | File             | Good Template For                           |
| ------------ | ---------------- | ------------------------------------------- |
| `SolidColor` | `solid_color.rs` | Simplest possible effect                    |
| `Breathing`  | `breathing.rs`   | Time-based animation                        |
| `AudioPulse` | `audio_pulse.rs` | Audio reactivity + beat decay               |
| `ColorWave`  | `color_wave.rs`  | Spatial animation across canvas             |
| `Rainbow`    | `rainbow.rs`     | Hue cycling                                 |
| `Gradient`   | `gradient.rs`    | Multi-stop color interpolation              |
| `ColorZones` | `color_zones.rs` | Multi-zone color grid with per-zone control |

## Testing

Test in `crates/hypercolor-core/tests/`. Create a renderer, feed it mock
`FrameInput` with synthetic `AudioData`, and verify Canvas output pixels.

## Detailed References

- **`references/effect-renderer-contract.md`**: Annotated examples from
  AudioPulse and ColorWave, control value edge cases, and Canvas pixel math
