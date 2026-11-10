use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use sha2::{Digest as _, Sha256};

use super::{
    DeadlineDirectLaunchdInspector, ExitWatch, LaunchctlAction, LaunchctlCommandBoundary,
    MacosDirectLaunchdBootstrapExpectation, MacosDirectLaunchdBootstrapSource,
    MacosDirectLaunchdMutationOutcome, MutationController, SubmittedCommand,
};
use crate::{
    MACOS_DIRECT_LAUNCHD_LABEL, MACOS_OWNER_RECORD_SCHEMA_VERSION, MacosDaemonOwner,
    MacosDirectLaunchdExecutableExpectation, MacosDirectLaunchdInspector,
    MacosDirectLaunchdPublicationExpectation, MacosDirectLaunchdState, MacosOwnerExecutionError,
    MacosOwnerIdentity, MacosOwnerRecord, MacosOwnerStore,
};

const DAEMON_PATH: &str = "/private/unit/bin/hypercolor-daemon";
const REQUIREMENT: &str = "designated-requirement";
const SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const CDHASH: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

#[derive(Debug)]
struct SharedInspector {
    state: MacosDirectLaunchdState,
    live_matches: bool,
    publication_matches: bool,
    exit: FakeExit,
    trace: Vec<Trace>,
    daemon: Option<FakeDaemonExit>,
}

/// launchd's exit timeout for these agents, as measured on macOS 15 and 26.
const LAUNCHD_EXIT_TIMEOUT: Duration = Duration::from_secs(5);

/// A daemon whose shutdown ends by clearing its session record, which takes
/// the store's coordination lock.
#[derive(Debug)]
struct FakeDaemonExit {
    store: MacosOwnerStore,
    /// How long its exit took, once it took place; `None` when launchd had
    /// to kill it at its exit timeout.
    clean_exit: Option<Duration>,
    shutdown: Option<std::thread::JoinHandle<()>>,
}

/// What the job's process does once something waits on its exit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FakeExit {
    /// It exits, and launchd's view of the job stays as it stands.
    Exits,
    /// It exits, and launchd drops the label: what launchd does when the
    /// process of a booted-out job ends.
    ExitsAndLaunchdDropsTheJob,
    /// It outlives every deadline.
    Never,
    /// It shuts down, clears its session record under the coordination
    /// lock, then exits; launchd kills it at its exit timeout if that lock
    /// never comes. launchd drops the label either way.
    ClearsItsSessionThenExits,
}

/// The order the engine armed watches and submitted commands in.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Trace {
    Watch(u32),
    Command(LaunchctlAction),
    ExitWaited,
}

#[derive(Debug, Clone)]
struct FakeInspector(Rc<RefCell<SharedInspector>>);

#[derive(Debug)]
struct FakeExitWatch(Rc<RefCell<SharedInspector>>);

impl ExitWatch for FakeExitWatch {
    fn exited_by(&mut self, deadline: Instant) -> Result<bool, MacosOwnerExecutionError> {
        require_time(deadline)?;
        let mut shared = self.0.borrow_mut();
        shared.trace.push(Trace::ExitWaited);
        Ok(match shared.exit {
            FakeExit::Exits => true,
            FakeExit::ExitsAndLaunchdDropsTheJob => {
                shared.state = MacosDirectLaunchdState::NotLoaded;
                true
            }
            FakeExit::Never => false,
            FakeExit::ClearsItsSessionThenExits => {
                let daemon = shared
                    .daemon
                    .as_mut()
                    .expect("a daemon exit needs its store");
                let store = daemon.store.clone();
                let (exited_tx, exited_rx) = std::sync::mpsc::sync_channel(1);
                let started = Instant::now();
                daemon.shutdown = Some(std::thread::spawn(move || {
                    // Takes the coordination lock exactly as the daemon's
                    // session clear does.
                    let _ = store.load_daemon_session_attestation();
                    let _ = exited_tx.send(started.elapsed());
                }));
                daemon.clean_exit = exited_rx.recv_timeout(LAUNCHD_EXIT_TIMEOUT).ok();
                shared.state = MacosDirectLaunchdState::NotLoaded;
                true
            }
        })
    }
}

impl MacosDirectLaunchdInspector for FakeInspector {
    fn inspect_direct_launchd(
        &mut self,
    ) -> Result<MacosDirectLaunchdState, MacosOwnerExecutionError> {
        Ok(self.0.borrow().state)
    }

    fn live_identity_matches(
        &mut self,
        _identity: &MacosOwnerIdentity,
    ) -> Result<bool, MacosOwnerExecutionError> {
        Ok(self.0.borrow().live_matches)
    }

    fn publication_identity_matches(
        &mut self,
        _identity: &MacosOwnerIdentity,
        _executable: &MacosDirectLaunchdExecutableExpectation,
    ) -> Result<bool, MacosOwnerExecutionError> {
        Ok(self.0.borrow().publication_matches)
    }
}

impl DeadlineDirectLaunchdInspector for FakeInspector {
    type ExitWatch = FakeExitWatch;

    fn watch_exit(&mut self, pid: u32) -> Result<FakeExitWatch, MacosOwnerExecutionError> {
        self.0.borrow_mut().trace.push(Trace::Watch(pid));
        Ok(FakeExitWatch(Rc::clone(&self.0)))
    }

    fn inspect_direct_launchd_until(
        &mut self,
        deadline: Instant,
    ) -> Result<MacosDirectLaunchdState, MacosOwnerExecutionError> {
        require_time(deadline)?;
        MacosDirectLaunchdInspector::inspect_direct_launchd(self)
    }

    fn live_identity_matches_until(
        &mut self,
        identity: &MacosOwnerIdentity,
        deadline: Instant,
    ) -> Result<bool, MacosOwnerExecutionError> {
        require_time(deadline)?;
        MacosDirectLaunchdInspector::live_identity_matches(self, identity)
    }

    fn publication_identity_matches_until(
        &mut self,
        identity: &MacosOwnerIdentity,
        executable: &MacosDirectLaunchdExecutableExpectation,
        deadline: Instant,
    ) -> Result<bool, MacosOwnerExecutionError> {
        require_time(deadline)?;
        MacosDirectLaunchdInspector::publication_identity_matches(self, identity, executable)
    }
}

fn require_time(deadline: Instant) -> Result<(), MacosOwnerExecutionError> {
    if Instant::now() < deadline {
        Ok(())
    } else {
        Err(MacosOwnerExecutionError::new(
            "fake inspection deadline expired",
        ))
    }
}

#[derive(Debug)]
struct CommandStep {
    action: LaunchctlAction,
    result: Result<SubmittedCommand, MacosOwnerExecutionError>,
    state: Option<MacosDirectLaunchdState>,
    publish: bool,
}

#[derive(Debug)]
struct FakeCommands {
    store: MacosOwnerStore,
    shared: Rc<RefCell<SharedInspector>>,
    steps: VecDeque<CommandStep>,
    actions: Vec<LaunchctlAction>,
    deadlines: Vec<Instant>,
    source_matches: VecDeque<Result<bool, MacosOwnerExecutionError>>,
}

impl FakeCommands {
    fn new(store: MacosOwnerStore, shared: Rc<RefCell<SharedInspector>>) -> Self {
        Self {
            store,
            shared,
            steps: VecDeque::new(),
            actions: Vec::new(),
            deadlines: Vec::new(),
            source_matches: VecDeque::from([Ok(true), Ok(true)]),
        }
    }

    fn push(&mut self, action: LaunchctlAction, result: SubmittedCommand) {
        self.steps.push_back(CommandStep {
            action,
            result: Ok(result),
            state: None,
            publish: false,
        });
    }

    fn push_effect(
        &mut self,
        action: LaunchctlAction,
        result: SubmittedCommand,
        state: Option<MacosDirectLaunchdState>,
        publish: bool,
    ) {
        self.steps.push_back(CommandStep {
            action,
            result: Ok(result),
            state,
            publish,
        });
    }

    fn push_error(&mut self, action: LaunchctlAction, detail: &'static str) {
        self.steps.push_back(CommandStep {
            action,
            result: Err(MacosOwnerExecutionError::new(detail)),
            state: None,
            publish: false,
        });
    }
}

impl LaunchctlCommandBoundary for FakeCommands {
    fn run(
        &mut self,
        action: &LaunchctlAction,
        deadline: Instant,
    ) -> Result<SubmittedCommand, MacosOwnerExecutionError> {
        self.actions.push(action.clone());
        self.deadlines.push(deadline);
        self.shared
            .borrow_mut()
            .trace
            .push(Trace::Command(action.clone()));
        let step = self.steps.pop_front().expect("command step should exist");
        assert_eq!(&step.action, action);
        if let Some(state) = step.state {
            self.shared.borrow_mut().state = state;
        }
        if step.publish {
            self.store
                .publish_owner(
                    MacosDaemonOwner::DirectLaunchd,
                    identity("candidate", DAEMON_PATH, REQUIREMENT, 82),
                )
                .expect("candidate should publish");
        }
        step.result
    }

    fn run_bootstrap(
        &mut self,
        _source: &mut MacosDirectLaunchdBootstrapSource,
        deadline: Instant,
    ) -> Result<SubmittedCommand, MacosOwnerExecutionError> {
        self.run(&LaunchctlAction::Bootstrap, deadline)
    }

    fn bootstrap_source_matches(
        &mut self,
        _source: &MacosDirectLaunchdBootstrapSource,
        deadline: Instant,
    ) -> Result<bool, MacosOwnerExecutionError> {
        require_time(deadline)?;
        self.source_matches
            .pop_front()
            .expect("source proof should be planned")
    }
}

fn completed(status: i32, stdout: &[u8]) -> SubmittedCommand {
    SubmittedCommand::Completed {
        status: Some(status),
        stdout: stdout.to_vec(),
        stderr: Vec::new(),
    }
}

fn identity(label: &str, path: &str, requirement: &str, pid: u32) -> MacosOwnerIdentity {
    MacosOwnerIdentity::new(label, path, digest(requirement.as_bytes()), pid)
        .expect("fixture identity should build")
}

fn record(owner: MacosDaemonOwner, epoch: u64, path: &str, pid: u32) -> MacosOwnerRecord {
    MacosOwnerRecord {
        schema_version: MACOS_OWNER_RECORD_SCHEMA_VERSION,
        active_owner: owner,
        active_identity: identity("audit", path, REQUIREMENT, pid),
        owner_epoch: epoch,
        conflict: None,
        selected_external_owner: None,
    }
}

fn publication(after_epoch: u64) -> MacosDirectLaunchdPublicationExpectation {
    let executable = MacosDirectLaunchdExecutableExpectation::new(
        DAEMON_PATH,
        REQUIREMENT,
        digest(REQUIREMENT.as_bytes()),
        CDHASH,
        SHA256,
        0o555,
        1_024,
        1,
        2,
    )
    .expect("executable expectation should build");
    MacosDirectLaunchdPublicationExpectation::new(after_epoch, executable)
        .expect("publication expectation should build")
}

fn bootstrap() -> MacosDirectLaunchdBootstrapExpectation {
    MacosDirectLaunchdBootstrapExpectation::new(
        PathBuf::from("/private/unit/launchd.plist"),
        SHA256,
        0o600,
        512,
        1,
        3,
    )
    .expect("bootstrap expectation should build")
}

fn bootstrap_source() -> MacosDirectLaunchdBootstrapSource {
    // A portable anonymous file: /dev/null does not exist on the Windows
    // stub build, which runs this suite too.
    MacosDirectLaunchdBootstrapSource::new(
        tempfile::tempfile().expect("retained fixture file should open"),
        bootstrap(),
    )
}

fn digest(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut output, byte| {
            write!(&mut output, "{byte:02x}").expect("write digest");
            output
        })
}

fn fixture(
    state: MacosDirectLaunchdState,
) -> (
    tempfile::TempDir,
    MacosOwnerStore,
    FakeInspector,
    FakeCommands,
) {
    let directory = tempfile::tempdir().expect("temporary directory should build");
    let store = MacosOwnerStore::new(directory.path());
    let shared = Rc::new(RefCell::new(SharedInspector {
        state,
        live_matches: true,
        publication_matches: true,
        exit: FakeExit::Exits,
        trace: Vec::new(),
        daemon: None,
    }));
    let inspector = FakeInspector(Rc::clone(&shared));
    let commands = FakeCommands::new(store.clone(), shared);
    (directory, store, inspector, commands)
}

/// Deadline handed to the mutation engine by every fixture.
///
/// The engine returns as soon as its proof lands, so the bound only has to
/// outlast real write-through store IO on a loaded CI disk; one second lost
/// that race on the Windows lane.
fn fixture_deadline() -> Duration {
    Duration::from_secs(30)
}

#[test]
fn bootstrap_kickstart_uses_one_deadline_and_returns_exact_publication() {
    let (_directory, store, mut inspector, mut commands) =
        fixture(MacosDirectLaunchdState::NotLoaded);
    let prior = store
        .publish_owner(
            MacosDaemonOwner::AppSidecar,
            identity("prior", "/Applications/Hypercolor", REQUIREMENT, 41),
        )
        .expect("prior should publish");
    commands.push(LaunchctlAction::Bootstrap, completed(0, b""));
    commands.push_effect(
        LaunchctlAction::Kickstart,
        completed(0, b"82\n"),
        Some(MacosDirectLaunchdState::Loaded { pid: 82 }),
        true,
    );
    let mut controller = MutationController {
        store: &store,
        inspector: &mut inspector,
        commands: &mut commands,
    };

    let outcome = controller
        .bootstrap_and_kickstart_exact(
            &mut bootstrap_source(),
            &publication(prior.owner_epoch),
            fixture_deadline(),
        )
        .expect("start should reconcile");

    let MacosDirectLaunchdMutationOutcome::Complete(proof) = outcome else {
        panic!("exact publication should complete");
    };
    assert_eq!(proof.record().active_identity.pid, 82);
    assert_eq!(commands.deadlines.len(), 2);
    assert_eq!(commands.deadlines[0], commands.deadlines[1]);
}

#[test]
fn ambiguous_bootstrap_never_submits_kickstart() {
    let (_directory, store, mut inspector, mut commands) =
        fixture(MacosDirectLaunchdState::NotLoaded);
    commands.push(LaunchctlAction::Bootstrap, SubmittedCommand::Unknown);
    let mut controller = MutationController {
        store: &store,
        inspector: &mut inspector,
        commands: &mut commands,
    };

    assert_eq!(
        controller
            .bootstrap_and_kickstart_exact(
                &mut bootstrap_source(),
                &publication(0),
                // Ambiguity resolves only by the deadline expiring, so this one
                // stays short on purpose; a slow host just polls less often.
                Duration::from_secs(1),
            )
            .expect("submitted ambiguity is a typed outcome"),
        MacosDirectLaunchdMutationOutcome::SubmittedUnknown
    );
    assert_eq!(commands.actions.len(), 1);
    assert!(matches!(commands.actions[0], LaunchctlAction::Bootstrap));
}

#[test]
fn nonzero_bootstrap_reconciles_a_late_exact_publication() {
    let (_directory, store, mut inspector, mut commands) =
        fixture(MacosDirectLaunchdState::NotLoaded);
    commands.push_effect(
        LaunchctlAction::Bootstrap,
        completed(78, b""),
        Some(MacosDirectLaunchdState::Loaded { pid: 82 }),
        true,
    );
    let mut controller = MutationController {
        store: &store,
        inspector: &mut inspector,
        commands: &mut commands,
    };

    assert!(matches!(
        controller
            .bootstrap_and_kickstart_exact(
                &mut bootstrap_source(),
                &publication(0),
                fixture_deadline(),
            )
            .expect("late publication should reconcile"),
        MacosDirectLaunchdMutationOutcome::Complete(_)
    ));
    assert_eq!(commands.actions.len(), 1);
}

#[test]
fn source_drift_after_publication_returns_submitted_unknown() {
    let (_directory, store, mut inspector, mut commands) =
        fixture(MacosDirectLaunchdState::NotLoaded);
    commands.source_matches = VecDeque::from([Ok(true), Ok(false)]);
    commands.push_effect(
        LaunchctlAction::Bootstrap,
        completed(78, b""),
        Some(MacosDirectLaunchdState::Loaded { pid: 82 }),
        true,
    );
    let mut controller = MutationController {
        store: &store,
        inspector: &mut inspector,
        commands: &mut commands,
    };

    assert_eq!(
        controller
            .bootstrap_and_kickstart_exact(
                &mut bootstrap_source(),
                &publication(0),
                fixture_deadline(),
            )
            .expect("post-submission drift is a typed outcome"),
        MacosDirectLaunchdMutationOutcome::SubmittedUnknown
    );
}

#[test]
fn kickstart_spawn_failure_after_bootstrap_is_submitted_unknown() {
    let (_directory, store, mut inspector, mut commands) =
        fixture(MacosDirectLaunchdState::NotLoaded);
    commands.push(LaunchctlAction::Bootstrap, completed(0, b""));
    commands.push_error(LaunchctlAction::Kickstart, "kickstart did not spawn");
    let mut controller = MutationController {
        store: &store,
        inspector: &mut inspector,
        commands: &mut commands,
    };

    assert_eq!(
        controller
            .bootstrap_and_kickstart_exact(
                &mut bootstrap_source(),
                &publication(0),
                fixture_deadline(),
            )
            .expect("a prior bootstrap submission prevents ordinary failure"),
        MacosDirectLaunchdMutationOutcome::SubmittedUnknown
    );
}

#[test]
fn exact_publication_replay_submits_no_command() {
    let (_directory, store, mut inspector, mut commands) =
        fixture(MacosDirectLaunchdState::Loaded { pid: 82 });
    store
        .publish_owner(
            MacosDaemonOwner::DirectLaunchd,
            identity("candidate", DAEMON_PATH, REQUIREMENT, 82),
        )
        .expect("candidate should publish");
    let mut controller = MutationController {
        store: &store,
        inspector: &mut inspector,
        commands: &mut commands,
    };

    assert!(matches!(
        controller
            .bootstrap_and_kickstart_exact(
                &mut bootstrap_source(),
                &publication(0),
                fixture_deadline(),
            )
            .expect("replay should reconcile"),
        MacosDirectLaunchdMutationOutcome::Complete(_)
    ));
    assert!(commands.actions.is_empty());
}

#[test]
fn bootout_revalidates_proof_and_requires_two_terminal_observations() {
    let (_directory, store, mut inspector, mut commands) =
        fixture(MacosDirectLaunchdState::Loaded { pid: 42 });
    let current = store
        .publish_owner(
            MacosDaemonOwner::DirectLaunchd,
            identity("current", DAEMON_PATH, REQUIREMENT, 42),
        )
        .expect("current should publish");
    let proof = super::super::corroborate_direct_launchd_owner(&current, &mut inspector)
        .expect("current should corroborate");
    commands.push_effect(
        LaunchctlAction::Bootout,
        SubmittedCommand::Unknown,
        Some(MacosDirectLaunchdState::NotLoaded),
        false,
    );
    let mut controller = MutationController {
        store: &store,
        inspector: &mut inspector,
        commands: &mut commands,
    };

    assert_eq!(
        controller
            .bootout_exact(&proof, fixture_deadline())
            .expect("exact terminal state should reconcile"),
        MacosDirectLaunchdMutationOutcome::Complete(())
    );
    assert_eq!(commands.actions, vec![LaunchctlAction::Bootout]);
}

#[test]
fn unconditional_bootout_needs_no_owner_record_and_a_terminal_proof() {
    let (_directory, store, mut inspector, mut commands) =
        fixture(MacosDirectLaunchdState::Loaded { pid: 42 });
    commands.push_effect(
        LaunchctlAction::Bootout,
        SubmittedCommand::Unknown,
        Some(MacosDirectLaunchdState::NotLoaded),
        false,
    );
    let mut controller = MutationController {
        store: &store,
        inspector: &mut inspector,
        commands: &mut commands,
    };
    assert_eq!(
        controller
            .bootout_unconditional(fixture_deadline())
            .expect("a daemon with no owner record still unloads"),
        MacosDirectLaunchdMutationOutcome::Complete(())
    );
    assert_eq!(
        controller
            .bootout_unconditional(fixture_deadline())
            .expect("already unloaded"),
        MacosDirectLaunchdMutationOutcome::Complete(()),
        "repeating it submits nothing"
    );
    assert_eq!(commands.actions, vec![LaunchctlAction::Bootout]);

    let (_directory, store, mut inspector, mut commands) =
        fixture(MacosDirectLaunchdState::Loaded { pid: 42 });
    commands.push(LaunchctlAction::Bootout, SubmittedCommand::Unknown);
    let mut controller = MutationController {
        store: &store,
        inspector: &mut inspector,
        commands: &mut commands,
    };
    assert_eq!(
        controller
            .bootout_unconditional(fixture_deadline())
            .expect("submitted"),
        MacosDirectLaunchdMutationOutcome::SubmittedUnknown,
        "a job that stays loaded is reported, never assumed gone"
    );
}

/// launchctl answers a bootout once launchd has signalled the job, on macOS
/// 15 and 26 alike, and launchd keeps the job loaded (`state = SIGTERMed`,
/// same pid) until its process exits or is killed at the exit timeout.
#[test]
fn unconditional_bootout_waits_for_the_signalled_job_to_exit() {
    let (_directory, store, mut inspector, mut commands) =
        fixture(MacosDirectLaunchdState::Loaded { pid: 42 });
    inspector.0.borrow_mut().exit = FakeExit::ExitsAndLaunchdDropsTheJob;
    commands.push(LaunchctlAction::Bootout, completed(0, b""));
    let mut controller = MutationController {
        store: &store,
        inspector: &mut inspector,
        commands: &mut commands,
    };

    assert_eq!(
        controller
            .bootout_unconditional(fixture_deadline())
            .expect("the job leaves when its process exits"),
        MacosDirectLaunchdMutationOutcome::Complete(())
    );
    assert_eq!(
        inspector.0.borrow().trace,
        vec![
            Trace::Watch(42),
            Trace::Command(LaunchctlAction::Bootout),
            Trace::ExitWaited
        ],
        "the exit watch is armed before the bootout can end the process"
    );
}

#[test]
fn exact_bootout_waits_for_the_signalled_job_to_exit() {
    let (_directory, store, mut inspector, mut commands) =
        fixture(MacosDirectLaunchdState::Loaded { pid: 42 });
    let current = store
        .publish_owner(
            MacosDaemonOwner::DirectLaunchd,
            identity("current", DAEMON_PATH, REQUIREMENT, 42),
        )
        .expect("current should publish");
    let proof = super::super::corroborate_direct_launchd_owner(&current, &mut inspector)
        .expect("current should corroborate");
    inspector.0.borrow_mut().exit = FakeExit::ExitsAndLaunchdDropsTheJob;
    commands.push(LaunchctlAction::Bootout, completed(0, b""));
    let mut controller = MutationController {
        store: &store,
        inspector: &mut inspector,
        commands: &mut commands,
    };

    assert_eq!(
        controller
            .bootout_exact(&proof, fixture_deadline())
            .expect("the owner leaves when its process exits"),
        MacosDirectLaunchdMutationOutcome::Complete(())
    );
    assert_eq!(
        inspector.0.borrow().trace,
        vec![
            Trace::Watch(42),
            Trace::Command(LaunchctlAction::Bootout),
            Trace::ExitWaited
        ]
    );
}

/// macOS 15's launchctl has no `--wait` and answers status 64 with a usage
/// text, leaving the job running. A refusal is launchctl's error, never an
/// unconfirmed stop and never a wait on a process nobody signalled.
#[test]
fn refused_bootout_is_an_error_in_launchctls_words() {
    const REFUSAL: &[u8] = b"Unrecognized target specifier.\nUsage: launchctl bootout \
        <domain-target> [service-path1, service-path2, ...] | <service-target>\n";
    let refused = || SubmittedCommand::Completed {
        status: Some(64),
        stdout: Vec::new(),
        stderr: REFUSAL.to_vec(),
    };

    let (_directory, store, mut inspector, mut commands) =
        fixture(MacosDirectLaunchdState::Loaded { pid: 42 });
    commands.push(LaunchctlAction::Bootout, refused());
    let mut controller = MutationController {
        store: &store,
        inspector: &mut inspector,
        commands: &mut commands,
    };
    let error = controller
        .bootout_unconditional(fixture_deadline())
        .expect_err("a refused bootout is an error");
    assert!(
        error.to_string().contains("status 64")
            && error.to_string().contains("Unrecognized target specifier"),
        "{error}"
    );
    assert!(!inspector.0.borrow().trace.contains(&Trace::ExitWaited));

    let (_directory, store, mut inspector, mut commands) =
        fixture(MacosDirectLaunchdState::Loaded { pid: 42 });
    let current = store
        .publish_owner(
            MacosDaemonOwner::DirectLaunchd,
            identity("current", DAEMON_PATH, REQUIREMENT, 42),
        )
        .expect("current should publish");
    let proof = super::super::corroborate_direct_launchd_owner(&current, &mut inspector)
        .expect("current should corroborate");
    commands.push(LaunchctlAction::Bootout, refused());
    let mut controller = MutationController {
        store: &store,
        inspector: &mut inspector,
        commands: &mut commands,
    };
    let error = controller
        .bootout_exact(&proof, fixture_deadline())
        .expect_err("a refused bootout is an error");
    assert!(error.to_string().contains("status 64"), "{error}");
    assert!(!inspector.0.borrow().trace.contains(&Trace::ExitWaited));
}

/// `Boot-out failed: 3: No such process`: the job left between the first
/// read and the request, which is the state that was asked for.
#[test]
fn refused_bootout_of_a_job_that_already_left_completes() {
    let (_directory, store, mut inspector, mut commands) =
        fixture(MacosDirectLaunchdState::Loaded { pid: 42 });
    commands.push_effect(
        LaunchctlAction::Bootout,
        SubmittedCommand::Completed {
            status: Some(3),
            stdout: Vec::new(),
            stderr: b"Boot-out failed: 3: No such process\n".to_vec(),
        },
        Some(MacosDirectLaunchdState::NotLoaded),
        false,
    );
    let mut controller = MutationController {
        store: &store,
        inspector: &mut inspector,
        commands: &mut commands,
    };
    assert_eq!(
        controller
            .bootout_unconditional(fixture_deadline())
            .expect("an absent job is unloaded"),
        MacosDirectLaunchdMutationOutcome::Complete(())
    );
}

#[test]
fn a_job_that_outlives_the_deadline_is_submitted_unknown() {
    let (_directory, store, mut inspector, mut commands) =
        fixture(MacosDirectLaunchdState::Loaded { pid: 42 });
    inspector.0.borrow_mut().exit = FakeExit::Never;
    commands.push(LaunchctlAction::Bootout, completed(0, b""));
    let mut controller = MutationController {
        store: &store,
        inspector: &mut inspector,
        commands: &mut commands,
    };
    assert_eq!(
        controller
            .bootout_unconditional(fixture_deadline())
            .expect("submitted"),
        MacosDirectLaunchdMutationOutcome::SubmittedUnknown
    );
}

#[test]
fn proof_drift_prevents_bootout_submission() {
    let (_directory, store, mut inspector, mut commands) =
        fixture(MacosDirectLaunchdState::Loaded { pid: 42 });
    let current = record(MacosDaemonOwner::DirectLaunchd, 7, DAEMON_PATH, 42);
    let proof = super::super::corroborate_direct_launchd_owner(&current, &mut inspector)
        .expect("fixture should corroborate");
    store
        .publish_owner(
            MacosDaemonOwner::DirectLaunchd,
            identity("replacement", DAEMON_PATH, REQUIREMENT, 43),
        )
        .expect("replacement should publish");
    let mut controller = MutationController {
        store: &store,
        inspector: &mut inspector,
        commands: &mut commands,
    };

    assert!(
        controller
            .bootout_exact(&proof, fixture_deadline())
            .is_err()
    );
    assert!(commands.actions.is_empty());
}

#[test]
fn bootout_terminal_fence_keeps_publication_outside_the_stop_lock() {
    use std::sync::mpsc;

    struct LockProbeCommands {
        shared: Rc<RefCell<SharedInspector>>,
        start_tx: mpsc::SyncSender<()>,
        attempt_rx: mpsc::Receiver<()>,
        published_rx: mpsc::Receiver<MacosOwnerRecord>,
    }

    impl LaunchctlCommandBoundary for LockProbeCommands {
        fn run(
            &mut self,
            action: &LaunchctlAction,
            _deadline: Instant,
        ) -> Result<SubmittedCommand, MacosOwnerExecutionError> {
            assert_eq!(action, &LaunchctlAction::Bootout);
            self.start_tx.send(()).expect("publisher should start");
            self.attempt_rx
                .recv()
                .expect("publication attempt should be visible");
            assert!(
                self.published_rx
                    .recv_timeout(Duration::from_millis(50))
                    .is_err(),
                "new publication must remain blocked through the stop submission"
            );
            self.shared.borrow_mut().state = MacosDirectLaunchdState::NotLoaded;
            Ok(SubmittedCommand::Unknown)
        }

        fn run_bootstrap(
            &mut self,
            _source: &mut MacosDirectLaunchdBootstrapSource,
            _deadline: Instant,
        ) -> Result<SubmittedCommand, MacosOwnerExecutionError> {
            unreachable!("stop does not submit bootstrap")
        }

        fn bootstrap_source_matches(
            &mut self,
            _source: &MacosDirectLaunchdBootstrapSource,
            _deadline: Instant,
        ) -> Result<bool, MacosOwnerExecutionError> {
            unreachable!("stop does not inspect a bootstrap source")
        }
    }

    let (_directory, store, mut inspector, _) =
        fixture(MacosDirectLaunchdState::Loaded { pid: 42 });
    let current = store
        .publish_owner(
            MacosDaemonOwner::DirectLaunchd,
            identity("current", DAEMON_PATH, REQUIREMENT, 42),
        )
        .expect("current should publish");
    let proof = super::super::corroborate_direct_launchd_owner(&current, &mut inspector)
        .expect("current should corroborate");
    let publisher_store = store.clone();
    let (start_tx, start_rx) = mpsc::sync_channel(0);
    let (attempt_tx, attempt_rx) = mpsc::sync_channel(0);
    let (published_tx, published_rx) = mpsc::sync_channel(0);
    let publisher = std::thread::spawn(move || {
        start_rx.recv().expect("publisher should be released");
        attempt_tx.send(()).expect("attempt should be visible");
        let replacement = publisher_store
            .publish_owner(
                MacosDaemonOwner::DirectLaunchd,
                identity("replacement", DAEMON_PATH, REQUIREMENT, 43),
            )
            .expect("replacement should publish after stop fencing");
        published_tx
            .send(replacement)
            .expect("replacement should be observable");
    });
    let shared = Rc::clone(&inspector.0);
    let mut commands = LockProbeCommands {
        shared,
        start_tx,
        attempt_rx,
        published_rx,
    };
    let mut controller = MutationController {
        store: &store,
        inspector: &mut inspector,
        commands: &mut commands,
    };

    assert_eq!(
        controller
            .bootout_exact(&proof, fixture_deadline())
            .expect("stop should reconcile under the lock"),
        MacosDirectLaunchdMutationOutcome::Complete(())
    );
    let replacement = commands
        .published_rx
        .recv_timeout(fixture_deadline())
        .expect("publication should finish after terminal fencing");
    publisher.join().expect("publisher should finish");
    assert!(replacement.owner_epoch > current.owner_epoch);
}

/// The exiting daemon clears its session record under the coordination lock
/// the stop holds while it binds the bootout to the exact owner. Waiting
/// for the exit inside that lock would leave the daemon blocked until
/// launchd killed it at its exit timeout, with its session record left
/// behind.
#[test]
fn exact_bootout_waits_for_the_daemon_outside_the_store_lock() {
    let (_directory, store, mut inspector, mut commands) =
        fixture(MacosDirectLaunchdState::Loaded { pid: 42 });
    let current = store
        .publish_owner(
            MacosDaemonOwner::DirectLaunchd,
            identity("current", DAEMON_PATH, REQUIREMENT, 42),
        )
        .expect("current should publish");
    let proof = super::super::corroborate_direct_launchd_owner(&current, &mut inspector)
        .expect("current should corroborate");
    {
        let mut shared = inspector.0.borrow_mut();
        shared.exit = FakeExit::ClearsItsSessionThenExits;
        shared.daemon = Some(FakeDaemonExit {
            store: store.clone(),
            clean_exit: None,
            shutdown: None,
        });
    }
    commands.push(LaunchctlAction::Bootout, completed(0, b""));
    let mut controller = MutationController {
        store: &store,
        inspector: &mut inspector,
        commands: &mut commands,
    };

    assert_eq!(
        controller
            .bootout_exact(&proof, fixture_deadline())
            .expect("the owner leaves"),
        MacosDirectLaunchdMutationOutcome::Complete(())
    );
    let mut daemon = inspector
        .0
        .borrow_mut()
        .daemon
        .take()
        .expect("the daemon exit ran");
    daemon
        .shutdown
        .take()
        .expect("the daemon shut down")
        .join()
        .expect("the shutdown thread finishes");
    let clean_exit = daemon
        .clean_exit
        .expect("the daemon exits on its own instead of at launchd's exit timeout");
    assert!(
        clean_exit < LAUNCHD_EXIT_TIMEOUT / 5,
        "the session clear waited {clean_exit:?} for the lock"
    );
}

#[test]
fn autostart_mutation_requires_two_exact_terminal_reads() {
    let (_directory, store, mut inspector, mut commands) =
        fixture(MacosDirectLaunchdState::NotLoaded);
    let enabled = b"disabled services = {\n}\n";
    let disabled =
        format!("disabled services = {{\n\t\"{MACOS_DIRECT_LAUNCHD_LABEL}\" => disabled\n}}\n");
    commands.push(LaunchctlAction::PrintDisabled, completed(0, enabled));
    commands.push(LaunchctlAction::Disable, completed(0, b""));
    commands.push(
        LaunchctlAction::PrintDisabled,
        completed(0, disabled.as_bytes()),
    );
    commands.push(
        LaunchctlAction::PrintDisabled,
        completed(0, disabled.as_bytes()),
    );
    let mut controller = MutationController {
        store: &store,
        inspector: &mut inspector,
        commands: &mut commands,
    };

    assert_eq!(
        controller
            .set_autostart(false, fixture_deadline())
            .expect("disabled state should reconcile"),
        MacosDirectLaunchdMutationOutcome::Complete(())
    );
    assert!(commands.deadlines.windows(2).all(|pair| pair[0] == pair[1]));
}

#[test]
fn autostart_terminal_drift_after_submission_is_unknown() {
    let (_directory, store, mut inspector, mut commands) =
        fixture(MacosDirectLaunchdState::NotLoaded);
    let enabled = b"disabled services = {\n}\n";
    let disabled =
        format!("disabled services = {{\n\t\"{MACOS_DIRECT_LAUNCHD_LABEL}\" => disabled\n}}\n");
    commands.push(LaunchctlAction::PrintDisabled, completed(0, enabled));
    commands.push(LaunchctlAction::Disable, completed(0, b""));
    commands.push(
        LaunchctlAction::PrintDisabled,
        completed(0, disabled.as_bytes()),
    );
    commands.push(LaunchctlAction::PrintDisabled, completed(0, enabled));
    let mut controller = MutationController {
        store: &store,
        inspector: &mut inspector,
        commands: &mut commands,
    };

    assert_eq!(
        controller
            .set_autostart(false, fixture_deadline())
            .expect("post-submission drift is a typed outcome"),
        MacosDirectLaunchdMutationOutcome::SubmittedUnknown
    );
}
