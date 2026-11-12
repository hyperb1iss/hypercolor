use hypercolor_core::blend_math::apply_layer_adjust_rgba_pixels_in_place;
use hypercolor_core::spatial::{FrameEdge, LayerPlacement};
use hypercolor_types::canvas::{Canvas, Rgba};
use hypercolor_types::viewport::FitMode;

use super::{CompositionAdjust, CompositionTransform};

/// The smallest scale a layer's transform draws at, as a fraction of the
/// target. The compose shader holds the same number.
const MINIMUM_SCALE: f32 = 0.01;

pub(super) fn process_layer_canvas(
    source: Canvas,
    target_width: u32,
    target_height: u32,
    transform: Option<CompositionTransform>,
    adjust: Option<CompositionAdjust>,
) -> Canvas {
    let transform_required =
        transform.is_some() || source.width() != target_width || source.height() != target_height;
    let mut canvas = if transform_required {
        sample_transformed_layer(
            &source,
            target_width,
            target_height,
            transform.unwrap_or_default(),
        )
    } else {
        source
    };

    if let Some(adjust) = adjust {
        apply_layer_adjust_rgba_pixels_in_place(
            canvas.as_rgba_bytes_mut(),
            &adjust.to_layer_adjust(),
        );
    }

    canvas
}

/// Draw a placed layer's source onto a transparent canvas of the target's
/// size: each target pixel the placement draws on takes the source texel
/// it gives that pixel, and every other pixel stays transparent, so the
/// blend that follows leaves the target alone there.
///
/// `opaque_frame` makes every pixel the placement draws on opaque, black
/// where the placed source does not reach: how a placed replace covers its
/// region.
pub(super) fn process_placed_layer_canvas(
    source: &Canvas,
    target_width: u32,
    target_height: u32,
    placement: &LayerPlacement,
    opaque_frame: bool,
    adjust: Option<CompositionAdjust>,
) -> Canvas {
    let mut target = Canvas::new(target_width, target_height);
    if source.width() == 0 || source.height() == 0 {
        return target;
    }
    target.fill(Rgba::TRANSPARENT);
    // A clipped frame draws inside its own box; one whose edge runs on
    // draws on every pixel.
    let (columns, rows) = match placement.edge {
        FrameEdge::Extend => (0..target_width, 0..target_height),
        FrameEdge::Clip => {
            let Some([left, top, right, bottom]) = placement.frame_bounds() else {
                return target;
            };
            (
                pixel_span(left, right, target_width),
                pixel_span(top, bottom, target_height),
            )
        }
    };
    for y in rows {
        for x in columns.clone() {
            let zone_px = [x as f32 + 0.5, y as f32 + 0.5];
            if !placement.covers(zone_px) {
                continue;
            }
            let mut color = placement.source_texel(zone_px).map_or(
                Rgba::TRANSPARENT,
                |[source_x, source_y]| {
                    source.get_pixel(
                        source_x.min(source.width() - 1),
                        source_y.min(source.height() - 1),
                    )
                },
            );
            if opaque_frame {
                color.a = 255;
            }
            target.set_pixel(x, y, color);
        }
    }
    if let Some(adjust) = adjust {
        apply_layer_adjust_rgba_pixels_in_place(
            target.as_rgba_bytes_mut(),
            &adjust.to_layer_adjust(),
        );
    }
    target
}

/// The pixels of an axis `extent` long whose centres can fall between `low`
/// and `high`, with one pixel of slack either side.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::as_conversions,
    reason = "the bounds are clamped to the canvas before they become indices"
)]
fn pixel_span(low: f32, high: f32, extent: u32) -> std::ops::Range<u32> {
    let limit = extent as f32;
    let start = (low.floor() - 1.0).clamp(0.0, limit) as u32;
    let end = (high.ceil() + 1.0).clamp(0.0, limit) as u32;
    start..end.max(start)
}

fn sample_transformed_layer(
    source: &Canvas,
    target_width: u32,
    target_height: u32,
    transform: CompositionTransform,
) -> Canvas {
    if source.width() == 0 || source.height() == 0 || target_width == 0 || target_height == 0 {
        return Canvas::new(target_width, target_height);
    }

    let mut target = Canvas::new(target_width, target_height);
    let sampler = LayerSampler::new(source.width(), source.height(), target_width, target_height);
    let prepared = sampler.prepare(transform);
    for y in 0..target_height {
        let (mut local_x, mut local_y) = prepared.row_origin(y);
        for x in 0..target_width {
            let color = match transform.fit {
                FitMode::Tile | FitMode::Mirror => {
                    prepared.sample_repeated(source, local_x, local_y)
                }
                FitMode::Contain | FitMode::Cover | FitMode::Stretch => prepared
                    .source_normalized_for(local_x, local_y)
                    .map_or(Rgba::TRANSPARENT, |(nx, ny)| source.sample_nearest(nx, ny)),
            };
            target.set_pixel(x, y, color);
            local_x += prepared.step_x.0;
            local_y += prepared.step_x.1;
        }
    }
    target
}

#[derive(Debug, Clone, Copy)]
struct LayerSampler {
    source_width: f32,
    source_height: f32,
    target_width: f32,
    target_height: f32,
}

impl LayerSampler {
    fn new(source_width: u32, source_height: u32, target_width: u32, target_height: u32) -> Self {
        Self {
            source_width: source_width as f32,
            source_height: source_height as f32,
            target_width: target_width as f32,
            target_height: target_height as f32,
        }
    }

    fn prepare(self, transform: CompositionTransform) -> PreparedLayerSampler {
        let scale_x = transform.scale[0].max(MINIMUM_SCALE);
        let scale_y = transform.scale[1].max(MINIMUM_SCALE);
        let (sin, cos) = transform.rotation.sin_cos();
        let step_x = (cos / scale_x, -sin / scale_y);
        let step_y = (sin / scale_x, cos / scale_y);
        let dx = 0.5 - transform.anchor.x * self.target_width;
        let dy = 0.5 - transform.anchor.y * self.target_height;

        PreparedLayerSampler {
            sampler: self,
            transform,
            geometry: self.fit_geometry(transform.fit),
            origin: (
                cos.mul_add(dx, sin * dy) / scale_x,
                (-sin).mul_add(dx, cos * dy) / scale_y,
            ),
            step_x,
            step_y,
            repeat_anchor: (
                transform.anchor.x * self.target_width,
                transform.anchor.y * self.target_height,
            ),
        }
    }

    fn fit_geometry(self, fit: FitMode) -> FitGeometry {
        match fit {
            FitMode::Stretch | FitMode::Tile | FitMode::Mirror => FitGeometry {
                draw_width: self.target_width,
                draw_height: self.target_height,
                crop_x: 0.0,
                crop_y: 0.0,
                crop_width: self.source_width,
                crop_height: self.source_height,
            },
            FitMode::Contain => {
                let source_aspect = self.source_width / self.source_height;
                let target_aspect = self.target_width / self.target_height;
                let (draw_width, draw_height) = if target_aspect > source_aspect {
                    (self.target_height * source_aspect, self.target_height)
                } else {
                    (self.target_width, self.target_width / source_aspect)
                };
                FitGeometry {
                    draw_width,
                    draw_height,
                    crop_x: 0.0,
                    crop_y: 0.0,
                    crop_width: self.source_width,
                    crop_height: self.source_height,
                }
            }
            FitMode::Cover => {
                let source_aspect = self.source_width / self.source_height;
                let target_aspect = self.target_width / self.target_height;
                let (crop_x, crop_y, crop_width, crop_height) = if target_aspect > source_aspect {
                    let crop_height = self.source_width / target_aspect;
                    (
                        0.0,
                        (self.source_height - crop_height) * 0.5,
                        self.source_width,
                        crop_height,
                    )
                } else {
                    let crop_width = self.source_height * target_aspect;
                    (
                        (self.source_width - crop_width) * 0.5,
                        0.0,
                        crop_width,
                        self.source_height,
                    )
                };
                FitGeometry {
                    draw_width: self.target_width,
                    draw_height: self.target_height,
                    crop_x,
                    crop_y,
                    crop_width,
                    crop_height,
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct PreparedLayerSampler {
    sampler: LayerSampler,
    transform: CompositionTransform,
    geometry: FitGeometry,
    origin: (f32, f32),
    step_x: (f32, f32),
    step_y: (f32, f32),
    repeat_anchor: (f32, f32),
}

impl PreparedLayerSampler {
    fn row_origin(self, y: u32) -> (f32, f32) {
        let y = y as f32;
        (
            self.step_y.0.mul_add(y, self.origin.0),
            self.step_y.1.mul_add(y, self.origin.1),
        )
    }

    fn source_normalized_for(self, local_x: f32, local_y: f32) -> Option<(f32, f32)> {
        if !local_x.is_finite() || !local_y.is_finite() {
            return None;
        }
        let u = local_x / self.geometry.draw_width + 0.5;
        let v = local_y / self.geometry.draw_height + 0.5;
        if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
            return None;
        }

        let source_x = self.geometry.crop_x + u.mul_add(self.geometry.crop_width, -0.5);
        let source_y = self.geometry.crop_y + v.mul_add(self.geometry.crop_height, -0.5);
        Some((
            normalize_source_axis(source_x, self.sampler.source_width),
            normalize_source_axis(source_y, self.sampler.source_height),
        ))
    }

    fn sample_repeated(self, source: &Canvas, local_x: f32, local_y: f32) -> Rgba {
        if !local_x.is_finite() || !local_y.is_finite() {
            return Rgba::TRANSPARENT;
        }
        let source_x = repeated_axis(
            self.repeat_anchor.0 + local_x,
            source.width(),
            self.transform.fit,
        );
        let source_y = repeated_axis(
            self.repeat_anchor.1 + local_y,
            source.height(),
            self.transform.fit,
        );
        source_x
            .zip(source_y)
            .map_or(Rgba::TRANSPARENT, |(sx, sy)| source.get_pixel(sx, sy))
    }
}

#[derive(Debug, Clone, Copy)]
struct FitGeometry {
    draw_width: f32,
    draw_height: f32,
    crop_x: f32,
    crop_y: f32,
    crop_width: f32,
    crop_height: f32,
}

fn normalize_source_axis(value: f32, extent: f32) -> f32 {
    if extent <= 1.0 {
        0.0
    } else {
        (value / (extent - 1.0)).clamp(0.0, 1.0)
    }
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::as_conversions,
    reason = "bounded canvas coordinates are reduced into a valid repeated pixel index"
)]
fn repeated_axis(value: f32, extent: u32, fit: FitMode) -> Option<u32> {
    if extent == 0 || !value.is_finite() {
        return None;
    }
    let index = value.floor() as i64;
    let extent_i = i64::from(extent);
    if fit != FitMode::Mirror || extent == 1 {
        return u32::try_from(index.rem_euclid(extent_i)).ok();
    }

    let period = extent_i.saturating_mul(2);
    let phase = index.rem_euclid(period);
    let mirrored = if phase < extent_i {
        phase
    } else {
        period.saturating_sub(1).saturating_sub(phase)
    };
    u32::try_from(mirrored).ok()
}
