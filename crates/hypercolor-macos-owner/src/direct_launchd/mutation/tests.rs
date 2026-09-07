use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use sha2::{Digest as _, Sha256};

use super::{
    DeadlineDirectLaunchdInspector, LaunchctlAction, LaunchctlCommandBoundary,
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
}

#[derive(Debug, Clone)]
struct FakeInspector(Rc<RefCell<SharedInspector>>);

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
                "new publication must remain blocked through terminal fencing"
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
