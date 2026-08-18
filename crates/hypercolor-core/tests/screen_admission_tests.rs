use std::sync::{Arc, Barrier};
use std::thread;

use hypercolor_core::input::screen::{
    CaptureConfig, PixelExtent, ScreenAdmissionCapacity, ScreenAnalysisResourcePlan,
    ScreenByteAdmissionCoordinator, ScreenByteAdmissionError, ScreenByteReservation,
    ScreenCaptureInput,
};
use hypercolor_core::input::{InputData, InputSource};

#[test]
fn admission_reservation_reconciles_only_before_freeze() {
    let coordinator = ScreenByteAdmissionCoordinator::new(ScreenAdmissionCapacity::new(100, 80));
    let mut reservation = coordinator.try_acquire(70).expect("quote should fit");
    assert_eq!(coordinator.snapshot().reserved_bytes(), 70);

    reservation
        .reconcile_down(48)
        .expect("successful preparation may release quote slack");
    let lease = reservation.freeze();
    assert_eq!(lease.bytes(), 48);
    assert_eq!(coordinator.snapshot().reserved_bytes(), 48);

    let alias = lease.clone();
    drop(lease);
    assert_eq!(coordinator.snapshot().reserved_bytes(), 48);
    drop(alias);
    assert_eq!(coordinator.snapshot().reserved_bytes(), 0);
}

#[test]
fn live_lease_rebases_up_and_down_without_exposing_unadmitted_bytes() {
    let coordinator = ScreenByteAdmissionCoordinator::new(ScreenAdmissionCapacity::new(100, 90));
    let lease = coordinator
        .try_acquire(40)
        .expect("initial pool quote should fit")
        .freeze();

    lease
        .try_reconcile_exact(80)
        .expect("observed pool should fit");
    assert_eq!(lease.bytes(), 80);
    assert_eq!(coordinator.snapshot().reserved_bytes(), 80);

    assert_eq!(
        lease.try_reconcile_exact(95),
        Err(ScreenByteAdmissionError::CapacityExceeded {
            requested_bytes: 15,
            available_bytes: 10,
        })
    );
    assert_eq!(lease.bytes(), 80);
    assert_eq!(coordinator.snapshot().reserved_bytes(), 80);

    lease
        .try_reconcile_exact(56)
        .expect("exact pool observation may release variance");
    assert_eq!(lease.bytes(), 56);
    assert_eq!(coordinator.snapshot().reserved_bytes(), 56);
    drop(lease);
    assert_eq!(coordinator.snapshot().reserved_bytes(), 0);
}

#[test]
fn capacity_shrink_rejects_without_mutating_live_fence() {
    let coordinator = ScreenByteAdmissionCoordinator::new(ScreenAdmissionCapacity::new(100, 90));
    let lease = coordinator
        .try_acquire(70)
        .expect("initial reservation should fit")
        .freeze();

    assert_eq!(
        coordinator.try_set_capacity(ScreenAdmissionCapacity::new(60, 80)),
        Err(ScreenByteAdmissionError::CapacityShrinkRejected {
            requested_capacity: 60,
            reserved_bytes: 70,
        })
    );
    let snapshot = coordinator.snapshot();
    assert_eq!(snapshot.capacity(), ScreenAdmissionCapacity::new(100, 90));
    assert_eq!(snapshot.reserved_bytes(), 70);
    drop(lease);
}

#[test]
fn acquire_and_successful_shrink_never_publish_an_over_capacity_lease() {
    for _ in 0..256 {
        let coordinator =
            ScreenByteAdmissionCoordinator::new(ScreenAdmissionCapacity::new(100, 100));
        let barrier = Arc::new(Barrier::new(3));
        let acquire_coordinator = coordinator.clone();
        let acquire_barrier = Arc::clone(&barrier);
        let acquire = thread::spawn(move || {
            acquire_barrier.wait();
            acquire_coordinator
                .try_acquire(60)
                .map(ScreenByteReservation::freeze)
        });
        let shrink_coordinator = coordinator.clone();
        let shrink_barrier = Arc::clone(&barrier);
        let shrink = thread::spawn(move || {
            shrink_barrier.wait();
            shrink_coordinator.try_set_capacity(ScreenAdmissionCapacity::new(50, 50))
        });
        barrier.wait();
        let lease = acquire.join().expect("acquire thread should finish");
        let shrink = shrink.join().expect("shrink thread should finish");
        let snapshot = coordinator.snapshot();
        assert!(snapshot.reserved_bytes() <= snapshot.capacity().byte_budget());
        assert!(snapshot.reserved_bytes() <= snapshot.capacity().backend_capacity());
        assert_ne!(lease.is_ok(), shrink.is_ok());
        drop(lease);
    }
}

#[test]
fn acquire_overlapping_rejected_shrink_preserves_the_old_capacity() {
    let coordinator = ScreenByteAdmissionCoordinator::new(ScreenAdmissionCapacity::new(100, 100));
    let retained = coordinator
        .try_acquire(60)
        .expect("retained reservation should fit")
        .freeze();
    let barrier = Arc::new(Barrier::new(3));
    let acquire_coordinator = coordinator.clone();
    let acquire_barrier = Arc::clone(&barrier);
    let acquire = thread::spawn(move || {
        acquire_barrier.wait();
        acquire_coordinator
            .try_acquire(10)
            .map(ScreenByteReservation::freeze)
    });
    let shrink_coordinator = coordinator.clone();
    let shrink_barrier = Arc::clone(&barrier);
    let shrink = thread::spawn(move || {
        shrink_barrier.wait();
        shrink_coordinator.try_set_capacity(ScreenAdmissionCapacity::new(50, 50))
    });
    barrier.wait();
    let additional = acquire
        .join()
        .expect("acquire thread should finish")
        .expect("old capacity should admit the concurrent quote");
    assert!(matches!(
        shrink.join().expect("shrink thread should finish"),
        Err(ScreenByteAdmissionError::CapacityShrinkRejected { .. })
    ));
    let snapshot = coordinator.snapshot();
    assert_eq!(snapshot.capacity(), ScreenAdmissionCapacity::new(100, 100));
    assert_eq!(snapshot.reserved_bytes(), 70);
    drop((retained, additional));
}

#[test]
fn analyzer_plan_accounts_for_all_extent_storage() {
    let extent = PixelExtent::new(17, 11).expect("extent should be valid");
    let plan = ScreenAnalysisResourcePlan::try_new_for_extent(8, 6, 30, extent, u64::MAX)
        .expect("plan should be representable");
    assert_eq!(plan.extent_pixels(), 17 * 11);
    assert_eq!(plan.extent_retained_bytes(), 17 * 11 * 39);
}

#[test]
fn pinned_alternating_aspects_share_one_strict_three_slot_bound() {
    let config = CaptureConfig {
        grid_cols: 2,
        grid_rows: 2,
        ..CaptureConfig::default()
    };
    let extent = PixelExtent::new(8, 8).expect("extent should be valid");
    let mut input =
        ScreenCaptureInput::with_requested_extent(config, extent).expect("analyzer should prepare");
    let shapes = [(8, 4), (4, 8), (8, 8)];
    let mut pinned = Vec::new();
    for (width, height) in shapes {
        let frame = solid_rgba(width, height, [20, 40, 60, 255]);
        assert!(
            input
                .push_frame(&frame, width, height)
                .expect("frame should be valid")
        );
        let InputData::Screen(data) = input.sample().expect("sample should succeed") else {
            panic!("accepted frame should publish screen data");
        };
        pinned.push(data);
    }
    let last_good = pinned[2]
        .canvas_downscale
        .as_ref()
        .expect("third frame should include a surface")
        .storage_identity();

    let rejected = solid_rgba(16, 9, [90, 80, 70, 255]);
    assert!(
        !input
            .push_frame(&rejected, 16, 9)
            .expect("pressure is not a malformed frame")
    );
    let InputData::Screen(after_pressure) = input.sample().expect("sample should succeed") else {
        panic!("pressure should preserve last-good data");
    };
    assert_eq!(
        after_pressure
            .canvas_downscale
            .as_ref()
            .expect("last-good surface should remain")
            .storage_identity(),
        last_good
    );

    drop(after_pressure);
    pinned.remove(0);
    assert!(
        input
            .push_frame(&rejected, 16, 9)
            .expect("released slot should accept the next descriptor")
    );
}

#[test]
fn extracted_surface_keeps_analyzer_admission_alive() {
    let extent = PixelExtent::new(4, 4).expect("extent should be valid");
    let config = CaptureConfig {
        grid_cols: 1,
        grid_rows: 1,
        analysis_memory_bytes: 1_000_000,
        ..CaptureConfig::default()
    };
    let coordinator =
        ScreenByteAdmissionCoordinator::new(ScreenAdmissionCapacity::new(1_000_000, 1_000_000));
    let mut input = ScreenCaptureInput::with_requested_extent_and_admission(
        config,
        extent,
        coordinator.clone(),
    )
    .expect("analyzer should prepare");
    let frame = solid_rgba(4, 4, [10, 20, 30, 255]);
    assert!(
        input
            .push_frame(&frame, 4, 4)
            .expect("frame should be valid")
    );
    let InputData::Screen(data) = input.sample().expect("sample should succeed") else {
        panic!("accepted frame should publish screen data");
    };
    let surface = data
        .canvas_downscale
        .clone()
        .expect("screen data should expose its surface");
    drop((data, input));
    assert!(coordinator.snapshot().reserved_bytes() > 0);
    drop(surface);
    assert_eq!(coordinator.snapshot().reserved_bytes(), 0);
}

#[test]
fn extracted_zone_snapshot_keeps_its_admission_alive() {
    let extent = PixelExtent::new(4, 4).expect("extent should be valid");
    let config = CaptureConfig {
        grid_cols: 1,
        grid_rows: 1,
        analysis_memory_bytes: 1_000_000,
        ..CaptureConfig::default()
    };
    let coordinator =
        ScreenByteAdmissionCoordinator::new(ScreenAdmissionCapacity::new(1_000_000, 1_000_000));
    let mut input = ScreenCaptureInput::with_requested_extent_and_admission(
        config,
        extent,
        coordinator.clone(),
    )
    .expect("analyzer should prepare");
    let frame = solid_rgba(4, 4, [10, 20, 30, 255]);
    assert!(
        input
            .push_frame(&frame, 4, 4)
            .expect("frame should be valid")
    );
    let InputData::Screen(mut data) = input.sample().expect("sample should succeed") else {
        panic!("accepted frame should publish screen data");
    };
    data.canvas_downscale = None;

    drop(input);
    assert!(coordinator.snapshot().reserved_bytes() > 0);
    drop(data);
    assert_eq!(coordinator.snapshot().reserved_bytes(), 0);
}

fn solid_rgba(width: u32, height: u32, color: [u8; 4]) -> Vec<u8> {
    let pixel_count = usize::try_from(width)
        .expect("test width should fit")
        .checked_mul(usize::try_from(height).expect("test height should fit"))
        .expect("test pixel count should fit");
    color.repeat(pixel_count)
}
