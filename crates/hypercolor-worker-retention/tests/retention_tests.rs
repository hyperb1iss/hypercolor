use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use hypercolor_worker_retention::{retain_worker, spawn_worker};

#[test]
fn public_spawn_runs_a_joinable_worker() {
    let worker = spawn_worker(
        thread::Builder::new().name("public-spawn-test".to_owned()),
        || {},
    )
    .expect("worker spawns");

    worker.join().expect("worker joins");
}

#[test]
fn public_retention_preserves_a_live_worker_until_it_exits() {
    let (release_tx, release_rx) = mpsc::channel();
    let finished = Arc::new(AtomicBool::new(false));
    let finished_worker = Arc::clone(&finished);
    let worker = spawn_worker(
        thread::Builder::new().name("public-retention-test".to_owned()),
        move || {
            let _ = release_rx.recv();
            finished_worker.store(true, Ordering::Release);
        },
    )
    .expect("worker spawns");
    retain_worker(worker, "public retention test");
    release_tx.send(()).expect("worker is released");

    let deadline = Instant::now() + Duration::from_secs(1);
    while !finished.load(Ordering::Acquire) && Instant::now() < deadline {
        thread::yield_now();
    }
    assert!(finished.load(Ordering::Acquire));
}
