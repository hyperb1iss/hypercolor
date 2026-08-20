use hypercolor_core::scene::TransitionState;
use hypercolor_core::spatial::SpatialEngine;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct TransitionFrame {
    pub(crate) progress: f32,
    pub(crate) eased_progress: f32,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct FrameState {
    transition_epoch: Option<u64>,
    transition_progress: f32,
}

impl FrameState {
    pub(crate) fn reconcile(
        &mut self,
        plan: Option<&TransitionState>,
        delta_secs: f32,
    ) -> Option<TransitionFrame> {
        let Some(plan) = plan else {
            self.transition_epoch = None;
            self.transition_progress = 0.0;
            return None;
        };

        if self.transition_epoch != Some(plan.epoch) {
            self.transition_epoch = Some(plan.epoch);
            self.transition_progress = plan.progress;
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
    use hypercolor_core::scene::TransitionState;
    use hypercolor_types::scene::{ColorInterpolation, EasingFunction, SceneId, TransitionSpec};

    use super::FrameState;

    fn plan(epoch: u64) -> TransitionState {
        TransitionState::new(
            SceneId::new(),
            SceneId::new(),
            TransitionSpec {
                duration_ms: 1_000,
                easing: EasingFunction::Linear,
                color_interpolation: ColorInterpolation::Srgb,
            },
            Vec::new(),
            Vec::new(),
        )
        .with_epoch(epoch)
    }

    #[test]
    fn frame_state_preserves_progress_only_for_the_same_transition_plan() {
        let mut state = FrameState::default();
        let first = plan(1);
        let first_frame = state
            .reconcile(Some(&first), 0.25)
            .expect("first plan should be active");
        assert_eq!(first_frame.progress, 0.25);

        let same_plan = state
            .reconcile(Some(&first), 0.25)
            .expect("same plan should retain progress");
        assert_eq!(same_plan.progress, 0.5);

        let replacement = plan(2);
        let replacement_frame = state
            .reconcile(Some(&replacement), 0.25)
            .expect("replacement plan should start independently");
        assert_eq!(replacement_frame.progress, 0.25);
    }

    #[test]
    fn completed_or_absent_transition_has_no_frame_projection() {
        let mut state = FrameState::default();
        let transition = plan(1);
        assert!(state.reconcile(Some(&transition), 1.0).is_none());
        assert!(state.reconcile(None, 0.5).is_none());
    }
}
