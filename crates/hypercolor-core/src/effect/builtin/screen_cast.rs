//! Screen Cast renderer — maps the latest captured screen frame onto the effect canvas.
//!
//! The renderer consumes the exact CPU surface publication leased for the
//! effect, applies a normalized crop rect, and fits that region into the
//! output canvas. GPU-resident publications carry no CPU pixels, so the
//! canvas stays black until a CPU surface is published.

use std::path::PathBuf;

use hypercolor_types::canvas::Canvas;
use hypercolor_types::control::{ControlDeltaBatch, ControlValue};
use hypercolor_types::effect::{
    ControlDefinition, EffectCategory, EffectMetadata, EffectSource, PreviewSource,
};
use hypercolor_types::viewport::{FitMode, ViewportRect};

use super::common::{builtin_effect_id, dropdown_control, rect_control, slider_control};
use crate::effect::traits::{EffectRenderer, FrameInput, prepare_target_canvas};
use crate::spatial::sample_viewport;

/// Screen-reactive renderer backed by the current capture snapshot.
pub struct ScreenCastRenderer {
    viewport: ViewportRect,
    brightness: f32,
    fit_mode: FitMode,
}

impl ScreenCastRenderer {
    /// Create a screen cast renderer with a full-frame crop.
    #[must_use]
    pub fn new() -> Self {
        Self {
            viewport: ViewportRect::full(),
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
        let Some(source) = input.screen.and_then(|screen| screen.surface_canvas()) else {
            return Ok(());
        };

        sample_viewport(
            canvas,
            &source,
            self.viewport,
            self.fit_mode,
            self.brightness,
        );

        Ok(())
    }

    fn apply_controls(&mut self, batch: &ControlDeltaBatch<'_>) -> anyhow::Result<()> {
        for (control_id, value) in batch.changes {
            match control_id.as_str() {
                "viewport" => {
                    if let ControlValue::Rect(rect) = value {
                        self.viewport =
                            ViewportRect::new(rect.x, rect.y, rect.width, rect.height).clamp();
                    }
                }
                "brightness" => {
                    if let Some(value) = value.as_effect_f32() {
                        self.brightness = value.clamp(0.0, 1.0);
                    }
                }
                "fit_mode" => {
                    if let ControlValue::Enum(mode) | ControlValue::Text(mode) = value {
                        self.fit_mode = parse_fit_mode(mode);
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
    fn destroy(&mut self) {}
}

fn parse_fit_mode(value: &str) -> FitMode {
    match value.trim().to_ascii_lowercase().as_str() {
        "cover" => FitMode::Cover,
        "stretch" => FitMode::Stretch,
        _ => FitMode::Contain,
    }
}

fn controls() -> Vec<ControlDefinition> {
    vec![
        rect_control(
            "viewport",
            "Viewport",
            ViewportRect::full(),
            "Frame",
            "Normalized crop region of the captured screen preview.",
            PreviewSource::ScreenCapture,
            None,
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
        description: "Live screen crop with contain, cover, and stretch fit modes".into(),
        category: EffectCategory::Utility,
        tags: vec![
            "screen".into(),
            "screen-reactive".into(),
            "capture".into(),
            "utility".into(),
        ],
        controls: controls(),
        presets: Vec::new(),
        audio_reactive: false,
        screen_reactive: true,
        input_reactive: false,
        source: EffectSource::Native {
            path: PathBuf::from("builtin/screen_cast"),
        },
        license: Some("Apache-2.0".into()),
    }
}
