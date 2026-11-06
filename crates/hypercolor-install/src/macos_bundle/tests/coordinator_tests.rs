//! Whole transactions through the shared coordinator against the in-memory
//! installation, including a crash at every effect boundary.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::fakes::{
    Boundaries, Crash, FakeExecutor, FakeService, SharedBoundaries, SharedWorld, World, lock,
    survive,
};
use super::fixture::{Fixture, PROBATION, UnitTree};
use crate::macos_bundle::{MacosBundleIdentity, MacosBundleInstallPlatform, MacosBundleSlot};
use crate::{
    InstallCoordinator, InstallCoordinatorError, InstallDisposition, InstallOutcome,
    InstallRequest, InstallTargetPolicy, InstallTransactionId, UnitRecord,
};

type Platform = MacosBundleInstallPlatform<FakeExecutor, FakeService>;
type Arrange = (&'static str, &'static str, fn(&Harness));

pub(super) fn identity_of(unit: &UnitRecord) -> MacosBundleIdentity {
    crate::payload::bind_macos_bundle_unit(unit)
        .expect("bind unit")
        .identity()
        .clone()
}

struct Harness {
    fixture: Fixture,
    world: SharedWorld,
    boundaries: SharedBoundaries,
    prior: MacosBundleIdentity,
    candidate: MacosBundleIdentity,
}

impl Harness {
    fn new() -> Self {
        Self::from_fixture(Fixture::new())
    }

    fn from_fixture(fixture: Fixture) -> Self {
        let prior = identity_of(&fixture.prior);
        let candidate = identity_of(&fixture.candidate);
        let world = World::running(&prior, fixture.config().installed_daemon());
        Self {
            fixture,
            world: Arc::new(Mutex::new(world)),
            boundaries: Arc::new(Mutex::new(Boundaries::default())),
            prior,
            candidate,
        }
    }

    /// A first install: nothing active, installed or registered.
    fn fresh() -> Self {
        let harness = Self::new();
        harness.fixture.deactivate();
        {
            let mut world = lock(&harness.world);
            world.installed = MacosBundleSlot::Absent;
            world.registered = false;
            world.enabled = false;
            world.running = None;
        }
        harness
    }

    fn name(&self) -> &str {
        self.fixture.candidate.id().as_str()
    }

    /// A platform as a freshly started process builds it.
    fn platform(&self) -> Platform {
        MacosBundleInstallPlatform::new(
            FakeExecutor {
                world: Arc::clone(&self.world),
                boundaries: Arc::clone(&self.boundaries),
            },
            FakeService {
                world: Arc::clone(&self.world),
                boundaries: Arc::clone(&self.boundaries),
            },
            self.fixture.config(),
            [
                self.fixture.retain(self.fixture.prior.id()),
                self.fixture.retain(self.fixture.candidate.id()),
            ],
        )
        .expect("bundle platform")
    }

    fn request(&self, target_policy: InstallTargetPolicy) -> InstallRequest {
        InstallRequest {
            transaction_id: InstallTransactionId::new("bundle-transaction").expect("id"),
            candidate: self.fixture.candidate.clone(),
            target_policy,
        }
    }

    fn crash_at(&self, boundary: Option<usize>) {
        lock(&self.boundaries).crash_at = boundary;
    }

    fn install(
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

    fn recover(&self) -> Result<Result<Option<InstallOutcome>, InstallCoordinatorError>, Crash> {
        let mut platform = self.platform();
        survive(|| {
            self.fixture.with_lock(|lock| {
                InstallCoordinator::new(&self.fixture.store, &mut platform).recover_with_lock(lock)
            })
        })
    }

    /// Run transactions until the journal is terminal, recovering after
    /// every crash, then settle the staging root.
    fn finish(
        &self,
        first: impl FnOnce() -> Result<Option<InstallOutcome>, Crash>,
    ) -> InstallOutcome {
        let mut outcome = first();
        for _ in 0..64 {
            match outcome {
                Ok(Some(outcome)) => {
                    self.settle();
                    return outcome;
                }
                Ok(None) => panic!("no journal to recover"),
                Err(Crash) => {
                    outcome = self
                        .recover()
                        .map(|result| result.expect("recovery after a crash"));
                }
            }
        }
        panic!("recovery never converged");
    }

    /// Settle the terminal journal, settling again after a crash in it.
    fn settle(&self) -> bool {
        survive(|| self.fixture.settle(self.platform())).map_or_else(
            |Crash| {
                self.crash_at(None);
                self.fixture.settle(self.platform()).expect("settle again")
            },
            |settled| settled.expect("settle"),
        )
    }

    fn effects(&self, label: &str) -> usize {
        lock(&self.boundaries)
            .log
            .iter()
            .filter(|entry| *entry == label)
            .count()
    }

    fn assert_committed(&self, outcome: &InstallOutcome) {
        assert_eq!(
            outcome,
            &InstallOutcome::Committed {
                active_unit: self.fixture.candidate.id().clone()
            }
        );
        assert_eq!(
            self.fixture.active_unit().as_ref(),
            Some(self.fixture.candidate.id())
        );
        let world = lock(&self.world);
        assert_eq!(
            world.installed,
            MacosBundleSlot::Bundle(self.candidate.clone())
        );
        assert!(world.staged.is_empty(), "settled: {:?}", world.staged);
        assert!(world.interrupted.is_empty());
        assert_eq!(
            world
                .running
                .as_ref()
                .map(|running| running.cdhash.as_str()),
            Some(self.candidate.daemon.as_str())
        );
        assert!(world.enabled);
        assert!(!world.idle);
    }

    fn assert_rolled_back(&self, outcome: &InstallOutcome) {
        assert!(
            matches!(
                outcome,
                InstallOutcome::RolledBack {
                    abandoned: false,
                    ..
                }
            ),
            "{outcome:?}"
        );
        assert_eq!(
            self.fixture.active_unit().as_ref(),
            Some(self.fixture.prior.id())
        );
        let world = lock(&self.world);
        assert_eq!(world.installed, MacosBundleSlot::Bundle(self.prior.clone()));
        assert!(world.staged.is_empty(), "settled: {:?}", world.staged);
        assert!(world.interrupted.is_empty());
        let running = world.running.as_ref().expect("the prior runs again");
        assert_eq!(running.cdhash, self.prior.daemon);
        assert_ne!(running.pid, 100, "a new run of the prior");
        assert!(world.enabled, "the gate reopened with the prior");
    }
}

fn boundaries_of(run: impl FnOnce(&Harness) -> InstallOutcome) -> usize {
    let harness = Harness::new();
    run(&harness);
    lock(&harness.boundaries).passed
}

fn committed_run(harness: &Harness) -> InstallOutcome {
    harness.finish(|| {
        harness
            .install(InstallTargetPolicy::Preserve)
            .map(|result| Some(result.expect("install")))
    })
}

fn failing_run(harness: &Harness) -> InstallOutcome {
    lock(&harness.world).failing_proof = Some(harness.candidate.daemon.clone());
    committed_run(harness)
}

#[test]
fn a_commit_leaves_the_displaced_prior_staged_until_it_is_settled() {
    let harness = Harness::new();
    let outcome = harness
        .install(InstallTargetPolicy::Preserve)
        .expect("no crash")
        .expect("install");
    {
        let world = lock(&harness.world);
        assert_eq!(
            world.staged.get(harness.name()),
            Some(&MacosBundleSlot::Bundle(harness.prior.clone())),
            "the displaced prior waits at the staging name until settled"
        );
    }
    assert!(harness.settle());
    assert!(!harness.settle(), "settling twice removes nothing more");
    harness.assert_committed(&outcome);
}

#[test]
fn a_candidate_that_fails_its_proof_is_swapped_back_and_its_copy_removed() {
    let harness = Harness::new();
    let outcome = failing_run(&harness);
    harness.assert_rolled_back(&outcome);
    let InstallOutcome::RolledBack { restored, .. } = &outcome else {
        unreachable!("asserted above");
    };
    let restored = restored
        .as_ref()
        .expect("the rollback names the restored prior");
    assert_eq!(&restored.unit, harness.fixture.prior.id());
    assert_eq!(restored.version, "0.9.0");
    assert_eq!(
        harness.effects("exchange:after"),
        2,
        "swapped in, then back"
    );
}

#[test]
fn clean_runs_take_exactly_the_journaled_steps_in_order() {
    let harness = Harness::new();
    committed_run(&harness);
    assert_eq!(
        lock(&harness.boundaries).log,
        [
            "stop:before",
            "stop:after",
            // launchd may not load the agent across the swap.
            "disable:before",
            "disable:after",
            "materialize:before",
            "materialize:partial",
            "materialize:after",
            "exchange:before",
            "exchange:after",
            "enable:before",
            "enable:after",
            "start:before",
            "start:after",
            "prove",
            // Settling removes the displaced prior.
            "discard:before",
            "discard:renamed",
            "discard:after",
        ]
    );

    let harness = Harness::new();
    failing_run(&harness);
    assert_eq!(
        lock(&harness.boundaries).log,
        [
            "stop:before",
            "stop:after",
            "disable:before",
            "disable:after",
            "materialize:before",
            "materialize:partial",
            "materialize:after",
            "exchange:before",
            "exchange:after",
            "enable:before",
            "enable:after",
            "start:before",
            "start:after",
            "prove",
            // The candidate fails its proof: stop it, swap back with the
            // gate closed again and retire its copy in one rename, then
            // reopen the gate and restart the prior.
            "stop:before",
            "stop:after",
            "disable:before",
            "disable:after",
            "exchange:before",
            "exchange:after",
            "retire:before",
            "retire:after",
            "enable:before",
            "enable:after",
            "start:before",
            "start:after",
            "prove",
            // Settling removes the retired candidate.
            "discard:before",
            "discard:after",
        ]
    );
}

#[test]
fn a_crash_at_every_commit_boundary_recovers_to_the_candidate_without_redoing_the_swap() {
    let total = boundaries_of(committed_run);
    assert!(total >= 10, "every effect has boundaries: {total}");
    for crash in 1..=total {
        let harness = Harness::new();
        harness.crash_at(Some(crash));
        let outcome = committed_run(&harness);
        harness.assert_committed(&outcome);
        assert_eq!(
            harness.effects("exchange:after"),
            1,
            "crash at boundary {crash} must not swap twice"
        );
    }
}

#[test]
fn a_crash_at_every_rollback_boundary_recovers_to_the_prior() {
    let total = boundaries_of(failing_run);
    for crash in 1..=total {
        let harness = Harness::new();
        harness.crash_at(Some(crash));
        let outcome = failing_run(&harness);
        harness.assert_rolled_back(&outcome);
    }
}

#[test]
fn successive_crashes_at_every_next_boundary_still_converge() {
    for failing in [false, true] {
        let harness = Harness::new();
        if failing {
            lock(&harness.world).failing_proof = Some(harness.candidate.daemon.clone());
        }
        harness.crash_at(Some(2));
        let mut outcome = harness
            .install(InstallTargetPolicy::Preserve)
            .map(|result| Some(result.expect("install")));
        // Each recovery crashes one boundary later into its own run than
        // the last, so a step that restarts from scratch still finishes.
        for gap in 2..64 {
            if outcome.is_ok() {
                break;
            }
            let passed = lock(&harness.boundaries).passed;
            harness.crash_at(Some(passed + gap));
            outcome = harness.recover().map(|result| result.expect("recover"));
        }
        let outcome = outcome.expect("converged").expect("terminal");
        harness.settle();
        if failing {
            harness.assert_rolled_back(&outcome);
        } else {
            harness.assert_committed(&outcome);
        }
    }
}

#[test]
fn a_power_cut_between_the_swap_and_the_proof_starts_nothing_at_login() {
    let harness = Harness::new();
    let at = boundary_of("exchange:after");
    harness.crash_at(Some(at));
    assert!(harness.install(InstallTargetPolicy::Preserve).is_err());
    {
        let mut world = lock(&harness.world);
        world.login();
        assert!(world.running.is_none(), "the gate holds across a restart");
        assert!(!world.enabled);
    }
    harness.crash_at(None);
    let outcome = harness.finish(|| harness.recover().map(|result| result.expect("recover")));
    harness.assert_committed(&outcome);
    assert_eq!(harness.effects("exchange:after"), 1);
    assert_eq!(harness.effects("stop:after"), 1, "only the prior ever ran");
}

#[test]
fn a_power_cut_after_the_gate_reopened_stops_the_autostarted_candidate() {
    let harness = Harness::new();
    let at = boundary_of("enable:after");
    harness.crash_at(Some(at));
    assert!(harness.install(InstallTargetPolicy::Preserve).is_err());
    lock(&harness.world).login();
    harness.crash_at(None);
    let outcome = harness.finish(|| harness.recover().map(|result| result.expect("recover")));
    harness.assert_committed(&outcome);
    assert_eq!(harness.effects("exchange:after"), 1);
}

#[test]
fn a_power_cut_during_probation_rolls_back_the_candidate_that_comes_back() {
    let harness = Harness::new();
    let at = boundary_of("prove");
    harness.crash_at(Some(at));
    assert!(harness.install(InstallTargetPolicy::Preserve).is_err());
    {
        // The candidate started once and holds an open gate, so login runs
        // it again under a fresh process.
        let mut world = lock(&harness.world);
        world.running = None;
        world.login();
    }
    harness.crash_at(None);
    let outcome = harness.finish(|| harness.recover().map(|result| result.expect("recover")));
    harness.assert_rolled_back(&outcome);
}

#[test]
fn an_installer_killed_during_probation_holds_the_same_candidate_a_whole_window_again() {
    let harness = Harness::new();
    let at = boundary_of("prove");
    harness.crash_at(Some(at));
    assert!(harness.install(InstallTargetPolicy::Preserve).is_err());
    harness.crash_at(None);
    let outcome = harness.finish(|| harness.recover().map(|result| result.expect("recover")));
    harness.assert_committed(&outcome);
    assert_eq!(
        harness.effects("start:after"),
        1,
        "the candidate kept running"
    );
    assert_eq!(lock(&harness.world).probations, [PROBATION]);
}

#[test]
fn only_a_candidate_is_held_through_probation() {
    let harness = Harness::new();
    let outcome = failing_run(&harness);
    harness.assert_rolled_back(&outcome);
    assert_eq!(
        lock(&harness.world).probations,
        [PROBATION, Duration::ZERO],
        "the restored prior is proven once"
    );
}

#[test]
fn a_prior_whose_gate_was_closed_keeps_it_closed() {
    let harness = Harness::new();
    {
        let mut world = lock(&harness.world);
        world.enabled = false;
        world.running = None;
    }
    let outcome = harness.finish(|| {
        harness
            .install(InstallTargetPolicy::Preserve)
            .map(|result| Some(result.expect("install")))
    });
    assert_eq!(
        outcome,
        InstallOutcome::Committed {
            active_unit: harness.fixture.candidate.id().clone()
        }
    );
    let world = lock(&harness.world);
    assert!(!world.enabled, "an installation that does not autostart");
    assert!(world.running.is_none());
    assert!(world.probations.is_empty(), "nothing ran to prove");
}

#[test]
fn a_stopped_prior_gets_its_open_gate_back_at_every_crash_with_nothing_to_start() {
    let run = |harness: &Harness| {
        lock(&harness.world).running = None;
        committed_run(harness)
    };
    let probe = Harness::new();
    run(&probe);
    let total = lock(&probe.boundaries).passed;
    assert_eq!(probe.effects("disable:after"), 1, "held shut for the swap");
    for crash in 0..=total {
        let harness = Harness::new();
        harness.crash_at((crash > 0).then_some(crash));
        let outcome = run(&harness);
        assert!(matches!(outcome, InstallOutcome::Committed { .. }));
        let world = lock(&harness.world);
        assert!(world.enabled, "crash at {crash} must give the gate back");
        assert!(world.running.is_none());
        assert!(world.probations.is_empty(), "nothing ran to prove");
    }
}

#[test]
fn a_crashed_daemons_job_is_unloaded_before_the_swap_back() {
    let harness = Harness::new();
    lock(&harness.world).failing_proof = Some(harness.candidate.daemon.clone());
    let at = boundary_of("prove");
    harness.crash_at(Some(at));
    assert!(harness.install(InstallTargetPolicy::Preserve).is_err());
    {
        // The candidate crashed: launchd keeps its job loaded and idle.
        let mut world = lock(&harness.world);
        world.running = None;
        world.idle = true;
    }
    harness.crash_at(None);
    let outcome = harness.finish(|| harness.recover().map(|result| result.expect("recover")));
    harness.assert_rolled_back(&outcome);
    assert!(!lock(&harness.world).idle, "the job was unloaded");
}

#[test]
fn a_prior_restarted_before_its_unload_was_journaled_abandons_with_no_effect() {
    let harness = Harness::new();
    let at = boundary_of("stop:after");
    harness.crash_at(Some(at));
    assert!(harness.install(InstallTargetPolicy::Preserve).is_err());
    lock(&harness.world).login();
    harness.crash_at(None);
    let outcome = harness
        .recover()
        .expect("no crash")
        .expect("recover")
        .expect("journal");
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
    let world = lock(&harness.world);
    assert_eq!(
        world.installed,
        MacosBundleSlot::Bundle(harness.prior.clone())
    );
    assert!(world.staged.is_empty() && world.interrupted.is_empty());
    assert!(
        world.enabled,
        "an abandoned transaction never closed the gate"
    );
}

#[test]
fn a_bundle_replaced_behind_the_transaction_is_drift_not_a_second_swap() {
    let harness = Harness::new();
    let at = boundary_of("exchange:after");
    harness.crash_at(Some(at));
    assert!(harness.install(InstallTargetPolicy::Preserve).is_err());
    let foreign = MacosBundleIdentity {
        app: "f".repeat(40),
        daemon: "e".repeat(40),
        cli: "d".repeat(40),
    };
    lock(&harness.world).installed = MacosBundleSlot::Bundle(foreign.clone());
    harness.crash_at(None);
    let error = harness
        .recover()
        .expect("no crash")
        .expect_err("a foreign bundle is drift");
    assert!(
        matches!(error, InstallCoordinatorError::StateDrift { .. }),
        "{error}"
    );
    assert_eq!(harness.effects("exchange:after"), 1);
    assert_eq!(
        lock(&harness.world).installed,
        MacosBundleSlot::Bundle(foreign)
    );
    let journal = harness.fixture.journal().expect("journal");
    assert_eq!(journal.disposition, InstallDisposition::Forward);
    assert!(
        harness.fixture.settle(harness.platform()).is_err(),
        "nonterminal"
    );
}

#[test]
fn a_first_install_moves_the_staged_bundle_into_the_empty_slot() {
    let harness = Harness::fresh();
    let outcome = harness
        .install(InstallTargetPolicy::Preserve)
        .expect("no crash")
        .expect("install");
    assert_eq!(
        outcome,
        InstallOutcome::Committed {
            active_unit: harness.fixture.candidate.id().clone()
        }
    );
    assert_eq!(harness.effects("install:after"), 1);
    assert_eq!(harness.effects("exchange:before"), 0);
    assert!(harness.settle(), "the empty staging name is removed");
    let world = lock(&harness.world);
    assert_eq!(
        world.installed,
        MacosBundleSlot::Bundle(harness.candidate.clone())
    );
    assert!(world.staged.is_empty());
    assert!(world.running.is_none());
}

#[test]
fn preparation_refuses_what_it_cannot_swap_and_writes_nothing() {
    let cases: [Arrange; 6] = [
        (
            "installed bundle is not the active unit's",
            "adopt it",
            |harness| {
                lock(&harness.world).installed = MacosBundleSlot::Bundle(harness.candidate.clone());
            },
        ),
        (
            "earlier transaction left a staged copy",
            "settle it first",
            |harness| {
                lock(&harness.world).staged.insert(
                    "e".repeat(64),
                    MacosBundleSlot::Bundle(harness.prior.clone()),
                );
            },
        ),
        (
            "earlier transaction left a leftover",
            "settle it first",
            |harness| {
                lock(&harness.world)
                    .interrupted
                    .insert(format!("{}.discard", "e".repeat(64)));
            },
        ),
        (
            "an unsettled rollback of this candidate",
            "settle it first",
            |harness| {
                let name = harness.fixture.candidate.id().as_str().to_owned();
                lock(&harness.world)
                    .interrupted
                    .insert(format!("{name}.discard"));
            },
        ),
        ("not enough room", "insufficient disk", |harness| {
            lock(&harness.world).available_bytes = 1024;
        }),
        (
            "a registered agent with no active unit",
            "no unit is active",
            |harness| {
                harness.fixture.deactivate();
                let mut world = lock(&harness.world);
                world.installed = MacosBundleSlot::Absent;
                world.running = None;
            },
        ),
    ];
    for (case, detail, arrange) in cases {
        let harness = Harness::new();
        arrange(&harness);
        let before = lock(&harness.world).clone();
        let error = harness
            .install(InstallTargetPolicy::Preserve)
            .expect("no crash")
            .expect_err(case);
        assert!(
            matches!(error, InstallCoordinatorError::PreparePlatform(_))
                && error.to_string().contains(detail),
            "{case}: {error}"
        );
        assert!(harness.fixture.journal().is_none(), "{case}");
        assert_eq!(*lock(&harness.world), before, "{case}");
        assert_eq!(lock(&harness.boundaries).passed, 0, "{case}");
    }
}

#[test]
fn preparation_refuses_daemons_it_could_not_tell_apart_and_agent_registration() {
    let harness = Harness::from_fixture(Fixture::with_candidate(UnitTree::new(
        "candidate",
        "prior",
        "0.9.1",
    )));
    let error = harness
        .install(InstallTargetPolicy::Preserve)
        .expect("no crash")
        .expect_err("same daemon cdhash");
    assert!(error.to_string().contains("cdhash"), "{error}");

    let harness = Harness::fresh();
    let error = harness
        .install(InstallTargetPolicy::EnabledAndRunning)
        .expect("no crash")
        .expect_err("registering the agent");
    assert!(error.to_string().contains("installer"), "{error}");

    let harness = Harness::new();
    let error = harness
        .install(InstallTargetPolicy::Disabled)
        .expect("no crash")
        .expect_err("disabling the agent");
    assert!(error.to_string().contains("installer"), "{error}");
}

#[test]
fn room_that_vanishes_after_preparation_rolls_back_before_the_prior_stops() {
    let harness = Harness::new();
    let mut platform = harness.platform();
    harness.fixture.with_lock(|lock| {
        let journal = InstallCoordinator::new(&harness.fixture.store, &mut platform)
            .prepare_with_lock(harness.request(InstallTargetPolicy::Preserve), lock)
            .expect("prepare");
        harness
            .fixture
            .store
            .write_journal(&journal, lock)
            .expect("write journal");
    });
    lock(&harness.world).available_bytes = 1024;
    let outcome = harness
        .recover()
        .expect("no crash")
        .expect("recover")
        .expect("journal");
    let InstallOutcome::RolledBack { failure, .. } = &outcome else {
        panic!("{outcome:?}");
    };
    assert!(failure.contains("insufficient disk"), "{failure}");
    assert_eq!(lock(&harness.boundaries).passed, 0, "nothing was touched");
    assert_eq!(
        lock(&harness.world)
            .running
            .as_ref()
            .map(|running| running.pid),
        Some(100)
    );
}

/// The boundary number of the first `label` in a clean commit.
fn boundary_of(label: &str) -> usize {
    let probe = Harness::new();
    committed_run(&probe);
    let log = lock(&probe.boundaries).log.clone();
    log.iter()
        .position(|entry| entry == label)
        .map(|index| index + 1)
        .unwrap_or_else(|| panic!("{label} never happens"))
}
