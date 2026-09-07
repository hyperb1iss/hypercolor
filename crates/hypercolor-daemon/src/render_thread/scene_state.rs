use hypercolor_core::scene::{TransitionIdentity, TransitionPlan};
use hypercolor_core::spatial::SpatialEngine;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct TransitionFrame {
    pub(crate) progress: f32,
    pub(crate) eased_progress: f32,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct FrameState {
    transition_identity: Option<TransitionIdentity>,
    transition_progress: f32,
}

impl FrameState {
    pub(crate) fn reconcile(
        &mut self,
        plan: Option<&TransitionPlan>,
        delta_secs: f32,
    ) -> Option<TransitionFrame> {
        let Some(plan) = plan else {
            self.transition_identity = None;
            self.transition_progress = 0.0;
            return None;
        };

        let identity = plan.identity();
        if self.transition_identity != Some(identity) {
            self.transition_identity = Some(identity);
            self.transition_progress = 0.0;
        }

        if plan.spec.duration_ms == 0 {
            self.transition_progress = 1.0;
        } else {
            #[allow(clippy::cast_precision_loss, clippy::as_conversions)]
            let duration_secs = plan.spec.duration_ms as f64 / 1000.0;
            #[allow(clippy::cast_possible_truncation, clippy::as_conversions)]
            let increment = (f64::from(delta_secs) / duration_secs) as f32;
            self.transition_progress = (self.transition_progress + increment).clamp(0.0, 1.0);
        }

        if self.transition_progress >= 1.0 {
            return None;
        }

        Some(TransitionFrame {
            progress: self.transition_progress,
            eased_progress: plan.spec.easing.apply(self.transition_progress),
        })
    }
}

#[derive(Debug, Clone)]
pub(crate) struct RenderSceneState {
    spatial_engine: SpatialEngine,
    screen_capture_configured: bool,
    frame_state: FrameState,
}

impl RenderSceneState {
    pub(crate) fn new(spatial_engine: SpatialEngine, screen_capture_configured: bool) -> Self {
        Self {
            spatial_engine,
            screen_capture_configured,
            frame_state: FrameState::default(),
        }
    }

    pub(crate) fn replace_spatial_engine(&mut self, spatial_engine: SpatialEngine) {
        self.spatial_engine = spatial_engine;
    }

    pub(crate) fn set_screen_capture_configured(&mut self, configured: bool) {
        self.screen_capture_configured = configured;
    }

    pub(crate) fn spatial_engine(&self) -> &SpatialEngine {
        &self.spatial_engine
    }

    pub(crate) fn screen_capture_configured(&self) -> bool {
        self.screen_capture_configured
    }

    pub(crate) fn frame_state_mut(&mut self) -> &mut FrameState {
        &mut self.frame_state
    }
}

#[cfg(test)]
mod tests {
    use hypercolor_core::scene::TransitionPlan;
    use hypercolor_types::scene::{ColorInterpolation, EasingFunction, SceneId, TransitionSpec};

    use super::FrameState;

    fn plan_with(
        epoch: u64,
        from_scene: SceneId,
        to_scene: SceneId,
        duration_ms: u64,
        easing: EasingFunction,
    ) -> TransitionPlan {
        TransitionPlan::new(
            epoch,
            from_scene,
            to_scene,
            TransitionSpec {
                duration_ms,
                easing,
                color_interpolation: ColorInterpolation::Srgb,
            },
        )
    }

    fn plan(epoch: u64) -> TransitionPlan {
        plan_with(
            epoch,
            SceneId::new(),
            SceneId::new(),
            1_000,
            EasingFunction::Linear,
        )
    }

    #[test]
    fn frame_state_preserves_progress_only_for_the_same_transition_plan() {
        let mut state = FrameState::default();
        let from_scene = SceneId::new();
        let to_scene = SceneId::new();
        let first = plan_with(1, from_scene, to_scene, 1_000, EasingFunction::Linear);
        let first_frame = state
            .reconcile(Some(&first), 0.25)
            .expect("first plan should be active");
        assert_eq!(first_frame.progress, 0.25);

        let same_plan = state
            .reconcile(Some(&first), 0.25)
            .expect("same plan should retain progress");
        assert_eq!(same_plan.progress, 0.5);

        let replacement = plan_with(2, from_scene, to_scene, 1_000, EasingFunction::Linear);
        let replacement_frame = state
            .reconcile(Some(&replacement), 0.25)
            .expect("replacement plan should start independently");
        assert_eq!(replacement_frame.progress, 0.25);
    }

    #[test]
    fn frame_state_identity_includes_transition_endpoints() {
        let mut state = FrameState::default();
        let from_scene = SceneId::new();
        let first = plan_with(1, from_scene, SceneId::new(), 1_000, EasingFunction::Linear);
        assert_eq!(
            state
                .reconcile(Some(&first), 0.5)
                .expect("first transition should be active")
                .progress,
            0.5
        );

        let replacement = plan_with(1, from_scene, SceneId::new(), 1_000, EasingFunction::Linear);
        assert_eq!(
            state
                .reconcile(Some(&replacement), 0.25)
                .expect("different endpoints should restart progress")
                .progress,
            0.25
        );
    }

    #[test]
    fn frame_state_applies_easing_to_exact_render_local_progress() {
        let mut state = FrameState::default();
        let transition = plan_with(
            1,
            SceneId::new(),
            SceneId::new(),
            1_000,
            EasingFunction::EaseIn,
        );
        let frame = state
            .reconcile(Some(&transition), 0.25)
            .expect("transition should be active");
        assert_eq!(frame.progress, 0.25);
        assert_eq!(frame.eased_progress, 0.015_625);
    }

    #[test]
    fn completed_or_absent_transition_has_no_frame_projection() {
        let mut state = FrameState::default();
        let transition = plan(1);
        assert!(state.reconcile(Some(&transition), 1.0).is_none());
        assert!(state.reconcile(Some(&transition), 0.5).is_none());
        assert!(state.reconcile(None, 0.5).is_none());
    }
}
