//! Color wave renderer — traveling wavefronts.
//!
//! Produces spawned rectangular wave bands that sweep across the canvas,
//! with configurable direction, width, spawn rate, trail fade, and color modes.

use hypercolor_types::canvas::{Canvas, Oklch, Rgba, RgbaF32};
use hypercolor_types::effect::{ControlValue, EffectMetadata};

use crate::effect::traits::{EffectRenderer, FrameInput};

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
        for chunk in canvas.as_rgba_bytes_mut().chunks_exact_mut(4) {
            let dst = Rgba::new(chunk[0], chunk[1], chunk[2], chunk[3]).to_linear_f32();
            let blended = RgbaF32::new(
                dst.r + (background.r - dst.r) * opacity,
                dst.g + (background.g - dst.g) * opacity,
                dst.b + (background.b - dst.b) * opacity,
                1.0,
            )
            .to_srgb_u8();
            chunk[0] = blended[0];
            chunk[1] = blended[1];
            chunk[2] = blended[2];
            chunk[3] = blended[3];
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

    fn tick(&mut self, input: &FrameInput<'_>) -> anyhow::Result<Canvas> {
        if self.last_size != (input.canvas_width, input.canvas_height) {
            self.last_size = (input.canvas_width, input.canvas_height);
            self.reset_state();
        }

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

    for py in start_y..end_y {
        for px in start_x..end_x {
            let Ok(col) = u32::try_from(px) else {
                continue;
            };
            let Ok(row) = u32::try_from(py) else {
                continue;
            };
            canvas.set_pixel(col, row, color);
        }
    }
}
