use hypercolor_core::input::{InputManager, ScreenCaptureDemand};
use tracing::warn;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct CaptureDemand {
    audio_active: bool,
    screen: ScreenCaptureDemand,
    interaction_active: bool,
}

impl CaptureDemand {
    pub(super) const fn new(
        audio_active: bool,
        screen: ScreenCaptureDemand,
        interaction_active: bool,
    ) -> Self {
        Self {
            audio_active,
            screen,
            interaction_active,
        }
    }

    const fn is_active(self, domain: CaptureDomain) -> bool {
        match domain {
            CaptureDomain::Audio => self.audio_active,
            CaptureDomain::Screen => self.screen.is_active(),
            CaptureDomain::Interaction => self.interaction_active,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CaptureDemandKey {
    graph_generation: u64,
    desired_active: bool,
    screen_demand: ScreenCaptureDemand,
}

#[derive(Clone, Copy)]
enum CaptureDomain {
    Audio,
    Screen,
    Interaction,
}

impl CaptureDomain {
    fn apply(self, manager: &mut InputManager, demand: CaptureDemand) -> anyhow::Result<()> {
        let desired_active = demand.is_active(self);
        match self {
            Self::Audio => manager.set_audio_capture_active(desired_active),
            Self::Screen => manager.set_screen_capture_demand(demand.screen),
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
    audio: Option<CaptureDemandKey>,
    screen: Option<CaptureDemandKey>,
    interaction: Option<CaptureDemandKey>,
}

impl CaptureDemandState {
    pub(crate) fn is_current(&self, graph_generation: u64, demand: CaptureDemand) -> bool {
        self.cached_key(CaptureDomain::Audio)
            == Some(Self::key(graph_generation, demand, CaptureDomain::Audio))
            && self.cached_key(CaptureDomain::Screen)
                == Some(Self::key(graph_generation, demand, CaptureDomain::Screen))
            && self.cached_key(CaptureDomain::Interaction)
                == Some(Self::key(
                    graph_generation,
                    demand,
                    CaptureDomain::Interaction,
                ))
    }

    pub(crate) fn reconcile(&mut self, manager: &mut InputManager, demand: CaptureDemand) {
        let domains = [
            CaptureDomain::Audio,
            CaptureDomain::Screen,
            CaptureDomain::Interaction,
        ];
        let mut succeeded = [false; 3];

        for (index, domain) in domains.into_iter().enumerate() {
            let desired_active = demand.is_active(domain);
            let observed_generation = manager.source_graph_generation();
            let desired_key = Self::key(observed_generation, demand, domain);
            if self.cached_key(domain) == Some(desired_key) {
                succeeded[index] = true;
                continue;
            }

            match domain.apply(manager, demand) {
                Ok(()) => succeeded[index] = true,
                Err(error) => warn!(
                    domain = domain.name(),
                    desired_active,
                    %error,
                    "Failed to update capture demand"
                ),
            }
        }

        let resulting_generation = manager.source_graph_generation();
        for (index, domain) in domains.into_iter().enumerate() {
            if succeeded[index] {
                self.set_cached_key(domain, Self::key(resulting_generation, demand, domain));
            }
        }
    }

    fn key(
        graph_generation: u64,
        demand: CaptureDemand,
        domain: CaptureDomain,
    ) -> CaptureDemandKey {
        CaptureDemandKey {
            graph_generation,
            desired_active: demand.is_active(domain),
            screen_demand: if matches!(domain, CaptureDomain::Screen) {
                demand.screen
            } else {
                ScreenCaptureDemand::Inactive
            },
        }
    }

    fn cached_key(&self, domain: CaptureDomain) -> Option<CaptureDemandKey> {
        match domain {
            CaptureDomain::Audio => self.audio,
            CaptureDomain::Screen => self.screen,
            CaptureDomain::Interaction => self.interaction,
        }
    }

    fn set_cached_key(&mut self, domain: CaptureDomain, key: CaptureDemandKey) {
        match domain {
            CaptureDomain::Audio => self.audio = Some(key),
            CaptureDomain::Screen => self.screen = Some(key),
            CaptureDomain::Interaction => self.interaction = Some(key),
        }
    }
}
