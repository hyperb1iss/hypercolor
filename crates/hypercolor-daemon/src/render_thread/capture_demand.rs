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
        // A live preview subscriber (e.g. the Capture page) keeps the screen
        // pipeline running even while no screen-reactive effect is active and
        // even when outputs sleep — tuning needs a picture to tune against.
        let preview_active = state.event_bus.screen_canvas_receiver_count() > 0
            || state.event_bus.screen_zones_receiver_count() > 0;
        self.reconcile_screen(
            state,
            (!sleeping && effect_demand.screen_capture_active) || preview_active,
        )
        .await;
        self.reconcile_interaction(state, !sleeping && effect_demand.interaction_capture_active)
            .await;
    }

    pub(crate) async fn clear(&mut self, state: &RenderThreadState) {
        self.reconcile_audio(state, false).await;
        self.reconcile_screen(state, false).await;
        self.reconcile_interaction(state, false).await;
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

    /// Deliberately uncached: interaction sources can be added or removed
    /// live via config, and a cached last-value here would leave a freshly
    /// added source inactive while an interactive effect is already running.
    /// Sources no-op internally when the state is unchanged.
    pub(crate) async fn reconcile_interaction(
        &mut self,
        state: &RenderThreadState,
        desired_active: bool,
    ) {
        let result = {
            let mut input_manager = state.input_manager.lock().await;
            input_manager.set_interaction_capture_active(desired_active)
        };

        if let Err(error) = result {
            warn!(
                desired_active,
                %error,
                "Failed to update interaction capture demand"
            );
        }
    }
}
