//! The launchd service's decisions against a scripted agent, alone and
//! through whole transactions with a crash at every effect boundary.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::coordinator_tests::identity_of;
use super::fakes::{
    Boundaries, Crash, FakeExecutor, SharedBoundaries, SharedWorld, World, boundary, lock, survive,
};
use super::fixture::{Fixture, PROBATION};
use crate::macos_bundle::{
    MacosBundleAgent, MacosBundleAgentJob, MacosBundleConfig, MacosBundleIdentity,
    MacosBundleInstallPlatform, MacosBundleLaunchdService, MacosBundleRunningDaemon,
    MacosBundleService, MacosBundleSlot,
};
use crate::{
    InstallCoordinator, InstallOutcome, InstallPlatformError, InstallRequest, InstallTargetPolicy,
    InstallTransactionId,
};

const VERSION: &str = "0.9.1";

fn fail(detail: &str) -> InstallPlatformError {
    InstallPlatformError::new(detail)
}

/// What the agent does beyond the shared world, and what it was asked.
#[derive(Debug, Default)]
struct Script {
    /// The next answers of the local API; once empty, it answers as `version`.
    answers: Vec<Result<(), &'static str>>,
    /// The next exit waits report an exit.
    exits: Vec<bool>,
    /// The daemon never publishes its ownership.
    silent: bool,
    /// The guard stays held.
    guard_held: bool,
    /// launchd restarts a crashed job on the next read of it.
    restarts_on_read: bool,
    /// Any daemon with this cdhash exits during a window this long.
    exits_in_window: Option<(String, Duration)>,
    /// The next job reads show a process this many reads later.
    replaced_after_reads: Option<usize>,
    calls: Vec<String>,
}

type SharedScript = Arc<Mutex<Script>>;

struct FakeAgent {
    world: SharedWorld,
    script: SharedScript,
    boundaries: SharedBoundaries,
}

impl MacosBundleAgent for FakeAgent {
    fn registered(&mut self) -> Result<bool, InstallPlatformError> {
        Ok(lock(&self.world).registered)
    }

    fn autostart_enabled(&mut self) -> Result<bool, InstallPlatformError> {
        Ok(lock(&self.world).enabled)
    }

    fn set_autostart(&mut self, enabled: bool) -> Result<(), InstallPlatformError> {
        boundary(&self.boundaries, "autostart:before");
        lock(&self.world).enabled = enabled;
        boundary(&self.boundaries, "autostart:after");
        Ok(())
    }

    fn job(&mut self) -> Result<MacosBundleAgentJob, InstallPlatformError> {
        let mut world = lock(&self.world);
        let mut script = lock(&self.script);
        if let Some(reads) = script.replaced_after_reads.as_mut() {
            if *reads == 0 {
                script.replaced_after_reads = None;
                world.relaunch();
            } else {
                *reads -= 1;
            }
        }
        if world.idle && script.restarts_on_read {
            script.restarts_on_read = false;
            world.idle = false;
            world.relaunch();
        }
        Ok(match (&world.running, world.idle) {
            (Some(running), _) => MacosBundleAgentJob::Running { pid: running.pid },
            (None, true) => MacosBundleAgentJob::Idle,
            (None, false) => MacosBundleAgentJob::NotLoaded,
        })
    }

    fn process_image(
        &mut self,
        pid: u32,
    ) -> Result<Option<(PathBuf, String)>, InstallPlatformError> {
        Ok(lock(&self.world)
            .running
            .as_ref()
            .filter(|running| running.pid == pid)
            .map(|running| (running.executable.clone(), running.cdhash.clone())))
    }

    fn start(
        &mut self,
        executable: &Path,
        cdhash: &str,
        _timeout: Duration,
    ) -> Result<(), InstallPlatformError> {
        boundary(&self.boundaries, "start:before");
        {
            let mut world = lock(&self.world);
            if !world.enabled {
                return Err(fail("launchd refuses to load a disabled agent"));
            }
            assert_eq!(executable, world.installed_daemon);
            world.relaunch();
            assert_eq!(
                world
                    .running
                    .as_ref()
                    .map(|running| running.cdhash.as_str()),
                Some(cdhash)
            );
        }
        boundary(&self.boundaries, "start:after");
        Ok(())
    }

    fn unload(&mut self) -> Result<(), InstallPlatformError> {
        lock(&self.script).calls.push("unload".to_owned());
        boundary(&self.boundaries, "unload:before");
        {
            let mut world = lock(&self.world);
            world.running = None;
            world.idle = false;
        }
        boundary(&self.boundaries, "unload:after");
        Ok(())
    }

    fn wait_for_guard_release(&mut self) -> Result<bool, InstallPlatformError> {
        Ok(!lock(&self.script).guard_held)
    }

    fn wait_for_publication(
        &mut self,
        _daemon: &MacosBundleRunningDaemon,
        _timeout: Duration,
    ) -> Result<bool, InstallPlatformError> {
        let mut script = lock(&self.script);
        script.calls.push("publication".to_owned());
        Ok(!script.silent)
    }

    fn prove_local_api(&mut self, _version: &str) -> Result<(), InstallPlatformError> {
        let mut script = lock(&self.script);
        script.calls.push("api".to_owned());
        if script.answers.is_empty() {
            Ok(())
        } else {
            script.answers.remove(0).map_err(fail)
        }
    }

    fn wait_for_exit(&mut self, pid: u32, timeout: Duration) -> Result<bool, InstallPlatformError> {
        boundary(&self.boundaries, "watch");
        let mut world = lock(&self.world);
        let mut script = lock(&self.script);
        script.calls.push(format!("watch:{}", timeout.as_millis()));
        let doomed = script
            .exits_in_window
            .as_ref()
            .is_some_and(|(cdhash, window)| {
                *window == timeout
                    && world
                        .running
                        .as_ref()
                        .is_some_and(|running| running.pid == pid && running.cdhash == *cdhash)
            });
        let scripted = !script.exits.is_empty() && script.exits.remove(0);
        if doomed || scripted {
            world.running = None;
            world.idle = true;
            return Ok(true);
        }
        Ok(false)
    }
}

/// A service over a world running one daemon, for the proof alone.
struct Proof {
    world: SharedWorld,
    script: SharedScript,
    service: MacosBundleLaunchdService<FakeAgent>,
    daemon: MacosBundleRunningDaemon,
}

impl Proof {
    fn new() -> Self {
        let config = MacosBundleConfig {
            installed_bundle: PathBuf::from("/Applications/Hypercolor.app"),
            release_root: PathBuf::from("/Users/player/releases"),
            probation: PROBATION,
        };
        let identity = MacosBundleIdentity {
            app: "a".repeat(40),
            daemon: "b".repeat(40),
            cli: "c".repeat(40),
        };
        let world = World::running(&identity, config.installed_daemon());
        let daemon = world.running.clone().expect("running daemon");
        let world = Arc::new(Mutex::new(world));
        let script = SharedScript::default();
        let service = MacosBundleLaunchdService::new(
            FakeAgent {
                world: Arc::clone(&world),
                script: Arc::clone(&script),
                boundaries: Arc::default(),
            },
            &config,
        )
        .with_start_deadline(Duration::from_secs(5));
        Self {
            world,
            script,
            service,
            daemon,
        }
    }

    fn prove(&mut self, window: Duration) -> Result<(), InstallPlatformError> {
        self.service.prove(&self.daemon, VERSION, window)
    }

    fn calls(&self) -> Vec<String> {
        lock(&self.script).calls.clone()
    }
}

#[test]
fn a_candidate_is_proven_on_both_sides_of_its_window() {
    let mut proof = Proof::new();
    proof.prove(PROBATION).expect("healthy candidate");
    assert_eq!(
        proof.calls(),
        ["publication", "api", "watch:90000", "publication", "api"],
        "the wait is one exit watch, never a loop of probes"
    );
}

#[test]
fn a_prior_is_proven_once() {
    let mut proof = Proof::new();
    proof.prove(Duration::ZERO).expect("restored prior");
    assert_eq!(proof.calls(), ["publication", "api"]);
}

#[test]
fn a_daemon_that_exits_inside_the_window_fails_at_once() {
    let mut proof = Proof::new();
    lock(&proof.script).exits = vec![true];
    let failure = proof.prove(PROBATION).expect_err("exit in probation");
    assert!(
        failure.to_string().contains("failed probation"),
        "{failure}"
    );
    assert_eq!(
        proof.calls(),
        ["publication", "api", "watch:90000"],
        "nothing is asked of a daemon that is gone"
    );
}

#[test]
fn a_hung_api_at_the_end_of_the_window_gets_no_second_try() {
    let mut proof = Proof::new();
    lock(&proof.script).answers = vec![Ok(()), Err("timed out")];
    let failure = proof.prove(PROBATION).expect_err("hung API");
    assert!(failure.to_string().contains("end of its 90 s"), "{failure}");
    assert_eq!(
        proof.calls(),
        ["publication", "api", "watch:90000", "publication", "api"]
    );
}

#[test]
fn a_restart_under_the_window_fails_even_when_the_new_daemon_is_healthy() {
    let mut proof = Proof::new();
    // The first proof reads the job four times; launchd replaces the process
    // while the window runs.
    lock(&proof.script).replaced_after_reads = Some(4);
    let failure = proof.prove(PROBATION).expect_err("restart in probation");
    assert!(failure.to_string().contains("changed"), "{failure}");
}

#[test]
fn a_daemon_still_coming_up_is_asked_again_until_it_answers() {
    let mut proof = Proof::new();
    lock(&proof.script).answers = vec![Err("connection refused"), Err("connection refused")];
    proof.prove(Duration::ZERO).expect("comes up");
    assert_eq!(
        proof.calls(),
        ["publication", "api", "watch:250", "api", "watch:250", "api"]
    );
}

#[test]
fn a_daemon_that_crashes_while_coming_up_fails_at_once() {
    let mut proof = Proof::new();
    {
        let mut script = lock(&proof.script);
        script.answers = vec![Err("connection refused")];
        script.exits = vec![true];
    }
    let failure = proof.prove(Duration::ZERO).expect_err("crash");
    assert!(failure.to_string().contains("exited before"), "{failure}");
    assert_eq!(proof.calls(), ["publication", "api", "watch:250"]);
}

#[test]
fn a_daemon_that_never_answers_fails_at_its_deadline() {
    let mut proof = Proof::new();
    proof.service = MacosBundleLaunchdService::new(
        FakeAgent {
            world: Arc::clone(&proof.world),
            script: Arc::clone(&proof.script),
            boundaries: Arc::default(),
        },
        &MacosBundleConfig {
            installed_bundle: PathBuf::from("/Applications/Hypercolor.app"),
            release_root: PathBuf::from("/Users/player/releases"),
            probation: PROBATION,
        },
    )
    .with_start_deadline(Duration::ZERO);
    lock(&proof.script).answers = vec![Err("connection refused")];
    let failure = proof.prove(Duration::ZERO).expect_err("deadline");
    assert!(failure.to_string().contains("refused"), "{failure}");
    assert_eq!(proof.calls(), ["publication", "api"]);
}

#[test]
fn a_daemon_that_never_claims_the_installation_fails_its_proof() {
    let mut proof = Proof::new();
    lock(&proof.script).silent = true;
    let failure = proof.prove(PROBATION).expect_err("no publication");
    assert!(failure.to_string().contains("never claimed"), "{failure}");
    assert_eq!(proof.calls(), ["publication"]);
}

#[test]
fn stop_unloads_only_what_it_names() {
    let mut proof = Proof::new();
    let replacement = MacosBundleRunningDaemon {
        pid: proof.daemon.pid + 1,
        ..proof.daemon.clone()
    };
    assert!(proof.service.stop(Some(&replacement)).is_err());
    assert!(proof.service.stop(None).is_err(), "a daemon still runs");
    assert!(proof.calls().is_empty(), "nothing was unloaded");

    let daemon = proof.daemon.clone();
    proof.service.stop(Some(&daemon)).expect("the named daemon");
    assert_eq!(proof.calls(), ["unload"]);

    // A crashed daemon's job has no process to name.
    lock(&proof.world).idle = true;
    proof.service.stop(None).expect("the idle job");
    assert_eq!(proof.calls(), ["unload", "unload"]);
    assert!(!lock(&proof.world).idle);
}

#[test]
fn observing_never_reports_a_process_launchd_replaced_during_the_read() {
    let mut proof = Proof::new();
    lock(&proof.script).replaced_after_reads = Some(1);
    let observed = proof.service.observe().expect("observe");
    let running = observed.running.expect("the replacement runs");
    assert_ne!(running.pid, proof.daemon.pid);
    assert_eq!(running.executable, proof.daemon.executable);
}

#[test]
fn an_idle_job_reports_no_daemon_and_a_held_guard_fails() {
    let mut proof = Proof::new();
    {
        let mut world = lock(&proof.world);
        world.running = None;
        world.idle = true;
    }
    assert_eq!(proof.service.observe().expect("observe").running, None);
    lock(&proof.script).guard_held = true;
    assert!(proof.service.wait_for_release().is_err());
}

/// Whole transactions with the real service, as a fresh process builds it.
struct Transaction {
    fixture: Fixture,
    world: SharedWorld,
    script: SharedScript,
    boundaries: SharedBoundaries,
    candidate: MacosBundleIdentity,
}

type Platform = MacosBundleInstallPlatform<FakeExecutor, MacosBundleLaunchdService<FakeAgent>>;

impl Transaction {
    fn new() -> Self {
        let fixture = Fixture::new();
        let prior = identity_of(&fixture.prior);
        let candidate = identity_of(&fixture.candidate);
        let world = World::running(&prior, fixture.config().installed_daemon());
        Self {
            fixture,
            world: Arc::new(Mutex::new(world)),
            script: SharedScript::default(),
            boundaries: Arc::new(Mutex::new(Boundaries::default())),
            candidate,
        }
    }

    fn platform(&self) -> Platform {
        let config = self.fixture.config();
        let service = MacosBundleLaunchdService::new(
            FakeAgent {
                world: Arc::clone(&self.world),
                script: Arc::clone(&self.script),
                boundaries: Arc::clone(&self.boundaries),
            },
            &config,
        );
        MacosBundleInstallPlatform::new(
            FakeExecutor {
                world: Arc::clone(&self.world),
                boundaries: Arc::clone(&self.boundaries),
            },
            service,
            config,
            [
                self.fixture.retain(self.fixture.prior.id()),
                self.fixture.retain(self.fixture.candidate.id()),
            ],
        )
        .expect("bundle platform")
    }

    fn crash_at(&self, boundary: Option<usize>) {
        lock(&self.boundaries).crash_at = boundary;
    }

    /// Install, then recover after every crash until the journal is terminal.
    fn run(&self) -> InstallOutcome {
        let mut platform = self.platform();
        let request = InstallRequest {
            transaction_id: InstallTransactionId::new("launchd-transaction").expect("id"),
            candidate: self.fixture.candidate.clone(),
            target_policy: InstallTargetPolicy::Preserve,
        };
        let mut outcome = survive(|| {
            self.fixture.with_lock(|lock| {
                InstallCoordinator::new(&self.fixture.store, &mut platform)
                    .install_with_lock(request, lock)
            })
        })
        .map(|result| Some(result.expect("install")));
        for _ in 0..16 {
            match outcome {
                Ok(Some(outcome)) => return outcome,
                Ok(None) => panic!("no journal to recover"),
                Err(Crash) => {
                    let mut platform = self.platform();
                    outcome = survive(|| {
                        self.fixture.with_lock(|lock| {
                            InstallCoordinator::new(&self.fixture.store, &mut platform)
                                .recover_with_lock(lock)
                        })
                    })
                    .map(|result| result.expect("recover"));
                }
            }
        }
        panic!("recovery never converged");
    }

    fn doom_candidate(&self) {
        lock(&self.script).exits_in_window = Some((self.candidate.daemon.clone(), PROBATION));
    }

    fn passed(&self) -> usize {
        lock(&self.boundaries).passed
    }

    fn assert_committed(&self, outcome: &InstallOutcome) {
        assert!(
            matches!(outcome, InstallOutcome::Committed { .. }),
            "{outcome:?}"
        );
        let world = lock(&self.world);
        assert_eq!(
            world.installed,
            MacosBundleSlot::Bundle(self.candidate.clone())
        );
        assert!(world.enabled);
        assert_eq!(
            world
                .running
                .as_ref()
                .map(|running| running.cdhash.as_str()),
            Some(self.candidate.daemon.as_str())
        );
    }

    fn assert_rolled_back(&self, outcome: &InstallOutcome) {
        let InstallOutcome::RolledBack {
            abandoned: false,
            failure,
            ..
        } = outcome
        else {
            panic!("{outcome:?}");
        };
        assert!(failure.contains("probation"), "{failure}");
        let world = lock(&self.world);
        let prior = identity_of(&self.fixture.prior);
        assert_eq!(world.installed, MacosBundleSlot::Bundle(prior.clone()));
        assert!(world.enabled, "the gate reopened with the prior");
        assert!(!world.idle, "launchd restarts nothing behind the rollback");
        assert_eq!(
            world
                .running
                .as_ref()
                .map(|running| running.cdhash.as_str()),
            Some(prior.daemon.as_str())
        );
    }
}

#[test]
fn a_commit_through_launchd_survives_a_crash_at_every_boundary() {
    let probe = Transaction::new();
    probe.assert_committed(&probe.run());
    let total = probe.passed();
    assert!(total >= 12, "every effect has boundaries: {total}");
    for crash in 1..=total {
        let transaction = Transaction::new();
        transaction.crash_at(Some(crash));
        let outcome = transaction.run();
        transaction.assert_committed(&outcome);
    }
}

#[test]
fn a_candidate_that_exits_in_probation_rolls_back_through_a_crash_at_every_boundary() {
    let probe = Transaction::new();
    probe.doom_candidate();
    probe.assert_rolled_back(&probe.run());
    let total = probe.passed();
    for crash in 1..=total {
        let transaction = Transaction::new();
        transaction.doom_candidate();
        transaction.crash_at(Some(crash));
        let outcome = transaction.run();
        transaction.assert_rolled_back(&outcome);
    }
}

#[test]
fn a_candidate_launchd_restarts_after_its_exit_is_stopped_on_the_way_back() {
    let transaction = Transaction::new();
    transaction.doom_candidate();
    lock(&transaction.script).restarts_on_read = true;
    let outcome = transaction.run();
    transaction.assert_rolled_back(&outcome);
}

#[test]
fn a_candidate_that_exits_in_probation_leaves_no_job_to_restart() {
    let transaction = Transaction::new();
    transaction.doom_candidate();
    let outcome = transaction.run();
    transaction.assert_rolled_back(&outcome);
    let calls = lock(&transaction.script).calls.clone();
    assert!(
        calls.iter().any(|call| call == "unload"),
        "the idle job is unloaded before the swap back: {calls:?}"
    );
}
