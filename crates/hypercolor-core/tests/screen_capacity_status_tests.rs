use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, mpsc};
use std::time::Duration;

use hypercolor_core::input::screen::consumer::{ScreenByteAdmissionError, ScreenCaptureDemand};
use hypercolor_core::input::screen::planner::ScreenAdmissionCapacity;
use hypercolor_core::input::{
    InputData, InputManager, InputSource, ManagedSourceRole, ScreenReconfigurationConflict,
    ScreenSource, ScreenSourceRole, SourceRoleBinding,
};

fn register_test_source(manager: &mut InputManager, source: ManagedSourceRole) {
    manager
        .add_source(source)
        .expect("capacity fixture source should match its declared role");
}

struct PlannedScreenSource {
    running: bool,
    demand: ScreenCaptureDemand,
}

impl PlannedScreenSource {
    const fn new() -> Self {
        Self {
            running: false,
            demand: ScreenCaptureDemand::Inactive,
        }
    }
}

impl InputSource for PlannedScreenSource {
    fn name(&self) -> &'static str {
        "PlannedScreenSource"
    }

    fn start(&mut self) -> anyhow::Result<()> {
        self.running = true;
        Ok(())
    }

    fn stop(&mut self) {
        self.running = false;
        self.demand = ScreenCaptureDemand::Inactive;
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        Ok(InputData::None)
    }

    fn is_running(&self) -> bool {
        self.running
    }
}

impl ScreenSource for PlannedScreenSource {
    fn screen_capture_demand(&self) -> ScreenCaptureDemand {
        self.demand
    }

    fn set_screen_capture_demand(&mut self, demand: ScreenCaptureDemand) -> anyhow::Result<()> {
        self.demand = demand;
        Ok(())
    }
}

impl SourceRoleBinding for PlannedScreenSource {
    type Role = ScreenSourceRole;
}

#[test]
fn status_handle_tracks_policy_and_live_physical_reservations() {
    let manager = InputManager::new();
    let status = manager.screen_capacity_status_handle();
    let coordinator = manager.screen_admission_coordinator();
    let resource = ScreenAdmissionCapacity::new(1_000, 800);
    let total = ScreenAdmissionCapacity::new(600, 500);
    let publication = ScreenAdmissionCapacity::new(400, 300);
    manager
        .set_screen_capacity_plan(resource, total, publication)
        .expect("empty manager should accept exact capacity");

    let initial = status.snapshot();
    assert!(initial.policy().capacity_enforced());
    assert_eq!(initial.policy().total_capacity(), total);
    assert_eq!(initial.policy().publication_capacity(), publication);
    assert_eq!(
        initial.policy().capture_demand(),
        ScreenCaptureDemand::Inactive
    );
    assert_eq!(initial.physical().capacity(), resource);
    assert_eq!(initial.physical().reserved_bytes(), 0);
    assert_eq!(initial.physical().available_bytes(), 800);

    let lease = coordinator
        .try_acquire(125)
        .expect("test reservation should fit")
        .freeze();
    let reserved = status.snapshot();
    assert_eq!(reserved.physical().reserved_bytes(), 125);
    assert_eq!(reserved.physical().available_bytes(), 675);
    drop(lease);
    assert_eq!(status.snapshot().physical().available_bytes(), 800);

    coordinator
        .try_set_capacity(ScreenAdmissionCapacity::new(900, 700))
        .expect("unreserved coordinator should accept direct capacity change");
    let externally_updated = status.snapshot();
    assert_eq!(
        externally_updated.physical().capacity(),
        ScreenAdmissionCapacity::new(900, 700)
    );
    assert_eq!(externally_updated.policy().total_capacity(), total);
}

#[test]
fn concurrent_status_readers_observe_complete_physical_snapshots() {
    let manager = InputManager::new();
    let capacity = ScreenAdmissionCapacity::new(10_000, 8_000);
    manager
        .set_screen_capacity_plan(capacity, capacity, capacity)
        .expect("empty manager should accept exact capacity");
    let status = manager.screen_capacity_status_handle();
    let coordinator = manager.screen_admission_coordinator();
    let running = Arc::new(AtomicBool::new(true));
    let reads = Arc::new(AtomicUsize::new(0));
    let start = Arc::new(Barrier::new(2));
    let reader_running = Arc::clone(&running);
    let reader_reads = Arc::clone(&reads);
    let reader_start = Arc::clone(&start);
    let reader = std::thread::spawn(move || {
        reader_start.wait();
        loop {
            let snapshot = status.snapshot();
            let physical = snapshot.physical();
            assert!(snapshot.policy().capacity_enforced());
            assert_eq!(
                physical.available_bytes(),
                physical
                    .capacity()
                    .byte_budget()
                    .min(physical.capacity().backend_capacity())
                    .saturating_sub(physical.reserved_bytes())
            );
            reader_reads.fetch_add(1, Ordering::Relaxed);
            if !reader_running.load(Ordering::Acquire) {
                break;
            }
        }
    });

    start.wait();
    for bytes in 1..=1_000 {
        let lease = coordinator
            .try_acquire(bytes)
            .expect("each transient reservation should fit")
            .freeze();
        std::hint::black_box(coordinator.snapshot().reserved_bytes());
        drop(lease);
    }
    running.store(false, Ordering::Release);
    reader.join().expect("status reader should not panic");
    assert!(reads.load(Ordering::Acquire) > 0);
}

#[test]
fn concurrent_status_readers_never_observe_torn_capacity_transitions() {
    let manager = InputManager::new();
    let steps = Arc::new([
        (
            ScreenAdmissionCapacity::new(10_000, 8_000),
            ScreenAdmissionCapacity::new(9_000, 7_000),
            ScreenAdmissionCapacity::new(6_000, 5_000),
        ),
        (
            ScreenAdmissionCapacity::new(20_000, 16_000),
            ScreenAdmissionCapacity::new(18_000, 14_000),
            ScreenAdmissionCapacity::new(12_000, 10_000),
        ),
        (
            ScreenAdmissionCapacity::new(30_000, 24_000),
            ScreenAdmissionCapacity::new(27_000, 21_000),
            ScreenAdmissionCapacity::new(18_000, 15_000),
        ),
    ]);
    let (initial_resource, initial_total, initial_publication) = steps[0];
    manager
        .set_screen_capacity_plan(initial_resource, initial_total, initial_publication)
        .expect("empty manager should accept initial capacity");
    let coordinator = manager.screen_admission_coordinator();
    let _lease = coordinator
        .try_acquire(1_000)
        .expect("test reservation should fit every installed capacity")
        .freeze();
    let status = manager.screen_capacity_status_handle();
    let running = Arc::new(AtomicBool::new(true));
    let start = Arc::new(Barrier::new(2));
    let reader_running = Arc::clone(&running);
    let reader_start = Arc::clone(&start);
    let reader_steps = Arc::clone(&steps);
    let (finished_tx, finished_rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        reader_start.wait();
        let mut reads = 0;
        loop {
            let snapshot = status.snapshot();
            let observed = (
                snapshot.physical().capacity(),
                snapshot.policy().total_capacity(),
                snapshot.policy().publication_capacity(),
            );
            assert!(
                reader_steps.contains(&observed),
                "status exposed a torn capacity transition: {observed:?}"
            );
            reads += 1;
            if !reader_running.load(Ordering::Acquire) {
                break;
            }
        }
        finished_tx
            .send(reads)
            .expect("status reader completion should be observed");
    });

    start.wait();
    for iteration in 0..1_000 {
        let (resource, total, publication) = steps[iteration % steps.len()];
        manager
            .set_screen_capacity_plan(resource, total, publication)
            .expect("installed capacity should retain the live reservation");
        assert!(matches!(
            manager.set_screen_capacity_plan(
                ScreenAdmissionCapacity::new(999, 999),
                ScreenAdmissionCapacity::new(777, 777),
                ScreenAdmissionCapacity::new(555, 555),
            ),
            Err(ScreenByteAdmissionError::CapacityShrinkRejected { .. })
        ));
        if iteration % 32 == 0 {
            std::thread::yield_now();
        }
    }
    running.store(false, Ordering::Release);
    let reads = finished_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("status reader should finish after capacity transitions stop");
    reader.join().expect("status reader should not panic");
    assert!(reads > 0);
}

#[test]
fn compound_capacity_and_runtime_commit_publishes_one_coherent_policy() {
    let mut manager = InputManager::new();
    let status = manager.screen_capacity_status_handle();
    let demand = ScreenCaptureDemand::active();
    let reserved_peak_bytes = 4_096_u64;
    let physical = ScreenAdmissionCapacity::new(u64::MAX, u64::MAX);
    let initial_total =
        ScreenAdmissionCapacity::new(reserved_peak_bytes + 2_000, reserved_peak_bytes + 2_000);
    manager
        .set_screen_capacity_plan(physical, initial_total, initial_total)
        .expect("empty manager should accept initial capacity");
    manager
        .set_screen_capture_demand(demand)
        .expect("demand should cache before source installation");
    let before = status.snapshot().policy();
    assert_eq!(before.capture_demand(), demand);

    let mut source = Box::new(PlannedScreenSource::new());
    source
        .set_screen_capture_demand(demand)
        .expect("prepared source should accept demand");
    source.start().expect("prepared source should start");
    let committed_total =
        ScreenAdmissionCapacity::new(reserved_peak_bytes + 1_000, reserved_peak_bytes + 750);
    let capacity = manager
        .prepare_screen_capacity_plan(committed_total, reserved_peak_bytes)
        .expect("candidate split should prepare")
        .expect("capacity enforcement should be installed");
    let runtime = manager
        .plan_screen_source_swap(true, Some(capacity))
        .expect("unique screen swap should plan");
    let mut replacement = Some(source as Box<dyn ScreenSource>);
    let mut prepared = runtime
        .prepare(&mut replacement)
        .expect("screen replacement should prepare");
    manager
        .commit_screen_source_swap(&mut prepared, |commit| {
            Ok::<_, std::convert::Infallible>(commit.commit(|| {}))
        })
        .expect("unchanged capacity and runtime should commit")
        .retire();

    let committed = status.snapshot().policy();
    assert_eq!(committed.capture_demand(), demand);
    assert_eq!(committed.total_capacity(), committed_total);
    assert_eq!(
        committed.publication_capacity(),
        ScreenAdmissionCapacity::new(1_000, 750)
    );

    let stale_capacity = manager
        .prepare_screen_capacity_plan(initial_total, 0)
        .expect("disable capacity should prepare")
        .expect("capacity enforcement should remain installed");
    let stale_runtime = manager
        .plan_screen_source_swap(false, Some(stale_capacity))
        .expect("unique screen removal should plan");
    let mut no_replacement = None;
    let mut stale_prepared = stale_runtime
        .prepare(&mut no_replacement)
        .expect("screen removal should prepare");
    register_test_source(
        &mut manager,
        ManagedSourceRole::data(Box::new(hypercolor_core::input::MediaSource::new())),
    );
    let stable = status.snapshot().policy();
    assert!(matches!(
        manager.commit_screen_source_swap(&mut stale_prepared, |commit| {
            Ok::<_, std::convert::Infallible>(commit.commit(|| {}))
        }),
        Err(
            hypercolor_core::input::ScreenSourceSwapCommitError::Conflict(
                ScreenReconfigurationConflict::Source(
                    hypercolor_core::input::SourceSwapConflict::GraphChanged
                )
            )
        )
    ));
    assert_eq!(status.snapshot().policy(), stable);
}
