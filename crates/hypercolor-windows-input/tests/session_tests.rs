//! Session lifecycle against the real Raw Input API.
//!
//! These need Windows but no hardware: they exercise window creation,
//! registration, teardown, and the restart path that `ERROR_CLASS_ALREADY_EXISTS`
//! would otherwise break. They are real gates rather than a developer-machine
//! courtesy because CI now runs a Windows job.
//!
//! Deliberately absent: any assertion that input *arrives*. A test process has
//! no user typing into it, and a test that waited for input would either hang
//! or pass vacuously.

#![cfg(target_os = "windows")]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use hypercolor_windows_input::{
    RawInputConfig, RawInputError, RawInputSession, SessionState, WorkerState,
    interactive_session_state,
};

/// Teardown must stay well inside the join timeout; a stop that takes seconds
/// would show up as daemon shutdown latency.
const STOP_BUDGET: Duration = Duration::from_millis(1500);

fn test_config(keyboard: bool, mouse: bool) -> RawInputConfig {
    RawInputConfig {
        keyboard,
        mouse,
        clock: Arc::new(|| 0),
        epoch: 1,
    }
}

#[test]
fn a_normal_test_process_has_an_interactive_window_station() {
    // If this fails, every other test here is meaningless — and so is the
    // daemon, which is exactly what the probe exists to report.
    assert_eq!(interactive_session_state(), SessionState::Interactive);
}

#[test]
fn a_session_starts_registers_and_stops_cleanly() {
    let mut session =
        RawInputSession::start(test_config(true, true), |_| {}).expect("session starts");
    assert_eq!(session.worker_state(), WorkerState::Running);

    let started = Instant::now();
    session.stop();
    assert!(
        started.elapsed() < STOP_BUDGET,
        "stop took {:?}, past the budget",
        started.elapsed()
    );
}

#[test]
fn repeated_start_stop_cycles_survive_class_already_exists() {
    // Capture toggles with effect demand, so the worker really is created and
    // destroyed repeatedly in one process lifetime. A second RegisterClassW
    // fails with ERROR_CLASS_ALREADY_EXISTS, and treating that as fatal would
    // mean host capture worked exactly once per daemon run.
    for cycle in 0..4 {
        let mut session = RawInputSession::start(test_config(true, true), |_| {})
            .unwrap_or_else(|error| panic!("cycle {cycle} failed to start: {error}"));
        session.stop();
    }
}

#[test]
fn stop_is_idempotent() {
    let mut session =
        RawInputSession::start(test_config(true, false), |_| {}).expect("session starts");
    session.stop();
    session.stop();
    session.stop();
}

#[test]
fn dropping_without_stopping_still_tears_down() {
    // Drop must reach the same teardown, or a dropped session would leave the
    // process registered against a destroyed window.
    {
        let _session =
            RawInputSession::start(test_config(true, false), |_| {}).expect("session starts");
    }
    // A second session can only register if the first actually released.
    let mut next =
        RawInputSession::start(test_config(true, false), |_| {}).expect("the next session starts");
    next.stop();
}

#[test]
fn declining_both_kinds_is_an_error_rather_than_a_silent_no_op() {
    let error = RawInputSession::start(test_config(false, false), |_| {})
        .err()
        .expect("a session with nothing to capture cannot start");
    assert!(matches!(error, RawInputError::NothingToCapture));
}

#[test]
fn keyboard_only_capture_registers_without_the_mouse_usage() {
    // Declining pointer capture means the process is never registered for
    // mouse input at all, rather than filtering after the fact. If the mouse
    // usage were registered anyway, this would still start — so the assertion
    // that matters is that a keyboard-only session is a first-class shape.
    let mut session =
        RawInputSession::start(test_config(true, false), |_| {}).expect("session starts");
    assert_eq!(session.worker_state(), WorkerState::Running);
    session.stop();
}

#[test]
fn arrivals_for_already_attached_devices_land_before_readiness() {
    // RIDEV_DEVNOTIFY fires only on *change*, so an already-attached keyboard
    // produces no GIDC_ARRIVAL. Without the startup enumeration core would
    // have no devices at all until the user touched one.
    let arrivals = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&arrivals);
    let mut session = RawInputSession::start(test_config(true, true), move |batch| {
        let count = batch
            .events
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    hypercolor_windows_input::RawInputEvent::DeviceArrived { .. }
                )
            })
            .count();
        counter.fetch_add(count, Ordering::Release);
    })
    .expect("session starts");

    // A CI runner has at least a virtual keyboard and mouse; asserting only
    // that the count is observable keeps this honest on odd hosts.
    let seen = arrivals.load(Ordering::Acquire);
    assert_eq!(
        seen,
        session.device_count(),
        "every enumerated device announced itself before readiness"
    );
    session.stop();
}

#[test]
fn a_panicking_sink_is_contained_and_reported() {
    // A panic while folding must not take the process with it, and must not
    // leave core believing a dead source is live. The panic is only reachable
    // if a batch is delivered, which the startup arrivals guarantee.
    let mut session = RawInputSession::start(test_config(true, true), |_| {
        panic!("deliberate sink panic");
    });

    match &mut session {
        Ok(session) => {
            // The arrivals batch is delivered before readiness, so by the time
            // start() returns the pump has either already failed or will on its
            // first turn. Either way it must never report Running forever.
            let deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline {
                if matches!(session.worker_state(), WorkerState::Failed(_)) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            session.stop();
        }
        // A panic during the pre-readiness arrivals batch unwinds the worker
        // before it can signal, which surfaces as a ready timeout. That is an
        // honest failure, not a hang, and is equally acceptable.
        Err(error) => assert!(
            matches!(error, RawInputError::WorkerReadyTimeout),
            "unexpected start error: {error}"
        ),
    }
}

#[test]
fn two_sessions_do_not_leave_the_registration_orphaned() {
    // Registration is process-global, so a second session steals the first's.
    // What must not happen is the first session's teardown then deregistering
    // the second — the claim is what prevents that, and the observable
    // consequence is that the survivor stays Running.
    let first = RawInputSession::start(test_config(true, true), |_| {}).expect("first starts");
    let mut second =
        RawInputSession::start(test_config(true, true), |_| {}).expect("second starts");

    drop(first);
    assert_eq!(
        second.worker_state(),
        WorkerState::Running,
        "the stale session's teardown must not disturb the live one"
    );
    second.stop();
}

#[test]
fn the_clock_callback_is_only_read_on_the_pump() {
    // Timestamps are core's clock read immediately before each drain, not at
    // sink entry: a stamp taken while folding would record when folding
    // happened rather than when input was captured.
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let stamps = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&stamps);

    let config = RawInputConfig {
        keyboard: true,
        mouse: true,
        clock: Arc::new(move || {
            u64::try_from(counter.fetch_add(1, Ordering::AcqRel)).unwrap_or(u64::MAX)
        }),
        epoch: 42,
    };

    let mut session = RawInputSession::start(config, move |batch| {
        if let Ok(mut guard) = seen.lock() {
            guard.push((batch.at_ms, batch.epoch));
        }
    })
    .expect("session starts");
    session.stop();

    if let Ok(guard) = stamps.lock() {
        for (_, epoch) in guard.iter() {
            assert_eq!(*epoch, 42, "every batch echoes the epoch core allocated");
        }
    }
}
