//! Temporal smoothing — EMA with scene-cut detection.
//!
//! Prevents LED flicker during action scenes and fast camera pans by applying
//! an exponential moving average per zone. Scene cuts are detected when frame
//! difference exceeds a threshold, causing an immediate reset to the new colors.

use crate::types::canvas::{linear_to_srgb_u8, srgb_u8_to_linear};

// ── TemporalSmoother ──────────────────────────────────────────────────────

/// Per-zone exponential moving average smoother with scene-cut detection.
///
/// Each zone's R, G, B channels are smoothed independently. When the total
/// frame difference (sum of absolute channel deltas across all zones) exceeds
/// the scene-cut threshold, smoothing is bypassed for that frame so the new
/// scene snaps in immediately.
#[derive(Debug, Clone)]
pub struct TemporalSmoother {
    /// EMA factor: 0.0 = frozen (infinite smoothing), 1.0 = no smoothing.
    /// Typical range: 0.1 (cinema) to 0.5 (gaming).
    alpha: f32,

    /// Scene-cut detection threshold. When the per-frame difference metric
    /// exceeds this value, smoothing is reset. Higher = less sensitive.
    scene_cut_threshold: f32,

    /// Previous frame's smoothed colors, one `[R, G, B]` per zone.
    ///
    /// Values are stored in linear-light byte units (`0.0..=255.0`) so the
    /// scene-cut threshold can stay on the same rough scale as before while
    /// avoiding gamma-space EMA artifacts.
    prev: Vec<[f32; 3]>,
}

impl TemporalSmoother {
    /// Create a new smoother.
    ///
    /// * `alpha` — Smoothing factor, clamped to `0.0..=1.0`. Default: `0.3`.
    /// * `scene_cut_threshold` — Frame difference threshold for scene-cut
    ///   detection. Default: `100.0` (sum of absolute channel deltas).
    #[must_use]
    pub fn new(alpha: f32, scene_cut_threshold: f32) -> Self {
        Self {
            alpha: alpha.clamp(0.0, 1.0),
            scene_cut_threshold,
            prev: Vec::new(),
        }
    }

    /// Create a smoother with default parameters.
    ///
    /// Alpha: `0.3`, scene-cut threshold: `100.0`.
    #[must_use]
    pub fn default_params() -> Self {
        Self::new(0.3, 100.0)
    }

    /// Current smoothing factor.
    #[must_use]
    pub fn alpha(&self) -> f32 {
        self.alpha
    }

    /// Update alpha at runtime (e.g., when switching between cinema/gaming profiles).
    pub fn set_alpha(&mut self, alpha: f32) {
        self.alpha = alpha.clamp(0.0, 1.0);
    }

    /// Apply temporal smoothing to a set of zone colors in-place.
    ///
    /// Each entry in `colors` is `[R, G, B]` as `u8`. The smoother maintains
    /// internal state across calls. First call initializes state without smoothing.
    ///
    /// # Scene-Cut Detection
    ///
    /// The frame difference metric is the sum of absolute per-channel deltas
    /// across all zones: `sum(|prev_r - new_r| + |prev_g - new_g| + |prev_b - new_b|)`.
    /// When this exceeds `scene_cut_threshold`, the smoother copies the new
    /// colors directly (no blending), effectively resetting to the new scene.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::as_conversions
    )]
    pub fn apply(&mut self, colors: &mut [[u8; 3]]) {
        // First frame or zone count changed — initialize without smoothing.
        if self.prev.len() != colors.len() {
            self.prev = colors
                .iter()
                .map(|c| {
                    [
                        srgb_u8_to_linear(c[0]) * 255.0,
                        srgb_u8_to_linear(c[1]) * 255.0,
                        srgb_u8_to_linear(c[2]) * 255.0,
                    ]
                })
                .collect();
            return;
        }

        // Compute frame difference metric.
        let diff = self.frame_difference(colors);

        // Scene cut detected — snap to new colors immediately.
        if diff > self.scene_cut_threshold {
            self.prev = colors
                .iter()
                .map(|c| {
                    [
                        srgb_u8_to_linear(c[0]) * 255.0,
                        srgb_u8_to_linear(c[1]) * 255.0,
                        srgb_u8_to_linear(c[2]) * 255.0,
                    ]
                })
                .collect();
            return;
        }

        // Normal EMA smoothing: smoothed = prev + alpha * (new - prev)
        let alpha = self.alpha;
        for (i, color) in colors.iter_mut().enumerate() {
            let prev = &mut self.prev[i];
            let new_r = srgb_u8_to_linear(color[0]) * 255.0;
            let new_g = srgb_u8_to_linear(color[1]) * 255.0;
            let new_b = srgb_u8_to_linear(color[2]) * 255.0;

            prev[0] += alpha * (new_r - prev[0]);
            prev[1] += alpha * (new_g - prev[1]);
            prev[2] += alpha * (new_b - prev[2]);

            color[0] = linear_to_srgb_u8((prev[0] / 255.0).clamp(0.0, 1.0));
            color[1] = linear_to_srgb_u8((prev[1] / 255.0).clamp(0.0, 1.0));
            color[2] = linear_to_srgb_u8((prev[2] / 255.0).clamp(0.0, 1.0));
        }
    }

    /// Reset internal state. Next call to `apply` will initialize fresh.
    pub fn reset(&mut self) {
        self.prev.clear();
    }

    /// Compute frame difference metric: sum of absolute per-channel deltas.
    fn frame_difference(&self, colors: &[[u8; 3]]) -> f32 {
        self.prev
            .iter()
            .zip(colors.iter())
            .map(|(prev, new)| {
                let dr = (prev[0] - (srgb_u8_to_linear(new[0]) * 255.0)).abs();
                let dg = (prev[1] - (srgb_u8_to_linear(new[1]) * 255.0)).abs();
                let db = (prev[2] - (srgb_u8_to_linear(new[2]) * 255.0)).abs();
                dr + dg + db
            })
            .sum()
    }
}
