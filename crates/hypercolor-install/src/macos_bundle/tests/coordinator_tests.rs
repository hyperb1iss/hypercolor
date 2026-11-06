//! Whole transactions through the shared coordinator against the in-memory
//! installation, including a crash at every effect boundary.

use std::sync::{Arc, Mutex};

use super::fakes::{
    Boundaries, Crash, FakeExecutor, FakeService, SharedBoundaries, SharedWorld, World, lock,
    survive,
};
use super::fixture::{Fixture, UnitTree};
use crate::macos_bundle::{MacosBundleIdentity, MacosBundleInstallPlatform, MacosBundleSlot};
use crate::{
    InstallCoordinator, InstallCoordinatorError, InstallDisposition, InstallOutcome,
    InstallRequest, InstallTargetPolicy, InstallTransactionId, UnitRecord,
};

type Platform = MacosBundleInstallPlatform<FakeExecutor, FakeService>;
type Arrange = fn(&Harness);

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
            "materialize:before",
            "materialize:partial",
            "materialize:after",
            "exchange:before",
            "exchange:after",
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
            "materialize:before",
            "materialize:partial",
            "materialize:after",
            "exchange:before",
            "exchange:after",
            "start:before",
            "start:after",
            "prove",
            // The candidate fails its proof: stop it, swap back and retire
            // its copy in one rename, then restart the prior.
            "stop:before",
            "stop:after",
            "exchange:before",
            "exchange:after",
            "retire:before",
            "retire:after",
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
fn an_autostarted_candidate_after_a_power_cut_is_stopped_and_the_swap_counts_as_done() {
    let harness = Harness::new();
    let at = boundary_of("exchange:after");
    harness.crash_at(Some(at));
    assert!(harness.install(InstallTargetPolicy::Preserve).is_err());
    lock(&harness.world).autostart();
    harness.crash_at(None);
    let outcome = harness.finish(|| harness.recover().map(|result| result.expect("recover")));
    harness.assert_committed(&outcome);
    assert_eq!(harness.effects("exchange:after"), 1);
    assert_eq!(
        harness.effects("stop:after"),
        2,
        "the prior, then the autostarted candidate"
    );
}

#[test]
fn a_prior_restarted_before_its_unload_was_journaled_abandons_with_no_effect() {
    let harness = Harness::new();
    let at = boundary_of("stop:after");
    harness.crash_at(Some(at));
    assert!(harness.install(InstallTargetPolicy::Preserve).is_err());
    lock(&harness.world).autostart();
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
    let cases: [(&str, Arrange); 5] = [
        ("installed bundle is not the active unit's", |harness| {
            lock(&harness.world).installed = MacosBundleSlot::Bundle(harness.candidate.clone());
        }),
        ("earlier transaction left a staged copy", |harness| {
            lock(&harness.world).staged.insert(
                "e".repeat(64),
                MacosBundleSlot::Bundle(harness.prior.clone()),
            );
        }),
        ("earlier transaction left a leftover", |harness| {
            lock(&harness.world)
                .interrupted
                .insert(format!("{}.discard", "e".repeat(64)));
        }),
        ("not enough room", |harness| {
            lock(&harness.world).available_bytes = 1024;
        }),
        ("a registered agent with no active unit", |harness| {
            harness.fixture.deactivate();
            let mut world = lock(&harness.world);
            world.installed = MacosBundleSlot::Absent;
            world.running = None;
        }),
    ];
    for (case, arrange) in cases {
        let harness = Harness::new();
        arrange(&harness);
        let before = lock(&harness.world).clone();
        let error = harness
            .install(InstallTargetPolicy::Preserve)
            .expect("no crash")
            .expect_err(case);
        assert!(
            matches!(error, InstallCoordinatorError::PreparePlatform(_)),
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
