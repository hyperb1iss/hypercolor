//! Screen Cast renderer — maps the latest captured screen frame onto the effect canvas.
//!
//! The renderer consumes a downscaled screen snapshot from the input pipeline,
//! applies a normalized crop rect, and fits that region into the output canvas.

use std::path::PathBuf;

use hypercolor_types::canvas::{Canvas, RgbaF32};
use hypercolor_types::effect::{
    ControlDefinition, ControlValue, EffectCategory, EffectMetadata, EffectSource,
};

use super::common::{builtin_effect_id, dropdown_control, slider_control};
use crate::effect::traits::{EffectRenderer, FrameInput, prepare_target_canvas};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FitMode {
    Contain,
    Cover,
    Stretch,
}

impl FitMode {
    fn from_str(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "cover" => Self::Cover,
            "stretch" => Self::Stretch,
            _ => Self::Contain,
        }
    }
}

/// Screen-reactive renderer backed by the current capture snapshot.
pub struct ScreenCastRenderer {
    frame_x: f32,
    frame_y: f32,
    frame_width: f32,
    frame_height: f32,
    brightness: f32,
    fit_mode: FitMode,
}

impl ScreenCastRenderer {
    /// Create a screen cast renderer with a full-frame crop.
    #[must_use]
    pub fn new() -> Self {
        Self {
            frame_x: 0.0,
            frame_y: 0.0,
            frame_width: 1.0,
            frame_height: 1.0,
            brightness: 1.0,
            fit_mode: FitMode::Contain,
        }
    }
}

impl Default for ScreenCastRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl EffectRenderer for ScreenCastRenderer {
    fn init(&mut self, _metadata: &EffectMetadata) -> anyhow::Result<()> {
        Ok(())
    }

    #[allow(clippy::cast_precision_loss, clippy::as_conversions)]
    fn render_into(&mut self, input: &FrameInput<'_>, canvas: &mut Canvas) -> anyhow::Result<()> {
        prepare_target_canvas(canvas, input.canvas_width, input.canvas_height);
        canvas.clear();
        let Some(screen) = input.screen else {
            return Ok(());
        };
        let Some(source_surface) = screen.canvas_downscale.as_ref() else {
            return Ok(());
        };
        let source = Canvas::from_published_surface(source_surface);

        let crop = normalized_crop(
            source.width(),
            source.height(),
            self.frame_x,
            self.frame_y,
            self.frame_width,
            self.frame_height,
        );

        match self.fit_mode {
            FitMode::Stretch => blit_stretch(canvas, &source, crop, self.brightness),
            FitMode::Contain => blit_contain(canvas, &source, crop, self.brightness),
            FitMode::Cover => blit_cover(canvas, &source, crop, self.brightness),
        }

        Ok(())
    }

    fn set_control(&mut self, name: &str, value: &ControlValue) {
        match name {
            "frame_x" => {
                if let Some(v) = value.as_f32() {
                    self.frame_x = v.clamp(0.0, 1.0);
                }
            }
            "frame_y" => {
                if let Some(v) = value.as_f32() {
                    self.frame_y = v.clamp(0.0, 1.0);
                }
            }
            "frame_width" => {
                if let Some(v) = value.as_f32() {
                    self.frame_width = v.clamp(0.05, 1.0);
                }
            }
            "frame_height" => {
                if let Some(v) = value.as_f32() {
                    self.frame_height = v.clamp(0.05, 1.0);
                }
            }
            "brightness" => {
                if let Some(v) = value.as_f32() {
                    self.brightness = v.clamp(0.0, 1.0);
                }
            }
            "fit_mode" => {
                if let ControlValue::Enum(mode) | ControlValue::Text(mode) = value {
                    self.fit_mode = FitMode::from_str(mode);
                }
            }
            _ => {}
        }
    }

    fn destroy(&mut self) {}
}

#[derive(Debug, Clone, Copy)]
struct SourceRect {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

#[allow(clippy::cast_precision_loss, clippy::as_conversions)]
fn normalized_crop(
    source_width: u32,
    source_height: u32,
    frame_x: f32,
    frame_y: f32,
    frame_width: f32,
    frame_height: f32,
) -> SourceRect {
    let crop_width = frame_width.clamp(0.05, 1.0);
    let crop_height = frame_height.clamp(0.05, 1.0);
    let x = frame_x.clamp(0.0, (1.0 - crop_width).max(0.0));
    let y = frame_y.clamp(0.0, (1.0 - crop_height).max(0.0));

    SourceRect {
        x: x * source_width.max(1) as f32,
        y: y * source_height.max(1) as f32,
        width: (crop_width * source_width.max(1) as f32).max(1.0),
        height: (crop_height * source_height.max(1) as f32).max(1.0),
    }
}

#[allow(clippy::cast_precision_loss, clippy::as_conversions)]
fn blit_stretch(canvas: &mut Canvas, source: &Canvas, crop: SourceRect, brightness: f32) {
    let out_width = canvas.width().max(1) as f32;
    let out_height = canvas.height().max(1) as f32;
    for y in 0..canvas.height() {
        let ny = (y as f32 + 0.5) / out_height;
        for x in 0..canvas.width() {
            let nx = (x as f32 + 0.5) / out_width;
            let pixel = sample_source(
                source,
                crop.x + nx * crop.width,
                crop.y + ny * crop.height,
                brightness,
            );
            canvas.set_pixel(x, y, pixel);
        }
    }
}

#[allow(clippy::cast_precision_loss, clippy::as_conversions)]
fn blit_contain(canvas: &mut Canvas, source: &Canvas, crop: SourceRect, brightness: f32) {
    let crop_aspect = crop.width / crop.height.max(f32::EPSILON);
    let out_width = canvas.width().max(1) as f32;
    let out_height = canvas.height().max(1) as f32;
    let out_aspect = out_width / out_height;

    let (draw_width, draw_height) = if out_aspect > crop_aspect {
        (out_height * crop_aspect, out_height)
    } else {
        (out_width, out_width / crop_aspect)
    };
    let offset_x = (out_width - draw_width) * 0.5;
    let offset_y = (out_height - draw_height) * 0.5;

    for y in 0..canvas.height() {
        let yf = y as f32 + 0.5;
        if yf < offset_y || yf > offset_y + draw_height {
            continue;
        }
        let ny = ((yf - offset_y) / draw_height).clamp(0.0, 1.0);
        for x in 0..canvas.width() {
            let xf = x as f32 + 0.5;
            if xf < offset_x || xf > offset_x + draw_width {
                continue;
            }
            let nx = ((xf - offset_x) / draw_width).clamp(0.0, 1.0);
            let pixel = sample_source(
                source,
                crop.x + nx * crop.width,
                crop.y + ny * crop.height,
                brightness,
            );
            canvas.set_pixel(x, y, pixel);
        }
    }
}

#[allow(clippy::cast_precision_loss, clippy::as_conversions)]
fn blit_cover(canvas: &mut Canvas, source: &Canvas, crop: SourceRect, brightness: f32) {
    let out_aspect = canvas.width().max(1) as f32 / canvas.height().max(1) as f32;
    let crop_aspect = crop.width / crop.height.max(f32::EPSILON);
    let mut fitted = crop;

    if out_aspect > crop_aspect {
        fitted.height = (crop.width / out_aspect).max(1.0);
        fitted.y += (crop.height - fitted.height).max(0.0) * 0.5;
    } else if out_aspect < crop_aspect {
        fitted.width = (crop.height * out_aspect).max(1.0);
        fitted.x += (crop.width - fitted.width).max(0.0) * 0.5;
    }

    blit_stretch(canvas, source, fitted, brightness);
}

#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::as_conversions
)]
fn sample_source(
    source: &Canvas,
    x: f32,
    y: f32,
    brightness: f32,
) -> hypercolor_types::canvas::Rgba {
    let src_x = x
        .floor()
        .clamp(0.0, source.width().saturating_sub(1) as f32) as u32;
    let src_y = y
        .floor()
        .clamp(0.0, source.height().saturating_sub(1) as f32) as u32;
    let pixel = source.get_pixel(src_x, src_y).to_linear_f32();
    let scaled = RgbaF32::new(
        pixel.r * brightness,
        pixel.g * brightness,
        pixel.b * brightness,
        pixel.a,
    );
    scaled.to_srgba()
}

fn controls() -> Vec<ControlDefinition> {
    vec![
        slider_control(
            "frame_x",
            "Frame X",
            0.0,
            0.0,
            1.0,
            0.01,
            "Frame",
            "Normalized left edge of the capture frame.",
        ),
        slider_control(
            "frame_y",
            "Frame Y",
            0.0,
            0.0,
            1.0,
            0.01,
            "Frame",
            "Normalized top edge of the capture frame.",
        ),
        slider_control(
            "frame_width",
            "Frame Width",
            1.0,
            0.05,
            1.0,
            0.01,
            "Frame",
            "Normalized width of the captured region.",
        ),
        slider_control(
            "frame_height",
            "Frame Height",
            1.0,
            0.05,
            1.0,
            0.01,
            "Frame",
            "Normalized height of the captured region.",
        ),
        dropdown_control(
            "fit_mode",
            "Fit Mode",
            "Contain",
            &["Contain", "Cover", "Stretch"],
            "Frame",
            "How the selected capture frame maps onto the effect canvas.",
        ),
        slider_control(
            "brightness",
            "Brightness",
            1.0,
            0.0,
            1.0,
            0.01,
            "Output",
            "Master output brightness for the sampled screen image.",
        ),
    ]
}

pub(super) fn metadata() -> EffectMetadata {
    EffectMetadata {
        id: builtin_effect_id("screen_cast"),
        name: "Screen Cast".into(),
        author: "Hypercolor".into(),
        version: "0.1.0".into(),
        description: "Live Wayland screen crop with contain, cover, and stretch fit modes".into(),
        category: EffectCategory::Utility,
        tags: vec![
            "screen".into(),
            "capture".into(),
            "utility".into(),
            "wayland".into(),
        ],
        controls: controls(),
        presets: Vec::new(),
        audio_reactive: false,
        screen_reactive: true,
        source: EffectSource::Native {
            path: PathBuf::from("builtin/screen_cast"),
        },
        license: Some("Apache-2.0".into()),
    }
}
