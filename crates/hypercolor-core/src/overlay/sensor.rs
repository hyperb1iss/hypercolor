use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};
use cosmic_text::{FontSystem, SwashCache};
use tiny_skia::{
    GradientStop, LineCap, LinearGradient, Paint, PathBuilder, Pixmap, Point, Rect, SpreadMode,
    Stroke, Transform,
};

use hypercolor_types::overlay::{SensorDisplayStyle, SensorOverlayConfig};

use super::common::{
    OverlayColor, draw_text_line, format_sensor_value, lerp_color, paint_from_color,
    parse_hex_color, render_svg_template, resolve_template_path,
};
use super::{OverlayBuffer, OverlayError, OverlayInput, OverlayRenderer, OverlaySize};

const GAUGE_START_ANGLE: f32 = std::f32::consts::PI * 0.75;
const GAUGE_SWEEP: f32 = std::f32::consts::PI * 1.5;

pub struct SensorRenderer {
    config: SensorOverlayConfig,
    min_color: OverlayColor,
    max_color: OverlayColor,
    font_system: FontSystem,
    swash_cache: SwashCache,
    target_size: OverlaySize,
    template_path: Option<PathBuf>,
    template_buffer: Option<OverlayBuffer>,
}

impl SensorRenderer {
    pub fn new(config: SensorOverlayConfig) -> Result<Self> {
        let min_color = parse_hex_color(&config.color_min, "sensor overlay")?;
        let max_color = parse_hex_color(&config.color_max, "sensor overlay")?;
        let template_path = config
            .template
            .as_deref()
            .map(|path| resolve_template_path(Path::new(path), "sensor overlay template"))
            .transpose()?;

        Ok(Self {
            config,
            min_color,
            max_color,
            font_system: FontSystem::new(),
            swash_cache: SwashCache::new(),
            target_size: OverlaySize::new(1, 1),
            template_path,
            template_buffer: None,
        })
    }

    fn reload(&mut self, target_size: OverlaySize) -> Result<()> {
        self.target_size = target_size;
        self.template_buffer = self
            .template_path
            .as_deref()
            .map(|path| render_svg_template(path, target_size, "sensor overlay"))
            .transpose()?;
        Ok(())
    }

    fn render_state(&self, input: &OverlayInput<'_>) -> SensorRenderState {
        let reading = input.sensors.reading(&self.config.sensor);
        let value = reading
            .as_ref()
            .map(|reading| reading.value)
            .filter(|value| value.is_finite());
        let ratio = value.map_or(0.0, |value| self.normalized_value(value));
        let accent = lerp_color(self.min_color, self.max_color, ratio);
        let unit_text = self
            .config
            .unit_label
            .clone()
            .or_else(|| {
                reading
                    .as_ref()
                    .map(|reading| reading.unit.symbol().to_owned())
            })
            .unwrap_or_default();

        SensorRenderState {
            value_text: value.map_or_else(|| "--".to_owned(), format_sensor_value),
            unit_text,
            ratio,
            accent,
        }
    }

    fn normalized_value(&self, value: f32) -> f32 {
        ((value - self.config.range_min) / (self.config.range_max - self.config.range_min))
            .clamp(0.0, 1.0)
    }

    fn render_numeric(
        &mut self,
        pixmap: &mut Pixmap,
        state: &SensorRenderState,
        compact: bool,
    ) -> Result<()> {
        let width = self.target_size.width.max(1) as f32;
        let height = self.target_size.height.max(1) as f32;
        let value_height = if compact || state.unit_text.is_empty() {
            height
        } else {
            height * 0.7
        };
        let unit_height = (height - value_height).max(0.0);
        let font_scale = if compact { 0.72 } else { 0.68 };
        let value_font_size = fitted_font_size(
            width * 0.92,
            value_height.max(height * 0.45),
            &state.value_text,
            font_scale,
        );

        draw_text_line(
            pixmap,
            &mut self.font_system,
            &mut self.swash_cache,
            &state.value_text,
            self.config.font_family.as_deref(),
            value_font_size,
            state.accent,
            0.0,
            value_height,
        )?;

        if !compact && !state.unit_text.is_empty() {
            let unit_font_size = fitted_font_size(
                width * 0.72,
                unit_height.max(height * 0.18),
                &state.unit_text,
                0.7,
            );
            draw_text_line(
                pixmap,
                &mut self.font_system,
                &mut self.swash_cache,
                &state.unit_text,
                self.config.font_family.as_deref(),
                unit_font_size,
                state.accent.with_alpha(200),
                value_height,
                unit_height.max(height * 0.18),
            )?;
        }

        Ok(())
    }

    fn render_gauge(&mut self, pixmap: &mut Pixmap, state: &SensorRenderState) -> Result<()> {
        let width = self.target_size.width.max(1) as f32;
        let height = self.target_size.height.max(1) as f32;
        let center_x = width / 2.0;
        let center_y = height * 0.56;
        let radius = width.min(height) * 0.34;
        let stroke_width = (radius * 0.18).max(6.0);
        let track_color = self.min_color.with_alpha(72);

        if self.template_buffer.is_none() {
            let track = arc_path(
                center_x,
                center_y,
                radius,
                GAUGE_START_ANGLE,
                GAUGE_START_ANGLE + GAUGE_SWEEP,
                80,
            )?;
            let mut stroke = Stroke::default();
            stroke.width = stroke_width;
            stroke.line_cap = LineCap::Round;
            pixmap.stroke_path(
                &track,
                &paint_from_color(track_color),
                &stroke,
                Transform::identity(),
                None,
            );
        }

        if state.ratio > 0.0 {
            let progress = arc_path(
                center_x,
                center_y,
                radius,
                GAUGE_START_ANGLE,
                GAUGE_START_ANGLE + GAUGE_SWEEP * state.ratio,
                arc_segments(state.ratio),
            )?;
            let mut stroke = Stroke::default();
            stroke.width = stroke_width;
            stroke.line_cap = LineCap::Round;
            pixmap.stroke_path(
                &progress,
                &paint_from_color(state.accent),
                &stroke,
                Transform::identity(),
                None,
            );
        }

        let value_font_size = fitted_font_size(width * 0.72, height * 0.22, &state.value_text, 0.9);
        draw_text_line(
            pixmap,
            &mut self.font_system,
            &mut self.swash_cache,
            &state.value_text,
            self.config.font_family.as_deref(),
            value_font_size,
            state.accent,
            height * 0.38,
            height * 0.2,
        )?;

        if !state.unit_text.is_empty() {
            let unit_font_size =
                fitted_font_size(width * 0.52, height * 0.12, &state.unit_text, 0.9);
            draw_text_line(
                pixmap,
                &mut self.font_system,
                &mut self.swash_cache,
                &state.unit_text,
                self.config.font_family.as_deref(),
                unit_font_size,
                state.accent.with_alpha(200),
                height * 0.58,
                height * 0.12,
            )?;
        }

        Ok(())
    }

    fn render_bar(&mut self, pixmap: &mut Pixmap, state: &SensorRenderState) -> Result<()> {
        let width = self.target_size.width.max(1) as f32;
        let height = self.target_size.height.max(1) as f32;
        let padding = width.min(height) * 0.08;
        let horizontal = width >= height;

        if horizontal {
            let value_font_size =
                fitted_font_size(width * 0.8, height * 0.38, &state.value_text, 0.86);
            draw_text_line(
                pixmap,
                &mut self.font_system,
                &mut self.swash_cache,
                &state.value_text,
                self.config.font_family.as_deref(),
                value_font_size,
                state.accent,
                0.0,
                height * 0.45,
            )?;

            let track_rect = Rect::from_xywh(
                padding,
                height * 0.63,
                (width - padding * 2.0).max(1.0),
                (height * 0.18).max(6.0),
            )
            .ok_or_else(|| anyhow!("failed to build sensor bar track"))?;
            pixmap.fill_rect(
                track_rect,
                &paint_from_color(self.min_color.with_alpha(72)),
                Transform::identity(),
                None,
            );

            let fill_width = track_rect.width() * state.ratio;
            if fill_width > 0.0 {
                let fill_rect = Rect::from_xywh(
                    track_rect.x(),
                    track_rect.y(),
                    fill_width.max(1.0),
                    track_rect.height(),
                )
                .ok_or_else(|| anyhow!("failed to build sensor bar fill"))?;
                pixmap.fill_rect(
                    fill_rect,
                    &gradient_paint(
                        Point::from_xy(track_rect.x(), track_rect.y()),
                        Point::from_xy(track_rect.right(), track_rect.y()),
                        self.min_color,
                        self.max_color,
                        state.accent,
                    ),
                    Transform::identity(),
                    None,
                );
            }
        } else {
            let value_font_size =
                fitted_font_size(width * 0.88, height * 0.2, &state.value_text, 0.9);
            draw_text_line(
                pixmap,
                &mut self.font_system,
                &mut self.swash_cache,
                &state.value_text,
                self.config.font_family.as_deref(),
                value_font_size,
                state.accent,
                0.0,
                height * 0.24,
            )?;

            let track_rect = Rect::from_xywh(
                width * 0.34,
                height * 0.28,
                (width * 0.32).max(6.0),
                (height - height * 0.38 - padding).max(1.0),
            )
            .ok_or_else(|| anyhow!("failed to build vertical sensor bar track"))?;
            pixmap.fill_rect(
                track_rect,
                &paint_from_color(self.min_color.with_alpha(72)),
                Transform::identity(),
                None,
            );

            let fill_height = track_rect.height() * state.ratio;
            if fill_height > 0.0 {
                let fill_rect = Rect::from_xywh(
                    track_rect.x(),
                    track_rect.bottom() - fill_height.max(1.0),
                    track_rect.width(),
                    fill_height.max(1.0),
                )
                .ok_or_else(|| anyhow!("failed to build vertical sensor bar fill"))?;
                pixmap.fill_rect(
                    fill_rect,
                    &gradient_paint(
                        Point::from_xy(track_rect.x(), track_rect.bottom()),
                        Point::from_xy(track_rect.x(), track_rect.y()),
                        self.min_color,
                        self.max_color,
                        state.accent,
                    ),
                    Transform::identity(),
                    None,
                );
            }
        }

        Ok(())
    }
}

impl OverlayRenderer for SensorRenderer {
    fn init(&mut self, target_size: OverlaySize) -> Result<()> {
        self.reload(target_size)
    }

    fn resize(&mut self, target_size: OverlaySize) -> Result<()> {
        self.reload(target_size)
    }

    fn render_into(
        &mut self,
        input: &OverlayInput<'_>,
        target: &mut OverlayBuffer,
    ) -> std::result::Result<(), OverlayError> {
        if target.width != self.target_size.width || target.height != self.target_size.height {
            return Err(OverlayError::Fatal(format!(
                "sensor overlay target mismatch: renderer prepared {}x{}, target was {}x{}",
                self.target_size.width, self.target_size.height, target.width, target.height
            )));
        }

        let mut pixmap =
            Pixmap::new(target.width.max(1), target.height.max(1)).ok_or_else(|| {
                OverlayError::Fatal("failed to allocate sensor overlay pixmap".to_owned())
            })?;
        if let Some(background) = &self.template_buffer {
            pixmap.data_mut().copy_from_slice(&background.pixels);
        }

        let state = self.render_state(input);
        let render_result = match self.config.style {
            SensorDisplayStyle::Numeric => self.render_numeric(&mut pixmap, &state, false),
            SensorDisplayStyle::Minimal => self.render_numeric(&mut pixmap, &state, true),
            SensorDisplayStyle::Gauge => self.render_gauge(&mut pixmap, &state),
            SensorDisplayStyle::Bar => self.render_bar(&mut pixmap, &state),
        };
        render_result.map_err(|error| OverlayError::Fatal(error.to_string()))?;

        target
            .copy_from_pixmap(&pixmap)
            .map_err(|error| OverlayError::Fatal(error.to_string()))?;
        Ok(())
    }

    fn content_changed(&self, _input: &OverlayInput<'_>) -> bool {
        false
    }
}

struct SensorRenderState {
    value_text: String,
    unit_text: String,
    ratio: f32,
    accent: OverlayColor,
}

fn fitted_font_size(width: f32, height: f32, text: &str, height_ratio: f32) -> f32 {
    let char_count = text.chars().count().max(1) as f32;
    let width_limited = width / char_count * 1.85;
    width_limited.min(height * height_ratio).max(1.0)
}

fn arc_segments(ratio: f32) -> usize {
    ((ratio.clamp(0.05, 1.0) * 64.0).round() as usize).max(8)
}

fn arc_path(
    center_x: f32,
    center_y: f32,
    radius: f32,
    start_angle: f32,
    end_angle: f32,
    segments: usize,
) -> Result<tiny_skia::Path> {
    let mut builder = PathBuilder::new();
    let start = polar_point(center_x, center_y, radius, start_angle);
    builder.move_to(start.0, start.1);

    let sweep = end_angle - start_angle;
    for step in 1..=segments.max(1) {
        let progress = step as f32 / segments.max(1) as f32;
        let angle = start_angle + sweep * progress;
        let point = polar_point(center_x, center_y, radius, angle);
        builder.line_to(point.0, point.1);
    }

    builder
        .finish()
        .ok_or_else(|| anyhow!("failed to build sensor gauge arc"))
}

fn polar_point(center_x: f32, center_y: f32, distance: f32, angle: f32) -> (f32, f32) {
    (
        center_x + distance * angle.cos(),
        center_y + distance * angle.sin(),
    )
}

fn gradient_paint(
    start: Point,
    end: Point,
    min_color: OverlayColor,
    max_color: OverlayColor,
    fallback: OverlayColor,
) -> Paint<'static> {
    let mut paint = paint_from_color(fallback);
    if let Some(shader) = LinearGradient::new(
        start,
        end,
        vec![
            GradientStop::new(0.0, min_color.skia_color()),
            GradientStop::new(1.0, max_color.skia_color()),
        ],
        SpreadMode::Pad,
        Transform::identity(),
    ) {
        paint.shader = shader;
    }
    paint
}
