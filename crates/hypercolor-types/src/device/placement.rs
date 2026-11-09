use super::SegmentPlacement;
use crate::spatial::NormalizedPosition;

impl SegmentPlacement {
    /// Fit sibling boxes into a canvas region with one uniform physical scale.
    ///
    /// Input coordinates share the device's isotropic plane. Output coordinates
    /// are normalized to the canvas, preserving physical proportions even on a
    /// non-square canvas. Missing or invalid boxes remain `None`.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn fit_all(
        placements: &[Option<Self>],
        center: NormalizedPosition,
        bounds: NormalizedPosition,
        canvas: (u32, u32),
    ) -> Vec<Option<Self>> {
        let valid = |placement: &Self| {
            [
                placement.position.x,
                placement.position.y,
                placement.size.x,
                placement.size.y,
                placement.rotation,
            ]
            .iter()
            .all(|value| value.is_finite())
                && placement.size.x > 0.0
                && placement.size.y > 0.0
        };
        if ![center.x, center.y, bounds.x, bounds.y]
            .iter()
            .all(|value| value.is_finite())
            || bounds.x <= 0.0
            || bounds.y <= 0.0
        {
            return vec![None; placements.len()];
        }
        let mut min_x = f32::INFINITY;
        let mut min_y = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut max_y = f32::NEG_INFINITY;
        for placement in placements.iter().flatten().filter(|p| valid(p)) {
            let (sin, cos) = placement.rotation.sin_cos();
            let half_x = (placement.size.x * cos.abs()).midpoint(placement.size.y * sin.abs());
            let half_y = (placement.size.x * sin.abs()).midpoint(placement.size.y * cos.abs());
            min_x = min_x.min(placement.position.x - half_x);
            max_x = max_x.max(placement.position.x + half_x);
            min_y = min_y.min(placement.position.y - half_y);
            max_y = max_y.max(placement.position.y + half_y);
        }
        let width = max_x - min_x;
        let height = max_y - min_y;
        if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
            return vec![None; placements.len()];
        }
        let aspect = canvas.0.max(1) as f32 / canvas.1.max(1) as f32;
        let scale = (bounds.x * aspect / width).min(bounds.y / height);
        let midpoint = NormalizedPosition::new(min_x + width * 0.5, min_y + height * 0.5);
        placements
            .iter()
            .map(|placement| {
                placement.filter(valid).map(|placement| Self {
                    position: NormalizedPosition::new(
                        center.x + (placement.position.x - midpoint.x) * scale / aspect,
                        center.y + (placement.position.y - midpoint.y) * scale,
                    ),
                    size: NormalizedPosition::new(
                        placement.size.x * scale / aspect,
                        placement.size.y * scale,
                    ),
                    rotation: placement.rotation,
                })
            })
            .collect()
    }
}
