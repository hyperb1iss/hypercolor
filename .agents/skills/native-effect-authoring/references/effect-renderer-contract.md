# Effect Renderer Contract Details

Annotated patterns from existing Hypercolor native effects.

## AudioPulse: Audio Reactivity Template

The canonical audio-reactive effect. It paints an RMS-driven ambient floor and
spawns a radial ring from the canvas center on every detected beat. Beat energy
drives ring *motion*, never a canvas-wide brightness add.

```rust
/// A single ring expanding outward from the canvas center.
struct RadialWave {
    /// Normalized radius. 0.0 at the center, 1.0 at the farthest corner.
    radius: f32,
}

pub struct AudioPulseRenderer {
    base_color: [f32; 4],   // Linear RGBA ambient floor
    peak_color: [f32; 4],   // Linear RGBA reached at peak RMS
    sensitivity: f32,       // Multiplier on rms_level
    wave_speed: f32,        // Normalized radii per second
    wave_width: f32,        // Ring half-width, fraction of the canvas radius
    beat_decay_secs: f32,   // ~95% decay time of the beat envelope
    brightness: f32,        // Master brightness scalar
    waves: Vec<RadialWave>, // Live rings, capped at MAX_WAVES = 12
    beat_energy: f32,       // Decaying envelope, drives motion only
}
```

### Beat Envelope Pattern

Decay against elapsed time, not against frames:

```rust
/// `beat_decay_secs` is the ~95% decay time (three time constants),
/// so `tau = beat_decay_secs / 3`.
fn decay_beat_energy(&mut self, delta_secs: f32) {
    if self.beat_decay_secs <= 1e-4 {
        self.beat_energy = 0.0;
        return;
    }
    let tau = self.beat_decay_secs / 3.0;
    self.beat_energy *= (-delta_secs / tau).exp();
    if self.beat_energy < 1e-3 {
        self.beat_energy = 0.0;
    }
}
```

```rust
fn render_into(
    &mut self,
    input: &FrameInput<'_>,
    canvas: &mut Canvas,
) -> anyhow::Result<()> {
    prepare_target_canvas(canvas, input.canvas_width, input.canvas_height);

    let delta = input.delta_secs.max(0.0);
    if input.audio.beat_detected {
        self.beat_energy = 1.0;
        self.spawn_wave();
    } else {
        self.decay_beat_energy(delta);
    }

    // Beat energy surges the rings outward faster and widens them.
    // It never brightens the canvas.
    let beat_surge = 1.0 + self.beat_energy * 0.8;
    for wave in &mut self.waves {
        wave.radius += self.wave_speed * beat_surge * delta;
    }
    self.waves.retain(|w| w.radius < 2.0);

    let base = LinearRgba::new(self.base_color[0], self.base_color[1],
                               self.base_color[2], self.base_color[3]);
    let peak = LinearRgba::new(self.peak_color[0], self.peak_color[1],
                               self.peak_color[2], self.peak_color[3]);

    // RMS drives the ambient floor; rings wash toward a boosted accent.
    let rms_t = (input.audio.rms_level * self.sensitivity).clamp(0.0, 1.0);
    let ambient = base.lerp(peak, rms_t);
    let half_width = self.wave_width.max(1e-4) * (1.0 + self.beat_energy * 0.5);

    // ... per-pixel ring accumulation, then write the encoded pixel.
    Ok(())
}
```

Per pixel, accumulate the contribution of every live ring by distance:

```rust
let dist_norm = (dx * dx + dy * dy).sqrt() / half_diag;

let mut wave_accum = 0.0_f32;
for wave in &self.waves {
    let age_fade = (1.0 - wave.radius * 0.5).clamp(0.0, 1.0);
    if age_fade <= 0.0 {
        continue;
    }
    let ring_dist = (dist_norm - wave.radius).abs();
    if ring_dist < half_width {
        let falloff = 1.0 - (ring_dist / half_width);
        wave_accum += age_fade * falloff * falloff;
    }
}
let wave_t = wave_accum.clamp(0.0, 1.0);
```

**Key insight**: a per-frame multiplier such as `beat_flash *= 0.85` decays at a
rate that depends entirely on frame rate, so the same effect reads as a snap at
20 FPS and a smear at 60 FPS. Hypercolor's render loop shifts tiers at runtime,
so a per-frame multiplier changes the look mid-playback with no control input.
Express the tail as a duration instead and convert: `tau` is the decay time
constant, and `energy *= (-delta_secs / tau).exp()` is frame-rate independent.

**Key insight**: `beat_energy` feeds `beat_surge` (ring speed) and `half_width`
(ring thickness), never the color mix. Lerping the whole canvas toward white on
beat is the anti-pattern the skill forbids, and the engine test
`audio_pulse_beat_does_not_brighten_far_field` asserts that a beat leaves the
far corner byte-identical.

### Color Control Value Handling

Color controls arrive as `ControlValue::ColorLinear(LinearRgba)` in linear
RGBA. The picker works in sRGB hex and converts with
`LinearRgba::from_hex_srgb` before it sends, so both the wire and the renderer
see linear light.

Do all math in linear space. Convert to sRGB only at the final `canvas.fill()` / `canvas.set_pixel()` / byte-slice write.

## ColorWave: Spatial Animation Template

Traveling wavefronts over a retained framebuffer, which is what makes fade
trails possible:

```rust
fn render_into(
    &mut self,
    input: &FrameInput<'_>,
    canvas: &mut Canvas,
) -> anyhow::Result<()> {
    if self.last_size != (input.canvas_width, input.canvas_height) {
        self.last_size = (input.canvas_width, input.canvas_height);
        self.reset_state();
    }

    // Reuse last frame's pixels when the size still matches so trails persist.
    if let Some(previous) = self.framebuffer.take()
        && previous.width() == input.canvas_width
        && previous.height() == input.canvas_height
    {
        *canvas = previous;
    } else {
        prepare_target_canvas(canvas, input.canvas_width, input.canvas_height);
        canvas.fill(self.background_fill());
    }

    self.fade_canvas(canvas);

    // Delta-driven spawning: the wave cadence is a duration, not a frame count.
    let spawn_interval = self.spawn_interval_secs();
    self.spawn_accumulator += input.delta_secs.max(0.0);
    while self.spawn_accumulator >= spawn_interval {
        self.spawn_wave(input.canvas_width, input.canvas_height);
        self.spawn_accumulator -= spawn_interval;
    }

    self.advance_waves(input.delta_secs);
    self.retain_visible_waves(input.canvas_width, input.canvas_height);
    self.draw_waves(canvas, input.time_secs, input.canvas_width, input.canvas_height);

    self.framebuffer = Some(canvas.clone());
    Ok(())
}
```

**Pattern**: three separable pieces. State resets on canvas resize, because
positions cached in pixels are meaningless at a new size. Spawning runs off an
accumulator so the wave cadence is a duration rather than a frame count. The
previous frame is retained and faded rather than cleared, which is where the
trails come from.

**Typing**: `input.time_secs` is `f64`. `draw_waves` takes it as `f64` and
narrows only where it needs to; a helper typed `f32` needs an explicit cast at
the call site.

For bulk pixel writes, take `canvas.as_rgba_bytes_mut()` once and index by
`row_offset + x * BYTES_PER_PIXEL` rather than calling `set_pixel` per pixel.

## Gradient: Multi-Stop Interpolation

The builtin gradient has no gradient-stop control. It builds up to three stops
from `color_start`, an optional `color_mid` gated by `use_mid_color` at
position `midpoint`, and `color_end`, then prepares each stop once per frame in
the space named by the `interpolation` dropdown:

```rust
enum PreparedGradientColor {
    Direct(LinearRgba),
    Smooth(Oklab),
    Vivid(Oklch),
}

impl PreparedGradientColor {
    fn interpolate(self, other: Self, t: f32) -> LinearRgba {
        match (self, other) {
            (Self::Direct(a), Self::Direct(b)) => a.lerp(b, t),
            (Self::Smooth(a), Self::Smooth(b)) => a.lerp(b, t).to_linear(),
            (Self::Vivid(a), Self::Vivid(b)) => a.lerp(b, t).to_linear(),
            _ => unreachable!("prepared stops always share the same interpolation mode"),
        }
    }
}
```

Converting each stop once, up front, keeps the per-pixel path to a single lerp
in the chosen space. Never interpolate in encoded sRGB because the midpoints
desaturate. Oklch (Vivid) preserves hue and chroma; Oklab (Smooth) blends
evenly; Direct mixes linear RGB.

`ControlValue::Gradient(Vec<GradientStop>)` exists in the control vocabulary
(`GradientStop { position: f32, color: [f32; 4] }`, linear RGBA, 2 to 8 stops)
but no builtin uses it today.

## Control Value Type Reference

Effect-facing variants:

| ControlValue Variant          | Rust Type       | Typical Use                   |
| ----------------------------- | --------------- | ----------------------------- |
| `Float(f64)`                  | f64             | Speed, sensitivity, frequency |
| `Bool(bool)`                  | bool            | Toggle features on/off        |
| `ColorLinear(LinearRgba)`     | LinearRgba      | Linear RGBA                   |
| `Gradient(Vec<GradientStop>)` | Vec             | Multi-stop color ramp         |
| `Rect(NormalizedRect)`        | NormalizedRect  | Crop and viewport regions     |
| `Enum(String)`                | String          | Named options (palette, mode) |
| `Int(i64)`                    | i64             | Discrete counts               |
| `Text(String)`                | String          | Labels, names, asset ids      |

Use `value.as_effect_f32()` for a range-checked renderer scalar; it narrows
`Float` and `Int` and returns `None` for everything else, including non-finite
floats. Match on the variant for everything else, and accept both
`ControlValue::Enum` and `ControlValue::Text` for dropdown controls, the way
`color_wave.rs` does.

## Testing Native Effects

Fixtures live at the top of `crates/hypercolor-core/tests/builtin_effect_tests.rs`:

```rust
const W: u32 = 32;
const H: u32 = 16;
static SILENCE: LazyLock<AudioData> = LazyLock::new(AudioData::silence);
static DEFAULT_INTERACTION: LazyLock<InteractionData> = LazyLock::new(InteractionData::default);
static EMPTY_SENSORS: LazyLock<SystemSnapshot> = LazyLock::new(SystemSnapshot::empty);

fn frame_with_audio(time_secs: f64, audio: &AudioData) -> FrameInput<'_> {
    FrameInput {
        time_secs,
        delta_secs: 1.0 / 60.0,
        frame_number: 0,
        audio,
        interaction: &DEFAULT_INTERACTION,
        screen: None,
        sensors: &EMPTY_SENSORS,
        sources: hypercolor_core::effect::FrameDataSources::default(),
        canvas_width: W,
        canvas_height: H,
    }
}
```

A test then reads:

```rust
#[test]
fn audio_pulse_responds_to_beat() {
    let mut r = AudioPulseRenderer::new();
    r.init(&make_metadata("audio_pulse")).expect("init");

    // Same RMS on both frames so only the beat differs.
    let mut steady = AudioData::silence();
    steady.rms_level = 0.5;
    let canvas_no_beat = r
        .render_frame(&frame_with_audio(0.0, &steady))
        .expect("render no beat");

    let mut beat_audio = AudioData::silence();
    beat_audio.beat_detected = true;
    beat_audio.rms_level = 0.5;
    let canvas_beat = r
        .render_frame(&frame_with_audio(0.0, &beat_audio))
        .expect("render with beat");

    assert_ne!(
        canvas_no_beat.get_pixel(W / 2, H / 2),
        canvas_beat.get_pixel(W / 2, H / 2),
        "a beat should spawn a ring visible at the canvas center"
    );
}
```

`render_frame` is a local extension trait over `EffectRenderer` that allocates
a `Canvas` at the input dimensions and calls `render_into`. Setting a control
goes through `apply_test_control` from `tests/support/control_renderer.rs`,
which wraps one `(ControlId, ControlValue)` pair in a `ControlDeltaBatch`:

```rust
r.apply_test_control("wave_width", &ControlValue::Float(8.0));
r.apply_test_control(
    "background_color",
    &ControlValue::linear_color([0.0, 0.0, 0.0, 1.0]),
);
```

Three things trip up hand-written fixtures:

- `AudioData` has no `Default` impl. Its three spectral vectors are fixed
  length (200 / 24 / 12), so start from `AudioData::silence()` and mutate.
- Every one of `FrameInput`'s ten fields must appear in the literal, including
  `screen`, `sensors`, and `sources`.
- Hold `AudioData`, `InteractionData`, and `SystemSnapshot` in `LazyLock`
  statics when you want a `FrameInput<'static>`; otherwise the borrow dies with
  the local.

Prefer differential assertions (same renderer, two inputs that differ in one
field) over exact pixel values. They survive palette and tuning changes and
they state the actual contract, as
`audio_pulse_beat_does_not_brighten_far_field` does when it asserts the far
corner is unchanged by a beat.
