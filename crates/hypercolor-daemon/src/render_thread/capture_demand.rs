use std::time::{Duration, Instant};

use hypercolor_core::input::{
    InputCaptureDemand, InputManager, ScreenCaptureDemand, TryInputManagerIntent,
};
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

pub(super) enum CaptureDemandReconcile {
    Busy,
    Stale,
    Applied,
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

    pub(crate) fn reconcile(
        &mut self,
        manager: &InputManager,
        demand: CaptureDemand,
        is_current: impl FnOnce() -> bool,
    ) -> CaptureDemandReconcile {
        let domains = [
            CaptureDomain::Audio,
            CaptureDomain::Screen,
            CaptureDomain::Interaction,
        ];
        let now = Instant::now();
        let application = manager.try_apply_capture_demand_if(
            InputCaptureDemand::new(
                demand.audio_active,
                demand.screen,
                demand.interaction_active,
            ),
            is_current,
        );
        let TryInputManagerIntent::Applied(application) = application else {
            return match application {
                TryInputManagerIntent::Busy => CaptureDemandReconcile::Busy,
                TryInputManagerIntent::Stale => CaptureDemandReconcile::Stale,
                TryInputManagerIntent::Applied(_) => unreachable!(),
            };
        };
        let (resulting_generation, audio, screen, interaction) = application.into_parts();
        let results = [audio, screen, interaction];

        for (domain, (observed_generation, result)) in domains.into_iter().zip(results) {
            let desired_active = demand.is_active(domain);
            let desired_key = Self::key(resulting_generation, demand, domain);
            match result {
                Ok(()) => {
                    self.set_cached_key(domain, desired_key);
                    self.set_failed_attempt(domain, None);
                }
                Err(error) => {
                    if self.failure_warn_due(
                        domain,
                        Self::key(observed_generation, demand, domain),
                        now,
                    ) {
                        warn!(
                            domain = domain.name(),
                            desired_active,
                            %error,
                            "Failed to update capture demand"
                        );
                        self.set_failed_attempt(
                            domain,
                            Some(FailedCaptureDemand {
                                key: desired_key,
                                warn_again_at: now + FAILED_DEMAND_WARN_INTERVAL,
                            }),
                        );
                    }
                }
            }
        }
        CaptureDemandReconcile::Applied
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
    use std::sync::mpsc;

    use hypercolor_core::input::{
        AudioSource, AudioSourceRole, InputData, InputSource, ManagedSourceRole, MediaSource,
        ScreenSource, ScreenSourceRole, SourceRoleBinding,
    };

    use super::*;

    struct FailingScreenSource {
        attempts: Arc<AtomicUsize>,
        fail: Arc<AtomicBool>,
    }

    struct BlockingAudioDemandSource {
        entered: mpsc::SyncSender<()>,
        release: mpsc::Receiver<()>,
        running: bool,
    }

    impl InputSource for BlockingAudioDemandSource {
        fn name(&self) -> &'static str {
            "blocking_audio_demand"
        }

        fn start(&mut self) -> anyhow::Result<()> {
            self.running = true;
            Ok(())
        }

        fn stop(&mut self) {
            self.running = false;
        }

        fn sample(&mut self) -> anyhow::Result<InputData> {
            Ok(InputData::None)
        }

        fn is_running(&self) -> bool {
            self.running
        }
    }

    impl SourceRoleBinding for BlockingAudioDemandSource {
        type Role = AudioSourceRole;
    }

    impl AudioSource for BlockingAudioDemandSource {
        fn set_audio_capture_active(&mut self, _active: bool) -> anyhow::Result<()> {
            self.entered
                .send(())
                .expect("audio demand observer should remain connected");
            self.release
                .recv()
                .expect("audio demand release should remain connected");
            Ok(())
        }
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
        let manager = InputManager::new();
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

        assert!(matches!(
            state.reconcile(&manager, demand, || true),
            CaptureDemandReconcile::Applied
        ));
        assert_eq!(attempts.load(Ordering::Acquire), 1);
        assert!(!state.is_current(manager.source_graph_generation(), demand));
        let first_warn_window = state
            .failed_screen
            .expect("failed attempt is recorded")
            .warn_again_at;

        // The application retries on the very next tick; only the warn is
        // suppressed inside the pacing window.
        assert!(matches!(
            state.reconcile(&manager, demand, || true),
            CaptureDemandReconcile::Applied
        ));
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
        assert!(matches!(
            state.reconcile(&manager, demand, || true),
            CaptureDemandReconcile::Applied
        ));
        assert_eq!(attempts.load(Ordering::Acquire), 3);
        assert!(state.is_current(manager.source_graph_generation(), demand));
        assert!(state.failed_screen.is_none());
    }

    #[test]
    fn graph_swap_cannot_interleave_between_capture_domains_or_stamp_them_current() {
        let (entered_tx, entered_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let manager = InputManager::new();
        manager
            .add_source(ManagedSourceRole::audio(Box::new(
                BlockingAudioDemandSource {
                    entered: entered_tx,
                    release: release_rx,
                    running: false,
                },
            )))
            .expect("blocking audio source should register");
        manager
            .add_source(ManagedSourceRole::screen(Box::new(FailingScreenSource {
                attempts: Arc::new(AtomicUsize::new(0)),
                fail: Arc::new(AtomicBool::new(false)),
            })))
            .expect("screen source should register");
        manager.start_all().expect("capture sources should start");
        let extent = hypercolor_core::input::screen::PixelExtent::new(640, 480)
            .expect("fixture extent is valid");
        let demand = CaptureDemand::new(true, ScreenCaptureDemand::active(extent), true);
        let reconcile_manager = manager.clone();
        let reconciler = std::thread::spawn(move || {
            let mut state = CaptureDemandState::default();
            let outcome = state.reconcile(&reconcile_manager, demand, || true);
            (state, outcome)
        });
        entered_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("audio domain should hold the lifecycle transaction");
        let (attempted_tx, attempted_rx) = mpsc::sync_channel(1);
        let swap_manager = manager.clone();
        let swapper = std::thread::spawn(move || {
            attempted_tx
                .send(())
                .expect("swap observer should remain connected");
            swap_manager
                .add_source(ManagedSourceRole::data(Box::new(MediaSource::new())))
                .expect("media source should register after capture reconciliation");
        });
        attempted_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("graph swap should be waiting on lifecycle ownership");
        release_tx
            .send(())
            .expect("capture reconciliation should resume");
        let (state, outcome) = reconciler
            .join()
            .expect("capture reconciliation thread should finish");
        swapper.join().expect("graph swap thread should finish");

        assert!(matches!(outcome, CaptureDemandReconcile::Applied));
        assert!(
            !state.is_current(manager.source_graph_generation(), demand),
            "the later graph swap must invalidate every domain cache"
        );
    }
}
