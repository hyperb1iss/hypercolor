//! Gradient renderer — configurable linear and radial gradients.
//!
//! Interpolates between colors in Oklch (vivid) or Oklab (smooth) perceptual
//! space for rich transitions. Supports an optional middle stop, geometry
//! controls, motion animation, and post-process saturation and easing.

use hypercolor_types::canvas::{BYTES_PER_PIXEL, Canvas, Oklab, Oklch, RgbaF32};
use hypercolor_types::effect::{ControlValue, EffectMetadata};

use crate::effect::traits::{EffectRenderer, FrameInput, prepare_target_canvas};

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

/// Color space used for interpolation between stops.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InterpolationMode {
    /// Oklch (polar) — preserves hue and chroma for vivid transitions.
    Vivid,
    /// Oklab (cartesian) — smooth but may desaturate between distant hues.
    Smooth,
    /// Linear sRGB — direct channel mixing, basic but predictable.
    Direct,
}

impl InterpolationMode {
    fn from_str(value: &str) -> Self {
        match normalize_choice(value).as_str() {
            "smooth" => Self::Smooth,
            "direct" => Self::Direct,
            _ => Self::Vivid,
        }
    }
}

/// Easing curve applied to the gradient distribution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EasingMode {
    Linear,
    EaseIn,
    EaseOut,
    SmoothStep,
}

impl EasingMode {
    fn from_str(value: &str) -> Self {
        match normalize_choice(value).as_str() {
            "ease_in" => Self::EaseIn,
            "ease_out" => Self::EaseOut,
            "smooth" => Self::SmoothStep,
            _ => Self::Linear,
        }
    }

    fn apply(self, t: f32) -> f32 {
        match self {
            Self::Linear => t,
            Self::EaseIn => t * t,
            Self::EaseOut => 1.0 - (1.0 - t) * (1.0 - t),
            Self::SmoothStep => t * t * (3.0 - 2.0 * t),
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum PreparedGradientColor {
    Direct(RgbaF32),
    Smooth(Oklab),
    Vivid(Oklch),
}

impl PreparedGradientColor {
    fn from_color(color: [f32; 4], interpolation: InterpolationMode) -> Self {
        match interpolation {
            InterpolationMode::Direct => Self::Direct(color_to_rgba(color)),
            InterpolationMode::Smooth => Self::Smooth(rgba_to_oklab(color)),
            InterpolationMode::Vivid => Self::Vivid(rgba_to_oklch(color)),
        }
    }

    fn into_rgba(self) -> RgbaF32 {
        match self {
            Self::Direct(rgba) => rgba,
            Self::Smooth(lab) => RgbaF32::from_oklab(lab),
            Self::Vivid(lch) => RgbaF32::from_oklch(lch),
        }
    }

    fn interpolate(self, other: Self, t: f32) -> RgbaF32 {
        match (self, other) {
            (Self::Direct(a), Self::Direct(b)) => RgbaF32::lerp(&a, &b, t),
            (Self::Smooth(a), Self::Smooth(b)) => RgbaF32::from_oklab(Oklab::lerp(a, b, t)),
            (Self::Vivid(a), Self::Vivid(b)) => RgbaF32::from_oklch(a.lerp(b, t)),
            _ => unreachable!("prepared stops always share the same interpolation mode"),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct PreparedGradientStop {
    position: f32,
    color: PreparedGradientColor,
}

#[derive(Debug, Clone, Copy)]
struct PreparedGradientStops {
    stops: [PreparedGradientStop; 3],
    len: usize,
}

impl PreparedGradientStops {
    fn new(renderer: &GradientRenderer) -> Self {
        let start = PreparedGradientStop {
            position: 0.0,
            color: PreparedGradientColor::from_color(renderer.color_start, renderer.interpolation),
        };
        let end = PreparedGradientStop {
            position: 1.0,
            color: PreparedGradientColor::from_color(renderer.color_end, renderer.interpolation),
        };

        if renderer.use_mid_color {
            let mid = PreparedGradientStop {
                position: renderer.midpoint.clamp(0.05, 0.95),
                color: PreparedGradientColor::from_color(
                    renderer.color_mid,
                    renderer.interpolation,
                ),
            };
            Self {
                stops: [start, mid, end],
                len: 3,
            }
        } else {
            Self {
                stops: [start, end, end],
                len: 2,
            }
        }
    }

    fn sample(self, easing: EasingMode, raw_t: f32) -> RgbaF32 {
        let t = easing.apply(raw_t);
        let first = self.stops[0];

        if t <= first.position {
            return first.color.into_rgba();
        }

        let second = self.stops[1];
        if self.len == 2 || t <= second.position {
            return interpolate_stop_pair(first, second, t);
        }

        let third = self.stops[2];
        if t <= third.position {
            return interpolate_stop_pair(second, third, t);
        }

        third.color.into_rgba()
    }
}

#[derive(Debug, Clone, Copy)]
enum PreparedGradientGeometry {
    Linear {
        center_x: f32,
        center_y: f32,
        axis_x: f32,
        axis_y: f32,
        min_extent: f32,
        inv_span: f32,
    },
    Radial {
        center_x: f32,
        center_y: f32,
        inv_max_radius: f32,
    },
}

impl PreparedGradientGeometry {
    fn new(mode: GradientMode, center_x: f32, center_y: f32, angle_degrees: f32) -> Self {
        match mode {
            GradientMode::Linear => {
                let angle = angle_degrees.to_radians();
                let axis_x = angle.cos();
                let axis_y = angle.sin();
                let project = |x: f32, y: f32| (x - center_x) * axis_x + (y - center_y) * axis_y;
                let extents = [
                    project(0.0, 0.0),
                    project(1.0, 0.0),
                    project(0.0, 1.0),
                    project(1.0, 1.0),
                ];
                let min_extent = extents.iter().copied().fold(f32::INFINITY, f32::min);
                let max_extent = extents.iter().copied().fold(f32::NEG_INFINITY, f32::max);
                let span = (max_extent - min_extent).max(f32::EPSILON);

                Self::Linear {
                    center_x,
                    center_y,
                    axis_x,
                    axis_y,
                    min_extent,
                    inv_span: span.recip(),
                }
            }
            GradientMode::Radial => {
                let max_radius = [
                    (0.0f32 - center_x).hypot(0.0 - center_y),
                    (1.0f32 - center_x).hypot(0.0 - center_y),
                    (0.0f32 - center_x).hypot(1.0 - center_y),
                    (1.0f32 - center_x).hypot(1.0 - center_y),
                ]
                .into_iter()
                .fold(0.0, f32::max)
                .max(f32::EPSILON);

                Self::Radial {
                    center_x,
                    center_y,
                    inv_max_radius: max_radius.recip(),
                }
            }
        }
    }

    fn position(self, nx: f32, ny: f32) -> f32 {
        match self {
            Self::Linear {
                center_x,
                center_y,
                axis_x,
                axis_y,
                min_extent,
                inv_span,
            } => {
                let projection = (nx - center_x) * axis_x + (ny - center_y) * axis_y;
                ((projection - min_extent) * inv_span).clamp(0.0, 1.0)
            }
            Self::Radial {
                center_x,
                center_y,
                inv_max_radius,
            } => ((nx - center_x).hypot(ny - center_y) * inv_max_radius).clamp(0.0, 1.0),
        }
    }
}

/// Animated multi-stop gradient with configurable geometry, motion, and color science.
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
    interpolation: InterpolationMode,
    /// Chroma multiplier applied in Oklch space (1.0 = neutral).
    saturation: f32,
    easing: EasingMode,
}

impl GradientRenderer {
    /// Create a vivid neon gradient with Oklch interpolation and animation disabled.
    #[must_use]
    pub fn new() -> Self {
        Self {
            color_start: [0.88, 0.08, 1.0, 1.0],
            color_mid: [1.0, 0.25, 0.55, 1.0],
            color_end: [0.0, 1.0, 0.85, 1.0],
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
            interpolation: InterpolationMode::Vivid,
            saturation: 1.0,
            easing: EasingMode::Linear,
        }
    }

    /// Post-process: boost or reduce chroma in Oklch space.
    fn apply_saturation(&self, mut rgba: RgbaF32) -> RgbaF32 {
        if (self.saturation - 1.0).abs() > f32::EPSILON {
            let mut lch = rgba.to_oklch();
            lch.c *= self.saturation;
            rgba = RgbaF32::from_oklch(lch);
        }
        rgba
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
    fn render_into(&mut self, input: &FrameInput<'_>, canvas: &mut Canvas) -> anyhow::Result<()> {
        prepare_target_canvas(canvas, input.canvas_width, input.canvas_height);
        let width = input.canvas_width.max(1) as f32;
        let height = input.canvas_height.max(1) as f32;
        let animated_offset = self.offset + input.time_secs * self.speed;
        let scale = self.scale.max(0.1);
        let geometry = PreparedGradientGeometry::new(
            self.mode,
            self.center_x,
            self.center_y,
            self.angle_degrees,
        );
        let stops = PreparedGradientStops::new(self);
        let row_len = input.canvas_width as usize * BYTES_PER_PIXEL;

        if row_len == 0 {
            return Ok(());
        }

        for (y, row) in canvas
            .as_rgba_bytes_mut()
            .chunks_exact_mut(row_len)
            .enumerate()
        {
            let ny = (y as f32 + 0.5) / height;
            for (x, pixel) in row.chunks_exact_mut(BYTES_PER_PIXEL).enumerate() {
                let nx = (x as f32 + 0.5) / width;
                let raw_t = geometry.position(nx, ny);
                let transformed = match self.mode {
                    GradientMode::Linear => ((raw_t - 0.5) / scale) + 0.5 + animated_offset,
                    GradientMode::Radial => (raw_t / scale) + animated_offset,
                };
                let t = apply_repeat_mode(transformed, self.repeat_mode);

                let mut rgba = stops.sample(self.easing, t);
                rgba = self.apply_saturation(rgba);
                rgba.r *= self.brightness;
                rgba.g *= self.brightness;
                rgba.b *= self.brightness;
                let encoded = rgba.to_srgb_u8();
                pixel.copy_from_slice(&encoded);
            }
        }

        Ok(())
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
            "interpolation" => {
                if let ControlValue::Enum(choice) | ControlValue::Text(choice) = value {
                    self.interpolation = InterpolationMode::from_str(choice);
                }
            }
            "saturation" => {
                if let Some(v) = value.as_f32() {
                    self.saturation = v.clamp(0.5, 1.5);
                }
            }
            "easing" => {
                if let ControlValue::Enum(choice) | ControlValue::Text(choice) = value {
                    self.easing = EasingMode::from_str(choice);
                }
            }
            _ => {}
        }
    }

    fn destroy(&mut self) {}
}

// ── Color Conversion Helpers ─────────────────────────────────────────────────

fn interpolate_stop_pair(
    left: PreparedGradientStop,
    right: PreparedGradientStop,
    t: f32,
) -> RgbaF32 {
    let span = (right.position - left.position).max(f32::EPSILON);
    let local_t = ((t - left.position) / span).clamp(0.0, 1.0);
    left.color.interpolate(right.color, local_t)
}

fn color_to_rgba(color: [f32; 4]) -> RgbaF32 {
    RgbaF32::new(color[0], color[1], color[2], color[3])
}

fn rgba_to_oklab(color: [f32; 4]) -> Oklab {
    RgbaF32::new(color[0], color[1], color[2], color[3]).to_oklab()
}

fn rgba_to_oklch(color: [f32; 4]) -> Oklch {
    RgbaF32::new(color[0], color[1], color[2], color[3]).to_oklch()
}

// ── Geometry Helpers ─────────────────────────────────────────────────────────

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
