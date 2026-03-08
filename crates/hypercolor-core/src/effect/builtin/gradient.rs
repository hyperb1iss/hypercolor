//! Gradient renderer — configurable linear and radial gradients.
//!
//! Interpolates between colors in Oklab perceptual space for smooth transitions.
//! Supports an optional middle stop, geometry controls, and simple motion modes.

use hypercolor_types::canvas::{Canvas, Oklab, RgbaF32};
use hypercolor_types::effect::{ControlValue, EffectMetadata};

use crate::effect::traits::{EffectRenderer, FrameInput};

/// High-level gradient shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GradientMode {
    Linear,
    Radial,
}

impl GradientMode {
    fn from_str(value: &str) -> Self {
        match normalize_choice(value).as_str() {
            "radial" => Self::Radial,
            _ => Self::Linear,
        }
    }
}

/// How the gradient behaves outside its natural 0-1 range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RepeatMode {
    Clamp,
    Repeat,
    Mirror,
}

impl RepeatMode {
    fn from_str(value: &str) -> Self {
        match normalize_choice(value).as_str() {
            "repeat" => Self::Repeat,
            "mirror" => Self::Mirror,
            _ => Self::Clamp,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct GradientStop {
    position: f32,
    color: Oklab,
}

/// Animated multi-stop gradient with configurable geometry and motion.
pub struct GradientRenderer {
    color_start: [f32; 4],
    color_mid: [f32; 4],
    color_end: [f32; 4],
    use_mid_color: bool,
    midpoint: f32,
    mode: GradientMode,
    repeat_mode: RepeatMode,
    angle_degrees: f32,
    center_x: f32,
    center_y: f32,
    scale: f32,
    offset: f32,
    /// Animation speed in cycles per second.
    speed: f32,
    brightness: f32,
}

impl GradientRenderer {
    /// Create a configurable neon gradient with animation disabled by default.
    #[must_use]
    pub fn new() -> Self {
        Self {
            color_start: [0.88, 0.21, 1.0, 1.0],
            color_mid: [1.0, 0.42, 0.76, 1.0],
            color_end: [0.5, 1.0, 0.92, 1.0],
            use_mid_color: false,
            midpoint: 0.5,
            mode: GradientMode::Linear,
            repeat_mode: RepeatMode::Clamp,
            angle_degrees: 0.0,
            center_x: 0.5,
            center_y: 0.5,
            scale: 1.0,
            offset: 0.0,
            speed: 0.0,
            brightness: 1.0,
        }
    }

    fn gradient_stops(&self) -> Vec<GradientStop> {
        let mut stops = vec![GradientStop {
            position: 0.0,
            color: rgba_to_oklab(self.color_start),
        }];

        if self.use_mid_color {
            stops.push(GradientStop {
                position: self.midpoint.clamp(0.05, 0.95),
                color: rgba_to_oklab(self.color_mid),
            });
        }

        stops.push(GradientStop {
            position: 1.0,
            color: rgba_to_oklab(self.color_end),
        });

        stops
    }
}

impl Default for GradientRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl EffectRenderer for GradientRenderer {
    fn init(&mut self, _metadata: &EffectMetadata) -> anyhow::Result<()> {
        Ok(())
    }

    #[allow(clippy::cast_precision_loss, clippy::as_conversions)]
    fn tick(&mut self, input: &FrameInput) -> anyhow::Result<Canvas> {
        let mut canvas = Canvas::new(input.canvas_width, input.canvas_height);
        let width = input.canvas_width.max(1) as f32;
        let height = input.canvas_height.max(1) as f32;
        let animated_offset = self.offset + input.time_secs * self.speed;
        let scale = self.scale.max(0.1);
        let stops = self.gradient_stops();

        for y in 0..input.canvas_height {
            let ny = (y as f32 + 0.5) / height;
            for x in 0..input.canvas_width {
                let nx = (x as f32 + 0.5) / width;
                let raw_t = match self.mode {
                    GradientMode::Linear => {
                        linear_position(nx, ny, self.center_x, self.center_y, self.angle_degrees)
                    }
                    GradientMode::Radial => radial_position(nx, ny, self.center_x, self.center_y),
                };

                let transformed = match self.mode {
                    GradientMode::Linear => ((raw_t - 0.5) / scale) + 0.5 + animated_offset,
                    GradientMode::Radial => (raw_t / scale) + animated_offset,
                };
                let t = apply_repeat_mode(transformed, self.repeat_mode);

                let mut rgba = sample_gradient(&stops, t);
                rgba.r *= self.brightness;
                rgba.g *= self.brightness;
                rgba.b *= self.brightness;
                canvas.set_pixel(x, y, rgba.to_srgba());
            }
        }

        Ok(canvas)
    }

    fn set_control(&mut self, name: &str, value: &ControlValue) {
        match name {
            "color_start" => {
                if let ControlValue::Color(c) = value {
                    self.color_start = *c;
                }
            }
            "color_mid" => {
                if let ControlValue::Color(c) = value {
                    self.color_mid = *c;
                }
            }
            "color_end" => {
                if let ControlValue::Color(c) = value {
                    self.color_end = *c;
                }
            }
            "use_mid_color" => {
                if let ControlValue::Boolean(flag) = value {
                    self.use_mid_color = *flag;
                }
            }
            "midpoint" => {
                if let Some(v) = value.as_f32() {
                    self.midpoint = v.clamp(0.05, 0.95);
                }
            }
            "mode" => {
                if let ControlValue::Enum(choice) | ControlValue::Text(choice) = value {
                    self.mode = GradientMode::from_str(choice);
                }
            }
            "repeat_mode" => {
                if let ControlValue::Enum(choice) | ControlValue::Text(choice) = value {
                    self.repeat_mode = RepeatMode::from_str(choice);
                }
            }
            "direction" => {
                if let ControlValue::Enum(choice) | ControlValue::Text(choice) = value {
                    self.angle_degrees = legacy_direction_to_angle(choice);
                    self.mode = if normalize_choice(choice) == "radial" {
                        GradientMode::Radial
                    } else {
                        GradientMode::Linear
                    };
                }
            }
            "angle" => {
                if let Some(v) = value.as_f32() {
                    self.angle_degrees = v.rem_euclid(360.0);
                }
            }
            "center_x" => {
                if let Some(v) = value.as_f32() {
                    self.center_x = v.clamp(0.0, 1.0);
                }
            }
            "center_y" => {
                if let Some(v) = value.as_f32() {
                    self.center_y = v.clamp(0.0, 1.0);
                }
            }
            "scale" => {
                if let Some(v) = value.as_f32() {
                    self.scale = v.max(0.1);
                }
            }
            "offset" => {
                if let Some(v) = value.as_f32() {
                    self.offset = v;
                }
            }
            "speed" => {
                if let Some(v) = value.as_f32() {
                    self.speed = v;
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

    fn destroy(&mut self) {}
}

fn rgba_to_oklab(color: [f32; 4]) -> Oklab {
    RgbaF32::new(color[0], color[1], color[2], color[3]).to_oklab()
}

fn sample_gradient(stops: &[GradientStop], t: f32) -> RgbaF32 {
    if stops.len() == 1 {
        return RgbaF32::from_oklab(stops[0].color);
    }

    if t <= stops[0].position {
        return RgbaF32::from_oklab(stops[0].color);
    }

    for pair in stops.windows(2) {
        let left = pair[0];
        let right = pair[1];
        if t <= right.position {
            let span = (right.position - left.position).max(f32::EPSILON);
            let local_t = ((t - left.position) / span).clamp(0.0, 1.0);
            return RgbaF32::from_oklab(Oklab::lerp(left.color, right.color, local_t));
        }
    }

    let last = stops.last().copied().unwrap_or(GradientStop {
        position: 1.0,
        color: Oklab::new(0.0, 0.0, 0.0, 1.0),
    });
    RgbaF32::from_oklab(last.color)
}

fn linear_position(nx: f32, ny: f32, center_x: f32, center_y: f32, angle_degrees: f32) -> f32 {
    let angle = angle_degrees.to_radians();
    let axis_x = angle.cos();
    let axis_y = angle.sin();

    let project = |x: f32, y: f32| (x - center_x) * axis_x + (y - center_y) * axis_y;
    let proj = project(nx, ny);
    let extents = [
        project(0.0, 0.0),
        project(1.0, 0.0),
        project(0.0, 1.0),
        project(1.0, 1.0),
    ];
    let min_extent = extents.iter().copied().fold(f32::INFINITY, f32::min);
    let max_extent = extents.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let span = (max_extent - min_extent).max(f32::EPSILON);

    ((proj - min_extent) / span).clamp(0.0, 1.0)
}

fn radial_position(nx: f32, ny: f32, center_x: f32, center_y: f32) -> f32 {
    let dist = (nx - center_x).hypot(ny - center_y);
    let max_radius = [
        (0.0f32 - center_x).hypot(0.0 - center_y),
        (1.0f32 - center_x).hypot(0.0 - center_y),
        (0.0f32 - center_x).hypot(1.0 - center_y),
        (1.0f32 - center_x).hypot(1.0 - center_y),
    ]
    .into_iter()
    .fold(0.0, f32::max)
    .max(f32::EPSILON);

    (dist / max_radius).clamp(0.0, 1.0)
}

fn apply_repeat_mode(value: f32, repeat_mode: RepeatMode) -> f32 {
    match repeat_mode {
        RepeatMode::Clamp => value.clamp(0.0, 1.0),
        RepeatMode::Repeat => value.rem_euclid(1.0),
        RepeatMode::Mirror => {
            let mirrored = value.rem_euclid(2.0);
            if mirrored <= 1.0 {
                mirrored
            } else {
                2.0 - mirrored
            }
        }
    }
}

fn legacy_direction_to_angle(value: &str) -> f32 {
    match normalize_choice(value).as_str() {
        "vertical" => 90.0,
        "diagonal" => 45.0,
        _ => 0.0,
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
