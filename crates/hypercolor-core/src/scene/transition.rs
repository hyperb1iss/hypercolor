//! Transition engine — cross-fade blending between scene states.
//!
//! Manages the runtime state of an in-progress transition, advancing
//! progress on each tick and blending zone assignments between the
//! outgoing and incoming scenes using perceptual Oklab interpolation.

use std::collections::HashMap;

use crate::types::canvas::{Oklab, RgbaF32};
use crate::types::scene::{ColorInterpolation, SceneId, TransitionSpec, ZoneAssignment};

// ── TransitionState ────────────────────────────────────────────────────

/// Runtime state for an in-progress scene transition.
///
/// Created when a scene switch begins. The `tick()` method advances
/// progress each frame using wall-clock delta time. Blended zone
/// assignments can be retrieved via `blend()` at any point during
/// the transition.
#[derive(Debug, Clone)]
pub struct TransitionState {
    /// Scene being transitioned away from.
    pub from_scene: SceneId,

    /// Scene being transitioned toward.
    pub to_scene: SceneId,

    /// The transition specification (duration, easing, color space).
    pub spec: TransitionSpec,

    /// Current linear progress in `[0.0, 1.0]`.
    pub progress: f32,

    /// Zone assignments from the outgoing scene, keyed by zone name.
    from_assignments: HashMap<String, ZoneAssignment>,

    /// Zone assignments from the incoming scene, keyed by zone name.
    to_assignments: HashMap<String, ZoneAssignment>,
}

impl TransitionState {
    /// Create a new transition between two scene states.
    ///
    /// If `duration_ms` is zero the transition starts already complete.
    #[must_use]
    pub fn new(
        from_scene: SceneId,
        to_scene: SceneId,
        spec: TransitionSpec,
        from_assignments: Vec<ZoneAssignment>,
        to_assignments: Vec<ZoneAssignment>,
    ) -> Self {
        let from_map: HashMap<String, ZoneAssignment> = from_assignments
            .into_iter()
            .map(|za| (za.zone_name.clone(), za))
            .collect();

        let to_map: HashMap<String, ZoneAssignment> = to_assignments
            .into_iter()
            .map(|za| (za.zone_name.clone(), za))
            .collect();

        let progress = if spec.duration_ms == 0 { 1.0 } else { 0.0 };

        Self {
            from_scene,
            to_scene,
            spec,
            progress,
            from_assignments: from_map,
            to_assignments: to_map,
        }
    }

    /// Advance the transition by `delta_secs` seconds of wall-clock time.
    ///
    /// Progress is clamped to `[0.0, 1.0]`. Once complete, further
    /// ticks are no-ops.
    pub fn tick(&mut self, delta_secs: f32) {
        if self.is_complete() {
            return;
        }

        if self.spec.duration_ms == 0 {
            self.progress = 1.0;
            return;
        }

        #[allow(clippy::cast_precision_loss, clippy::as_conversions)]
        let duration_secs = self.spec.duration_ms as f64 / 1000.0;

        // Avoid precision loss: compute in f64, convert result back.
        #[allow(clippy::cast_possible_truncation, clippy::as_conversions)]
        let increment = (f64::from(delta_secs) / duration_secs) as f32;

        self.progress = (self.progress + increment).clamp(0.0, 1.0);
    }

    /// Returns `true` when the transition has reached or exceeded completion.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.progress >= 1.0
    }

    /// The eased progress value, applying the transition's easing function
    /// to the raw linear progress.
    #[must_use]
    pub fn eased_progress(&self) -> f32 {
        self.spec.easing.apply(self.progress)
    }

    /// Blend the from/to zone assignments at the current eased progress.
    ///
    /// Returns a merged list of [`ZoneAssignment`]s. Zones present in
    /// only one side are included at full weight for their side.
    /// Brightness values are linearly interpolated; effect names and
    /// parameters come from the `to` side once progress crosses 0.5.
    #[must_use]
    pub fn blend(&self) -> Vec<ZoneAssignment> {
        let t = self.eased_progress();

        // Collect all zone names from both sides.
        let mut all_zones: Vec<&String> = self
            .from_assignments
            .keys()
            .chain(self.to_assignments.keys())
            .collect();
        all_zones.sort();
        all_zones.dedup();

        all_zones
            .into_iter()
            .map(|zone_name| {
                match (
                    self.from_assignments.get(zone_name),
                    self.to_assignments.get(zone_name),
                ) {
                    (Some(from), Some(to)) => {
                        blend_zone_assignment(from, to, t, &self.spec.color_interpolation)
                    }
                    (Some(from), None) => {
                        // Zone only in outgoing scene — fade brightness toward 0.
                        let mut blended = from.clone();
                        let from_b = from.brightness.unwrap_or(1.0);
                        blended.brightness = Some(from_b * (1.0 - t));
                        blended
                    }
                    (None, Some(to)) => {
                        // Zone only in incoming scene — fade brightness from 0.
                        let mut blended = to.clone();
                        let to_b = to.brightness.unwrap_or(1.0);
                        blended.brightness = Some(to_b * t);
                        blended
                    }
                    (None, None) => {
                        // Unreachable — zone came from one of the two maps.
                        // Return a neutral assignment to satisfy the compiler.
                        ZoneAssignment {
                            zone_name: zone_name.clone(),
                            effect_name: String::from("static"),
                            parameters: HashMap::new(),
                            brightness: Some(0.0),
                        }
                    }
                }
            })
            .collect()
    }
}

// ── Blending Helpers ────────────────────────────────────────────────────

/// Blend two zone assignments at progress `t`.
///
/// Brightness is linearly interpolated. Effect name and parameters
/// switch from `from` to `to` once `t` crosses 0.5 (prevents jarring
/// mid-transition effect swaps at the halfway point).
fn blend_zone_assignment(
    from: &ZoneAssignment,
    to: &ZoneAssignment,
    t: f32,
    _color_interp: &ColorInterpolation,
) -> ZoneAssignment {
    let from_b = from.brightness.unwrap_or(1.0);
    let to_b = to.brightness.unwrap_or(1.0);
    let blended_brightness = lerp(from_b, to_b, t);

    // Effect name and parameters swap at the midpoint.
    let (effect_name, parameters) = if t < 0.5 {
        (from.effect_name.clone(), from.parameters.clone())
    } else {
        (to.effect_name.clone(), to.parameters.clone())
    };

    ZoneAssignment {
        zone_name: from.zone_name.clone(),
        effect_name,
        parameters,
        brightness: Some(blended_brightness),
    }
}

/// Scalar linear interpolation.
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a.mul_add(1.0 - t, b * t)
}

/// Interpolate two colors in Oklab perceptual space.
///
/// `t = 0.0` returns `a`, `t = 1.0` returns `b`.
#[must_use]
pub fn interpolate_oklab(a: &RgbaF32, b: &RgbaF32, t: f32) -> RgbaF32 {
    let a_lab = a.to_oklab();
    let b_lab = b.to_oklab();
    let mixed = Oklab::new(
        lerp(a_lab.l, b_lab.l, t),
        lerp(a_lab.a, b_lab.a, t),
        lerp(a_lab.b, b_lab.b, t),
        lerp(a_lab.alpha, b_lab.alpha, t),
    );
    RgbaF32::from_oklab(mixed)
}

/// Interpolate two colors in linear sRGB space.
///
/// `t = 0.0` returns `a`, `t = 1.0` returns `b`.
#[must_use]
pub fn interpolate_srgb(a: &RgbaF32, b: &RgbaF32, t: f32) -> RgbaF32 {
    RgbaF32::lerp(a, b, t)
}

/// Interpolate two colors using the specified color space.
#[must_use]
pub fn interpolate_color(a: &RgbaF32, b: &RgbaF32, t: f32, space: &ColorInterpolation) -> RgbaF32 {
    match space {
        ColorInterpolation::Oklab => interpolate_oklab(a, b, t),
        ColorInterpolation::Srgb => interpolate_srgb(a, b, t),
    }
}
