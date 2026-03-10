//! Tests for built-in native effect renderers.
//!
//! Verifies initialization, frame production, control responsiveness,
//! audio reactivity, and full lifecycle for every built-in renderer.

use std::path::PathBuf;
use std::sync::LazyLock;

use hypercolor_core::effect::builtin::{
    AudioPulseRenderer, BreathingRenderer, ColorWaveRenderer, GradientRenderer, RainbowRenderer,
    SolidColorRenderer, create_builtin_renderer, register_builtin_effects,
};
use hypercolor_core::effect::{EffectRegistry, EffectRenderer, FrameInput};
use hypercolor_core::input::InteractionData;
use hypercolor_types::audio::AudioData;
use hypercolor_types::canvas::{Canvas, Rgba};
use hypercolor_types::effect::{
    ControlValue, EffectCategory, EffectId, EffectMetadata, EffectSource,
};
use uuid::Uuid;

// ── Helpers ─────────────────────────────────────────────────────────────────

const W: u32 = 32;
const H: u32 = 16;
static SILENCE: LazyLock<AudioData> = LazyLock::new(AudioData::silence);
static DEFAULT_INTERACTION: LazyLock<InteractionData> = LazyLock::new(InteractionData::default);

fn make_metadata(name: &str) -> EffectMetadata {
    EffectMetadata {
        id: EffectId::new(Uuid::now_v7()),
        name: name.into(),
        author: "test".into(),
        version: "0.1.0".into(),
        description: "test effect".into(),
        category: EffectCategory::Ambient,
        tags: vec![],
        controls: Vec::new(),
        audio_reactive: false,
        source: EffectSource::Native {
            path: PathBuf::from(format!("builtin/{name}")),
        },
        license: None,
    }
}

fn frame(time_secs: f32, frame_number: u64) -> FrameInput<'static> {
    FrameInput {
        time_secs,
        delta_secs: 1.0 / 60.0,
        frame_number,
        audio: &SILENCE,
        interaction: &DEFAULT_INTERACTION,
        canvas_width: W,
        canvas_height: H,
    }
}

fn frame_with_audio(time_secs: f32, audio: &AudioData) -> FrameInput<'_> {
    FrameInput {
        time_secs,
        delta_secs: 1.0 / 60.0,
        frame_number: 0,
        audio,
        interaction: &DEFAULT_INTERACTION,
        canvas_width: W,
        canvas_height: H,
    }
}

/// Returns true if any pixel in the canvas is not opaque black.
fn has_non_black_pixels(canvas: &hypercolor_types::canvas::Canvas) -> bool {
    canvas.pixels().any(|[r, g, b, _a]| r > 0 || g > 0 || b > 0)
}

/// Returns the pixel at (0, 0).
fn top_left(canvas: &hypercolor_types::canvas::Canvas) -> Rgba {
    canvas.get_pixel(0, 0)
}

fn tick_color_wave(renderer: &mut ColorWaveRenderer, frames: u64) -> Canvas {
    let mut canvas = renderer.tick(&frame(0.0, 0)).expect("tick");

    for i in 1..frames {
        #[allow(clippy::cast_precision_loss, clippy::as_conversions)]
        let t = i as f32 / 60.0;
        canvas = renderer.tick(&frame(t, i)).expect("tick");
    }

    canvas
}

fn count_non_black_pixels_in_row(canvas: &Canvas, y: u32) -> usize {
    (0..canvas.width())
        .filter(|&x| canvas.get_pixel(x, y) != Rgba::BLACK)
        .count()
}

// ── Initialization Tests ────────────────────────────────────────────────────

#[test]
fn solid_color_initializes() {
    let mut r = SolidColorRenderer::new();
    r.init(&make_metadata("solid_color"))
        .expect("init should succeed");
}

#[test]
fn gradient_initializes() {
    let mut r = GradientRenderer::new();
    r.init(&make_metadata("gradient"))
        .expect("init should succeed");
}

#[test]
fn rainbow_initializes() {
    let mut r = RainbowRenderer::new();
    r.init(&make_metadata("rainbow"))
        .expect("init should succeed");
}

#[test]
fn breathing_initializes() {
    let mut r = BreathingRenderer::new();
    r.init(&make_metadata("breathing"))
        .expect("init should succeed");
}

#[test]
fn audio_pulse_initializes() {
    let mut r = AudioPulseRenderer::new();
    r.init(&make_metadata("audio_pulse"))
        .expect("init should succeed");
}

#[test]
fn color_wave_initializes() {
    let mut r = ColorWaveRenderer::new();
    r.init(&make_metadata("color_wave"))
        .expect("init should succeed");
}

// ── Non-Black Canvas Tests ──────────────────────────────────────────────────

#[test]
fn solid_color_produces_non_black() {
    let mut r = SolidColorRenderer::new();
    r.init(&make_metadata("solid_color")).expect("init");
    let canvas = r.tick(&frame(0.0, 0)).expect("tick");
    assert!(
        has_non_black_pixels(&canvas),
        "solid color should produce non-black pixels"
    );
}

#[test]
fn gradient_produces_non_black() {
    let mut r = GradientRenderer::new();
    r.init(&make_metadata("gradient")).expect("init");
    let canvas = r.tick(&frame(0.0, 0)).expect("tick");
    assert!(
        has_non_black_pixels(&canvas),
        "gradient should produce non-black pixels"
    );
}

#[test]
fn rainbow_produces_non_black() {
    let mut r = RainbowRenderer::new();
    r.init(&make_metadata("rainbow")).expect("init");
    let canvas = r.tick(&frame(0.0, 0)).expect("tick");
    assert!(
        has_non_black_pixels(&canvas),
        "rainbow should produce non-black pixels"
    );
}

#[test]
fn breathing_produces_non_black() {
    let mut r = BreathingRenderer::new();
    r.init(&make_metadata("breathing")).expect("init");
    // At t=0, sine wave is at midpoint, so we get some brightness
    let canvas = r.tick(&frame(0.5, 30)).expect("tick");
    assert!(
        has_non_black_pixels(&canvas),
        "breathing should produce non-black pixels at t=0.5"
    );
}

#[test]
fn audio_pulse_produces_non_black_with_audio() {
    let mut r = AudioPulseRenderer::new();
    r.init(&make_metadata("audio_pulse")).expect("init");
    let mut audio = AudioData::silence();
    audio.rms_level = 0.8;
    let canvas = r.tick(&frame_with_audio(0.0, &audio)).expect("tick");
    assert!(
        has_non_black_pixels(&canvas),
        "audio pulse should produce non-black pixels with audio"
    );
}

#[test]
fn color_wave_produces_non_black() {
    let mut r = ColorWaveRenderer::new();
    r.init(&make_metadata("color_wave")).expect("init");
    r.set_control(
        "background_color",
        &ControlValue::Color([0.0, 0.0, 0.0, 1.0]),
    );
    r.set_control("wave_width", &ControlValue::Float(8.0));
    r.set_control("speed", &ControlValue::Float(100.0));
    r.set_control("trail", &ControlValue::Float(0.0));
    let canvas = tick_color_wave(&mut r, 1);
    assert!(
        has_non_black_pixels(&canvas),
        "color wave should produce non-black pixels"
    );
}

// ── Control Value Tests ─────────────────────────────────────────────────────

#[test]
fn solid_color_changes_with_control() {
    let mut r = SolidColorRenderer::new();
    r.init(&make_metadata("solid_color")).expect("init");

    // Default is white
    let canvas1 = r.tick(&frame(0.0, 0)).expect("tick");
    let p1 = top_left(&canvas1);

    // Change to pure red
    r.set_control("color", &ControlValue::Color([1.0, 0.0, 0.0, 1.0]));
    let canvas2 = r.tick(&frame(0.0, 1)).expect("tick");
    let p2 = top_left(&canvas2);

    assert_ne!(p1, p2, "changing color control should change output");
    assert_eq!(p2.r, 255);
    assert_eq!(p2.g, 0);
    assert_eq!(p2.b, 0);
}

#[test]
fn solid_color_brightness_control() {
    let mut r = SolidColorRenderer::new();
    r.init(&make_metadata("solid_color")).expect("init");

    r.set_control("color", &ControlValue::Color([1.0, 1.0, 1.0, 1.0]));
    r.set_control("brightness", &ControlValue::Float(0.5));
    let canvas = r.tick(&frame(0.0, 0)).expect("tick");
    let p = top_left(&canvas);

    // At 50% linear brightness, the sRGB-encoded canvas value is around 188.
    assert!(
        p.r > 180 && p.r < 195,
        "brightness should dim the color, got r={}",
        p.r
    );
}

#[test]
fn gradient_direction_control() {
    let mut r = GradientRenderer::new();
    r.init(&make_metadata("gradient")).expect("init");

    // Horizontal gradient: pixels across X should differ
    r.set_control("direction", &ControlValue::Enum("horizontal".into()));
    r.set_control("speed", &ControlValue::Float(0.0)); // freeze animation
    let canvas = r.tick(&frame(0.0, 0)).expect("tick");
    let left = canvas.get_pixel(0, 0);
    let right = canvas.get_pixel(W - 1, 0);
    assert_ne!(left, right, "horizontal gradient endpoints should differ");
}

#[test]
fn solid_color_split_pattern_uses_secondary_color() {
    let mut r = SolidColorRenderer::new();
    r.init(&make_metadata("solid_color")).expect("init");

    r.set_control("pattern", &ControlValue::Enum("Vertical Split".into()));
    r.set_control("position", &ControlValue::Float(0.5));
    r.set_control("color", &ControlValue::Color([1.0, 0.0, 0.0, 1.0]));
    r.set_control(
        "secondary_color",
        &ControlValue::Color([0.0, 0.0, 1.0, 1.0]),
    );

    let canvas = r.tick(&frame(0.0, 0)).expect("tick");
    let left = canvas.get_pixel(0, 0);
    let right = canvas.get_pixel(W - 1, 0);

    assert!(left.r > left.b, "left side should favor the primary color");
    assert!(
        right.b > right.r,
        "right side should favor the secondary color"
    );
}

#[test]
fn rainbow_speed_control() {
    let mut r = RainbowRenderer::new();
    r.init(&make_metadata("rainbow")).expect("init");

    r.set_control("speed", &ControlValue::Float(0.0));
    let canvas1 = r.tick(&frame(0.0, 0)).expect("tick t=0");
    let canvas2 = r.tick(&frame(10.0, 600)).expect("tick t=10");

    // With speed=0, frames at different times should be identical
    let p1 = top_left(&canvas1);
    let p2 = top_left(&canvas2);
    assert_eq!(p1, p2, "with speed=0, rainbow should be static");
}

#[test]
fn breathing_speed_control() {
    let mut r = BreathingRenderer::new();
    r.init(&make_metadata("breathing")).expect("init");

    r.set_control("speed", &ControlValue::Float(60.0)); // 60 BPM = 1 Hz
    r.set_control("color", &ControlValue::Color([1.0, 1.0, 1.0, 1.0]));
    r.set_control("min_brightness", &ControlValue::Float(0.0));
    r.set_control("max_brightness", &ControlValue::Float(1.0));

    // At t=0 sine is 0, brightness is midpoint
    let canvas_start = r.tick(&frame(0.0, 0)).expect("tick t=0");
    // At t=0.25 sine is 1 (peak)
    let canvas_peak = r.tick(&frame(0.25, 15)).expect("tick t=0.25");

    let p_start = top_left(&canvas_start);
    let p_peak = top_left(&canvas_peak);

    assert_ne!(
        p_start, p_peak,
        "breathing should vary brightness over time"
    );
}

// ── Audio Reactivity Tests ──────────────────────────────────────────────────

#[test]
fn audio_pulse_responds_to_silence_vs_loud() {
    let mut r = AudioPulseRenderer::new();
    r.init(&make_metadata("audio_pulse")).expect("init");

    // Silence
    let canvas_silent = r
        .tick(&frame_with_audio(0.0, &AudioData::silence()))
        .expect("tick silent");
    let p_silent = top_left(&canvas_silent);

    // Loud audio
    let mut loud = AudioData::silence();
    loud.rms_level = 1.0;
    let canvas_loud = r.tick(&frame_with_audio(0.0, &loud)).expect("tick loud");
    let p_loud = top_left(&canvas_loud);

    assert_ne!(
        p_silent, p_loud,
        "audio pulse should look different with silence vs loud audio"
    );
}

#[test]
fn audio_pulse_responds_to_beat() {
    let mut r = AudioPulseRenderer::new();
    r.init(&make_metadata("audio_pulse")).expect("init");

    // No beat
    let canvas_no_beat = r
        .tick(&frame_with_audio(0.0, &AudioData::silence()))
        .expect("tick no beat");

    // Beat detected
    let mut beat_audio = AudioData::silence();
    beat_audio.beat_detected = true;
    beat_audio.rms_level = 0.5;
    let canvas_beat = r
        .tick(&frame_with_audio(0.0, &beat_audio))
        .expect("tick with beat");

    let p_no_beat = top_left(&canvas_no_beat);
    let p_beat = top_left(&canvas_beat);

    assert_ne!(
        p_no_beat, p_beat,
        "audio pulse should flash differently on beat detection"
    );
}

// ── Gradient Spatial Tests ──────────────────────────────────────────────────

#[test]
fn gradient_has_spatial_variation() {
    let mut r = GradientRenderer::new();
    r.init(&make_metadata("gradient")).expect("init");

    r.set_control("direction", &ControlValue::Enum("horizontal".into()));
    r.set_control("speed", &ControlValue::Float(0.0));
    r.set_control("color_start", &ControlValue::Color([1.0, 0.0, 0.0, 1.0]));
    r.set_control("color_end", &ControlValue::Color([0.0, 0.0, 1.0, 1.0]));

    let canvas = r.tick(&frame(0.0, 0)).expect("tick");
    let left = canvas.get_pixel(0, 0);
    let right = canvas.get_pixel(W - 1, 0);

    assert_ne!(
        left, right,
        "gradient must produce different colors at opposite ends"
    );

    // Left should be reddish, right should be bluish
    assert!(left.r > left.b, "left end should be more red than blue");
    assert!(right.b > right.r, "right end should be more blue than red");
}

#[test]
fn gradient_vertical_varies_along_y() {
    let mut r = GradientRenderer::new();
    r.init(&make_metadata("gradient")).expect("init");

    r.set_control("direction", &ControlValue::Enum("vertical".into()));
    r.set_control("speed", &ControlValue::Float(0.0));

    let canvas = r.tick(&frame(0.0, 0)).expect("tick");
    let top = canvas.get_pixel(0, 0);
    let bottom = canvas.get_pixel(0, H - 1);

    assert_ne!(
        top, bottom,
        "vertical gradient should differ between top and bottom"
    );
}

#[test]
fn gradient_middle_color_changes_midpoint_output() {
    let mut r = GradientRenderer::new();
    r.init(&make_metadata("gradient")).expect("init");

    r.set_control("speed", &ControlValue::Float(0.0));
    r.set_control("use_mid_color", &ControlValue::Boolean(true));
    r.set_control("color_start", &ControlValue::Color([1.0, 0.0, 0.0, 1.0]));
    r.set_control("color_mid", &ControlValue::Color([0.0, 1.0, 0.0, 1.0]));
    r.set_control("color_end", &ControlValue::Color([0.0, 0.0, 1.0, 1.0]));
    r.set_control("midpoint", &ControlValue::Float(0.5));

    let canvas = r.tick(&frame(0.0, 0)).expect("tick");
    let center = canvas.get_pixel(W / 2, H / 2);

    assert!(
        center.g >= center.r && center.g >= center.b,
        "middle stop should influence the canvas center"
    );
}

#[test]
fn gradient_repeat_mode_wraps_offset() {
    let mut r = GradientRenderer::new();
    r.init(&make_metadata("gradient")).expect("init");

    r.set_control("speed", &ControlValue::Float(0.0));
    r.set_control("repeat_mode", &ControlValue::Enum("Repeat".into()));
    r.set_control("offset", &ControlValue::Float(1.0));
    let wrapped = r.tick(&frame(0.0, 0)).expect("wrapped tick");

    r.set_control("offset", &ControlValue::Float(0.0));
    let baseline = r.tick(&frame(0.0, 1)).expect("baseline tick");

    assert_eq!(
        top_left(&wrapped),
        top_left(&baseline),
        "repeat mode should wrap a full-cycle offset"
    );
}

// ── Rainbow Temporal Tests ──────────────────────────────────────────────────

#[test]
fn rainbow_changes_over_time() {
    let mut r = RainbowRenderer::new();
    r.init(&make_metadata("rainbow")).expect("init");

    let canvas0 = r.tick(&frame(0.0, 0)).expect("tick frame 0");
    let canvas100 = r.tick(&frame(5.0, 100)).expect("tick frame 100");

    let p0 = top_left(&canvas0);
    let p100 = top_left(&canvas100);

    assert_ne!(p0, p100, "rainbow should look different at t=0 vs t=5.0");
}

#[test]
fn rainbow_has_spatial_variation() {
    let mut r = RainbowRenderer::new();
    r.init(&make_metadata("rainbow")).expect("init");

    let canvas = r.tick(&frame(0.0, 0)).expect("tick");
    let left = canvas.get_pixel(0, 0);
    let right = canvas.get_pixel(W - 1, 0);

    assert_ne!(
        left, right,
        "rainbow should produce different hues across the canvas"
    );
}

#[test]
fn rainbow_defaults_to_vivid_red_at_origin() {
    let mut r = RainbowRenderer::new();
    r.init(&make_metadata("rainbow")).expect("init");

    let canvas = r.tick(&frame(0.0, 0)).expect("tick");
    let origin = top_left(&canvas);

    assert!(
        origin.r >= 180,
        "default rainbow should start with a strong red band, got {origin:?}"
    );
    assert!(
        origin.g <= 10 && origin.b <= 10,
        "default rainbow should not wash red into pastel orange, got {origin:?}"
    );
}

#[test]
fn rainbow_saturation_control_desaturates_to_grayscale() {
    let mut r = RainbowRenderer::new();
    r.init(&make_metadata("rainbow")).expect("init");

    r.set_control("speed", &ControlValue::Float(0.0));
    r.set_control("saturation", &ControlValue::Float(0.0));

    let canvas = r.tick(&frame(0.0, 0)).expect("tick");
    let origin = top_left(&canvas);

    assert_eq!(
        origin.r, origin.g,
        "desaturated rainbow should have equal red and green channels"
    );
    assert_eq!(
        origin.g, origin.b,
        "desaturated rainbow should have equal green and blue channels"
    );
}

// ── Full Lifecycle Tests ────────────────────────────────────────────────────

#[test]
fn solid_color_full_lifecycle() {
    let mut r = SolidColorRenderer::new();
    let meta = make_metadata("solid_color");

    // Init
    r.init(&meta).expect("init");

    // Tick 10 frames
    for i in 0..10 {
        #[allow(clippy::cast_precision_loss, clippy::as_conversions)]
        let t = i as f32 / 60.0;
        let canvas = r.tick(&frame(t, i)).expect("tick");
        assert_eq!(canvas.width(), W);
        assert_eq!(canvas.height(), H);
    }

    // Change control
    r.set_control("color", &ControlValue::Color([0.0, 1.0, 0.0, 1.0]));

    // Tick 10 more frames
    for i in 10..20 {
        #[allow(clippy::cast_precision_loss, clippy::as_conversions)]
        let t = i as f32 / 60.0;
        let canvas = r.tick(&frame(t, i)).expect("tick after control change");
        let p = top_left(&canvas);
        assert_eq!(p.g, 255, "should be green after control change");
    }

    // Destroy
    r.destroy();
}

#[test]
fn gradient_full_lifecycle() {
    let mut r = GradientRenderer::new();
    r.init(&make_metadata("gradient")).expect("init");

    for i in 0..10 {
        #[allow(clippy::cast_precision_loss, clippy::as_conversions)]
        let t = i as f32 / 60.0;
        r.tick(&frame(t, i)).expect("tick");
    }

    r.set_control("direction", &ControlValue::Enum("radial".into()));

    for i in 10..20 {
        #[allow(clippy::cast_precision_loss, clippy::as_conversions)]
        let t = i as f32 / 60.0;
        r.tick(&frame(t, i)).expect("tick after control change");
    }

    r.destroy();
}

#[test]
fn rainbow_full_lifecycle() {
    let mut r = RainbowRenderer::new();
    r.init(&make_metadata("rainbow")).expect("init");

    for i in 0..10 {
        #[allow(clippy::cast_precision_loss, clippy::as_conversions)]
        let t = i as f32 / 60.0;
        r.tick(&frame(t, i)).expect("tick");
    }

    r.set_control("scale", &ControlValue::Float(2.0));

    for i in 10..20 {
        #[allow(clippy::cast_precision_loss, clippy::as_conversions)]
        let t = i as f32 / 60.0;
        r.tick(&frame(t, i)).expect("tick after control change");
    }

    r.destroy();
}

#[test]
fn breathing_full_lifecycle() {
    let mut r = BreathingRenderer::new();
    r.init(&make_metadata("breathing")).expect("init");

    for i in 0..10 {
        #[allow(clippy::cast_precision_loss, clippy::as_conversions)]
        let t = i as f32 / 60.0;
        r.tick(&frame(t, i)).expect("tick");
    }

    r.set_control("speed", &ControlValue::Float(30.0));

    for i in 10..20 {
        #[allow(clippy::cast_precision_loss, clippy::as_conversions)]
        let t = i as f32 / 60.0;
        r.tick(&frame(t, i)).expect("tick after control change");
    }

    r.destroy();
}

#[test]
fn audio_pulse_full_lifecycle() {
    let mut r = AudioPulseRenderer::new();
    r.init(&make_metadata("audio_pulse")).expect("init");

    // 10 frames with silence
    for i in 0..10 {
        #[allow(clippy::cast_precision_loss, clippy::as_conversions)]
        let t = i as f32 / 60.0;
        r.tick(&frame(t, i)).expect("tick");
    }

    // Change sensitivity
    r.set_control("sensitivity", &ControlValue::Float(5.0));

    // 10 frames with audio
    for i in 10..20 {
        #[allow(clippy::cast_precision_loss, clippy::as_conversions)]
        let t = i as f32 / 60.0;
        let mut audio = AudioData::silence();
        audio.rms_level = 0.5;
        if i == 15 {
            audio.beat_detected = true;
        }
        let input = FrameInput {
            time_secs: t,
            delta_secs: 1.0 / 60.0,
            frame_number: i,
            audio: &audio,
            interaction: &DEFAULT_INTERACTION,
            canvas_width: W,
            canvas_height: H,
        };
        r.tick(&input).expect("tick with audio");
    }

    r.destroy();
}

#[test]
fn color_wave_full_lifecycle() {
    let mut r = ColorWaveRenderer::new();
    r.init(&make_metadata("color_wave")).expect("init");

    tick_color_wave(&mut r, 10);

    r.set_control("direction", &ControlValue::Enum("left".into()));
    r.set_control("wave_width", &ControlValue::Float(24.0));
    r.set_control("spawn_delay", &ControlValue::Float(80.0));

    tick_color_wave(&mut r, 10);

    r.destroy();
}

// ── Color Wave Spatial Tests ────────────────────────────────────────────────

#[test]
fn color_wave_has_spatial_variation() {
    let mut r = ColorWaveRenderer::new();
    r.init(&make_metadata("color_wave")).expect("init");
    r.set_control(
        "background_color",
        &ControlValue::Color([0.0, 0.0, 0.0, 1.0]),
    );
    r.set_control("wave_width", &ControlValue::Float(8.0));
    r.set_control("speed", &ControlValue::Float(100.0));
    r.set_control("trail", &ControlValue::Float(0.0));

    let canvas = tick_color_wave(&mut r, 1);

    // Collect unique brightness values across the top row
    let mut seen_values = std::collections::HashSet::new();
    for x in 0..W {
        let p = canvas.get_pixel(x, 0);
        seen_values.insert((p.r, p.g, p.b));
    }

    assert!(
        seen_values.len() > 1,
        "color wave should produce varying brightness across the canvas"
    );
}

#[test]
fn color_wave_wave_width_accepts_float_slider_values() {
    let mut r = ColorWaveRenderer::new();
    r.init(&make_metadata("color_wave")).expect("init");

    r.set_control(
        "background_color",
        &ControlValue::Color([0.0, 0.0, 0.0, 1.0]),
    );
    r.set_control("speed", &ControlValue::Float(100.0));
    r.set_control("trail", &ControlValue::Float(0.0));
    r.set_control("wave_width", &ControlValue::Float(24.0));
    let wide = tick_color_wave(&mut r, 1);

    r.set_control("wave_width", &ControlValue::Float(8.0));
    let narrow = tick_color_wave(&mut r, 1);

    assert!(
        count_non_black_pixels_in_row(&wide, 0) > count_non_black_pixels_in_row(&narrow, 0),
        "float-backed slider updates should change wave width"
    );
}

#[test]
fn color_wave_direction_accepts_vertical_pass() {
    let mut r = ColorWaveRenderer::new();
    r.init(&make_metadata("color_wave")).expect("init");

    r.set_control(
        "background_color",
        &ControlValue::Color([0.0, 0.0, 0.0, 1.0]),
    );
    r.set_control("wave_width", &ControlValue::Float(8.0));
    r.set_control("speed", &ControlValue::Float(100.0));
    r.set_control("trail", &ControlValue::Float(0.0));
    r.set_control("direction", &ControlValue::Enum("Vertical Pass".into()));

    let canvas = tick_color_wave(&mut r, 1);
    let mut seen_values = std::collections::HashSet::new();
    for y in 0..H {
        let p = canvas.get_pixel(0, y);
        seen_values.insert((p.r, p.g, p.b));
    }

    assert!(
        seen_values.len() > 1,
        "vertical pass should vary down the canvas height"
    );
}

// ── Factory & Registry Tests ────────────────────────────────────────────────

#[test]
fn factory_creates_all_builtins() {
    let names = [
        "solid_color",
        "gradient",
        "rainbow",
        "breathing",
        "audio_pulse",
        "color_wave",
    ];

    for name in &names {
        let renderer = create_builtin_renderer(name);
        assert!(
            renderer.is_some(),
            "factory should create renderer for '{name}'"
        );
    }
}

#[test]
fn factory_returns_none_for_unknown() {
    assert!(
        create_builtin_renderer("nonexistent_effect").is_none(),
        "factory should return None for unknown effect names"
    );
}

#[test]
fn register_builtin_effects_populates_registry() {
    let mut registry = EffectRegistry::default();
    register_builtin_effects(&mut registry);

    assert_eq!(registry.len(), 6, "should register all 6 built-in effects");

    // Verify category filtering works
    let ambient = registry.by_category(EffectCategory::Ambient);
    assert_eq!(ambient.len(), 5, "5 ambient effects expected");

    let audio = registry.by_category(EffectCategory::Audio);
    assert_eq!(audio.len(), 1, "1 audio effect expected");
}

#[test]
fn registered_builtins_use_human_readable_names_and_stable_native_keys() {
    let mut registry = EffectRegistry::default();
    register_builtin_effects(&mut registry);

    let expected = [
        ("Solid Color", "solid_color"),
        ("Gradient", "gradient"),
        ("Rainbow", "rainbow"),
        ("Breathing", "breathing"),
        ("Audio Pulse", "audio_pulse"),
        ("Color Wave", "color_wave"),
    ];

    for (display_name, source_key) in expected {
        let (_, entry) = registry
            .iter()
            .find(|(_, entry)| entry.metadata.name == display_name)
            .unwrap_or_else(|| panic!("missing built-in '{display_name}'"));

        assert_eq!(entry.metadata.source.source_stem(), Some(source_key));
        assert_eq!(
            entry.source_path,
            PathBuf::from(format!("builtin/{source_key}"))
        );
    }
}

#[test]
fn registered_builtins_expose_controls_and_capitalized_author() {
    let mut registry = EffectRegistry::default();
    register_builtin_effects(&mut registry);

    for (_, entry) in registry.iter() {
        assert_eq!(entry.metadata.author, "Hypercolor");
        assert!(
            !entry.metadata.controls.is_empty(),
            "built-in '{}' should expose controls",
            entry.metadata.name
        );
    }
}

#[test]
fn solid_color_metadata_includes_diagnostic_controls() {
    let mut registry = EffectRegistry::default();
    register_builtin_effects(&mut registry);

    let (_, entry) = registry
        .iter()
        .find(|(_, entry)| entry.metadata.source.source_stem() == Some("solid_color"))
        .expect("Solid Color should be registered");
    let ids: Vec<&str> = entry
        .metadata
        .controls
        .iter()
        .map(hypercolor_types::effect::ControlDefinition::control_id)
        .collect();

    assert!(ids.contains(&"pattern"));
    assert!(ids.contains(&"secondary_color"));
    assert!(ids.contains(&"softness"));
}

#[test]
fn gradient_metadata_includes_geometry_and_motion_controls() {
    let mut registry = EffectRegistry::default();
    register_builtin_effects(&mut registry);

    let (_, entry) = registry
        .iter()
        .find(|(_, entry)| entry.metadata.source.source_stem() == Some("gradient"))
        .expect("Gradient should be registered");
    let ids: Vec<&str> = entry
        .metadata
        .controls
        .iter()
        .map(hypercolor_types::effect::ControlDefinition::control_id)
        .collect();

    assert!(ids.contains(&"mode"));
    assert!(ids.contains(&"angle"));
    assert!(ids.contains(&"repeat_mode"));
    assert!(ids.contains(&"use_mid_color"));
}

#[test]
fn rainbow_metadata_includes_color_controls() {
    let mut registry = EffectRegistry::default();
    register_builtin_effects(&mut registry);

    let (_, entry) = registry
        .iter()
        .find(|(_, entry)| entry.metadata.source.source_stem() == Some("rainbow"))
        .expect("Rainbow should be registered");
    let ids: Vec<&str> = entry
        .metadata
        .controls
        .iter()
        .map(hypercolor_types::effect::ControlDefinition::control_id)
        .collect();

    assert!(ids.contains(&"speed"));
    assert!(ids.contains(&"scale"));
    assert!(ids.contains(&"saturation"));
    assert!(ids.contains(&"brightness"));
}

#[test]
fn registered_effects_searchable_by_name() {
    let mut registry = EffectRegistry::default();
    register_builtin_effects(&mut registry);

    let results = registry.search("rainbow");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].metadata.name, "Rainbow");
}

#[test]
fn registered_effects_searchable_by_tag() {
    let mut registry = EffectRegistry::default();
    register_builtin_effects(&mut registry);

    let results = registry.search("reactive");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].metadata.name, "Audio Pulse");
}
