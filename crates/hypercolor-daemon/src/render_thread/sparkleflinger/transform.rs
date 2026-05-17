use hypercolor_core::blend_math::apply_layer_adjust_rgba_pixels_in_place;
use hypercolor_core::types::canvas::{Canvas, Rgba};
use hypercolor_types::viewport::FitMode;

use super::{CompositionAdjust, CompositionTransform};

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
    for y in 0..target_height {
        for x in 0..target_width {
            let color = match transform.fit {
                FitMode::Tile | FitMode::Mirror => sampler.sample_repeated(source, x, y, transform),
                FitMode::Contain | FitMode::Cover | FitMode::Stretch => sampler
                    .source_normalized_for(x, y, transform)
                    .map_or(Rgba::TRANSPARENT, |(nx, ny)| source.sample_nearest(nx, ny)),
            };
            target.set_pixel(x, y, color);
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

    fn source_normalized_for(
        self,
        x: u32,
        y: u32,
        transform: CompositionTransform,
    ) -> Option<(f32, f32)> {
        let geometry = self.fit_geometry(transform.fit);
        let (local_x, local_y) = self.inverse_local_point(x, y, transform)?;
        let u = local_x / geometry.draw_width + 0.5;
        let v = local_y / geometry.draw_height + 0.5;
        if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
            return None;
        }

        let source_x = geometry.crop_x + u.mul_add(geometry.crop_width, -0.5);
        let source_y = geometry.crop_y + v.mul_add(geometry.crop_height, -0.5);
        Some((
            normalize_source_axis(source_x, self.source_width),
            normalize_source_axis(source_y, self.source_height),
        ))
    }

    fn sample_repeated(
        self,
        source: &Canvas,
        x: u32,
        y: u32,
        transform: CompositionTransform,
    ) -> Rgba {
        let Some((local_x, local_y)) = self.inverse_local_point(x, y, transform) else {
            return Rgba::TRANSPARENT;
        };
        let anchor_x = transform.anchor.x * self.target_width;
        let anchor_y = transform.anchor.y * self.target_height;
        let source_x = repeated_axis(anchor_x + local_x, source.width(), transform.fit);
        let source_y = repeated_axis(anchor_y + local_y, source.height(), transform.fit);
        source_x
            .zip(source_y)
            .map_or(Rgba::TRANSPARENT, |(sx, sy)| source.get_pixel(sx, sy))
    }

    fn inverse_local_point(
        self,
        x: u32,
        y: u32,
        transform: CompositionTransform,
    ) -> Option<(f32, f32)> {
        let anchor_x = transform.anchor.x * self.target_width;
        let anchor_y = transform.anchor.y * self.target_height;
        let dx = x as f32 + 0.5 - anchor_x;
        let dy = y as f32 + 0.5 - anchor_y;
        let scale_x = transform.scale[0].max(0.01);
        let scale_y = transform.scale[1].max(0.01);
        let cos = transform.rotation.cos();
        let sin = transform.rotation.sin();
        let local_x = cos.mul_add(dx, sin * dy) / scale_x;
        let local_y = (-sin).mul_add(dx, cos * dy) / scale_y;
        if local_x.is_finite() && local_y.is_finite() {
            Some((local_x, local_y))
        } else {
            None
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
