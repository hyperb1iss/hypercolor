//! Temporal smoothing — EMA with scene-cut detection.
//!
//! Prevents LED flicker during action scenes and fast camera pans by applying
//! an exponential moving average per zone. Scene cuts are detected when frame
//! difference exceeds a threshold, causing an immediate reset to the new colors.

use std::time::Duration;

use thiserror::Error;

use hypercolor_types::canvas::{SurfaceResourceError, linear_to_srgb_u8, srgb_u8_to_linear};

use super::{CaptureTransferFunction, ScreenSceneCutPolicy, ScreenSmoothingPolicy};

const REFERENCE_FPS: f32 = 60.0;

/// Plan-lifetime temporal state for one exact logical publication branch.
///
/// History is staged separately from committed state so a rejected publication
/// cannot influence the next accepted frame. Both buffers reserve their exact
/// maximum shape during preparation and never grow while processing frames.
#[derive(Clone, Debug)]
pub struct PreparedTemporalSmoother {
    policy: ScreenSmoothingPolicy,
    max_samples: usize,
    retained_byte_len: u64,
    committed: Vec<[f32; 3]>,
    committed_shape: Option<(u32, u32)>,
    staged: Vec<[f32; 3]>,
    staged_shape: Option<(u32, u32)>,
}

impl PreparedTemporalSmoother {
    /// Reserve exact history storage for a branch's maximum published shape.
    ///
    /// # Errors
    ///
    /// Returns a typed resource failure when the shape is empty, its byte
    /// ledger overflows, or either history buffer cannot be reserved.
    pub fn try_new(
        policy: ScreenSmoothingPolicy,
        width: u32,
        height: u32,
    ) -> Result<Self, SurfaceResourceError> {
        if width == 0 || height == 0 {
            return Err(SurfaceResourceError::EmptyDimensions { width, height });
        }
        let max_samples = checked_sample_count(width, height)?;
        let requested_byte_len = match policy {
            ScreenSmoothingPolicy::Disabled => 0,
            ScreenSmoothingPolicy::Exponential { .. } => u64::try_from(max_samples)
                .ok()
                .and_then(|count| {
                    u64::try_from(std::mem::size_of::<[f32; 3]>())
                        .ok()
                        .and_then(|item_size| count.checked_mul(item_size))
                })
                .and_then(|bytes| bytes.checked_mul(2))
                .ok_or(SurfaceResourceError::ByteLengthOverflow { width, height })?,
        };
        let requested_byte_len_usize = usize::try_from(requested_byte_len)
            .map_err(|_| SurfaceResourceError::ByteLengthOverflow { width, height })?;
        let mut committed = Vec::new();
        let mut staged = Vec::new();
        if matches!(policy, ScreenSmoothingPolicy::Exponential { .. }) {
            committed.try_reserve_exact(max_samples).map_err(|_| {
                SurfaceResourceError::AllocationFailed {
                    width,
                    height,
                    byte_len: requested_byte_len_usize,
                }
            })?;
            staged.try_reserve_exact(max_samples).map_err(|_| {
                SurfaceResourceError::AllocationFailed {
                    width,
                    height,
                    byte_len: requested_byte_len_usize,
                }
            })?;
        }
        let retained_byte_len = u64::try_from(
            committed
                .capacity()
                .checked_add(staged.capacity())
                .ok_or(SurfaceResourceError::ByteLengthOverflow { width, height })?,
        )
        .ok()
        .and_then(|count| {
            u64::try_from(std::mem::size_of::<[f32; 3]>())
                .ok()
                .and_then(|item_size| count.checked_mul(item_size))
        })
        .ok_or(SurfaceResourceError::ByteLengthOverflow { width, height })?;
        Ok(Self {
            policy,
            max_samples,
            retained_byte_len,
            committed,
            committed_shape: None,
            staged,
            staged_shape: None,
        })
    }

    /// Exact maximum sample count admitted during preparation.
    #[must_use]
    pub const fn max_samples(&self) -> usize {
        self.max_samples
    }

    /// Exact retained heap-byte ledger for both transactional histories.
    #[must_use]
    pub const fn retained_byte_len(&self) -> u64 {
        self.retained_byte_len
    }

    /// Reserved element capacities, useful for proving frame-time reuse.
    #[must_use]
    pub fn capacities(&self) -> (usize, usize) {
        (self.committed.capacity(), self.staged.capacity())
    }

    /// Stage smoothing for one encoded RGB grid without committing history.
    ///
    /// `reset_history` is used when content cropping changes spatial identity.
    /// `suppress_scene_cut_bypass` keeps smoothing active while a caller is
    /// already blending between color transforms.
    /// Scene-cut distance is normalized to `0.0..=1.0` per channel in linear
    /// light. The exponential response derives directly from the configured
    /// time constant and capture timestamp delta.
    ///
    /// # Errors
    ///
    /// Rejects unsupported transfer functions, invalid shapes, samples beyond
    /// the prepared maximum, or a second stage before commit/discard.
    pub fn stage(
        &mut self,
        colors: &mut [[u8; 3]],
        width: u32,
        height: u32,
        transfer: CaptureTransferFunction,
        elapsed: Duration,
        reset_history: bool,
        suppress_scene_cut_bypass: bool,
    ) -> Result<(), PreparedTemporalSmoothingError> {
        if self.staged_shape.is_some() {
            return Err(PreparedTemporalSmoothingError::StagePending);
        }
        if !matches!(
            transfer,
            CaptureTransferFunction::Srgb | CaptureTransferFunction::Linear
        ) {
            return Err(PreparedTemporalSmoothingError::UnsupportedTransferFunction(
                transfer,
            ));
        }
        let expected = checked_sample_count(width, height)
            .map_err(|_| PreparedTemporalSmoothingError::GeometryOverflow { width, height })?;
        if expected != colors.len() {
            return Err(PreparedTemporalSmoothingError::SampleCountMismatch {
                expected,
                actual: colors.len(),
            });
        }
        if expected > self.max_samples {
            return Err(PreparedTemporalSmoothingError::PreparedCapacityExceeded {
                maximum: self.max_samples,
                actual: expected,
            });
        }

        let ScreenSmoothingPolicy::Exponential {
            time_constant,
            scene_cut,
        } = self.policy
        else {
            return Ok(());
        };
        let shape = (width, height);
        self.staged.clear();
        self.staged_shape = Some(shape);
        let reset = reset_history
            || self.committed.len() != expected
            || self.committed_shape != Some(shape)
            || !suppress_scene_cut_bypass
                && scene_cut_detected(scene_cut, transfer, &self.committed, colors);
        if reset {
            self.staged.extend(
                colors
                    .iter()
                    .copied()
                    .map(|color| decode_rgb(color, transfer)),
            );
            return Ok(());
        }

        let alpha = time_constant_alpha(time_constant, elapsed);
        for (color, previous) in colors.iter_mut().zip(&self.committed) {
            let incoming = decode_rgb(*color, transfer);
            let next = [
                previous[0] + alpha * (incoming[0] - previous[0]),
                previous[1] + alpha * (incoming[1] - previous[1]),
                previous[2] + alpha * (incoming[2] - previous[2]),
            ];
            self.staged.push(next);
            *color = encode_rgb(next, transfer);
        }
        Ok(())
    }

    /// Commit the most recently staged history after publication acceptance.
    ///
    /// Returns `false` when smoothing is disabled or no stage is pending.
    pub fn commit_staged(&mut self) -> bool {
        if self.staged_shape.is_none() {
            return false;
        }
        std::mem::swap(&mut self.committed, &mut self.staged);
        self.committed_shape = self.staged_shape.take();
        self.staged.clear();
        true
    }

    /// Discard staged history after publication rejection.
    pub fn discard_staged(&mut self) {
        self.staged.clear();
        self.staged_shape = None;
    }

    /// Clear committed and staged state while retaining all reservations.
    pub fn reset(&mut self) {
        self.committed.clear();
        self.committed_shape = None;
        self.discard_staged();
    }
}

/// Frame-time validation failure for prepared temporal smoothing.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum PreparedTemporalSmoothingError {
    /// Only byte-addressable sRGB and linear output are currently supported.
    #[error("unsupported prepared smoothing transfer function: {0:?}")]
    UnsupportedTransferFunction(CaptureTransferFunction),
    /// Width and height did not produce an addressable sample count.
    #[error("prepared smoothing geometry overflows for {width}x{height}")]
    GeometryOverflow { width: u32, height: u32 },
    /// Caller supplied a slice different from the declared grid shape.
    #[error("smoothing grid has {actual} samples; expected exactly {expected}")]
    SampleCountMismatch { expected: usize, actual: usize },
    /// Caller exceeded the exact maximum reserved during preparation.
    #[error("smoothing grid has {actual} samples; prepared maximum is {maximum}")]
    PreparedCapacityExceeded { maximum: usize, actual: usize },
    /// Transactional state requires resolving the previous stage first.
    #[error("prepared smoothing already has a staged publication")]
    StagePending,
}

fn checked_sample_count(width: u32, height: u32) -> Result<usize, SurfaceResourceError> {
    usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .ok_or(SurfaceResourceError::ByteLengthOverflow { width, height })
}

fn scene_cut_detected(
    policy: ScreenSceneCutPolicy,
    transfer: CaptureTransferFunction,
    committed: &[[f32; 3]],
    colors: &[[u8; 3]],
) -> bool {
    let ScreenSceneCutPolicy::MeanAbsoluteDelta { threshold } = policy else {
        return false;
    };
    if colors.is_empty() {
        return false;
    }
    let total = committed
        .iter()
        .zip(colors)
        .map(|(previous, color)| {
            let incoming = decode_rgb(*color, transfer);
            (previous[0] - incoming[0]).abs()
                + (previous[1] - incoming[1]).abs()
                + (previous[2] - incoming[2]).abs()
        })
        .sum::<f32>();
    #[expect(
        clippy::cast_precision_loss,
        clippy::as_conversions,
        reason = "admitted sample counts are normalized only for a bounded color metric"
    )]
    let channel_count = (colors.len() * 3) as f32;
    total / channel_count >= threshold.value().clamp(0.0, 1.0)
}

fn decode_rgb(color: [u8; 3], transfer: CaptureTransferFunction) -> [f32; 3] {
    color.map(|channel| match transfer {
        CaptureTransferFunction::Srgb => srgb_u8_to_linear(channel),
        CaptureTransferFunction::Linear => f32::from(channel) / 255.0,
        _ => unreachable!("unsupported transfers are rejected before decoding"),
    })
}

fn encode_rgb(color: [f32; 3], transfer: CaptureTransferFunction) -> [u8; 3] {
    color.map(|channel| match transfer {
        CaptureTransferFunction::Srgb => linear_to_srgb_u8(channel.clamp(0.0, 1.0)),
        CaptureTransferFunction::Linear => linear_u8(channel),
        _ => unreachable!("unsupported transfers are rejected before encoding"),
    })
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::as_conversions,
    reason = "the rounded linear channel is clamped to the u8 range"
)]
fn linear_u8(channel: f32) -> u8 {
    (channel.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn time_constant_alpha(time_constant: Duration, elapsed: Duration) -> f32 {
    if elapsed.is_zero() {
        return 0.0;
    }
    if time_constant.is_zero() {
        return 1.0;
    }
    let alpha = 1.0 - (-(elapsed.as_secs_f64() / time_constant.as_secs_f64())).exp();
    alpha as f32
}

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

    /// Spatial identity of `prev`; equal counts with different geometry must
    /// never blend unrelated coordinates.
    prev_shape: Option<(u32, u32)>,

    staged: Vec<[f32; 3]>,
    staged_shape: Option<(u32, u32)>,
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
            prev_shape: None,
            staged: Vec::new(),
            staged_shape: None,
        }
    }

    pub(super) fn try_new_for_grid(
        alpha: f32,
        scene_cut_threshold: f32,
        width: u32,
        height: u32,
    ) -> Result<Self, SurfaceResourceError> {
        let pixel_count = usize::try_from(width)
            .ok()
            .and_then(|width| {
                usize::try_from(height)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .ok_or(SurfaceResourceError::ByteLengthOverflow { width, height })?;
        let history_bytes = pixel_count
            .checked_mul(std::mem::size_of::<[f32; 3]>())
            .and_then(|bytes| bytes.checked_mul(2))
            .ok_or(SurfaceResourceError::ByteLengthOverflow { width, height })?;
        let mut smoother = Self::new(alpha, scene_cut_threshold);
        smoother.prev.try_reserve_exact(pixel_count).map_err(|_| {
            SurfaceResourceError::AllocationFailed {
                width,
                height,
                byte_len: history_bytes,
            }
        })?;
        smoother
            .staged
            .try_reserve_exact(pixel_count)
            .map_err(|_| SurfaceResourceError::AllocationFailed {
                width,
                height,
                byte_len: history_bytes,
            })?;
        Ok(smoother)
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
        let width = u32::try_from(colors.len()).unwrap_or(u32::MAX);
        self.apply_for_elapsed_grid(colors, width, 1, elapsed);
    }

    /// Apply smoothing to a spatial grid using its full shape as state identity.
    ///
    /// An overflowing shape or one whose checked pixel count differs from
    /// `colors.len()` leaves both the colors and committed history unchanged.
    pub fn apply_for_elapsed_grid(
        &mut self,
        colors: &mut [[u8; 3]],
        width: u32,
        height: u32,
        elapsed: Duration,
    ) {
        if self.stage_for_elapsed_grid(colors, width, height, elapsed, false, false) {
            self.commit_staged();
        }
    }

    pub(super) fn stage_for_elapsed_grid(
        &mut self,
        colors: &mut [[u8; 3]],
        width: u32,
        height: u32,
        elapsed: Duration,
        reset_history: bool,
        suppress_scene_cut_bypass: bool,
    ) -> bool {
        let Some(expected_len) = usize::try_from(width)
            .ok()
            .and_then(|width| usize::try_from(height).ok()?.checked_mul(width))
        else {
            return false;
        };
        let history_bytes_fit = expected_len
            .checked_mul(std::mem::size_of::<[f32; 3]>())
            .and_then(|bytes| bytes.checked_mul(2))
            .is_some();
        if expected_len != colors.len() || !history_bytes_fit {
            return false;
        }
        if self
            .staged
            .try_reserve_exact(expected_len.saturating_sub(self.staged.len()))
            .is_err()
        {
            return false;
        }

        let shape = (width, height);
        self.staged.clear();
        self.staged_shape = Some(shape);

        if reset_history || self.prev.len() != colors.len() || self.prev_shape != Some(shape) {
            self.staged.extend(colors.iter().map(|color| {
                [
                    srgb_u8_to_linear(color[0]) * 255.0,
                    srgb_u8_to_linear(color[1]) * 255.0,
                    srgb_u8_to_linear(color[2]) * 255.0,
                ]
            }));
            return true;
        }

        if colors.is_empty() {
            return true;
        }

        // Compute frame difference metric.
        let diff = self.frame_difference(colors);

        // Scene cut detected — snap to new colors immediately.
        if !suppress_scene_cut_bypass && diff > self.scene_cut_threshold {
            self.staged.extend(colors.iter().map(|color| {
                [
                    srgb_u8_to_linear(color[0]) * 255.0,
                    srgb_u8_to_linear(color[1]) * 255.0,
                    srgb_u8_to_linear(color[2]) * 255.0,
                ]
            }));
            return true;
        }

        // Normal EMA smoothing: smoothed = prev + alpha * (new - prev)
        let alpha = elapsed_alpha(self.alpha, elapsed);
        for (color, previous) in colors.iter_mut().zip(self.prev.iter()) {
            let new_r = srgb_u8_to_linear(color[0]) * 255.0;
            let new_g = srgb_u8_to_linear(color[1]) * 255.0;
            let new_b = srgb_u8_to_linear(color[2]) * 255.0;

            let next = [
                previous[0] + alpha * (new_r - previous[0]),
                previous[1] + alpha * (new_g - previous[1]),
                previous[2] + alpha * (new_b - previous[2]),
            ];
            self.staged.push(next);

            color[0] = linear_to_srgb_u8((next[0] / 255.0).clamp(0.0, 1.0));
            color[1] = linear_to_srgb_u8((next[1] / 255.0).clamp(0.0, 1.0));
            color[2] = linear_to_srgb_u8((next[2] / 255.0).clamp(0.0, 1.0));
        }
        true
    }

    pub(super) fn commit_staged(&mut self) {
        std::mem::swap(&mut self.prev, &mut self.staged);
        self.prev_shape = self.staged_shape.take();
    }

    /// Reset internal state. Next call to `apply` will initialize fresh.
    pub fn reset(&mut self) {
        self.prev.clear();
        self.prev_shape = None;
        self.staged.clear();
        self.staged_shape = None;
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
