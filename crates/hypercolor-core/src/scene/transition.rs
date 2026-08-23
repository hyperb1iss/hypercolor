//! Immutable scene transition plans and color interpolation.

use hypercolor_types::canvas::{LinearRgba, Oklab};
use hypercolor_types::scene::{ColorInterpolation, SceneId, TransitionSpec};

// ── TransitionPlan ─────────────────────────────────────────────────────

/// Commit-stable instructions for a render-local scene transition.
#[derive(Debug, Clone)]
pub struct TransitionPlan {
    /// Stable activation identity used to reconcile render-local progress.
    pub epoch: u64,
    /// Scene being transitioned away from.
    pub from_scene: SceneId,
    /// Scene being transitioned toward.
    pub to_scene: SceneId,
    /// Authored duration, easing, and color-space policy.
    pub spec: TransitionSpec,
}

impl TransitionPlan {
    /// Create one immutable activation plan.
    #[must_use]
    pub fn new(epoch: u64, from_scene: SceneId, to_scene: SceneId, spec: TransitionSpec) -> Self {
        Self {
            epoch,
            from_scene,
            to_scene,
            spec,
        }
    }

    /// Return the stable identity used by render-local frame state.
    #[must_use]
    pub const fn identity(&self) -> TransitionIdentity {
        TransitionIdentity {
            epoch: self.epoch,
            from_scene: self.from_scene,
            to_scene: self.to_scene,
        }
    }
}

/// Identity of one admitted scene activation transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TransitionIdentity {
    pub epoch: u64,
    pub from_scene: SceneId,
    pub to_scene: SceneId,
}

/// Scalar linear interpolation.
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a.mul_add(1.0 - t, b * t)
}

/// Interpolate two colors in Oklab perceptual space.
///
/// `t = 0.0` returns `a`, `t = 1.0` returns `b`.
#[must_use]
pub fn interpolate_oklab(a: &LinearRgba, b: &LinearRgba, t: f32) -> LinearRgba {
    let a_lab = a.to_oklab();
    let b_lab = b.to_oklab();
    let mixed = Oklab::new(
        lerp(a_lab.l, b_lab.l, t),
        lerp(a_lab.a, b_lab.a, t),
        lerp(a_lab.b, b_lab.b, t),
        lerp(a_lab.alpha, b_lab.alpha, t),
    );
    mixed.to_linear()
}

/// Interpolate two colors in linear sRGB space.
///
/// `t = 0.0` returns `a`, `t = 1.0` returns `b`.
#[must_use]
pub fn interpolate_srgb(a: &LinearRgba, b: &LinearRgba, t: f32) -> LinearRgba {
    a.lerp(*b, t)
}

/// Interpolate two colors using the specified color space.
#[must_use]
pub fn interpolate_color(
    a: &LinearRgba,
    b: &LinearRgba,
    t: f32,
    space: &ColorInterpolation,
) -> LinearRgba {
    match space {
        ColorInterpolation::Oklab => interpolate_oklab(a, b, t),
        ColorInterpolation::Srgb => interpolate_srgb(a, b, t),
    }
}
