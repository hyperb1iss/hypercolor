use tracing::warn;

use super::RenderThreadState;
use super::scene_snapshot::EffectDemand;

#[derive(Debug, Default)]
pub(crate) struct CaptureDemandState {
    last_audio_capture_active: Option<bool>,
    last_screen_capture_active: Option<bool>,
}

impl CaptureDemandState {
    pub(crate) async fn reconcile_effect_demand(
        &mut self,
        state: &RenderThreadState,
        sleeping: bool,
        effect_demand: EffectDemand,
    ) {
        self.reconcile_audio(state, !sleeping && effect_demand.audio_capture_active)
            .await;
        self.reconcile_screen(state, !sleeping && effect_demand.screen_capture_active)
            .await;
    }

    pub(crate) async fn clear(&mut self, state: &RenderThreadState) {
        self.reconcile_audio(state, false).await;
        self.reconcile_screen(state, false).await;
    }

    pub(crate) async fn reconcile_audio(
        &mut self,
        state: &RenderThreadState,
        desired_active: bool,
    ) {
        if self
            .last_audio_capture_active
            .is_some_and(|previous| previous == desired_active)
        {
            return;
        }

        let result = {
            let mut input_manager = state.input_manager.lock().await;
            input_manager.set_audio_capture_active(desired_active)
        };

        match result {
            Ok(()) => {
                self.last_audio_capture_active = Some(desired_active);
            }
            Err(error) => {
                warn!(
                    desired_active,
                    %error,
                    "Failed to update audio capture demand"
                );
            }
        }
    }

    pub(crate) async fn reconcile_screen(
        &mut self,
        state: &RenderThreadState,
        desired_active: bool,
    ) {
        if self
            .last_screen_capture_active
            .is_some_and(|previous| previous == desired_active)
        {
            return;
        }

        let result = {
            let mut input_manager = state.input_manager.lock().await;
            input_manager.set_screen_capture_active(desired_active)
        };

        match result {
            Ok(()) => {
                self.last_screen_capture_active = Some(desired_active);
            }
            Err(error) => {
                warn!(
                    desired_active,
                    %error,
                    "Failed to update screen capture demand"
                );
            }
        }
    }
}
