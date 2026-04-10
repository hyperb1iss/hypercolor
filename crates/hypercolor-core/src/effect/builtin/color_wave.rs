//! Color wave renderer — traveling wavefronts.
//!
//! Produces spawned rectangular wave bands that sweep across the canvas,
//! with configurable direction, width, spawn rate, trail fade, and color modes.

use std::array;
use std::path::PathBuf;
use std::sync::LazyLock;

use hypercolor_types::canvas::{
    BYTES_PER_PIXEL, Canvas, Oklch, Rgba, RgbaF32, linear_to_srgb_u8, srgb_u8_to_linear,
};
use hypercolor_types::effect::{
    ControlDefinition, ControlValue, EffectCategory, EffectMetadata, EffectSource, PresetTemplate,
};

use super::common::{
    builtin_effect_id, color_control, dropdown_control, preset_with_desc, slider_control,
};
use crate::effect::traits::{EffectRenderer, FrameInput, prepare_target_canvas};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WaveDirection {
    Right,
    Left,
    Up,
    Down,
    VerticalPass,
    HorizontalPass,
}

impl WaveDirection {
    fn from_str(value: &str) -> Self {
        match normalize_choice(value).as_str() {
            "left" => Self::Left,
            "up" => Self::Up,
            "down" => Self::Down,
            "vertical_pass" => Self::VerticalPass,
            "horizontal_pass" => Self::HorizontalPass,
            _ => Self::Right,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WaveColorMode {
    Custom,
    Random,
    ColorCycle,
}

impl WaveColorMode {
    fn from_str(value: &str) -> Self {
        match normalize_choice(value).as_str() {
            "random" => Self::Random,
            "color_cycle" => Self::ColorCycle,
            _ => Self::Custom,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct WaveInstance {
    position: f32,
    lane: u8,
    hue_offset: f32,
}

/// Color wave with spawned sweep bands and persistent trails.
pub struct ColorWaveRenderer {
    wave_color: [f32; 4],
    background_color: [f32; 4],
    speed: f32,
    wave_width: f32,
    spawn_delay: f32,
    direction: WaveDirection,
    color_mode: WaveColorMode,
    cycle_speed: f32,
    trail: f32,
    brightness: f32,
    waves: Vec<WaveInstance>,
    spawn_accumulator: f32,
    rng_state: u64,
    last_size: (u32, u32),
    framebuffer: Option<Canvas>,
}

const LINEAR_ENCODE_LUT_SCALE: f32 = 65_535.0;

static SRGB_TO_LINEAR_LUT: LazyLock<[f32; 256]> = LazyLock::new(|| {
    array::from_fn(|index| {
        let channel = u8::try_from(index).expect("LUT index must fit in u8");
        srgb_u8_to_linear(channel)
    })
});
static LINEAR_TO_SRGB_LUT: LazyLock<Vec<u8>> = LazyLock::new(|| {
    (0_u16..=u16::MAX)
        .map(|index| linear_to_srgb_u8(f32::from(index) / LINEAR_ENCODE_LUT_SCALE))
        .collect()
});

impl ColorWaveRenderer {
    /// Create a color wave renderer with reference defaults.
    #[must_use]
    pub fn new() -> Self {
        Self {
            wave_color: [0.5, 1.0, 0.92, 1.0],
            background_color: [0.0, 0.02, 0.08, 1.0],
            speed: 85.0,
            wave_width: 50.0,
            spawn_delay: 50.0,
            direction: WaveDirection::Right,
            color_mode: WaveColorMode::Custom,
            cycle_speed: 50.0,
            trail: 50.0,
            brightness: 1.0,
            waves: Vec::new(),
            spawn_accumulator: 1.0,
            rng_state: 0x9e37_79b9_7f4a_7c15,
            last_size: (0, 0),
            framebuffer: None,
        }
    }

    fn reset_state(&mut self) {
        self.waves.clear();
        self.spawn_accumulator = self.spawn_interval_secs();
        self.framebuffer = None;
    }

    fn spawn_interval_secs(&self) -> f32 {
        ((110.0 - self.spawn_delay.clamp(0.0, 100.0)).max(1.0)) / 60.0
    }

    fn pixels_per_second(&self) -> f32 {
        self.speed.clamp(0.0, 100.0) * 6.0
    }

    fn wave_width_px(&self) -> f32 {
        self.wave_width.clamp(1.0, 100.0)
    }

    fn clear_opacity(&self) -> f32 {
        1.0 - (self.trail.clamp(0.0, 100.0) / 100.0)
    }

    fn next_random(&mut self) -> f32 {
        self.rng_state = self
            .rng_state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        #[allow(clippy::cast_precision_loss, clippy::as_conversions)]
        {
            let value = (self.rng_state >> 32) as u32;
            value as f32 / u32::MAX as f32
        }
    }

    #[expect(
        clippy::cast_precision_loss,
        clippy::as_conversions,
        reason = "canvas pixel dimensions are always safely representable as f32"
    )]
    fn spawn_wave(&mut self, width: u32, height: u32) {
        let wave_width = self.wave_width_px();
        let (position, lane) = match self.direction {
            WaveDirection::Right | WaveDirection::Down => (-wave_width, 0),
            WaveDirection::Left => (width as f32, 0),
            WaveDirection::Up => (height as f32, 0),
            WaveDirection::VerticalPass => {
                if self.next_random() < 0.5 {
                    (-wave_width, 0)
                } else {
                    (height as f32, 1)
                }
            }
            WaveDirection::HorizontalPass => {
                if self.next_random() < 0.5 {
                    (-wave_width, 0)
                } else {
                    (width as f32, 1)
                }
            }
        };

        let hue_offset = self.next_random() * 360.0;
        self.waves.push(WaveInstance {
            position,
            lane,
            hue_offset,
        });
    }

    #[allow(clippy::cast_precision_loss, clippy::as_conversions)]
    fn advance_waves(&mut self, delta_secs: f32) {
        let delta = self.pixels_per_second() * delta_secs.max(0.0);
        for wave in &mut self.waves {
            match self.direction {
                WaveDirection::Right | WaveDirection::Down => wave.position += delta,
                WaveDirection::Left | WaveDirection::Up => wave.position -= delta,
                WaveDirection::VerticalPass | WaveDirection::HorizontalPass => {
                    if wave.lane == 0 {
                        wave.position += delta;
                    } else {
                        wave.position -= delta;
                    }
                }
            }
        }
    }

    #[expect(
        clippy::cast_precision_loss,
        clippy::as_conversions,
        reason = "canvas pixel dimensions are always safely representable as f32"
    )]
    fn retain_visible_waves(&mut self, width: u32, height: u32) {
        let wave_width = self.wave_width_px();
        self.waves.retain(|wave| match self.direction {
            WaveDirection::Right => wave.position < width as f32,
            WaveDirection::Left | WaveDirection::Up => wave.position + wave_width > 0.0,
            WaveDirection::Down => wave.position < height as f32,
            WaveDirection::VerticalPass => {
                if wave.lane == 0 {
                    wave.position < height as f32
                } else {
                    wave.position + wave_width > 0.0
                }
            }
            WaveDirection::HorizontalPass => {
                if wave.lane == 0 {
                    wave.position < width as f32
                } else {
                    wave.position + wave_width > 0.0
                }
            }
        });
    }

    fn scaled_color(&self, rgba: [f32; 4]) -> RgbaF32 {
        RgbaF32::new(
            rgba[0] * self.brightness,
            rgba[1] * self.brightness,
            rgba[2] * self.brightness,
            rgba[3],
        )
    }

    fn current_wave_color(&self, wave: &WaveInstance, time_secs: f32) -> RgbaF32 {
        let shifted = match self.color_mode {
            WaveColorMode::Custom => self.wave_color,
            WaveColorMode::Random => hue_shift(self.wave_color, wave.hue_offset),
            WaveColorMode::ColorCycle => {
                hue_shift(self.wave_color, time_secs * self.cycle_speed * 1.2)
            }
        };
        self.scaled_color(shifted)
    }

    fn background_fill(&self) -> Rgba {
        self.scaled_color(self.background_color).to_srgba()
    }

    fn fade_canvas(&self, canvas: &mut Canvas) {
        let opacity = self.clear_opacity();
        if opacity <= 0.0 {
            return;
        }

        let background = self.scaled_color(self.background_color);
        let red_lut = fade_lut(background.r, opacity);
        let green_lut = fade_lut(background.g, opacity);
        let blue_lut = fade_lut(background.b, opacity);
        for chunk in canvas.as_rgba_bytes_mut().chunks_exact_mut(4) {
            chunk[0] = red_lut[usize::from(chunk[0])];
            chunk[1] = green_lut[usize::from(chunk[1])];
            chunk[2] = blue_lut[usize::from(chunk[2])];
            chunk[3] = 255;
        }
    }

    fn draw_waves(&self, canvas: &mut Canvas, time_secs: f32, width: u32, height: u32) {
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::as_conversions
        )]
        let wave_width = self.wave_width_px().round() as i32;

        for wave in &self.waves {
            let color = self.current_wave_color(wave, time_secs).to_srgba();
            match self.direction {
                WaveDirection::Right | WaveDirection::Left | WaveDirection::HorizontalPass => {
                    #[allow(
                        clippy::cast_possible_truncation,
                        clippy::cast_precision_loss,
                        clippy::as_conversions
                    )]
                    let x = wave.position.round() as i32;
                    fill_rect(
                        canvas,
                        x,
                        0,
                        wave_width,
                        i32::try_from(height).unwrap_or(i32::MAX),
                        color,
                    );
                }
                WaveDirection::Up | WaveDirection::Down | WaveDirection::VerticalPass => {
                    #[allow(
                        clippy::cast_possible_truncation,
                        clippy::cast_precision_loss,
                        clippy::as_conversions
                    )]
                    let y = wave.position.round() as i32;
                    fill_rect(
                        canvas,
                        0,
                        y,
                        i32::try_from(width).unwrap_or(i32::MAX),
                        wave_width,
                        color,
                    );
                }
            }
        }
    }
}

impl Default for ColorWaveRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl EffectRenderer for ColorWaveRenderer {
    fn init(&mut self, _metadata: &EffectMetadata) -> anyhow::Result<()> {
        self.reset_state();
        Ok(())
    }

    fn render_into(&mut self, input: &FrameInput<'_>, canvas: &mut Canvas) -> anyhow::Result<()> {
        if self.last_size != (input.canvas_width, input.canvas_height) {
            self.last_size = (input.canvas_width, input.canvas_height);
            self.reset_state();
        }

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

        let spawn_interval = self.spawn_interval_secs();
        self.spawn_accumulator += input.delta_secs.max(0.0);
        while self.spawn_accumulator >= spawn_interval {
            self.spawn_wave(input.canvas_width, input.canvas_height);
            self.spawn_accumulator -= spawn_interval;
        }

        self.advance_waves(input.delta_secs);
        self.retain_visible_waves(input.canvas_width, input.canvas_height);
        self.draw_waves(
            canvas,
            input.time_secs,
            input.canvas_width,
            input.canvas_height,
        );

        self.framebuffer = Some(canvas.clone());
        Ok(())
    }

    fn tick(&mut self, input: &FrameInput<'_>) -> anyhow::Result<Canvas> {
        let mut canvas = match self.framebuffer.take() {
            Some(existing)
                if existing.width() == input.canvas_width
                    && existing.height() == input.canvas_height =>
            {
                existing
            }
            _ => {
                let mut fresh = Canvas::new(input.canvas_width, input.canvas_height);
                fresh.fill(self.background_fill());
                fresh
            }
        };

        self.fade_canvas(&mut canvas);

        let spawn_interval = self.spawn_interval_secs();
        self.spawn_accumulator += input.delta_secs.max(0.0);
        while self.spawn_accumulator >= spawn_interval {
            self.spawn_wave(input.canvas_width, input.canvas_height);
            self.spawn_accumulator -= spawn_interval;
        }

        self.advance_waves(input.delta_secs);
        self.retain_visible_waves(input.canvas_width, input.canvas_height);
        self.draw_waves(
            &mut canvas,
            input.time_secs,
            input.canvas_width,
            input.canvas_height,
        );

        self.framebuffer = Some(canvas.clone());
        Ok(canvas)
    }

    fn set_control(&mut self, name: &str, value: &ControlValue) {
        match name {
            "color" | "wave_color" => {
                if let ControlValue::Color(c) = value {
                    self.wave_color = *c;
                }
            }
            "background_color" => {
                if let ControlValue::Color(c) = value {
                    self.background_color = *c;
                }
            }
            "speed" => {
                if let Some(v) = value.as_f32() {
                    self.speed = v.clamp(0.0, 100.0);
                }
            }
            "wave_width" => {
                if let Some(v) = value.as_f32() {
                    self.wave_width = v.clamp(1.0, 100.0);
                    self.reset_state();
                }
            }
            "spawn_delay" => {
                if let Some(v) = value.as_f32() {
                    self.spawn_delay = v.clamp(0.0, 100.0);
                }
            }
            "direction" => {
                if let ControlValue::Enum(choice) | ControlValue::Text(choice) = value {
                    self.direction = WaveDirection::from_str(choice);
                    self.reset_state();
                }
            }
            "color_mode" => {
                if let ControlValue::Enum(choice) | ControlValue::Text(choice) = value {
                    self.color_mode = WaveColorMode::from_str(choice);
                }
            }
            "cycle_speed" => {
                if let Some(v) = value.as_f32() {
                    self.cycle_speed = v.clamp(0.0, 100.0);
                }
            }
            "trail" => {
                if let Some(v) = value.as_f32() {
                    self.trail = v.clamp(0.0, 100.0);
                }
            }
            "brightness" => {
                if let Some(v) = value.as_f32() {
                    self.brightness = v.clamp(0.0, 1.0);
                }
            }
            _ => {}
        }
    }

    fn destroy(&mut self) {
        self.reset_state();
    }
}

fn normalize_choice(value: &str) -> String {
    let mut normalized = String::new();
    let mut last_was_separator = false;

    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            normalized.push(ch.to_ascii_lowercase());
            last_was_separator = false;
        } else if !last_was_separator {
            normalized.push('_');
            last_was_separator = true;
        }
    }

    normalized.trim_matches('_').to_owned()
}

fn hue_shift(color: [f32; 4], degrees: f32) -> [f32; 4] {
    let rgba = RgbaF32::new(color[0], color[1], color[2], color[3]);
    let mut lch = rgba.to_oklch();
    if lch.c <= 0.0001 {
        return color;
    }
    lch = Oklch::new(lch.l, lch.c, (lch.h + degrees).rem_euclid(360.0), lch.alpha);
    let shifted = RgbaF32::from_oklch(lch);
    [shifted.r, shifted.g, shifted.b, shifted.a]
}

fn decode_srgb_channel(channel: u8) -> f32 {
    SRGB_TO_LINEAR_LUT[usize::from(channel)]
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::as_conversions,
    reason = "channel is clamped to the 16-bit LUT domain before rounding to an index"
)]
fn encode_srgb_channel(channel: f32) -> u8 {
    let index = (channel.clamp(0.0, 1.0) * LINEAR_ENCODE_LUT_SCALE).round() as u16;
    LINEAR_TO_SRGB_LUT[usize::from(index)]
}

fn fade_lut(background_channel: f32, opacity: f32) -> [u8; 256] {
    array::from_fn(|channel| {
        let source =
            decode_srgb_channel(u8::try_from(channel).expect("fade LUT index must fit in u8"));
        encode_srgb_channel(source + (background_channel - source) * opacity)
    })
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::as_conversions
)]
fn fill_rect(canvas: &mut Canvas, x: i32, y: i32, width: i32, height: i32, color: Rgba) {
    if width <= 0 || height <= 0 {
        return;
    }

    let Ok(canvas_width) = i32::try_from(canvas.width()) else {
        return;
    };
    let Ok(canvas_height) = i32::try_from(canvas.height()) else {
        return;
    };

    let start_x = x.max(0);
    let start_y = y.max(0);
    let end_x = (x + width).min(canvas_width);
    let end_y = (y + height).min(canvas_height);

    if start_x >= end_x || start_y >= end_y {
        return;
    }

    let Ok(start_x) = usize::try_from(start_x) else {
        return;
    };
    let Ok(start_y) = usize::try_from(start_y) else {
        return;
    };
    let Ok(end_x) = usize::try_from(end_x) else {
        return;
    };
    let Ok(end_y) = usize::try_from(end_y) else {
        return;
    };
    let row_stride = usize::try_from(canvas.width()).unwrap_or(usize::MAX) * BYTES_PER_PIXEL;
    let row_start = start_x * BYTES_PER_PIXEL;
    let row_end = end_x * BYTES_PER_PIXEL;
    let color = [color.r, color.g, color.b, color.a];

    let bytes = canvas.as_rgba_bytes_mut();
    for row in start_y..end_y {
        let row_offset = row * row_stride;
        let slice = &mut bytes[row_offset + row_start..row_offset + row_end];
        for pixel in slice.chunks_exact_mut(BYTES_PER_PIXEL) {
            pixel.copy_from_slice(&color);
        }
    }
}

fn controls() -> Vec<ControlDefinition> {
    vec![
        color_control(
            "wave_color",
            "Wave Color",
            [0.5, 1.0, 0.92, 1.0],
            "Colors",
            "Primary color for the traveling wavefront.",
        ),
        color_control(
            "background_color",
            "Background Color",
            [0.0, 0.02, 0.08, 1.0],
            "Colors",
            "Base fill color that the trail fades back toward.",
        ),
        dropdown_control(
            "color_mode",
            "Color Mode",
            "Custom",
            &["Custom", "Random", "Color Cycle"],
            "Colors",
            "Use a fixed color, randomize each wave, or continuously hue-cycle the wavefronts.",
        ),
        slider_control(
            "cycle_speed",
            "Color Cycle Speed",
            50.0,
            0.0,
            100.0,
            1.0,
            "Colors",
            "Hue rotation speed when Color Cycle mode is enabled.",
        ),
        slider_control(
            "speed",
            "Effect Speed",
            85.0,
            0.0,
            100.0,
            1.0,
            "Motion",
            "How quickly each wavefront moves across the canvas.",
        ),
        slider_control(
            "spawn_delay",
            "Wave Spawn Speed",
            50.0,
            0.0,
            100.0,
            1.0,
            "Motion",
            "How often new wavefronts are emitted.",
        ),
        dropdown_control(
            "direction",
            "Wave Direction",
            "Right",
            &[
                "Right",
                "Left",
                "Up",
                "Down",
                "Vertical Pass",
                "Horizontal Pass",
            ],
            "Motion",
            "Direction and pass mode for spawned wavefronts.",
        ),
        slider_control(
            "wave_width",
            "Wave Width",
            50.0,
            1.0,
            100.0,
            1.0,
            "Shape",
            "Thickness of each rectangular wave band.",
        ),
        slider_control(
            "trail",
            "Wave Trail",
            50.0,
            0.0,
            100.0,
            1.0,
            "Output",
            "How much of the previous frame remains visible behind each wave.",
        ),
        slider_control(
            "brightness",
            "Brightness",
            1.0,
            0.0,
            1.0,
            0.01,
            "Output",
            "Master output brightness.",
        ),
    ]
}

#[expect(
    clippy::too_many_lines,
    reason = "preset catalog is intentionally data-heavy and easier to maintain as one table"
)]
fn presets() -> Vec<PresetTemplate> {
    vec![
        // ── Signature ────────────────────────────────────────────────────
        preset_with_desc(
            "Neon Scanner",
            "Fast cyan scan lines bouncing across the rig",
            &[
                ("wave_color", ControlValue::Color([0.5, 1.0, 0.92, 1.0])),
                (
                    "background_color",
                    ControlValue::Color([0.0, 0.01, 0.04, 1.0]),
                ),
                ("color_mode", ControlValue::Enum("Custom".to_owned())),
                ("speed", ControlValue::Float(95.0)),
                ("wave_width", ControlValue::Float(20.0)),
                ("spawn_delay", ControlValue::Float(65.0)),
                ("trail", ControlValue::Float(30.0)),
                (
                    "direction",
                    ControlValue::Enum("Horizontal Pass".to_owned()),
                ),
            ],
        ),
        preset_with_desc(
            "SilkCircuit Pulse",
            "Electric purple waves on deep void",
            &[
                ("wave_color", ControlValue::Color([0.88, 0.21, 1.0, 1.0])),
                (
                    "background_color",
                    ControlValue::Color([0.02, 0.0, 0.06, 1.0]),
                ),
                ("color_mode", ControlValue::Enum("Custom".to_owned())),
                ("speed", ControlValue::Float(70.0)),
                ("wave_width", ControlValue::Float(35.0)),
                ("spawn_delay", ControlValue::Float(55.0)),
                ("trail", ControlValue::Float(60.0)),
                ("direction", ControlValue::Enum("Right".to_owned())),
            ],
        ),
        // ── Cinematic ────────────────────────────────────────────────────
        preset_with_desc(
            "Lava Flow",
            "Slow molten waves with long ember trails",
            &[
                ("wave_color", ControlValue::Color([1.0, 0.3, 0.0, 1.0])),
                (
                    "background_color",
                    ControlValue::Color([0.15, 0.02, 0.0, 1.0]),
                ),
                ("color_mode", ControlValue::Enum("Custom".to_owned())),
                ("speed", ControlValue::Float(25.0)),
                ("wave_width", ControlValue::Float(80.0)),
                ("spawn_delay", ControlValue::Float(30.0)),
                ("trail", ControlValue::Float(90.0)),
                ("direction", ControlValue::Enum("Right".to_owned())),
            ],
        ),
        preset_with_desc(
            "Ocean Drift",
            "Gentle blue-green waves rolling downward",
            &[
                ("wave_color", ControlValue::Color([0.1, 0.5, 0.9, 1.0])),
                (
                    "background_color",
                    ControlValue::Color([0.0, 0.03, 0.1, 1.0]),
                ),
                ("color_mode", ControlValue::Enum("Custom".to_owned())),
                ("speed", ControlValue::Float(35.0)),
                ("wave_width", ControlValue::Float(60.0)),
                ("spawn_delay", ControlValue::Float(40.0)),
                ("trail", ControlValue::Float(75.0)),
                ("direction", ControlValue::Enum("Down".to_owned())),
            ],
        ),
        preset_with_desc(
            "Arctic Cascade",
            "Cool white-blue bands falling like snow",
            &[
                ("wave_color", ControlValue::Color([0.7, 0.85, 1.0, 1.0])),
                (
                    "background_color",
                    ControlValue::Color([0.02, 0.04, 0.1, 1.0]),
                ),
                ("color_mode", ControlValue::Enum("Custom".to_owned())),
                ("speed", ControlValue::Float(45.0)),
                ("wave_width", ControlValue::Float(25.0)),
                ("spawn_delay", ControlValue::Float(60.0)),
                ("trail", ControlValue::Float(50.0)),
                ("direction", ControlValue::Enum("Down".to_owned())),
            ],
        ),
        // ── Intense ──────────────────────────────────────────────────────
        preset_with_desc(
            "Blade Runner",
            "Fast pink slices on noir darkness",
            &[
                ("wave_color", ControlValue::Color([1.0, 0.1, 0.6, 1.0])),
                (
                    "background_color",
                    ControlValue::Color([0.01, 0.0, 0.03, 1.0]),
                ),
                ("color_mode", ControlValue::Enum("Custom".to_owned())),
                ("speed", ControlValue::Float(90.0)),
                ("wave_width", ControlValue::Float(12.0)),
                ("spawn_delay", ControlValue::Float(75.0)),
                ("trail", ControlValue::Float(15.0)),
                (
                    "direction",
                    ControlValue::Enum("Horizontal Pass".to_owned()),
                ),
            ],
        ),
        preset_with_desc(
            "Laser Grid",
            "Rapid thin beams crisscrossing vertically",
            &[
                ("wave_color", ControlValue::Color([0.0, 1.0, 0.4, 1.0])),
                (
                    "background_color",
                    ControlValue::Color([0.0, 0.02, 0.0, 1.0]),
                ),
                ("color_mode", ControlValue::Enum("Custom".to_owned())),
                ("speed", ControlValue::Float(85.0)),
                ("wave_width", ControlValue::Float(8.0)),
                ("spawn_delay", ControlValue::Float(80.0)),
                ("trail", ControlValue::Float(10.0)),
                ("direction", ControlValue::Enum("Vertical Pass".to_owned())),
            ],
        ),
        preset_with_desc(
            "Warning Strobe",
            "Amber hazard bands sweeping left",
            &[
                ("wave_color", ControlValue::Color([1.0, 0.7, 0.0, 1.0])),
                (
                    "background_color",
                    ControlValue::Color([0.08, 0.03, 0.0, 1.0]),
                ),
                ("color_mode", ControlValue::Enum("Custom".to_owned())),
                ("speed", ControlValue::Float(80.0)),
                ("wave_width", ControlValue::Float(40.0)),
                ("spawn_delay", ControlValue::Float(70.0)),
                ("trail", ControlValue::Float(20.0)),
                ("direction", ControlValue::Enum("Left".to_owned())),
            ],
        ),
        // ── Rainbow / Color Cycling ──────────────────────────────────────
        preset_with_desc(
            "Prism Parade",
            "Rainbow waves cycling through the full spectrum",
            &[
                ("wave_color", ControlValue::Color([1.0, 0.2, 0.3, 1.0])),
                (
                    "background_color",
                    ControlValue::Color([0.01, 0.01, 0.02, 1.0]),
                ),
                ("color_mode", ControlValue::Enum("Color Cycle".to_owned())),
                ("cycle_speed", ControlValue::Float(60.0)),
                ("speed", ControlValue::Float(55.0)),
                ("wave_width", ControlValue::Float(45.0)),
                ("spawn_delay", ControlValue::Float(55.0)),
                ("trail", ControlValue::Float(65.0)),
                ("direction", ControlValue::Enum("Right".to_owned())),
            ],
        ),
        preset_with_desc(
            "Confetti Storm",
            "Random-colored bands flying in all directions",
            &[
                ("wave_color", ControlValue::Color([1.0, 0.4, 0.8, 1.0])),
                (
                    "background_color",
                    ControlValue::Color([0.02, 0.01, 0.04, 1.0]),
                ),
                ("color_mode", ControlValue::Enum("Random".to_owned())),
                ("speed", ControlValue::Float(75.0)),
                ("wave_width", ControlValue::Float(18.0)),
                ("spawn_delay", ControlValue::Float(85.0)),
                ("trail", ControlValue::Float(25.0)),
                (
                    "direction",
                    ControlValue::Enum("Horizontal Pass".to_owned()),
                ),
            ],
        ),
        // ── Ambient ──────────────────────────────────────────────────────
        preset_with_desc(
            "Meditation",
            "Ultra-slow deep indigo wash",
            &[
                ("wave_color", ControlValue::Color([0.25, 0.1, 0.7, 1.0])),
                (
                    "background_color",
                    ControlValue::Color([0.01, 0.0, 0.04, 1.0]),
                ),
                ("color_mode", ControlValue::Enum("Custom".to_owned())),
                ("speed", ControlValue::Float(12.0)),
                ("wave_width", ControlValue::Float(100.0)),
                ("spawn_delay", ControlValue::Float(15.0)),
                ("trail", ControlValue::Float(95.0)),
                ("direction", ControlValue::Enum("Up".to_owned())),
                ("brightness", ControlValue::Float(0.7)),
            ],
        ),
        preset_with_desc(
            "Candlelight",
            "Warm flickering gold on soft amber",
            &[
                ("wave_color", ControlValue::Color([1.0, 0.65, 0.15, 1.0])),
                (
                    "background_color",
                    ControlValue::Color([0.12, 0.04, 0.0, 1.0]),
                ),
                ("color_mode", ControlValue::Enum("Random".to_owned())),
                ("speed", ControlValue::Float(20.0)),
                ("wave_width", ControlValue::Float(70.0)),
                ("spawn_delay", ControlValue::Float(25.0)),
                ("trail", ControlValue::Float(88.0)),
                ("direction", ControlValue::Enum("Vertical Pass".to_owned())),
                ("brightness", ControlValue::Float(0.8)),
            ],
        ),
    ]
}

pub(super) fn metadata() -> EffectMetadata {
    EffectMetadata {
        id: builtin_effect_id("color_wave"),
        name: "Color Wave".into(),
        author: "Hypercolor".into(),
        version: "0.1.0".into(),
        description:
            "Traveling wavefront strips with directional passes and configurable fade trails".into(),
        category: EffectCategory::Ambient,
        tags: vec!["wave".into(), "animation".into(), "pattern".into()],
        controls: controls(),
        presets: presets(),
        audio_reactive: false,
        screen_reactive: false,
        source: EffectSource::Native {
            path: PathBuf::from("builtin/color_wave"),
        },
        license: Some("Apache-2.0".into()),
    }
}
