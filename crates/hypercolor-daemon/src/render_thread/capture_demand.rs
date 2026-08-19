use std::time::{Duration, Instant};

use hypercolor_core::input::{InputManager, ScreenCaptureDemand};
use tracing::warn;

/// How often a persistently failing demand application repeats its warning.
/// Only the log line is paced: the application itself retries on every
/// reconcile tick, because delaying the retry would hold capture inactive
/// for the pacing interval after a transient failure (a nerf the render
/// pipeline's latency tests reject).
const FAILED_DEMAND_WARN_INTERVAL: Duration = Duration::from_secs(1);

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
struct FailedCaptureDemand {
    key: CaptureDemandKey,
    warn_again_at: Instant,
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
    failed_audio: Option<FailedCaptureDemand>,
    failed_screen: Option<FailedCaptureDemand>,
    failed_interaction: Option<FailedCaptureDemand>,
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
        let mut failed = [false; 3];
        let mut warned = [false; 3];
        let now = Instant::now();

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
                Err(error) => {
                    failed[index] = true;
                    if self.failure_warn_due(domain, desired_key, now) {
                        warned[index] = true;
                        warn!(
                            domain = domain.name(),
                            desired_active,
                            %error,
                            "Failed to update capture demand"
                        );
                    }
                }
            }
        }

        let resulting_generation = manager.source_graph_generation();
        for (index, domain) in domains.into_iter().enumerate() {
            if succeeded[index] {
                self.set_cached_key(domain, Self::key(resulting_generation, demand, domain));
                self.set_failed_attempt(domain, None);
            } else if failed[index] && warned[index] {
                // Refresh the marker only when a warning fired, so a
                // persistent failure keeps warning once per interval
                // instead of sliding the window forever.
                self.set_failed_attempt(
                    domain,
                    Some(FailedCaptureDemand {
                        key: Self::key(resulting_generation, demand, domain),
                        warn_again_at: now + FAILED_DEMAND_WARN_INTERVAL,
                    }),
                );
            }
        }
    }

    fn failure_warn_due(&self, domain: CaptureDomain, key: CaptureDemandKey, now: Instant) -> bool {
        self.failed_attempt(domain)
            .is_none_or(|attempt| attempt.key != key || now >= attempt.warn_again_at)
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

    fn failed_attempt(&self, domain: CaptureDomain) -> Option<FailedCaptureDemand> {
        match domain {
            CaptureDomain::Audio => self.failed_audio,
            CaptureDomain::Screen => self.failed_screen,
            CaptureDomain::Interaction => self.failed_interaction,
        }
    }

    fn set_failed_attempt(&mut self, domain: CaptureDomain, attempt: Option<FailedCaptureDemand>) {
        match domain {
            CaptureDomain::Audio => self.failed_audio = attempt,
            CaptureDomain::Screen => self.failed_screen = attempt,
            CaptureDomain::Interaction => self.failed_interaction = attempt,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use hypercolor_core::input::{
        InputData, InputSource, ManagedSourceRole, ScreenSource, ScreenSourceRole,
        SourceRoleBinding,
    };

    use super::*;

    struct FailingScreenSource {
        attempts: Arc<AtomicUsize>,
        fail: Arc<AtomicBool>,
    }

    impl InputSource for FailingScreenSource {
        fn name(&self) -> &'static str {
            "failing_screen"
        }

        fn start(&mut self) -> anyhow::Result<()> {
            Ok(())
        }

        fn stop(&mut self) {}

        fn sample(&mut self) -> anyhow::Result<InputData> {
            Ok(InputData::None)
        }

        fn is_running(&self) -> bool {
            true
        }
    }

    impl SourceRoleBinding for FailingScreenSource {
        type Role = ScreenSourceRole;
    }

    impl ScreenSource for FailingScreenSource {
        fn set_screen_capture_demand(&mut self, demand: ScreenCaptureDemand) -> anyhow::Result<()> {
            if !demand.is_active() {
                return Ok(());
            }
            self.attempts.fetch_add(1, Ordering::AcqRel);
            if self.fail.load(Ordering::Acquire) {
                anyhow::bail!("injected screen demand failure");
            }
            Ok(())
        }
    }

    #[test]
    fn failed_demand_retries_every_tick_and_paces_only_the_warning() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let fail = Arc::new(AtomicBool::new(true));
        let mut manager = InputManager::new();
        manager
            .add_source(ManagedSourceRole::screen(Box::new(FailingScreenSource {
                attempts: Arc::clone(&attempts),
                fail: Arc::clone(&fail),
            })))
            .expect("failing screen source should register");

        let extent = hypercolor_core::input::screen::PixelExtent::new(640, 480)
            .expect("fixture extent is valid");
        let demand = CaptureDemand::new(false, ScreenCaptureDemand::active(extent), false);
        let mut state = CaptureDemandState::default();

        state.reconcile(&mut manager, demand);
        assert_eq!(attempts.load(Ordering::Acquire), 1);
        assert!(!state.is_current(manager.source_graph_generation(), demand));
        let first_warn_window = state
            .failed_screen
            .expect("failed attempt is recorded")
            .warn_again_at;

        // The application retries on the very next tick; only the warn is
        // suppressed inside the pacing window.
        state.reconcile(&mut manager, demand);
        assert_eq!(attempts.load(Ordering::Acquire), 2);
        assert_eq!(
            state
                .failed_screen
                .expect("suppressed failure keeps the original warn window")
                .warn_again_at,
            first_warn_window
        );

        // Recovery is immediate once the failure clears.
        fail.store(false, Ordering::Release);
        state.reconcile(&mut manager, demand);
        assert_eq!(attempts.load(Ordering::Acquire), 3);
        assert!(state.is_current(manager.source_graph_generation(), demand));
        assert!(state.failed_screen.is_none());
    }
}
