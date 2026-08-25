use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use super::{WorkerReaper, retention_service, spawn_worker_after};

#[test]
fn worker_spawn_is_skipped_when_cleanup_capacity_cannot_start() {
    let worker_started = Arc::new(AtomicBool::new(false));
    let worker_flag = Arc::clone(&worker_started);
    let result = spawn_worker_after(
        Err(io::Error::other("injected cleanup service failure")),
        thread::Builder::new().name("must-not-start".to_owned()),
        move || worker_flag.store(true, Ordering::Release),
    );

    assert!(result.is_err());
    assert!(!worker_started.load(Ordering::Acquire));
}

#[test]
fn idle_service_eventually_observes_a_retained_worker() {
    let service = WorkerReaper::start().expect("test cleanup service starts");
    let baseline = service.reaped_count();
    let (release_tx, release_rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        let _ = release_rx.recv();
    });
    service.submit(worker, Arc::from("idle cleanup test worker"));
    release_tx.send(()).expect("test worker release succeeds");

    let deadline = Instant::now() + Duration::from_secs(1);
    while service.reaped_count() == baseline && Instant::now() < deadline {
        thread::yield_now();
    }
    assert_eq!(service.reaped_count(), baseline + 1);
}

#[test]
fn cleanup_service_spawn_failure_is_reported_without_a_live_thread() {
    let result = WorkerReaper::start_with(|_, _| {
        Err(io::Error::other("injected cleanup thread spawn failure"))
    });

    assert!(result.is_err());
}

#[test]
fn cleanup_service_is_a_process_singleton() {
    let first = std::ptr::from_ref(retention_service().expect("cleanup service starts"));
    let second = thread::spawn(|| {
        std::ptr::from_ref(retention_service().expect("cleanup service remains available")) as usize
    })
    .join()
    .expect("client joins");

    assert_eq!(first as usize, second);
}
