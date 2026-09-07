use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use super::{RawInputSession, WorkerState, run_initial_step, set_pump_failure};
use crate::pump::{PumpError, StopSignal};
use hypercolor_worker_retention::spawn_worker;

fn stalled_session(exit_after: Duration) -> RawInputSession {
    let stop = Arc::new(StopSignal::new());
    let window = Arc::new(Mutex::new(0));
    let device_count = Arc::new(AtomicUsize::new(0));
    let state = Arc::new(Mutex::new(WorkerState::Running));
    let (finished_tx, finished) = mpsc::sync_channel(1);
    let worker = spawn_worker(
        thread::Builder::new().name("hypercolor-stalled-raw-input-test".to_owned()),
        move || {
            thread::sleep(exit_after);
            let _ = finished_tx.send(());
        },
    )
    .expect("test Raw Input worker starts after its cleanup service");
    RawInputSession {
        stop,
        window,
        device_count,
        state,
        finished,
        worker: Some(worker),
        join_timed_out: false,
    }
}

#[test]
fn readiness_timeout_blocks_every_late_initial_sink_call() {
    let worker_delay = Duration::from_millis(40);
    let ready_timeout = Duration::from_millis(5);
    let stop = Arc::new(StopSignal::new());
    let sink_calls = Arc::new(AtomicUsize::new(0));
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let worker_stop = Arc::clone(&stop);
    let worker_sink_calls = Arc::clone(&sink_calls);
    let worker = thread::spawn(move || {
        thread::sleep(worker_delay);
        if run_initial_step(&worker_stop, || {})
            && run_initial_step(&worker_stop, || {
                worker_sink_calls.fetch_add(1, Ordering::SeqCst);
            })
        {
            let _ = ready_tx.send(());
        }
    });
    if ready_rx.recv_timeout(ready_timeout).is_err() {
        stop.request_stop();
    }
    worker.join().expect("test readiness worker exits");

    assert_eq!(sink_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn join_timeout_retains_worker_until_a_later_stop_observes_exit() {
    let mut session = stalled_session(Duration::from_millis(40));

    session.stop_with_timeout(Duration::from_millis(5));
    assert!(
        session.worker.is_some(),
        "bounded stop must retain a worker that has not exited"
    );

    thread::sleep(Duration::from_millis(50));
    session.stop_with_timeout(Duration::from_millis(20));
    assert!(
        session.worker.is_none(),
        "a later stop must join a worker after observing its exit"
    );
}

#[test]
fn drop_after_explicit_stop_timeout_delegates_without_waiting_twice() {
    let mut session = stalled_session(Duration::from_millis(250));
    session.stop_with_timeout(Duration::from_millis(5));
    assert!(matches!(session.worker_state(), WorkerState::Failed(_)));

    let started = Instant::now();
    drop(session);
    assert!(
        started.elapsed() < Duration::from_millis(75),
        "drop repeated a bounded join after explicit stop: {:?}",
        started.elapsed()
    );
}

#[test]
fn transient_backoff_is_interrupted_by_stop() {
    let stop = Arc::new(StopSignal::new());
    let waiter = {
        let stop = Arc::clone(&stop);
        thread::spawn(move || {
            let began = Instant::now();
            let active = stop.wait_for(Duration::from_secs(1));
            (active, began.elapsed())
        })
    };

    let deadline = Instant::now() + Duration::from_millis(100);
    while !stop.is_waiting() && Instant::now() < deadline {
        thread::yield_now();
    }
    assert!(
        stop.is_waiting(),
        "backoff waiter never entered the condvar"
    );
    stop.request_stop();
    let (active, elapsed) = waiter.join().expect("backoff waiter joins");
    assert!(!active);
    assert!(elapsed < Duration::from_millis(100), "elapsed: {elapsed:?}");
}

#[test]
fn terminal_pump_error_becomes_exact_worker_failure() {
    let state = Mutex::new(WorkerState::Running);
    set_pump_failure(&state, &PumpError::BufferReadFailed(5));

    assert_eq!(
        *state.lock().expect("worker state lock"),
        WorkerState::Failed("GetRawInputBuffer failed with Win32 error 5".to_owned())
    );
}
