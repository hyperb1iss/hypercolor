//! Whole transactions through the shared coordinator against the in-memory
//! machine, including a crash and a power loss at every effect boundary.

use std::sync::{Arc, Mutex};

use super::fakes::{
    Bootstrap, Boundaries, Broker, CANDIDATE_VERSION, Crash, FakeExecutor, FakeSession, Fixture,
    PRIOR_VERSION, SharedBoundaries, SharedWorld, UNITS_ROOT, World, identity_of, lock,
    member_path, survive,
};
use crate::windows::{
    BootstrapSlot, WINDOWS_APP_PROGRAM, WINDOWS_BOOTSTRAP_PROGRAM, WINDOWS_BROKER_PROGRAM,
    WINDOWS_DAEMON_PROGRAM, WindowsFirewallObservation, WindowsFirewallRule, WindowsInstallConfig,
    WindowsInstallPlatform, WindowsRunningProcess,
};
use crate::{
    InstallAction, InstallCoordinator, InstallCoordinatorError, InstallDisposition,
    InstallJournalV1, InstallModelError, InstallOutcome, InstallRequest, InstallTargetPolicy,
    InstallTransactionId, UnitId,
};

type Platform = WindowsInstallPlatform<FakeExecutor, FakeSession>;

const CONVERGENCE_RUNS: usize = 16;
/// The slot the update service runs from when a transaction begins.
const PROVEN: BootstrapSlot = BootstrapSlot::A;

/// The digest of the bootstrap `unit` ships.
fn bootstrap_of(unit: &UnitId) -> String {
    identity_of(&member_path(unit, WINDOWS_BOOTSTRAP_PROGRAM)).sha256
}

struct Harness {
    fixture: Fixture,
    world: SharedWorld,
    boundaries: SharedBoundaries,
}

impl Harness {
    /// An installation running the prior: its broker up and every name
    /// pointing at its unit.
    fn new() -> Self {
        let fixture = Fixture::new();
        let prior = fixture.prior.id().clone();
        let mut world = World {
            store_root: fixture.store.root().to_path_buf(),
            bootstrap: Some(Bootstrap {
                image: PROVEN,
                fallback: PROVEN.other(),
                slot_a: bootstrap_of(&prior),
                slot_b: bootstrap_of(&prior),
            }),
            broken_bootstrap: None,
            broker: Some(Broker {
                image: format!("\"{}\"", member_path(&prior, WINDOWS_BROKER_PROGRAM)),
                process: None,
            }),
            firewall: Some(WindowsFirewallObservation {
                daemon: Some(member_path(&prior, WINDOWS_DAEMON_PROGRAM)),
                app: Some(member_path(&prior, WINDOWS_APP_PROGRAM)),
            }),
            apps: Vec::new(),
            daemons: Vec::new(),
            next_pid: 100,
            clock: 0,
            session: true,
            app_reachable: true,
            app_obeys: true,
            app_hangs_on_quit: false,
            dies_after_start: false,
            looks_left: None,
            unhealthy_version: None,
            never_ready_version: None,
            failing_broker: None,
            failing_rule: None,
            dies_in_probation: false,
            hangs_in_probation: false,
            swapped_image: None,
            relocated_image: None,
            waited_out_deadlines: 0,
            probation_windows: Vec::new(),
        };
        world.reboot();
        world.session = true;
        world.launch(&prior);
        let harness = Self {
            fixture,
            world: Arc::new(Mutex::new(world)),
            boundaries: Arc::new(Mutex::new(Boundaries::default())),
        };
        harness.assert_runs(&prior);
        harness
    }

    fn prior(&self) -> UnitId {
        self.fixture.prior.id().clone()
    }

    fn candidate(&self) -> UnitId {
        self.fixture.candidate.id().clone()
    }

    /// A platform as a freshly started process builds it.
    fn platform(&self) -> Platform {
        WindowsInstallPlatform::new(
            FakeExecutor {
                world: Arc::clone(&self.world),
                boundaries: Arc::clone(&self.boundaries),
            },
            FakeSession {
                world: Arc::clone(&self.world),
                boundaries: Arc::clone(&self.boundaries),
            },
            WindowsInstallConfig::new(UNITS_ROOT),
            [self.fixture.prior.clone(), self.fixture.candidate.clone()],
        )
        .expect("Windows platform")
    }

    fn request(&self, target_policy: InstallTargetPolicy) -> InstallRequest {
        InstallRequest {
            transaction_id: InstallTransactionId::new("windows-transaction").expect("id"),
            candidate: self.fixture.candidate.clone(),
            target_policy,
        }
    }

    fn crash_at(&self, boundary: Option<usize>) {
        lock(&self.boundaries).crash_at = boundary;
    }

    fn passed(&self) -> usize {
        lock(&self.boundaries).passed
    }

    fn install_with(
        &self,
        target_policy: InstallTargetPolicy,
    ) -> Result<Result<InstallOutcome, InstallCoordinatorError>, Crash> {
        let mut platform = self.platform();
        let request = self.request(target_policy);
        survive(|| {
            self.fixture.with_lock(|lock| {
                InstallCoordinator::new(&self.fixture.store, &mut platform)
                    .install_with_lock(request, lock)
            })
        })
    }

    fn install(&self) -> Result<Result<InstallOutcome, InstallCoordinatorError>, Crash> {
        self.install_with(InstallTargetPolicy::Preserve)
    }

    /// One recovery run, and whether it stopped to wait for the owner.
    fn recover(
        &self,
    ) -> (
        Result<Result<Option<InstallOutcome>, InstallCoordinatorError>, Crash>,
        bool,
    ) {
        let mut platform = self.platform();
        let result = survive(|| {
            self.fixture.with_lock(|lock| {
                InstallCoordinator::new(&self.fixture.store, &mut platform).recover_with_lock(lock)
            })
        });
        (result, platform.waiting_for_owner())
    }

    /// Recover until the journal is terminal.
    fn converge(&self) -> InstallOutcome {
        for _ in 0..CONVERGENCE_RUNS {
            match self.recover().0 {
                Ok(Ok(Some(outcome))) => return outcome,
                Ok(Ok(None)) => panic!("no journal to recover"),
                Ok(Err(failure)) => panic!("recovery failed: {failure}"),
                Err(Crash) => {}
            }
        }
        panic!("recovery never converged");
    }

    fn outcome(&self) -> InstallOutcome {
        match self.install() {
            Ok(result) => result.expect("transaction"),
            Err(Crash) => panic!("no crash was armed"),
        }
    }

    fn effects(&self, label: &str) -> usize {
        lock(&self.boundaries)
            .log
            .iter()
            .filter(|entry| *entry == label)
            .count()
    }

    fn with_world(&self, change: impl FnOnce(&mut World)) {
        change(&mut lock(&self.world));
    }

    fn assert_names(&self, unit: &UnitId) {
        let world = lock(&self.world);
        if let Some(bootstrap) = &world.bootstrap {
            let image = if *unit == self.candidate() {
                PROVEN.other()
            } else {
                PROVEN
            };
            assert_eq!(bootstrap.image, image, "the update service's image");
            assert_eq!(
                bootstrap.held(bootstrap.fallback),
                bootstrap_of(&self.prior()),
                "the failure action names a proven bootstrap"
            );
            assert_eq!(
                bootstrap.held(PROVEN),
                bootstrap_of(&self.prior()),
                "the proven slot is never written"
            );
        }
        assert_eq!(
            world.broker.as_ref().map(|broker| broker.image.clone()),
            Some(format!("\"{}\"", member_path(unit, WINDOWS_BROKER_PROGRAM))),
            "broker image"
        );
        assert_eq!(
            world.firewall,
            Some(WindowsFirewallObservation {
                daemon: Some(member_path(unit, WINDOWS_DAEMON_PROGRAM)),
                app: Some(member_path(unit, WINDOWS_APP_PROGRAM)),
            }),
            "firewall rules"
        );
    }

    fn assert_runs(&self, unit: &UnitId) {
        let world = lock(&self.world);
        let images = |processes: &[WindowsRunningProcess]| {
            processes
                .iter()
                .map(|process| process.image.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            images(&world.apps),
            [member_path(unit, WINDOWS_APP_PROGRAM)]
        );
        assert_eq!(
            images(&world.daemons),
            [member_path(unit, WINDOWS_DAEMON_PROGRAM)]
        );
    }

    fn assert_broker_runs(&self) {
        assert!(
            lock(&self.world)
                .broker
                .as_ref()
                .is_some_and(|broker| broker.process.is_some()),
            "the broker must run"
        );
    }

    fn assert_committed(&self, outcome: &InstallOutcome) {
        let candidate = self.candidate();
        assert_eq!(
            outcome,
            &InstallOutcome::Committed {
                active_unit: candidate.clone()
            }
        );
        assert_eq!(self.fixture.active_unit().as_ref(), Some(&candidate));
        self.assert_names(&candidate);
        self.assert_runs(&candidate);
        self.assert_broker_runs();
    }

    /// The prior's names are back and it runs again.
    fn assert_restored(&self) {
        let prior = self.prior();
        assert_eq!(self.fixture.active_unit().as_ref(), Some(&prior));
        self.assert_names(&prior);
        self.assert_runs(&prior);
        self.assert_broker_runs();
    }

    fn journal(&self) -> InstallJournalV1 {
        self.fixture.journal().expect("a journal")
    }
}

fn is_rolled_back(outcome: &InstallOutcome) -> bool {
    matches!(outcome, InstallOutcome::RolledBack { .. })
}

#[test]
fn an_update_moves_every_name_and_runs_the_candidate() {
    let harness = Harness::new();
    let baseline = lock(&harness.world).daemons[0].id;

    let outcome = harness.outcome();

    harness.assert_committed(&outcome);
    let world = lock(&harness.world);
    assert_ne!(world.daemons[0].id, baseline, "the candidate is fresh");
    assert_eq!(
        world.probation_windows,
        [crate::windows::WINDOWS_PROBATION_WINDOW]
    );
    drop(world);
    let journal = harness.journal();
    assert!(journal.forced_stops.is_empty());
    assert_eq!(journal.layout_operation_count, 4);
    assert_eq!(
        lock(&harness.world)
            .bootstrap
            .as_ref()
            .map(|bootstrap| bootstrap.held(PROVEN.other()).to_owned()),
        Some(bootstrap_of(&harness.candidate())),
        "the other slot holds the candidate's bootstrap"
    );
    assert_eq!(harness.effects("terminate:before"), 0);
}

#[test]
fn an_update_stops_the_broker_only_after_the_daemon_and_starts_it_before_the_app() {
    let harness = Harness::new();
    harness.outcome();
    let log = lock(&harness.boundaries).log.clone();
    let at = |label: &str| {
        log.iter()
            .position(|entry| entry == label)
            .unwrap_or_else(|| panic!("{label} never ran"))
    };
    assert!(at("quit:after") < at("bootstrap-fallback:before"));
    assert!(at("bootstrap-fallback:after") < at("bootstrap-write:before"));
    assert!(at("bootstrap-write:after") < at("bootstrap-image:before"));
    assert!(at("bootstrap-image:after") < at("broker-stop:before"));
    assert!(at("broker-switch:after") < at("broker-start:before"));
    assert!(at("broker-start:after") < at("start:before"));
}

#[test]
fn a_crash_at_every_boundary_ends_committed_or_restored() {
    let total = {
        let harness = Harness::new();
        harness.outcome();
        harness.passed()
    };
    assert!(total > 20, "only {total} boundaries");
    for crash_at in 1..=total {
        let harness = Harness::new();
        harness.crash_at(Some(crash_at));
        let first = harness.install();
        assert!(matches!(first, Err(Crash)), "boundary {crash_at} never ran");
        let outcome = harness.converge();
        if is_rolled_back(&outcome) {
            harness.assert_restored();
        } else {
            harness.assert_committed(&outcome);
        }
    }
}

#[test]
fn a_crash_at_every_boundary_of_a_rollback_still_restores_the_prior() {
    let total = {
        let harness = Harness::new();
        fail_the_candidate(&harness);
        assert!(is_rolled_back(&harness.outcome()));
        harness.passed()
    };
    for crash_at in 1..=total {
        let harness = Harness::new();
        fail_the_candidate(&harness);
        harness.crash_at(Some(crash_at));
        assert!(
            matches!(harness.install(), Err(Crash)),
            "boundary {crash_at} never ran"
        );
        let outcome = harness.converge();
        assert!(is_rolled_back(&outcome), "boundary {crash_at}: {outcome:?}");
        harness.assert_restored();
    }
}

#[test]
fn a_power_loss_at_every_boundary_restores_the_prior_for_its_owner() {
    let total = {
        let harness = Harness::new();
        harness.outcome();
        harness.passed()
    };
    for crash_at in 1..=total {
        let harness = Harness::new();
        harness.crash_at(Some(crash_at));
        assert!(matches!(harness.install(), Err(Crash)));
        harness.with_world(World::reboot);

        // At boot nobody is signed in, so recovery can start nothing.
        let (booted, waiting) = harness.recover();
        let booted = booted.expect("no crash armed for this run");
        let committed = matches!(&booted, Ok(Some(InstallOutcome::Committed { .. })));
        if committed {
            // The candidate proved itself before the power went.
            continue;
        }
        if !matches!(&booted, Ok(Some(_))) {
            assert!(waiting, "boundary {crash_at}: {booted:?}");
            let journal = harness.journal();
            assert_eq!(journal.disposition, InstallDisposition::Rollback);
            assert_eq!(
                journal.next_action,
                Some(InstallAction::RestorePriorRuntime),
                "the journal rests at the step only the owner's session can finish"
            );
            assert_eq!(
                harness.fixture.active_unit().as_ref(),
                Some(&harness.prior())
            );
            harness.assert_names(&harness.prior());
        }

        // The owner signs in, and the recovery that wakes finishes the job.
        harness.with_world(|world| world.session = true);
        let outcome = harness.converge();
        assert!(is_rolled_back(&outcome), "boundary {crash_at}: {outcome:?}");
        harness.assert_restored();
    }
}

#[test]
fn a_power_loss_at_every_boundary_of_a_rollback_restores_the_prior_for_its_owner() {
    let total = {
        let harness = Harness::new();
        fail_the_candidate(&harness);
        assert!(is_rolled_back(&harness.outcome()));
        harness.passed()
    };
    for crash_at in 1..=total {
        let harness = Harness::new();
        fail_the_candidate(&harness);
        harness.crash_at(Some(crash_at));
        assert!(
            matches!(harness.install(), Err(Crash)),
            "boundary {crash_at} never ran"
        );
        harness.with_world(World::reboot);

        let (booted, waiting) = harness.recover();
        let booted = booted.expect("no crash armed for this run");
        assert!(
            waiting || matches!(booted, Ok(Some(_))),
            "boundary {crash_at} wedged: {booted:?}"
        );

        harness.with_world(|world| world.session = true);
        let outcome = harness.converge();
        assert!(is_rolled_back(&outcome), "boundary {crash_at}: {outcome:?}");
        harness.assert_restored();
    }
}

/// A journal resting at the prior's proof, with nothing running.
fn prior_lost_at_its_proof() -> Harness {
    // The candidate fails its first request; the second is the prior's.
    let proof = boundary_of(fail_the_candidate, "http", 2);
    let harness = Harness::new();
    fail_the_candidate(&harness);
    harness.crash_at(Some(proof));
    assert!(matches!(harness.install(), Err(Crash)));
    harness.crash_at(None);
    assert_eq!(
        harness.journal().next_action,
        Some(InstallAction::ProvePrior)
    );
    harness.with_world(|world| {
        world.apps.clear();
        world.daemons.clear();
    });
    harness
}

#[test]
fn a_prior_lost_at_its_proof_is_started_again() {
    let harness = prior_lost_at_its_proof();
    let starts = harness.effects("start:before");

    let outcome = harness.converge();

    assert!(is_rolled_back(&outcome), "{outcome:?}");
    harness.assert_restored();
    assert_eq!(harness.effects("start:before"), starts + 1);
}

#[test]
fn a_prior_that_keeps_dying_at_its_proof_ends_the_run() {
    let harness = prior_lost_at_its_proof();
    harness.with_world(|world| world.dies_after_start = true);
    let starts = harness.effects("start:before");

    let (result, _) = harness.recover();

    assert!(result.expect("no crash armed").is_err());
    assert_eq!(harness.effects("start:before"), starts + 1);

    harness.with_world(|world| world.dies_after_start = false);
    let outcome = harness.converge();
    assert!(is_rolled_back(&outcome), "{outcome:?}");
    harness.assert_restored();
}

#[test]
fn a_crash_inside_a_partial_stop_finishes_that_stop() {
    let quit = boundary_of(|_| {}, "quit:after", 1);
    let harness = Harness::new();
    harness.with_world(|world| world.app_hangs_on_quit = true);
    harness.crash_at(Some(quit));
    assert!(matches!(harness.install(), Err(Crash)));
    harness.crash_at(None);
    {
        let world = lock(&harness.world);
        assert_eq!((world.apps.len(), world.daemons.len()), (1, 0));
    }

    let outcome = harness.converge();

    harness.assert_committed(&outcome);
    assert_eq!(
        harness.journal().forced_stops,
        [harness.prior()],
        "the app that hung is forced down and said so"
    );
}

#[test]
fn a_prior_that_restarted_before_its_stop_abandons_the_transaction() {
    let harness = Harness::new();
    harness.crash_at(Some(1));
    assert!(matches!(harness.install(), Err(Crash)));
    harness.crash_at(None);
    assert_eq!(
        harness.journal().next_action,
        Some(InstallAction::UnloadPrior)
    );

    // The owner closed the app and opened it again before the stop landed.
    let prior = harness.prior();
    harness.with_world(|world| {
        world.apps.clear();
        world.daemons.clear();
        world.launch(&prior);
    });
    let running = lock(&harness.world).daemons[0].id;

    let outcome = harness.converge();

    assert!(
        matches!(
            outcome,
            InstallOutcome::RolledBack {
                abandoned: true,
                ..
            }
        ),
        "{outcome:?}"
    );
    harness.assert_restored();
    assert_eq!(
        lock(&harness.world).daemons[0].id,
        running,
        "a release the transaction never touched keeps running"
    );
}

#[test]
fn a_firewall_lost_in_a_rollback_holds_it_until_the_rules_can_be_restored() {
    let stop = boundary_of(fail_the_candidate, "quit:before", 2);
    let harness = Harness::new();
    fail_the_candidate(&harness);
    harness.crash_at(Some(stop));
    assert!(matches!(harness.install(), Err(Crash)));
    harness.crash_at(None);
    let rules = lock(&harness.world).firewall.take();

    let (result, _) = harness.recover();

    assert!(result.expect("no crash armed").is_err());
    assert_ne!(
        harness.journal().disposition,
        InstallDisposition::RolledBack,
        "a rollback must not finish over rules it could not restore"
    );

    harness.with_world(|world| world.firewall = rules);
    let outcome = harness.converge();
    assert!(is_rolled_back(&outcome), "{outcome:?}");
    harness.assert_restored();
}

#[test]
fn a_prior_the_launcher_started_at_sign_in_is_proven_and_its_broker_restarted() {
    let harness = Harness::new();
    harness.with_world(|world| world.session = false);
    let outcome = harness.install().expect("no crash");
    assert!(outcome.is_err(), "nobody is signed in: {outcome:?}");
    assert!(
        harness
            .journal()
            .failure
            .is_some_and(|failure| failure.contains("no session"))
    );

    let prior = harness.prior();
    harness.with_world(|world| {
        world.session = true;
        world.launch(&prior);
        if let Some(broker) = &mut world.broker {
            broker.process = None;
        }
    });

    let outcome = harness.converge();
    assert!(is_rolled_back(&outcome), "{outcome:?}");
    harness.assert_restored();
    assert_eq!(lock(&harness.world).apps.len(), 1);
}

#[test]
fn a_candidate_that_fails_its_proof_rolls_every_name_back() {
    let harness = Harness::new();
    fail_the_candidate(&harness);

    let outcome = harness.outcome();

    harness.assert_restored();
    let InstallOutcome::RolledBack {
        active_unit,
        failure,
        abandoned,
        restored,
    } = outcome
    else {
        panic!("the candidate must not commit");
    };
    assert_eq!(active_unit.as_ref(), Some(&harness.prior()));
    assert!(!abandoned);
    assert!(failure.contains("does not answer"), "{failure}");
    let restored = restored.expect("the rollback proved the prior");
    let world = lock(&harness.world);
    assert_eq!(restored.unit, harness.prior());
    assert_eq!(restored.version, PRIOR_VERSION);
    assert_eq!(restored.process_id, world.daemons[0].id.pid);
    assert_eq!(
        restored.executable_sha256,
        identity_of(&member_path(&harness.prior(), WINDOWS_DAEMON_PROGRAM)).sha256
    );
}

#[test]
fn a_bootstrap_that_will_not_load_costs_nothing_but_the_update() {
    let harness = Harness::new();
    let broken = bootstrap_of(&harness.candidate());
    harness.with_world(|world| world.broken_bootstrap = Some(broken));

    let outcome = harness.outcome();

    let InstallOutcome::RolledBack { failure, .. } = &outcome else {
        panic!("a bootstrap that does not load must not commit");
    };
    assert!(failure.contains("did not load"), "{failure}");
    harness.assert_restored();
    assert_eq!(
        harness.effects("bootstrap-image:before"),
        0,
        "ImagePath never moved"
    );
    assert_eq!(
        harness.effects("broker-stop:before"),
        0,
        "the broker was never stopped"
    );
}

#[test]
fn a_release_with_the_same_bootstrap_leaves_the_service_alone() {
    let harness = Harness::new();
    let same = bootstrap_of(&harness.candidate());
    harness.with_world(|world| {
        if let Some(bootstrap) = &mut world.bootstrap {
            bootstrap.slot_a.clone_from(&same);
        }
    });

    let outcome = harness.outcome();

    assert!(matches!(outcome, InstallOutcome::Committed { .. }));
    harness.assert_runs(&harness.candidate());
    assert_eq!(harness.effects("bootstrap-fallback:before"), 0);
    assert_eq!(harness.effects("bootstrap-write:before"), 0);
    assert_eq!(harness.effects("bootstrap-image:before"), 0);
    assert_eq!(
        lock(&harness.world)
            .bootstrap
            .as_ref()
            .map(|bootstrap| bootstrap.image),
        Some(PROVEN)
    );
}

#[test]
fn a_daemon_that_never_gets_ready_rolls_back() {
    let harness = Harness::new();
    harness.with_world(|world| {
        world.never_ready_version = Some(CANDIDATE_VERSION.to_owned());
    });
    let outcome = harness.outcome();
    assert!(is_rolled_back(&outcome), "{outcome:?}");
    harness.assert_restored();
}

#[test]
fn a_candidate_broker_that_will_not_start_rolls_back() {
    let harness = Harness::new();
    let broker = member_path(&harness.candidate(), WINDOWS_BROKER_PROGRAM);
    harness.with_world(|world| world.failing_broker = Some(broker));

    let outcome = harness.outcome();

    assert!(is_rolled_back(&outcome), "{outcome:?}");
    harness.assert_restored();
    assert_eq!(
        harness.effects("start:before"),
        1,
        "only the prior's app ever started"
    );
}

#[test]
fn a_broker_running_the_wrong_file_fails_the_proof() {
    let harness = Harness::new();
    let broker = member_path(&harness.candidate(), WINDOWS_BROKER_PROGRAM);
    harness.with_world(|world| world.swapped_image = Some(broker));
    let outcome = harness.outcome();
    assert!(is_rolled_back(&outcome), "{outcome:?}");
    harness.assert_restored();
}

#[test]
fn a_daemon_whose_image_is_not_the_unit_s_fails_the_proof() {
    let harness = Harness::new();
    let daemon = member_path(&harness.candidate(), WINDOWS_DAEMON_PROGRAM);
    harness.with_world(|world| world.swapped_image = Some(daemon));

    let outcome = harness.outcome();

    let InstallOutcome::RolledBack { failure, .. } = &outcome else {
        panic!("the swapped image must not commit");
    };
    assert!(failure.contains("is not the unit's"), "{failure}");
    harness.assert_restored();
    assert_eq!(
        harness.effects("probation"),
        0,
        "no probation for a failed proof"
    );
}

#[test]
fn a_broker_that_reports_another_path_fails_the_proof() {
    let harness = Harness::new();
    let broker = member_path(&harness.candidate(), WINDOWS_BROKER_PROGRAM);
    harness.with_world(|world| {
        world.relocated_image = Some((broker, r"C:\elsewhere\broker.exe".to_owned()));
    });

    let outcome = harness.outcome();

    let InstallOutcome::RolledBack { failure, .. } = &outcome else {
        panic!("a broker elsewhere must not commit");
    };
    assert!(failure.contains(r"C:\elsewhere\broker.exe"), "{failure}");
    harness.assert_restored();
}

/// The number of boundaries passed when `label` first ran `nth` times.
fn boundary_of(arrange: fn(&Harness), label: &str, nth: usize) -> usize {
    let harness = Harness::new();
    arrange(&harness);
    harness.outcome();
    let log = lock(&harness.boundaries).log.clone();
    log.iter()
        .enumerate()
        .filter(|(_, entry)| *entry == label)
        .map(|(index, _)| index + 1)
        .nth(nth - 1)
        .unwrap_or_else(|| panic!("{label} never ran {nth} times"))
}

fn fail_the_candidate(harness: &Harness) {
    harness.with_world(|world| {
        world.unhealthy_version = Some(CANDIDATE_VERSION.to_owned());
    });
}

#[test]
fn a_candidate_that_restarted_after_its_receipt_fails_its_proof() {
    // The first request of the proof comes after the receipt is journaled.
    let proof = boundary_of(|_| {}, "http", 1);
    let harness = Harness::new();
    harness.crash_at(Some(proof));
    assert!(matches!(harness.install(), Err(Crash)));
    harness.crash_at(None);
    assert_eq!(
        harness.journal().next_action,
        Some(InstallAction::ProveCandidate)
    );

    // The app restarted its daemon while nothing watched it.
    let candidate = harness.candidate();
    harness.with_world(|world| {
        world.apps.clear();
        world.daemons.clear();
        world.launch(&candidate);
    });

    let outcome = harness.converge();

    assert!(is_rolled_back(&outcome), "{outcome:?}");
    harness.assert_restored();
}

/// Fail the candidate and crash where the rollback restarts the prior, then
/// put the processes the transaction started from back in its place.
fn return_the_baseline_at_restart(arrange: fn(&Harness)) -> (Harness, WindowsRunningProcess) {
    let arrange_all = |harness: &Harness| {
        fail_the_candidate(harness);
        arrange(harness);
    };
    let total = {
        let harness = Harness::new();
        arrange_all(&harness);
        harness.outcome();
        let log = lock(&harness.boundaries).log.clone();
        log.iter()
            .enumerate()
            .filter(|(_, entry)| *entry == "start:before")
            .map(|(index, _)| index + 1)
            .nth(1)
            .expect("the prior restarts")
    };
    let harness = Harness::new();
    let (app, daemon) = {
        let world = lock(&harness.world);
        (world.apps[0].clone(), world.daemons[0].clone())
    };
    arrange_all(&harness);
    harness.crash_at(Some(total));
    assert!(matches!(harness.install(), Err(Crash)));
    harness.crash_at(None);
    assert_eq!(
        harness.journal().next_action,
        Some(InstallAction::RestorePriorRuntime)
    );

    // Whatever runs now carries the identity the transaction started from.
    harness.with_world(|world| {
        world.apps = vec![app];
        world.daemons = vec![daemon.clone()];
    });
    (harness, daemon)
}

#[test]
fn a_prior_that_never_stopped_does_not_pass_for_a_restored_one() {
    let (harness, baseline) = return_the_baseline_at_restart(|_| {});
    let starts = harness.effects("start:before");

    let outcome = harness.converge();

    assert!(is_rolled_back(&outcome), "{outcome:?}");
    harness.assert_restored();
    assert_eq!(harness.effects("start:before"), starts + 1);
    assert_ne!(lock(&harness.world).daemons[0].id, baseline.id);
}

#[test]
fn a_release_forced_down_twice_is_journaled_once() {
    let (harness, _) = return_the_baseline_at_restart(|harness| {
        harness.with_world(|world| world.app_reachable = false);
    });

    let outcome = harness.converge();

    assert!(is_rolled_back(&outcome), "{outcome:?}");
    assert_eq!(
        harness.journal().forced_stops,
        [harness.prior(), harness.candidate()]
    );
}

#[test]
fn a_daemon_that_dies_in_probation_rolls_back() {
    let harness = Harness::new();
    harness.with_world(|world| world.dies_in_probation = true);

    let outcome = harness.outcome();

    let InstallOutcome::RolledBack { failure, .. } = &outcome else {
        panic!("a dead daemon must not commit");
    };
    assert!(failure.contains("failed probation"), "{failure}");
}

#[test]
fn an_api_that_hangs_by_the_end_of_probation_rolls_back() {
    let harness = Harness::new();
    harness.with_world(|world| world.hangs_in_probation = true);

    let outcome = harness.outcome();

    let InstallOutcome::RolledBack { failure, .. } = &outcome else {
        panic!("a hung API must not commit");
    };
    assert!(failure.contains("end of its 90 s probation"), "{failure}");
    harness.assert_restored();
}

#[test]
fn an_app_that_ignores_the_request_is_forced_and_journaled() {
    let harness = Harness::new();
    harness.with_world(|world| world.app_obeys = false);

    let outcome = harness.outcome();

    harness.assert_committed(&outcome);
    assert_eq!(harness.journal().forced_stops, [harness.prior()]);
    let world = lock(&harness.world);
    assert_eq!(world.waited_out_deadlines, 1, "it got its whole deadline");
}

#[test]
fn an_app_out_of_reach_is_forced_without_waiting() {
    let harness = Harness::new();
    harness.with_world(|world| world.app_reachable = false);

    let outcome = harness.outcome();

    harness.assert_committed(&outcome);
    assert_eq!(harness.journal().forced_stops, [harness.prior()]);
    assert_eq!(lock(&harness.world).waited_out_deadlines, 0);
}

#[test]
fn a_candidate_forced_down_on_rollback_is_journaled_beside_the_prior() {
    let harness = Harness::new();
    fail_the_candidate(&harness);
    harness.with_world(|world| world.app_reachable = false);

    let outcome = harness.outcome();

    assert!(is_rolled_back(&outcome), "{outcome:?}");
    assert_eq!(
        harness.journal().forced_stops,
        [harness.prior(), harness.candidate()]
    );
}

#[test]
fn no_session_at_the_candidate_start_waits_for_the_owner() {
    let harness = Harness::new();
    harness.with_world(|world| world.session = false);
    let mut platform = harness.platform();
    let request = harness.request(InstallTargetPolicy::Preserve);

    let result = harness.fixture.with_lock(|lock| {
        InstallCoordinator::new(&harness.fixture.store, &mut platform)
            .install_with_lock(request, lock)
    });

    assert!(result.is_err(), "{result:?}");
    assert!(platform.waiting_for_owner());
    let journal = harness.journal();
    assert_eq!(
        journal.next_action,
        Some(InstallAction::RestorePriorRuntime)
    );
    assert_eq!(
        harness.fixture.active_unit().as_ref(),
        Some(&harness.prior())
    );
    harness.assert_names(&harness.prior());
    assert!(lock(&harness.world).apps.is_empty());

    harness.with_world(|world| world.session = true);
    let outcome = harness.converge();
    assert!(is_rolled_back(&outcome), "{outcome:?}");
    harness.assert_restored();
}

#[test]
fn a_broker_that_was_stopped_stays_stopped() {
    let harness = Harness::new();
    harness.with_world(|world| {
        if let Some(broker) = &mut world.broker {
            broker.process = None;
        }
    });

    let outcome = harness.install().expect("no crash").expect("transaction");

    assert!(matches!(outcome, InstallOutcome::Committed { .. }));
    harness.assert_names(&harness.candidate());
    assert_eq!(harness.effects("broker-start:before"), 0);
    assert!(
        lock(&harness.world)
            .broker
            .as_ref()
            .is_some_and(|broker| broker.process.is_none())
    );
}

#[test]
fn what_a_machine_does_not_have_is_left_alone() {
    let harness = Harness::new();
    harness.with_world(|world| {
        world.bootstrap = None;
        world.broker = None;
        world.firewall = None;
    });

    let outcome = harness.outcome();

    assert!(matches!(outcome, InstallOutcome::Committed { .. }));
    harness.assert_runs(&harness.candidate());
    assert_eq!(harness.effects("bootstrap-write:before"), 0);
    assert_eq!(harness.effects("broker-stop:before"), 0);
    assert_eq!(harness.effects("firewall:before"), 0);
}

#[test]
fn a_missing_rule_is_created_and_removed_again_on_rollback() {
    let harness = Harness::new();
    harness.with_world(|world| {
        if let Some(firewall) = &mut world.firewall {
            firewall.app = None;
        }
        world.unhealthy_version = Some(CANDIDATE_VERSION.to_owned());
    });

    let outcome = harness.outcome();

    assert!(is_rolled_back(&outcome), "{outcome:?}");
    let world = lock(&harness.world);
    let firewall = world.firewall.as_ref().expect("firewall");
    assert_eq!(firewall.app, None, "the rule that did not exist is gone");
    assert_eq!(
        firewall.daemon,
        Some(member_path(&harness.prior(), WINDOWS_DAEMON_PROGRAM))
    );
}

#[test]
fn a_firewall_refusal_rolls_the_finished_steps_back() {
    let harness = Harness::new();
    harness.with_world(|world| world.failing_rule = Some(WindowsFirewallRule::App));

    let outcome = harness.outcome();

    let InstallOutcome::RolledBack { failure, .. } = &outcome else {
        panic!("the transaction must not commit");
    };
    assert!(failure.contains("the firewall refused"), "{failure}");
    harness.assert_restored();
}

#[test]
fn a_first_install_creates_the_rules_and_a_rollback_removes_them() {
    let harness = Harness::new();
    harness.fixture.deactivate();
    harness.with_world(|world| {
        world.apps.clear();
        world.daemons.clear();
        world.bootstrap = None;
        world.broker = None;
        world.firewall = Some(WindowsFirewallObservation {
            daemon: None,
            app: None,
        });
    });

    let outcome = harness
        .install_with(InstallTargetPolicy::Disabled)
        .expect("no crash")
        .expect("transaction");

    assert!(matches!(outcome, InstallOutcome::Committed { .. }));
    let firewall = lock(&harness.world).firewall.clone().expect("firewall");
    assert_eq!(
        firewall.daemon,
        Some(member_path(&harness.candidate(), WINDOWS_DAEMON_PROGRAM))
    );

    let harness = Harness::new();
    harness.fixture.deactivate();
    harness.with_world(|world| {
        world.apps.clear();
        world.daemons.clear();
        world.bootstrap = None;
        world.broker = None;
        world.firewall = Some(WindowsFirewallObservation {
            daemon: None,
            app: None,
        });
        world.failing_rule = Some(WindowsFirewallRule::App);
    });

    let outcome = harness
        .install_with(InstallTargetPolicy::Disabled)
        .expect("no crash")
        .expect("transaction");

    assert!(is_rolled_back(&outcome), "{outcome:?}");
    assert_eq!(
        lock(&harness.world).firewall,
        Some(WindowsFirewallObservation {
            daemon: None,
            app: None,
        })
    );
    assert_eq!(harness.fixture.active_unit(), None);
}

/// What today's flat layout names, outside any unit.
const FLAT_ROOT: &str = r"C:\Program Files\Hypercolor";

fn flat_broker() -> String {
    format!(r#""{FLAT_ROOT}\tools\hypercolor-smbus-service.exe""#)
}

fn flat_rules() -> WindowsFirewallObservation {
    WindowsFirewallObservation {
        daemon: Some(format!(r"{FLAT_ROOT}\hypercolor-daemon.exe")),
        app: Some(format!(r"{FLAT_ROOT}\hypercolor-app.exe")),
    }
}

/// A flat installation as an installer finds it: no pointer and no unit in
/// use, the broker stopped for the copy, nothing running, and the broker and
/// firewall rules still naming the flat files.
fn flat_installation() -> Harness {
    let harness = Harness::new();
    harness.fixture.deactivate();
    harness.with_world(|world| {
        world.apps.clear();
        world.daemons.clear();
        world.bootstrap = None;
        world.broker = Some(Broker {
            image: flat_broker(),
            process: None,
        });
        world.firewall = Some(flat_rules());
    });
    harness
}

fn assert_adopted(harness: &Harness) {
    let candidate = harness.candidate();
    assert_eq!(harness.fixture.active_unit().as_ref(), Some(&candidate));
    let world = lock(&harness.world);
    assert_eq!(
        world.broker.as_ref().map(|broker| broker.image.clone()),
        Some(format!(
            "\"{}\"",
            member_path(&candidate, WINDOWS_BROKER_PROGRAM)
        ))
    );
    assert_eq!(
        world.firewall,
        Some(WindowsFirewallObservation {
            daemon: Some(member_path(&candidate, WINDOWS_DAEMON_PROGRAM)),
            app: Some(member_path(&candidate, WINDOWS_APP_PROGRAM)),
        })
    );
    assert!(world.apps.is_empty() && world.daemons.is_empty());
}

fn assert_still_flat(harness: &Harness) {
    assert_eq!(harness.fixture.active_unit(), None);
    let world = lock(&harness.world);
    assert_eq!(
        world.broker.as_ref().map(|broker| broker.image.clone()),
        Some(flat_broker())
    );
    assert_eq!(world.firewall, Some(flat_rules()));
}

#[test]
fn adopting_a_flat_install_moves_its_names_and_starts_nothing() {
    let harness = flat_installation();

    let outcome = harness
        .install_with(InstallTargetPolicy::Disabled)
        .expect("no crash")
        .expect("transaction");

    assert!(matches!(outcome, InstallOutcome::Committed { .. }));
    assert_adopted(&harness);
}

#[test]
fn an_installer_killed_at_every_boundary_of_an_adoption_is_finished_by_the_next() {
    let clean = flat_installation();
    clean
        .install_with(InstallTargetPolicy::Disabled)
        .expect("no crash")
        .expect("transaction");
    let boundaries = clean.passed();
    assert!(boundaries > 4, "only {boundaries} boundaries");

    for boundary in 1..=boundaries {
        let harness = flat_installation();
        harness.crash_at(Some(boundary));
        harness
            .install_with(InstallTargetPolicy::Disabled)
            .expect_err("the crash this test armed");
        harness.crash_at(None);

        // The next installer drives what it finds to an end, then starts
        // over if that end was not its own unit.
        if harness.fixture.journal().is_some() {
            harness.converge();
        }
        if harness.fixture.active_unit().as_ref() != Some(&harness.candidate()) {
            harness
                .install_with(InstallTargetPolicy::Disabled)
                .expect("no crash")
                .expect("transaction");
        }

        assert_adopted(&harness);
    }
}

#[test]
fn an_adoption_that_fails_puts_every_flat_name_back() {
    let harness = flat_installation();
    harness.with_world(|world| world.failing_rule = Some(WindowsFirewallRule::App));

    let outcome = harness
        .install_with(InstallTargetPolicy::Disabled)
        .expect("no crash")
        .expect("transaction");

    assert!(is_rolled_back(&outcome), "{outcome:?}");
    assert_still_flat(&harness);
}

#[test]
fn an_adoption_killed_in_its_rollback_still_ends_flat() {
    let probe = flat_installation();
    probe.with_world(|world| world.failing_rule = Some(WindowsFirewallRule::App));
    probe
        .install_with(InstallTargetPolicy::Disabled)
        .expect("no crash")
        .expect("transaction");
    let boundaries = probe.passed();

    for boundary in 1..=boundaries {
        let harness = flat_installation();
        harness.with_world(|world| world.failing_rule = Some(WindowsFirewallRule::App));
        harness.crash_at(Some(boundary));
        harness
            .install_with(InstallTargetPolicy::Disabled)
            .expect_err("the crash this test armed");
        harness.crash_at(None);
        if harness.fixture.journal().is_some() {
            harness.with_world(|world| world.failing_rule = None);
            let outcome = harness.converge();
            if matches!(outcome, InstallOutcome::Committed { .. }) {
                assert_adopted(&harness);
                continue;
            }
        }
        assert_still_flat(&harness);
    }
}

/// Prepare under `arrange` and return why the platform refused.
fn refusal(arrange: impl FnOnce(&Harness), policy: InstallTargetPolicy) -> String {
    let harness = Harness::new();
    arrange(&harness);
    let failure = harness
        .install_with(policy)
        .expect("no crash")
        .expect_err("the platform must refuse");
    assert!(
        matches!(failure, InstallCoordinatorError::PreparePlatform(_)),
        "{failure}"
    );
    assert!(
        harness.fixture.journal().is_none(),
        "a refused transaction writes nothing"
    );
    failure.to_string()
}

#[test]
fn preparation_refuses_a_runtime_it_cannot_name() {
    let foreign = |harness: &Harness| {
        harness.with_world(|world| {
            world.daemons[0].image = r"C:\Users\owner\target\hypercolor-daemon.exe".to_owned();
        });
    };
    assert!(refusal(foreign, InstallTargetPolicy::Preserve).contains("exactly the active unit"));

    let alone = |harness: &Harness| harness.with_world(|world| world.daemons.clear());
    assert!(refusal(alone, InstallTargetPolicy::Preserve).contains("exactly the active unit"));

    let two = |harness: &Harness| {
        let prior = harness.prior();
        harness.with_world(|world| world.launch(&prior));
    };
    assert!(refusal(two, InstallTargetPolicy::Preserve).contains("exactly the active unit"));
}

#[test]
fn preparation_refuses_a_broker_it_could_not_restore() {
    let elsewhere = |harness: &Harness| {
        harness.with_world(|world| {
            if let Some(broker) = &mut world.broker {
                broker.image =
                    r#""C:\Program Files\Hypercolor\tools\hypercolor-smbus-service.exe""#
                        .to_owned();
            }
        });
    };
    assert!(refusal(elsewhere, InstallTargetPolicy::Preserve).contains("adopt the installation"));

    let arguments = |harness: &Harness| {
        harness.with_world(|world| {
            if let Some(broker) = &mut world.broker {
                broker.image.push_str(" --flag");
            }
        });
    };
    assert!(refusal(arguments, InstallTargetPolicy::Preserve).contains("is not one program"));

    let unstarted = |harness: &Harness| {
        harness.with_world(|world| {
            world.apps.clear();
            world.daemons.clear();
        });
    };
    assert!(refusal(unstarted, InstallTargetPolicy::Preserve).contains("nothing to start"));
}

#[test]
fn preparation_refuses_to_turn_autostart_on() {
    let failure = refusal(|_| {}, InstallTargetPolicy::EnabledAndRunning);
    assert!(failure.contains("autostart"), "{failure}");
}

#[test]
fn preparation_refuses_the_unit_that_is_already_active() {
    let harness = Harness::new();
    let mut platform = harness.platform();
    let request = InstallRequest {
        candidate: harness.fixture.prior.clone(),
        ..harness.request(InstallTargetPolicy::Preserve)
    };
    let failure = harness
        .fixture
        .with_lock(|lock| {
            InstallCoordinator::new(&harness.fixture.store, &mut platform)
                .install_with_lock(request, lock)
        })
        .expect_err("the active unit is no candidate");
    assert!(
        failure.to_string().contains("already the active"),
        "{failure}"
    );
    assert!(harness.fixture.journal().is_none());
}

#[test]
fn a_first_install_must_name_its_launcher() {
    let fresh = |harness: &Harness| {
        harness.fixture.deactivate();
        harness.with_world(|world| {
            world.apps.clear();
            world.daemons.clear();
            world.broker = None;
        });
    };
    let failure = refusal(fresh, InstallTargetPolicy::Preserve);
    assert!(failure.contains("launcher"), "{failure}");
}

#[test]
fn a_process_from_neither_side_is_never_ended() {
    // The first stop is the prior's and the second the failed candidate's.
    let second_stop = boundary_of(fail_the_candidate, "quit:before", 2);
    let harness = Harness::new();
    fail_the_candidate(&harness);
    harness.crash_at(Some(second_stop));
    assert!(matches!(harness.install(), Err(Crash)));
    harness.crash_at(None);
    assert_eq!(
        harness.journal().next_action,
        Some(InstallAction::UnloadCandidateRuntime)
    );
    harness.with_world(|world| {
        world.daemons[0].image = r"C:\Users\owner\target\hypercolor-daemon.exe".to_owned();
    });

    let (result, _) = harness.recover();

    let failure = result
        .expect("no crash armed")
        .expect_err("a stranger's process stops the transaction");
    assert!(failure.to_string().contains("neither side"), "{failure}");
    assert_eq!(lock(&harness.world).daemons.len(), 1);
    assert_eq!(harness.effects("terminate:before"), 0);
}

#[test]
fn a_journal_cannot_name_a_forced_stop_outside_its_transaction() {
    let harness = Harness::new();
    harness.outcome();
    let mut journal = harness.journal();
    journal.forced_stops = vec![UnitId::new("c".repeat(64)).expect("unit")];
    assert_eq!(
        journal.validate(),
        Err(InstallModelError::InvalidForcedStop)
    );
    let prior = harness.prior();
    journal.forced_stops = vec![prior.clone(), prior];
    assert_eq!(
        journal.validate(),
        Err(InstallModelError::InvalidForcedStop)
    );
}

#[test]
fn a_journal_with_no_forced_stop_keeps_its_serialized_form() {
    let harness = Harness::new();
    harness.outcome();
    let text = serde_json::to_string(&harness.journal()).expect("encode the journal");
    assert!(!text.contains("forced_stops"), "{text}");
}

#[test]
fn a_record_of_another_platform_is_refused() {
    let harness = Harness::new();
    let mut platform = harness.platform();
    let record = crate::PlatformTransactionRecord::linux(1, b"{}".to_vec()).expect("record");
    let state = crate::PlatformState {
        layout_unit: None,
        launcher_unit: None,
        loaded: false,
        running_unit: None,
        autostart_enabled: false,
    };
    let failure = crate::InstallPlatform::reload_manager(&mut platform, &state, &record)
        .expect_err("foreign record");
    assert!(
        failure.to_string().contains("another platform"),
        "{failure}"
    );
}
