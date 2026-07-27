#[cfg(feature = "servo")]
compile_error!(
    "allocation-contract-tests owns the process allocator and cannot be combined with servo; run `just alloc-contracts`"
);

use std::{
    alloc::System,
    hint::black_box,
    time::{Duration, Instant},
};

use hypercolor_core::input::{SourceKind, SourceSessionWriter, SourceStatusWriter};
use stats_alloc::{INSTRUMENTED_SYSTEM, Region, Stats, StatsAlloc};

#[cfg_attr(not(feature = "servo"), global_allocator)]
static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

fn allocation_control() -> (Stats, Stats) {
    let mut region = Region::new(GLOBAL);
    region.reset();

    let allocation = black_box(vec![0_u8; 4_096]);
    let after_allocation = region.change();
    drop(allocation);

    (after_allocation, region.change())
}

fn preallocated_control(storage: &mut Vec<u8>) -> Stats {
    let mut region = Region::new(GLOBAL);
    region.reset();

    let storage = black_box(storage);
    storage.push(7);
    black_box(storage.as_slice());
    let value = storage.pop();
    black_box(value);

    region.change()
}

fn sample_round(session: &SourceSessionWriter, base: Instant, first_offset: u64) -> Stats {
    let mut region = Region::new(GLOBAL);
    region.reset();
    let mut all_accepted = true;

    for offset in first_offset..first_offset + 128 {
        let sampled_at = base + Duration::from_millis(offset);
        let deadline = base + Duration::from_mins(2) + Duration::from_millis(offset);
        all_accepted &= black_box(session.record_sample(sampled_at, deadline, 1)) == Ok(true);
    }

    let stats = region.change();
    assert!(all_accepted);
    stats
}

fn steady_source_sample_control() -> (Stats, Stats) {
    let (writer, _) = SourceStatusWriter::new(
        "allocation-source",
        SourceKind::Screen,
        "test",
        true,
        true,
        true,
    );
    let session = writer
        .begin_session(1)
        .expect("allocation source session should start");
    let base = Instant::now();
    assert_eq!(
        session.record_sample(base, base + Duration::from_mins(1), 1),
        Ok(true)
    );

    (
        sample_round(&session, base, 1),
        sample_round(&session, base, 129),
    )
}

#[test]
fn counting_allocator_is_active_and_scoped() {
    drop(black_box(vec![0_u8; 4_096]));

    let first_allocation = allocation_control();
    let second_allocation = allocation_control();

    assert_eq!(first_allocation, second_allocation);
    assert_eq!(first_allocation.0.allocations, 1);
    assert_eq!(first_allocation.0.deallocations, 0);
    assert_eq!(first_allocation.0.reallocations, 0);
    assert_eq!(first_allocation.0.bytes_allocated, 4_096);
    assert_eq!(first_allocation.0.bytes_deallocated, 0);
    assert_eq!(first_allocation.0.bytes_reallocated, 0);
    assert_eq!(first_allocation.1.allocations, 1);
    assert_eq!(first_allocation.1.deallocations, 1);
    assert_eq!(first_allocation.1.reallocations, 0);
    assert_eq!(first_allocation.1.bytes_allocated, 4_096);
    assert_eq!(first_allocation.1.bytes_deallocated, 4_096);
    assert_eq!(first_allocation.1.bytes_reallocated, 0);

    let mut storage = Vec::with_capacity(1);
    storage.push(7);
    black_box(storage.pop());

    let first_preallocated = preallocated_control(&mut storage);
    let second_preallocated = preallocated_control(&mut storage);

    assert_eq!(first_preallocated, Stats::default());
    assert_eq!(second_preallocated, first_preallocated);

    let (first_samples, second_samples) = steady_source_sample_control();
    assert_eq!(first_samples, Stats::default());
    assert_eq!(second_samples, Stats::default());
}
