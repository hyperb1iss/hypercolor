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

Every fallible method returns `anyhow::Result<()>`; there is no dedicated
control error type. Five methods sit outside that shape, all visible in the
signatures below: `destroy`, `bind_asset_library`, and `set_display_descriptor`
return `()`, `preview_canvas` returns `Option<Canvas>`, and `render_output`
returns `anyhow::Result<EffectRenderOutput>`. Four methods are required, seven
have default bodies.

```rust
pub trait EffectRenderer: Send {
    // Required.
    fn init(&mut self, metadata: &EffectMetadata) -> anyhow::Result<()>;
    fn render_into(&mut self, input: &FrameInput<'_>, target: &mut Canvas)
        -> anyhow::Result<()>;
    fn apply_controls(&mut self, batch: &ControlDeltaBatch<'_>) -> anyhow::Result<()>;
    fn destroy(&mut self);

    // Defaulted: override only when you need them.
    fn init_with_canvas_size(
        &mut self,
        metadata: &EffectMetadata,
        canvas_width: u32,
        canvas_height: u32,
    ) -> anyhow::Result<()> { /* forwards to init */ }
    fn render_output(&mut self, input: &FrameInput<'_>)
        -> anyhow::Result<EffectRenderOutput> { /* wraps render_into in Cpu(canvas) */ }
    fn advance_output(&mut self, input: &FrameInput<'_>) -> anyhow::Result<()> { Ok(()) }
    fn initialize_controls(&mut self, controls: &ControlSet)
        -> anyhow::Result<()> { /* replays the whole set through apply_controls */ }
    fn bind_asset_library(&mut self, library: Arc<RwLock<AssetLibrary>>) {}
    fn set_display_descriptor(&mut self, descriptor: Option<DisplayDescriptor>) {}
    fn preview_canvas(&self) -> Option<Canvas> { None }
}
```

Required methods:

- `init`: called once when the effect activates, receives full metadata
- `render_into`: called every frame with a caller-owned, reused target canvas
- `apply_controls`: one ordered atomic batch of resolved changes, may arrive
  between frames
- `destroy`: cleanup (rarely needed for native effects)

Defaulted methods, and when to override:

- `init_with_canvas_size`: the renderer needs the presentation size before its
  first frame. The default ignores the size and forwards to `init`.
- `render_output` / `advance_output`: the GPU-resident output path
  (`EffectRenderOutput::Gpu`). Servo overrides these; the default allocates a
  `Canvas` and delegates to `render_into`, so CPU builtins leave them alone.
- `initialize_controls`: the default hands the complete `ControlSet` to
  `apply_controls` as resolution sequence zero, which is right for almost every
  effect. Override only when replacing derived state needs behavior distinct
  from applying an ordinary delta. `SetRevision` is never a parameter; it rides
  inside `ControlDeltaBatch`.
- `bind_asset_library`: the effect resolves uploaded media by asset id
  (`media_player.rs:189`).
- `set_display_descriptor`: the effect drives a device display and adapts to
  device truth (shape, safe area, fps).
- `preview_canvas`: the effect renders a higher-resolution source image the
  control panel can preview on demand (`web_viewport.rs:357`).

## FrameInput: What's Available Per Frame

```rust
pub struct FrameInput<'a> {
    pub time_secs: f64,          // elapsed since activation (f64, not f32)
    pub delta_secs: f32,         // frame delta (use for animation!)
    pub frame_number: u64,       // monotonic counter
    pub audio: &'a AudioData,    // full audio analysis snapshot
    pub interaction: &'a InteractionData,
    pub screen: Option<&'a Arc<ScreenBranchPublication>>,
    pub sensors: &'a SystemSnapshot,
    pub sources: FrameDataSources<'a>,
    pub canvas_width: u32,       // default 640 (configurable)
    pub canvas_height: u32,      // default 480 (configurable)
}
```

All ten fields are required when you construct one in a test. `time_secs` is
`f64`, so mixing it into `f32` phase math needs an explicit cast.

**Always use `delta_secs` for animation.** Frame rate is adaptive (10-60 FPS),
so fixed increments produce stuttery motion at lower tiers.

### screen, sensors, sources

Three input channels beyond audio and interaction:

- `screen` is the leased screen publication, `None` when no screen source is
  routed. Read CPU pixels with `screen.surface_canvas()`, which returns
  `Option<Canvas>` and yields `None` for GPU-resident publications, so a
  screen-reactive effect always needs a no-content path. `screen_cast.rs:55`
  is the reference use.
- `sensors` is a `&SystemSnapshot` shared by every renderer: `cpu_load_percent`,
  `cpu_loads`, `ram_used_percent`, `ram_used_mb`, `ram_total_mb`,
  `polled_at_ms`, the raw `components` readings, and the optional
  `cpu_temp_celsius`, `gpu_temp_celsius`, `gpu_load_percent`,
  `gpu_vram_used_mb`. It is never absent; it is `SystemSnapshot::empty()`
  before the first poll.
- `sources` is a `FrameDataSources<'a>` carrying the cadenced data feeds that
  display faces read: `media: Option<&MediaState>`, `net: Option<&NetStats>`
  (1 Hz), `lighting: Option<&LightingState>`, and `input_availability`, an
  `InputSourceAvailability { routed, healthy, fresh, degraded }` describing the
  routed interaction source independently of activity. Each `Option` stays
  `None` until its producer delivers a snapshot, including on platforms that
  have no producer at all.

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
| `spectral_centroid` | f32        | 0.0-1.0    | Brightness (high = treble-heavy)                  |
| `spectral_flux`     | f32        | 0.0-1.0    | Rate of spectral change                           |
| `onset_detected`    | bool       | n/a        | Onset (broader than beat)                         |
| `onset_pulse`       | f32        | 0.0-1.0    | Decaying onset impulse                            |

`AudioData::silence()` builds a correctly sized zero-filled snapshot; there is
no `Default` impl, because the three spectral vectors have to be allocated at
their fixed lengths. `bass()`, `mid()`, and `treble()` average the spectrum
over bins 0-39, 40-129, and 130-199, and each returns 0.0 when the spectrum is
shorter than its range.

## Control Dispatch Pattern

Apply the complete batch before returning. Native renderers normally cannot
fail once values have passed control admission, so updating derived fields in
one method call is atomic from the render loop's perspective.

```rust
fn apply_controls(&mut self, batch: &ControlDeltaBatch<'_>) -> anyhow::Result<()> {
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
- `canvas.sample(nx, ny, method)`: normalized `[0.0, 1.0]` read, clamped, where
  `method` is `SamplingMethod::{Nearest, Bilinear, Area { radius }}`
- `canvas.sample_nearest(nx, ny)` / `sample_bilinear(nx, ny)` /
  `sample_area(nx, ny, radius)`: the same three directly. Bilinear and area
  blend in linear light, so they do not desaturate midpoints.

**Call `prepare_target_canvas(target, input.canvas_width, input.canvas_height)`
first, every frame.** The helper lives in `effect::traits` and reallocates the
target only when the requested dimensions changed. Skipping it is a live bug,
not a style nit: the engine resizes the canvas at runtime, and a renderer that
writes through stale dimensions either panics on the byte slice or draws into
the wrong stride.

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

Registering a builtin takes five edits in `src/effect/builtin/mod.rs`, on top
of the new module file itself:

1. `mod my_effect;` in the module list
2. `pub use self::my_effect::MyEffectRenderer;` in the re-exports
3. A `my_effect::metadata()` entry in `builtin_metadata()`
4. A `"my_effect" => Some(Box::new(MyEffectRenderer::new()))` arm in
   `create_builtin_renderer()`
5. A row in the module-level doc table

**Step 3 is the silent failure.** Skip it and everything still compiles and the
match arm still resolves, but the effect never enters the registry, so nothing
can list or select it. Skip step 4 instead and the effect appears in listings
and then fails to instantiate with "native effect '<name>' is registered but
has no built-in renderer implementation", which at least says what is wrong.

The factory in `src/effect/factory.rs` dispatches `EffectSource::Native`
effects to `create_builtin_renderer` automatically, so nothing outside
`builtin/mod.rs` needs touching.

### Metadata

Your module's `metadata()` returns an `EffectMetadata` whose `id` is
`builtin_effect_id("my_effect")` (a stable UUID derived from the stem name) and
whose `source` is `EffectSource::Native { path: PathBuf::from("builtin/my_effect") }`.
Control definitions go in `EffectMetadata.controls`, presets in
`EffectMetadata.presets`. Build both from the shared constructors in
`builtin/common.rs` rather than filling the structs by hand: `color_control`,
`slider_control`, `toggle_control`, `dropdown_control`, `asset_control`,
`rect_control`, and `text_control` (that last one is gated on the `servo`
feature), plus `preset` and `preset_with_desc`.

**Preset names must be unique within one effect.** `PresetId::stable(name)`
derives the id from the name, and `register_builtin_effects` logs an error and
skips the *entire effect* when it finds a duplicate id, so one copy-pasted
preset name silently removes the whole effect from the registry.

## Existing Builtins as Templates

| Effect        | File              | Good Template For                                    |
| ------------- | ----------------- | ---------------------------------------------------- |
| `SolidColor`  | `solid_color.rs`  | Simplest possible effect, plus split/checker patterns |
| `Breathing`   | `breathing.rs`    | Time-based animation                                 |
| `AudioPulse`  | `audio_pulse.rs`  | Audio reactivity with time-based envelopes           |
| `ColorWave`   | `color_wave.rs`   | Retained framebuffer, spawned wavefronts, trails     |
| `Rainbow`     | `rainbow.rs`      | Hue cycling                                          |
| `Gradient`    | `gradient.rs`     | Multi-stop Oklch/Oklab interpolation                 |
| `ColorZones`  | `color_zones.rs`  | Multi-zone color grid with per-zone control          |
| `ScreenCast`  | `screen_cast.rs`  | Reading `FrameInput::screen`                         |
| `MediaPlayer` | `media_player.rs` | Asset-backed effects via `bind_asset_library`        |
| `Calibration` | `calibration.rs`  | High-contrast layout calibration patterns            |
| `WebViewport` | `web_viewport.rs` | Servo-backed page render (feature `servo`)           |

## Testing

Test in `crates/hypercolor-core/tests/builtin_effect_tests.rs`. Create a
renderer, feed it a `FrameInput` built from synthetic `AudioData`, and verify
Canvas output pixels. That file already carries the fixtures worth copying:
`SILENCE` / `DEFAULT_INTERACTION` / `EMPTY_SENSORS` as `LazyLock` statics, a
`frame()` / `frame_with_audio()` builder pair, and a `render_frame` extension
trait. Control setup goes through `apply_test_control` from
`tests/support/control_renderer.rs`, which wraps one change in a
`ControlDeltaBatch`.

Two things bite here: `AudioData` has no `Default` impl (start from
`AudioData::silence()` and mutate fields), and every one of `FrameInput`'s ten
fields has to be present in the literal, `screen`, `sensors`, and `sources`
included.

## Detailed References

- **`references/effect-renderer-contract.md`**: Annotated examples from
  AudioPulse and ColorWave, control value edge cases, and Canvas pixel math
