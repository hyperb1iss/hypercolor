# Effect Renderer Contract Details

Annotated patterns from existing Hypercolor native effects.

## AudioPulse: Audio Reactivity Template

The canonical audio-reactive effect. Key implementation patterns:

```rust
pub struct AudioPulseRenderer {
    base_color: [f32; 4],     // Linear RGBA — from Color control
    peak_color: [f32; 4],     // Linear RGBA
    sensitivity: f32,         // Multiplier for RMS level
    beat_decay: f32,          // 0.85 typical — exponential per-frame decay
    beat_flash: f32,          // Current decay state (0.0 to 1.0)
    brightness: f32,          // Master brightness scalar
}
```

### Beat Decay Pattern

```rust
fn tick(&mut self, input: &FrameInput<'_>) -> anyhow::Result<Canvas> {
    let mut canvas = Canvas::new(input.canvas_width, input.canvas_height);

    // RMS-driven base blend
    let rms_t = (input.audio.rms_level * self.sensitivity).clamp(0.0, 1.0);

    // Beat: spike to 1.0 on detect, then decay each frame
    if input.audio.beat_detected {
        self.beat_flash = 1.0;
    } else {
        self.beat_flash *= self.beat_decay;
    }

    let base = RgbaF32::new(self.base_color[0], self.base_color[1],
                             self.base_color[2], self.base_color[3]);
    let peak = RgbaF32::new(self.peak_color[0], self.peak_color[1],
                             self.peak_color[2], self.peak_color[3]);
    let white = RgbaF32::new(1.0, 1.0, 1.0, 1.0);

    // Compose: RMS for color blend, beat for white accent
    let rms_color = RgbaF32::lerp(&base, &peak, rms_t);
    let mut final_color = RgbaF32::lerp(&rms_color, &white, self.beat_flash * 0.6);

    // Apply brightness by scaling RGB channels directly
    final_color.r *= self.brightness;
    final_color.g *= self.brightness;
    final_color.b *= self.brightness;

    canvas.fill(final_color.to_srgba());
    Ok(canvas)
}
```

**Key insight**: `beat_decay` of 0.85 means the flash halves in ~4 frames at 30 FPS (~133ms). Adjust for desired tail length. Lower values = snappier, higher = smoother trails. Brightness is applied by direct field multiplication on `RgbaF32` -- there is no `scale_rgb` method on the type.

### Color Control Value Handling

Color controls arrive as `ControlValue::Color([f32; 4])` in **linear RGBA** (0.0-1.0 range). The UI color picker produces sRGB, which the API converts to linear before delivering to the renderer.

Do all math in linear space. Convert to sRGB only at the final `canvas.fill()` / `canvas.set_pixel()` step.

## ColorWave: Spatial Animation Template

Shows per-pixel rendering across the canvas:

```rust
fn tick(&mut self, input: &FrameInput<'_>) -> anyhow::Result<Canvas> {
    let mut canvas = Canvas::new(input.canvas_width, input.canvas_height);
    let width = input.canvas_width as f32;

    for x in 0..input.canvas_width {
        let t = x as f32 / width;
        let phase = t * self.frequency + input.time_secs * self.speed;
        let wave = (phase.sin() + 1.0) * 0.5; // normalize to 0.0-1.0

        let color = RgbaF32::lerp(&self.color_a, &self.color_b, wave);
        let srgb = color.to_srgba();

        for y in 0..input.canvas_height {
            canvas.set_pixel(x, y, srgb);
        }
    }
    Ok(canvas)
}
```

**Pattern**: Iterate columns (x), compute color per column, fill all rows (y). This is efficient because most LED layouts sample horizontally — vertical variation is secondary.

## Gradient: Multi-Stop Interpolation

Uses Oklch for perceptually uniform gradients:

```rust
// Gradient stops defined as ControlValue::Gradient(Vec<GradientStop>)
// Each stop: { position: f32 (0.0-1.0), color: [f32; 4] (linear RGBA) }

fn sample_gradient(stops: &[GradientStop], t: f32) -> RgbaF32 {
    // Find surrounding stops
    // Convert both to Oklch
    // Interpolate in Oklch space (avoids muddy midpoints)
    // Convert back to linear RGBA
}
```

Never interpolate gradients in sRGB — the midpoints desaturate. Oklch produces clean, vibrant transitions.

## Control Value Type Reference

| ControlValue Variant | Rust Type | Typical Use |
|---------------------|-----------|-------------|
| `Float(f32)` | f32 | Speed, sensitivity, frequency |
| `Boolean(bool)` | bool | Toggle features on/off |
| `Color([f32; 4])` | [f32; 4] | Linear RGBA, 0.0-1.0 |
| `Gradient(Vec<GradientStop>)` | Vec | Multi-stop color ramp |
| `Enum(String)` | String | Named options (palette, mode) |
| `Integer(i32)` | i32 | Discrete counts |
| `Text(String)` | String | Labels, names |

Use `value.as_f32()` for safe Float extraction. Match on the variant for everything else.

## Testing Native Effects

```rust
#[test]
fn audio_pulse_fills_canvas_with_blended_color() {
    let mut renderer = AudioPulseRenderer::new();
    renderer.set_control("base_color", &ControlValue::Color([1.0, 0.0, 0.0, 1.0]));
    renderer.set_control("peak_color", &ControlValue::Color([0.0, 0.0, 1.0, 1.0]));

    let audio = AudioData {
        rms_level: 0.5,
        beat_detected: false,
        ..AudioData::default()
    };
    let input = FrameInput {
        time_secs: 1.0,
        delta_secs: 0.033,
        frame_number: 30,
        audio: &audio,
        interaction: &InteractionData::default(),
        canvas_width: 320,
        canvas_height: 200,
    };

    let canvas = renderer.tick(&input).unwrap();

    // At 50% RMS, color should be midpoint blend
    let pixel = canvas.get_pixel(0, 0);
    assert!(pixel.r > 0 && pixel.b > 0); // both channels present
    assert!(pixel.g < 10); // no green in red+blue blend
}
```

Create mock `AudioData` and `FrameInput` to test rendering without daemon or audio input. Verify pixel values match expected blends.
