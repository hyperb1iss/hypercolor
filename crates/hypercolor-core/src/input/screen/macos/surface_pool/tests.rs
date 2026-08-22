use std::alloc::Layout;
use std::mem::size_of;
use std::sync::{Arc, Barrier};

use super::*;
use crate::input::screen::ScreenAdmissionCapacity;

fn pool(
    coordinator: &ScreenByteAdmissionCoordinator,
    telemetry: Arc<MacosScreenRuntimeTelemetry>,
) -> MacosSurfacePool {
    MacosSurfacePool::reserve(coordinator, telemetry, 100, 32)
        .expect("initial queue reserve should fit")
}

fn metadata_bytes(pool: &MacosSurfacePool) -> u64 {
    pool.inner.metadata_lease.bytes()
}

fn initial_reserve_bytes(pool: &MacosSurfacePool) -> u64 {
    lock(&pool.inner.state).initial_surface_reserve.bytes()
}

fn live_surface_count(pool: &MacosSurfacePool) -> usize {
    let state = lock(&pool.inner.state);
    let mut count = 0;
    let mut current = state.live.as_deref();
    while let Some(surface) = current {
        count += 1;
        current = surface.next.as_deref();
    }
    count
}

fn take_pool_drop_events() -> Vec<&'static str> {
    POOL_DROP_EVENTS.with(|events| std::mem::take(&mut *events.borrow_mut()))
}

fn take_top_up_events() -> Vec<&'static str> {
    TOP_UP_EVENTS.with(|events| std::mem::take(&mut *events.borrow_mut()))
}

fn take_top_up_peak_snapshots() -> Vec<(&'static str, u64, u64)> {
    TOP_UP_PEAK_SNAPSHOTS.with(|snapshots| std::mem::take(&mut *snapshots.borrow_mut()))
}

fn arc_allocation_bytes_oracle<T>() -> u64 {
    arc_allocation_bytes_for_layout_oracle(Layout::new::<T>())
}

fn lease_allocation_bytes_oracle() -> u64 {
    let (payload, _) = Layout::new::<Arc<()>>()
        .extend(Layout::new::<AtomicU64>())
        .expect("lease payload layout should build");
    arc_allocation_bytes_for_layout_oracle(payload.pad_to_align())
}

fn arc_allocation_bytes_for_layout_oracle(payload: Layout) -> u64 {
    let header = Layout::array::<AtomicUsize>(2).expect("Arc header layout should build");
    let (allocation, _) = header
        .extend(payload)
        .expect("Arc allocation layout should build");
    u64::try_from(allocation.pad_to_align().size()).expect("Arc allocation size fits u64")
}

fn pool_tracking_bytes_oracle() -> u64 {
    arc_allocation_bytes_oracle::<MacosSurfacePoolInner>() + 2 * lease_allocation_bytes_oracle()
}

fn surface_token_tracking_bytes_oracle() -> u64 {
    arc_allocation_bytes_oracle::<MacosSurfaceAdmissionToken>() + lease_allocation_bytes_oracle()
}

fn live_surface_tracking_bytes_oracle() -> u64 {
    u64::try_from(size_of::<LiveSurface>()).expect("live index allocation size fits u64")
        + surface_token_tracking_bytes_oracle()
}

#[test]
fn byte_quotes_enumerate_every_pool_and_live_heap_allocation() {
    let coordinator =
        ScreenByteAdmissionCoordinator::new(ScreenAdmissionCapacity::new(1_000_000, 1_000_000));
    let telemetry = Arc::new(MacosScreenRuntimeTelemetry::default());
    let pool = pool(&coordinator, Arc::clone(&telemetry));
    let pool_bytes = 32 + pool_tracking_bytes_oracle();
    let live_tracking_bytes = live_surface_tracking_bytes_oracle();

    assert_eq!(metadata_bytes(&pool), pool_bytes);
    assert_eq!(
        initial_reserve_bytes(&pool),
        u64::try_from(MACOS_STREAM_QUEUE_DEPTH).expect("queue depth fits u64")
            * (100 + live_tracking_bytes)
    );

    let token = pool.observe(29, 120).expect("surface should fit");
    assert_eq!(
        token.admitted_bytes.load(Ordering::Acquire),
        120 + live_tracking_bytes
    );
    assert_eq!(token.lease.bytes(), 120 + live_tracking_bytes);
    assert!(lock(&token.pool).is_some());

    drop(pool);
    let pinned_bytes = 120 + surface_token_tracking_bytes_oracle();
    assert!(lock(&token.pool).is_none());
    assert_eq!(coordinator.snapshot().reserved_bytes(), pinned_bytes);
    assert_eq!(
        telemetry.admitted_native_bytes.load(Ordering::Acquire),
        pinned_bytes
    );

    drop(token);
    assert_eq!(coordinator.snapshot().reserved_bytes(), 0);
    assert_eq!(telemetry.admitted_native_bytes.load(Ordering::Acquire), 0);
}

#[test]
fn pinned_generations_retain_tokens_without_retaining_pool_allocations() {
    let coordinator =
        ScreenByteAdmissionCoordinator::new(ScreenAdmissionCapacity::new(1_000_000, 1_000_000));
    let telemetry = Arc::new(MacosScreenRuntimeTelemetry::default());
    let pinned_surface_bytes = 120 + surface_token_tracking_bytes_oracle();
    let mut tokens = Vec::new();

    for generation in 1..=9 {
        let generation_pool = pool(&coordinator, Arc::clone(&telemetry));
        let token = generation_pool
            .observe(31, 120)
            .expect("generation surface should fit");
        drop(generation_pool);

        assert!(lock(&token.pool).is_none());
        tokens.push(token);
        assert_eq!(
            coordinator.snapshot().reserved_bytes(),
            generation * pinned_surface_bytes
        );
        assert_eq!(
            telemetry.admitted_native_bytes.load(Ordering::Acquire),
            generation * pinned_surface_bytes
        );
    }

    drop(tokens);
    assert_eq!(coordinator.snapshot().reserved_bytes(), 0);
    assert_eq!(telemetry.admitted_native_bytes.load(Ordering::Acquire), 0);
}

#[test]
fn pool_drop_frees_live_index_before_releasing_its_tracking() {
    let coordinator =
        ScreenByteAdmissionCoordinator::new(ScreenAdmissionCapacity::new(1_000_000, 1_000_000));
    let pool = pool(
        &coordinator,
        Arc::new(MacosScreenRuntimeTelemetry::default()),
    );
    let token = pool.observe(37, 120).expect("surface should fit");
    drop(take_pool_drop_events());

    drop(pool);

    assert_eq!(
        take_pool_drop_events(),
        vec!["live_surface_drop", "index_tracking_release"]
    );
    drop(token);
}

#[test]
fn ninth_historical_surface_is_admitted_after_prior_tokens_drop() {
    let coordinator =
        ScreenByteAdmissionCoordinator::new(ScreenAdmissionCapacity::new(1_000_000, 1_000_000));
    let telemetry = Arc::new(MacosScreenRuntimeTelemetry::default());
    let pool = pool(&coordinator, Arc::clone(&telemetry));
    let metadata_bytes = metadata_bytes(&pool);

    for iosurface_id in 1..=9 {
        let token = pool
            .observe(iosurface_id, 100)
            .expect("historical identity must not consume a live slot");
        assert_eq!(live_surface_count(&pool), 1);
        drop(token);
        assert_eq!(live_surface_count(&pool), 0);
    }

    assert_eq!(initial_reserve_bytes(&pool), 0);
    assert_eq!(coordinator.snapshot().reserved_bytes(), metadata_bytes);
    assert_eq!(
        telemetry.admitted_native_bytes.load(Ordering::Acquire),
        metadata_bytes
    );
}

#[test]
fn ninth_simultaneous_surface_depends_only_on_byte_capacity() {
    let coordinator =
        ScreenByteAdmissionCoordinator::new(ScreenAdmissionCapacity::new(1_000_000, 1_000_000));
    let telemetry = Arc::new(MacosScreenRuntimeTelemetry::default());
    let pool = pool(&coordinator, Arc::clone(&telemetry));
    let metadata_bytes = metadata_bytes(&pool);
    let per_surface_bytes = 100 + live_surface_tracking_bytes_oracle();
    let tokens = (1..=9)
        .map(|iosurface_id| {
            pool.observe(iosurface_id, 100)
                .expect("real byte capacity admits more than queue depth")
        })
        .collect::<Vec<_>>();

    assert_eq!(live_surface_count(&pool), 9);
    assert_eq!(
        coordinator.snapshot().reserved_bytes(),
        metadata_bytes + 9 * per_surface_bytes
    );
    assert_eq!(
        telemetry.admitted_native_bytes.load(Ordering::Acquire),
        metadata_bytes + 9 * per_surface_bytes
    );
    drop(tokens);
    assert_eq!(coordinator.snapshot().reserved_bytes(), metadata_bytes);
}

#[test]
fn ninth_simultaneous_surface_is_rejected_when_byte_capacity_is_full() {
    let coordinator =
        ScreenByteAdmissionCoordinator::new(ScreenAdmissionCapacity::new(1_000_000, 1_000_000));
    let pool = pool(
        &coordinator,
        Arc::new(MacosScreenRuntimeTelemetry::default()),
    );
    let initial_bytes = coordinator.snapshot().reserved_bytes();
    coordinator
        .try_set_capacity(ScreenAdmissionCapacity::new(initial_bytes, initial_bytes))
        .expect("exact current capacity installs");
    let tokens = (1..=8)
        .map(|iosurface_id| {
            pool.observe(iosurface_id, 100)
                .expect("initial queue reserve covers eight live surfaces")
        })
        .collect::<Vec<_>>();

    assert!(matches!(
        pool.observe(9, 100),
        Err(MacosCaptureError::ScreenResourceExhausted {
            requested_bytes,
            available_bytes: 0,
        }) if requested_bytes
            == 100
                + live_surface_tracking_bytes_oracle()
                + lease_allocation_bytes_oracle()
    ));
    assert_eq!(live_surface_count(&pool), 8);
    assert_eq!(coordinator.snapshot().reserved_bytes(), initial_bytes);
    drop(tokens);
}

#[test]
fn top_up_peak_admits_the_temporary_lease_allocation() {
    let coordinator =
        ScreenByteAdmissionCoordinator::new(ScreenAdmissionCapacity::new(1_000_000, 1_000_000));
    let pool = pool(
        &coordinator,
        Arc::new(MacosScreenRuntimeTelemetry::default()),
    );
    let tokens = (1..=8)
        .map(|iosurface_id| {
            pool.observe(iosurface_id, 100)
                .expect("initial queue reserve covers eight live surfaces")
        })
        .collect::<Vec<_>>();
    let reserved_before = coordinator.snapshot().reserved_bytes();
    let final_surface_bytes = 100 + live_surface_tracking_bytes_oracle();
    let temporary_lease_bytes = lease_allocation_bytes_oracle();
    coordinator
        .try_set_capacity(ScreenAdmissionCapacity::new(
            reserved_before + final_surface_bytes,
            reserved_before + final_surface_bytes,
        ))
        .expect("capacity for only the final surface installs");
    drop(take_top_up_events());
    drop(take_top_up_peak_snapshots());

    assert!(matches!(
        pool.observe(9, 100),
        Err(MacosCaptureError::ScreenResourceExhausted {
            requested_bytes,
            available_bytes,
        }) if requested_bytes == final_surface_bytes + temporary_lease_bytes
            && available_bytes == final_surface_bytes
    ));
    assert_eq!(coordinator.snapshot().reserved_bytes(), reserved_before);
    assert_eq!(
        pool.inner
            .telemetry
            .admitted_native_bytes
            .load(Ordering::Acquire),
        reserved_before
    );
    assert_eq!(
        take_top_up_events(),
        vec!["peak_precharged", "coordinator_rejected", "peak_released"]
    );
    assert!(take_top_up_peak_snapshots().is_empty());

    coordinator
        .try_set_capacity(ScreenAdmissionCapacity::new(
            reserved_before + final_surface_bytes + temporary_lease_bytes,
            reserved_before + final_surface_bytes + temporary_lease_bytes,
        ))
        .expect("capacity for the exact top-up peak installs");
    let ninth = pool
        .observe(9, 100)
        .expect("exact temporary peak capacity admits the surface");
    assert_eq!(
        coordinator.snapshot().reserved_bytes(),
        reserved_before + final_surface_bytes
    );
    assert_eq!(
        take_top_up_peak_snapshots(),
        vec![
            (
                "before_final_lease_split",
                reserved_before + final_surface_bytes + temporary_lease_bytes,
                reserved_before + final_surface_bytes + temporary_lease_bytes,
            ),
            (
                "after_final_lease_split",
                reserved_before + final_surface_bytes + temporary_lease_bytes,
                reserved_before + final_surface_bytes + temporary_lease_bytes,
            ),
            (
                "steady_state",
                reserved_before + final_surface_bytes,
                reserved_before + final_surface_bytes,
            ),
        ]
    );
    assert_eq!(
        take_top_up_events(),
        vec![
            "peak_precharged",
            "temporary_lease_admitted",
            "final_lease_split",
            "temporary_lease_freed",
            "temporary_peak_released",
        ]
    );
    assert_eq!(
        pool.inner
            .telemetry
            .admitted_native_bytes
            .load(Ordering::Acquire),
        reserved_before + final_surface_bytes
    );

    drop(ninth);
    drop(tokens);
}

#[test]
fn rejected_top_up_restores_the_unconsumed_initial_reserve() {
    let coordinator =
        ScreenByteAdmissionCoordinator::new(ScreenAdmissionCapacity::new(1_000_000, 1_000_000));
    let telemetry = Arc::new(MacosScreenRuntimeTelemetry::default());
    let pool = pool(&coordinator, Arc::clone(&telemetry));
    let initial_bytes = coordinator.snapshot().reserved_bytes();
    let initial_surface_reserve = initial_reserve_bytes(&pool);
    coordinator
        .try_set_capacity(ScreenAdmissionCapacity::new(initial_bytes, initial_bytes))
        .expect("exact current capacity installs");

    assert!(matches!(
        pool.observe(1, initial_surface_reserve),
        Err(MacosCaptureError::ScreenResourceExhausted { .. })
    ));
    assert_eq!(live_surface_count(&pool), 0);
    assert_eq!(initial_reserve_bytes(&pool), initial_surface_reserve);
    assert_eq!(coordinator.snapshot().reserved_bytes(), initial_bytes);
    assert_eq!(
        telemetry.admitted_native_bytes.load(Ordering::Acquire),
        initial_bytes
    );
}

#[test]
fn repeated_live_observations_share_one_token_and_release_exactly_once() {
    let coordinator =
        ScreenByteAdmissionCoordinator::new(ScreenAdmissionCapacity::new(1_000_000, 1_000_000));
    let telemetry = Arc::new(MacosScreenRuntimeTelemetry::default());
    let pool = pool(&coordinator, Arc::clone(&telemetry));
    let reserved_before = coordinator.snapshot().reserved_bytes();
    let tracking_bytes = live_surface_tracking_bytes_oracle();
    let first = pool.observe(7, 120).expect("first observation fits");
    let repeated = pool.observe(7, 120).expect("live reuse fits");

    assert!(Arc::ptr_eq(&first, &repeated));
    assert_eq!(live_surface_count(&pool), 1);
    assert_eq!(
        initial_reserve_bytes(&pool),
        8 * (100 + tracking_bytes) - (120 + tracking_bytes)
    );
    assert_eq!(coordinator.snapshot().reserved_bytes(), reserved_before);
    drop(first);
    assert_eq!(coordinator.snapshot().reserved_bytes(), reserved_before);
    drop(repeated);
    assert_eq!(
        coordinator.snapshot().reserved_bytes(),
        reserved_before - (120 + tracking_bytes)
    );
    assert_eq!(live_surface_count(&pool), 0);
}

#[test]
fn concurrent_live_observations_share_one_token() {
    const OBSERVERS: usize = 16;

    let coordinator =
        ScreenByteAdmissionCoordinator::new(ScreenAdmissionCapacity::new(1_000_000, 1_000_000));
    let telemetry = Arc::new(MacosScreenRuntimeTelemetry::default());
    let pool = pool(&coordinator, telemetry);
    let tracking_bytes = live_surface_tracking_bytes_oracle();
    let barrier = Arc::new(Barrier::new(OBSERVERS));
    let tokens = std::thread::scope(|scope| {
        let handles = (0..OBSERVERS)
            .map(|_| {
                let pool = pool.clone();
                let barrier = Arc::clone(&barrier);
                scope.spawn(move || {
                    barrier.wait();
                    pool.observe(11, 144).expect("shared observation fits")
                })
            })
            .collect::<Vec<_>>();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("observer thread succeeds"))
            .collect::<Vec<_>>()
    });

    assert!(tokens.iter().all(|token| Arc::ptr_eq(&tokens[0], token)));
    assert_eq!(live_surface_count(&pool), 1);
    assert_eq!(
        initial_reserve_bytes(&pool),
        8 * (100 + tracking_bytes) - (144 + tracking_bytes)
    );
}

#[test]
fn live_allocation_conflicts_fail_closed_and_recycled_ids_admit_fresh() {
    let coordinator =
        ScreenByteAdmissionCoordinator::new(ScreenAdmissionCapacity::new(1_000_000, 1_000_000));
    let telemetry = Arc::new(MacosScreenRuntimeTelemetry::default());
    let pool = pool(&coordinator, telemetry);
    let first = pool.observe(19, 120).expect("first observation fits");
    let reserved = coordinator.snapshot().reserved_bytes();

    assert!(matches!(
        pool.observe(19, 121),
        Err(MacosCaptureError::InvalidSurface)
    ));
    assert_eq!(coordinator.snapshot().reserved_bytes(), reserved);
    drop(first);

    let recycled = pool
        .observe(19, 121)
        .expect("fully released identity is admitted fresh");
    assert_eq!(recycled.allocation_bytes, 121);
    assert_eq!(live_surface_count(&pool), 1);
}

#[test]
fn pinned_old_generation_retains_only_its_live_surface_bytes() {
    let coordinator =
        ScreenByteAdmissionCoordinator::new(ScreenAdmissionCapacity::new(10_000, 10_000));
    let pinned_bytes = 120 + surface_token_tracking_bytes_oracle();
    let old_telemetry = Arc::new(MacosScreenRuntimeTelemetry::default());
    let old_pool = pool(&coordinator, Arc::clone(&old_telemetry));
    let pinned = old_pool
        .observe(23, 120)
        .expect("old generation surface fits");
    drop(old_pool);

    assert_eq!(coordinator.snapshot().reserved_bytes(), pinned_bytes);
    assert_eq!(
        old_telemetry.admitted_native_bytes.load(Ordering::Acquire),
        pinned_bytes
    );

    let candidate_telemetry = Arc::new(MacosScreenRuntimeTelemetry::default());
    let candidate = pool(&coordinator, Arc::clone(&candidate_telemetry));
    let candidate_bytes = coordinator.snapshot().reserved_bytes() - pinned_bytes;
    drop(pinned);
    assert_eq!(coordinator.snapshot().reserved_bytes(), candidate_bytes);
    assert_eq!(
        old_telemetry.admitted_native_bytes.load(Ordering::Acquire),
        0
    );
    drop(candidate);
    assert_eq!(coordinator.snapshot().reserved_bytes(), 0);
}
