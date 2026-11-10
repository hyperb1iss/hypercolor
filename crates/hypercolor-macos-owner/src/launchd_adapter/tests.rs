use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use super::{ExitWatch, LaunchctlReply, LaunchdAdapter, LaunchdHost, bootout_until_gone};
use crate::MacosOwnerExecutionError;

const LABEL: &str = "tech.hyperbliss.hypercolor.update-activate";

/// What launchd holds for the fixture label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Job {
    NotLoaded,
    /// Loaded with no process, waiting out its throttle.
    Idle,
    /// Loaded with a running process; `signalled` once a bootout asked it
    /// to leave (`state = SIGTERMed`).
    Running {
        pid: u32,
        signalled: bool,
    },
}

/// How launchctl answers `bootout`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Bootout {
    /// Status 0 once launchd has signalled the job, as on macOS 15 and 26:
    /// a running process leaves later, an idle job leaves at once.
    Accepted,
    /// Status 3: the job left before the request reached launchd.
    AlreadyGone,
    /// A refusal that leaves the job as it was.
    Refused,
}

/// What the job's process does once something waits on its exit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProcessExit {
    /// It exits, and launchd drops a signalled job with it.
    Exits,
    /// It outlives every deadline.
    Never,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Trace {
    Print,
    Watch(u32),
    Bootout,
    ExitWaited,
}

#[derive(Debug)]
struct State {
    job: Job,
    bootout: Bootout,
    exit: ProcessExit,
    trace: Vec<Trace>,
}

#[derive(Debug, Clone)]
struct FakeLaunchd(Rc<RefCell<State>>);

impl FakeLaunchd {
    fn new(job: Job, bootout: Bootout, exit: ProcessExit) -> Self {
        Self(Rc::new(RefCell::new(State {
            job,
            bootout,
            exit,
            trace: Vec::new(),
        })))
    }

    fn job(&self) -> Job {
        self.0.borrow().job
    }

    fn trace(&self) -> Vec<Trace> {
        self.0.borrow().trace.clone()
    }
}

fn reply(status: i32, stdout: &str, stderr: &str) -> LaunchctlReply {
    LaunchctlReply {
        status: Some(status),
        stdout: stdout.as_bytes().to_vec(),
        stderr: stderr.as_bytes().to_vec(),
    }
}

impl LaunchdHost for FakeLaunchd {
    type ExitWatch = FakeExitWatch;

    fn launchctl(&mut self, args: &[&str]) -> Result<LaunchctlReply, MacosOwnerExecutionError> {
        let mut state = self.0.borrow_mut();
        let service = format!("gui/501/{LABEL}");
        match args {
            ["print", target] if *target == service => {
                state.trace.push(Trace::Print);
                Ok(match state.job {
                    Job::NotLoaded => reply(
                        113,
                        "",
                        &format!(
                            "Bad request.\nCould not find service \"{LABEL}\" in domain for user gui: 501\n"
                        ),
                    ),
                    Job::Idle => reply(0, "\tstate = spawn scheduled\n", ""),
                    Job::Running { pid, signalled } => reply(
                        0,
                        &format!(
                            "\tstate = {}\n\tpid = {pid}\n",
                            if signalled { "SIGTERMed" } else { "running" }
                        ),
                        "",
                    ),
                })
            }
            ["bootout", target] if *target == service => {
                state.trace.push(Trace::Bootout);
                Ok(match state.bootout {
                    Bootout::Accepted => {
                        state.job = match state.job {
                            Job::Running { pid, .. } => Job::Running {
                                pid,
                                signalled: true,
                            },
                            Job::Idle | Job::NotLoaded => Job::NotLoaded,
                        };
                        reply(0, "", "")
                    }
                    Bootout::AlreadyGone => {
                        state.job = Job::NotLoaded;
                        reply(3, "", "Boot-out failed: 3: No such process\n")
                    }
                    Bootout::Refused => reply(
                        64,
                        "",
                        "Unrecognized target specifier.\nUsage: launchctl bootout <domain-target> \
                         [service-path1, service-path2, ...] | <service-target>\n",
                    ),
                })
            }
            other => panic!("unexpected launchctl call {other:?}"),
        }
    }

    fn watch_exit(&mut self, pid: u32) -> Result<FakeExitWatch, MacosOwnerExecutionError> {
        self.0.borrow_mut().trace.push(Trace::Watch(pid));
        Ok(FakeExitWatch(Rc::clone(&self.0)))
    }
}

#[derive(Debug)]
struct FakeExitWatch(Rc<RefCell<State>>);

impl ExitWatch for FakeExitWatch {
    fn exited_by(&mut self, deadline: Instant) -> Result<bool, MacosOwnerExecutionError> {
        assert!(
            Instant::now() < deadline,
            "the wait starts inside its deadline"
        );
        let mut state = self.0.borrow_mut();
        state.trace.push(Trace::ExitWaited);
        match (state.exit, state.job) {
            (ProcessExit::Never, _) => Ok(false),
            (
                ProcessExit::Exits,
                Job::Running {
                    signalled: true, ..
                },
            ) => {
                state.job = Job::NotLoaded;
                Ok(true)
            }
            (ProcessExit::Exits, Job::Running { .. }) => {
                state.job = Job::Idle;
                Ok(true)
            }
            (ProcessExit::Exits, Job::Idle | Job::NotLoaded) => Ok(true),
        }
    }
}

fn target() -> super::LaunchdTarget {
    LaunchdAdapter::with_uid("501").target(LABEL)
}

fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(30)
}

/// The defect: launchctl answers before the job leaves, so a caller that
/// bootstraps the same label right after collides with a job launchd still
/// holds. The adapter returns only once launchd has dropped it.
#[test]
fn bootout_returns_once_launchd_has_dropped_the_running_job() {
    let mut launchd = FakeLaunchd::new(
        Job::Running {
            pid: 42,
            signalled: false,
        },
        Bootout::Accepted,
        ProcessExit::Exits,
    );
    bootout_until_gone(&mut launchd, &target(), deadline()).expect("the job leaves");
    assert_eq!(
        launchd.job(),
        Job::NotLoaded,
        "nothing is left to collide with"
    );
    assert_eq!(
        launchd.trace(),
        vec![
            Trace::Print,
            Trace::Watch(42),
            Trace::Bootout,
            Trace::ExitWaited,
            Trace::Print
        ],
        "the exit watch is armed before the bootout can end the process"
    );
}

#[test]
fn an_idle_job_leaves_with_the_bootout_itself() {
    let mut launchd = FakeLaunchd::new(Job::Idle, Bootout::Accepted, ProcessExit::Never);
    bootout_until_gone(&mut launchd, &target(), deadline()).expect("the idle job leaves");
    assert_eq!(launchd.job(), Job::NotLoaded);
    assert_eq!(
        launchd.trace(),
        vec![Trace::Print, Trace::Bootout, Trace::Print]
    );
}

#[test]
fn a_job_that_is_not_loaded_needs_no_bootout() {
    let mut launchd = FakeLaunchd::new(Job::NotLoaded, Bootout::Refused, ProcessExit::Never);
    bootout_until_gone(&mut launchd, &target(), deadline()).expect("nothing to unload");
    assert_eq!(launchd.trace(), vec![Trace::Print]);
}

#[test]
fn a_job_that_left_before_the_request_is_unloaded() {
    let mut launchd = FakeLaunchd::new(
        Job::Running {
            pid: 42,
            signalled: false,
        },
        Bootout::AlreadyGone,
        ProcessExit::Exits,
    );
    bootout_until_gone(&mut launchd, &target(), deadline())
        .expect("status 3 with the job gone is the state that was asked for");
    assert_eq!(launchd.job(), Job::NotLoaded);
}

#[test]
fn a_refused_bootout_is_launchctls_error_and_never_waits() {
    let mut launchd = FakeLaunchd::new(
        Job::Running {
            pid: 42,
            signalled: false,
        },
        Bootout::Refused,
        ProcessExit::Exits,
    );
    let error =
        bootout_until_gone(&mut launchd, &target(), deadline()).expect_err("a refusal is an error");
    assert!(
        error.to_string().contains("Some(64)")
            && error.to_string().contains("Unrecognized target specifier"),
        "{error}"
    );
    assert!(!launchd.trace().contains(&Trace::ExitWaited));
}

#[test]
fn a_process_that_outlives_the_deadline_is_an_error() {
    let mut launchd = FakeLaunchd::new(
        Job::Running {
            pid: 42,
            signalled: false,
        },
        Bootout::Accepted,
        ProcessExit::Never,
    );
    let error = bootout_until_gone(&mut launchd, &target(), deadline())
        .expect_err("a job still running is not unloaded");
    assert!(error.to_string().contains("did not exit"), "{error}");
}
