use hypercolor_core::input::{InputGraphHandle, InputManager};
use tracing::warn;

use super::RenderThreadState;
use super::scene_snapshot::EffectDemand;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CaptureDemandKey {
    graph_generation: u64,
    desired_active: bool,
}

#[derive(Clone, Copy)]
enum CaptureDomain {
    Audio,
    Screen,
    Interaction,
}

impl CaptureDomain {
    fn apply(self, manager: &mut InputManager, desired_active: bool) -> anyhow::Result<()> {
        match self {
            Self::Audio => manager.set_audio_capture_active(desired_active),
            Self::Screen => manager.set_screen_capture_active(desired_active),
            Self::Interaction => manager.set_interaction_capture_active(desired_active),
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Audio => "audio",
            Self::Screen => "screen",
            Self::Interaction => "interaction",
        }
    }
}

#[derive(Default)]
pub(crate) struct CaptureDemandState {
    input_graph: Option<InputGraphHandle>,
    last_audio: Option<CaptureDemandKey>,
    last_screen: Option<CaptureDemandKey>,
    last_interaction: Option<CaptureDemandKey>,
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
        self.reconcile_domain(state, CaptureDomain::Audio, desired_active)
            .await;
    }

    pub(crate) async fn reconcile_screen(
        &mut self,
        state: &RenderThreadState,
        desired_active: bool,
    ) {
        self.reconcile_domain(state, CaptureDomain::Screen, desired_active)
            .await;
    }

    pub(crate) async fn reconcile_interaction(
        &mut self,
        state: &RenderThreadState,
        desired_active: bool,
    ) {
        self.reconcile_domain(state, CaptureDomain::Interaction, desired_active)
            .await;
    }

    async fn reconcile_domain(
        &mut self,
        state: &RenderThreadState,
        domain: CaptureDomain,
        desired_active: bool,
    ) {
        let input_graph = self.input_graph(state).await;
        let observed_generation = input_graph.snapshot().generation();
        let desired_key = CaptureDemandKey {
            graph_generation: observed_generation,
            desired_active,
        };
        if self.cached_key(domain) == Some(desired_key) {
            return;
        }

        let (result, resulting_generation) = {
            let mut input_manager = state.input_manager.lock().await;
            let result = domain.apply(&mut input_manager, desired_active);
            (result, input_manager.source_graph_generation())
        };

        match result {
            Ok(()) => self.set_cached_key(
                domain,
                CaptureDemandKey {
                    graph_generation: resulting_generation,
                    desired_active,
                },
            ),
            Err(error) => warn!(
                domain = domain.name(),
                desired_active,
                %error,
                "Failed to update capture demand"
            ),
        }
    }

    async fn input_graph(&mut self, state: &RenderThreadState) -> InputGraphHandle {
        if self.input_graph.is_none() {
            self.input_graph = Some(state.input_manager.lock().await.input_graph_handle());
        }
        self.input_graph
            .as_ref()
            .expect("input graph handle is initialized before reconciliation")
            .clone()
    }

    fn cached_key(&self, domain: CaptureDomain) -> Option<CaptureDemandKey> {
        match domain {
            CaptureDomain::Audio => self.last_audio,
            CaptureDomain::Screen => self.last_screen,
            CaptureDomain::Interaction => self.last_interaction,
        }
    }

    fn set_cached_key(&mut self, domain: CaptureDomain, key: CaptureDemandKey) {
        match domain {
            CaptureDomain::Audio => self.last_audio = Some(key),
            CaptureDomain::Screen => self.last_screen = Some(key),
            CaptureDomain::Interaction => self.last_interaction = Some(key),
        }
    }
}
