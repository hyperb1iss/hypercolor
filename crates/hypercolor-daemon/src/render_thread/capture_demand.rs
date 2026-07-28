use hypercolor_core::input::{InputManager, SourceKind};
use tracing::warn;

use super::input_publication::InputPublicationDemand;

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
    audio: Option<CaptureDemandKey>,
    screen: Option<CaptureDemandKey>,
    interaction: Option<CaptureDemandKey>,
}

impl CaptureDemandState {
    pub(crate) fn is_current(&self, graph_generation: u64, demand: InputPublicationDemand) -> bool {
        self.cached_key(CaptureDomain::Audio)
            == Some(Self::key(graph_generation, demand, SourceKind::Audio))
            && self.cached_key(CaptureDomain::Screen)
                == Some(Self::key(graph_generation, demand, SourceKind::Screen))
            && self.cached_key(CaptureDomain::Interaction)
                == Some(Self::key(graph_generation, demand, SourceKind::Interaction))
    }

    pub(crate) fn reconcile(&mut self, manager: &mut InputManager, demand: InputPublicationDemand) {
        let domains = [
            (CaptureDomain::Audio, SourceKind::Audio),
            (CaptureDomain::Screen, SourceKind::Screen),
            (CaptureDomain::Interaction, SourceKind::Interaction),
        ];
        let mut succeeded = [false; 3];

        for (index, (domain, source)) in domains.into_iter().enumerate() {
            let desired_active = demand.requested_hz(source) > 0;
            let observed_generation = manager.source_graph_generation();
            let desired_key = CaptureDemandKey {
                graph_generation: observed_generation,
                desired_active,
            };
            if self.cached_key(domain) == Some(desired_key) {
                succeeded[index] = true;
                continue;
            }

            match domain.apply(manager, desired_active) {
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
        for (index, (domain, source)) in domains.into_iter().enumerate() {
            if succeeded[index] {
                self.set_cached_key(domain, Self::key(resulting_generation, demand, source));
            }
        }
    }

    const fn key(
        graph_generation: u64,
        demand: InputPublicationDemand,
        source: SourceKind,
    ) -> CaptureDemandKey {
        CaptureDemandKey {
            graph_generation,
            desired_active: demand.requested_hz(source) > 0,
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
