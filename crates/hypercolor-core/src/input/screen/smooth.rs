//! Temporal smoothing — EMA with scene-cut detection.
//!
//! Prevents LED flicker during action scenes and fast camera pans by applying
//! an exponential moving average per zone. Scene cuts are detected when frame
//! difference exceeds a threshold, causing an immediate reset to the new colors.

use std::time::Duration;

use crate::types::canvas::{linear_to_srgb_u8, srgb_u8_to_linear};

const REFERENCE_FPS: f32 = 60.0;

// ── TemporalSmoother ──────────────────────────────────────────────────────

/// Per-zone exponential moving average smoother with scene-cut detection.
///
/// Each zone's R, G, B channels are smoothed independently. When the mean
/// per-zone channel difference exceeds the scene-cut threshold, smoothing is
/// bypassed for that frame so the new scene snaps in immediately.
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
    ///   detection. Default: `100.0` (mean per-zone channel delta).
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

    /// Update the scene-cut threshold at runtime.
    pub fn set_scene_cut_threshold(&mut self, threshold: f32) {
        self.scene_cut_threshold = threshold.max(0.0);
    }

    /// Apply temporal smoothing to a set of zone colors in-place.
    ///
    /// Each entry in `colors` is `[R, G, B]` as `u8`. The smoother maintains
    /// internal state across calls. First call initializes state without smoothing.
    ///
    /// # Scene-Cut Detection
    ///
    /// The frame difference metric is the mean absolute RGB-channel delta per
    /// zone. When this exceeds `scene_cut_threshold`, the smoother copies the
    /// new colors directly, effectively resetting to the new scene.
    pub fn apply(&mut self, colors: &mut [[u8; 3]]) {
        self.apply_for_elapsed(colors, Duration::from_secs_f32(1.0 / REFERENCE_FPS));
    }

    /// Apply smoothing using the real interval since the previous frame.
    ///
    /// The configured alpha is interpreted at 60 Hz and converted to an
    /// equivalent elapsed-time alpha, keeping response time stable when the
    /// capture cadence changes.
    pub fn apply_for_elapsed(&mut self, colors: &mut [[u8; 3]], elapsed: Duration) {
        // First frame or zone count changed — initialize without smoothing.
        if self.prev.len() != colors.len() {
            self.prev.clear();
            self.prev.extend(colors.iter().map(|color| {
                [
                    srgb_u8_to_linear(color[0]) * 255.0,
                    srgb_u8_to_linear(color[1]) * 255.0,
                    srgb_u8_to_linear(color[2]) * 255.0,
                ]
            }));
            return;
        }

        if colors.is_empty() {
            return;
        }

        // Compute frame difference metric.
        let diff = self.frame_difference(colors);

        // Scene cut detected — snap to new colors immediately.
        if diff > self.scene_cut_threshold {
            for (previous, color) in self.prev.iter_mut().zip(colors.iter()) {
                *previous = [
                    srgb_u8_to_linear(color[0]) * 255.0,
                    srgb_u8_to_linear(color[1]) * 255.0,
                    srgb_u8_to_linear(color[2]) * 255.0,
                ];
            }
            return;
        }

        // Normal EMA smoothing: smoothed = prev + alpha * (new - prev)
        let alpha = elapsed_alpha(self.alpha, elapsed);
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

    /// Compute the mean absolute RGB-channel delta per zone.
    fn frame_difference(&self, colors: &[[u8; 3]]) -> f32 {
        let total = self
            .prev
            .iter()
            .zip(colors.iter())
            .map(|(prev, new)| {
                let dr = (prev[0] - (srgb_u8_to_linear(new[0]) * 255.0)).abs();
                let dg = (prev[1] - (srgb_u8_to_linear(new[1]) * 255.0)).abs();
                let db = (prev[2] - (srgb_u8_to_linear(new[2]) * 255.0)).abs();
                dr + dg + db
            })
            .sum::<f32>();
        total / colors.len() as f32
    }
}

fn elapsed_alpha(alpha: f32, elapsed: Duration) -> f32 {
    if alpha <= 0.0 || elapsed.is_zero() {
        return 0.0;
    }
    if alpha >= 1.0 {
        return 1.0;
    }

    1.0 - (1.0 - alpha).powf(elapsed.as_secs_f32() * REFERENCE_FPS)
}
