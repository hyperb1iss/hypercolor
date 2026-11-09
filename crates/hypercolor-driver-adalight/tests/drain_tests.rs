//! Drain-progress watchdog tests (Spec 91 §7.2).
//!
//! Linux ptys always report an empty output queue, so the watchdog's
//! decisions are pinned here with synthetic readings and clocks.

use std::time::{Duration, Instant};

use hypercolor_driver_adalight::{DrainVerdict, DrainWatch};

const FRAME: usize = 906;
const BUDGET: Duration = Duration::from_secs(1);

#[test]
fn less_than_a_frame_queued_is_clear() {
    let mut watch = DrainWatch::new();
    let now = Instant::now();
    assert_eq!(watch.observe(0, FRAME, BUDGET, now), DrainVerdict::Clear);
    assert_eq!(watch.observe(905, FRAME, BUDGET, now), DrainVerdict::Clear);
}

#[test]
fn a_full_frame_queued_skips_the_frame() {
    let mut watch = DrainWatch::new();
    let now = Instant::now();
    assert_eq!(
        watch.observe(906, FRAME, BUDGET, now),
        DrainVerdict::Backlogged
    );
    assert_eq!(
        watch.observe(906, FRAME, BUDGET, now + Duration::from_millis(500)),
        DrainVerdict::Backlogged
    );
}

#[test]
fn a_queue_that_never_drains_stalls_after_the_budget() {
    let mut watch = DrainWatch::new();
    let start = Instant::now();
    assert_eq!(
        watch.observe(1_200, FRAME, BUDGET, start),
        DrainVerdict::Backlogged
    );
    assert_eq!(
        watch.observe(1_200, FRAME, BUDGET, start + Duration::from_millis(999)),
        DrainVerdict::Backlogged
    );
    assert_eq!(
        watch.observe(1_200, FRAME, BUDGET, start + BUDGET),
        DrainVerdict::Stalled {
            queued: 1_200,
            stalled_for: BUDGET,
        }
    );
    // It keeps failing until the queue moves or the session is replaced.
    assert!(matches!(
        watch.observe(1_200, FRAME, BUDGET, start + Duration::from_secs(3)),
        DrainVerdict::Stalled { .. }
    ));
}

#[test]
fn a_slowly_draining_queue_keeps_coalescing() {
    let mut watch = DrainWatch::new();
    let start = Instant::now();
    let mut queued = 4_000_u32;
    // Each reading arrives 900 ms after the last and finds the queue a
    // little shorter: slow, but progressing, so it never stalls.
    for step in 0..4_u32 {
        let now = start + Duration::from_millis(900) * step;
        assert_eq!(
            watch.observe(queued, FRAME, BUDGET, now),
            DrainVerdict::Backlogged,
            "step {step}"
        );
        queued -= 100;
    }
}

#[test]
fn a_growing_queue_is_not_progress() {
    let mut watch = DrainWatch::new();
    let start = Instant::now();
    watch.observe(1_000, FRAME, BUDGET, start);
    assert!(matches!(
        watch.observe(1_500, FRAME, BUDGET, start + BUDGET),
        DrainVerdict::Stalled { queued: 1_500, .. }
    ));
}

#[test]
fn clearing_the_backlog_restarts_the_clock() {
    let mut watch = DrainWatch::new();
    let start = Instant::now();
    watch.observe(1_000, FRAME, BUDGET, start);
    assert_eq!(
        watch.observe(10, FRAME, BUDGET, start + Duration::from_millis(900)),
        DrainVerdict::Clear
    );
    // A new backlog long after the first one starts fresh.
    assert_eq!(
        watch.observe(1_000, FRAME, BUDGET, start + Duration::from_secs(5)),
        DrainVerdict::Backlogged
    );

    watch.reset();
    assert_eq!(
        watch.observe(1_000, FRAME, BUDGET, start + Duration::from_secs(9)),
        DrainVerdict::Backlogged
    );
}
