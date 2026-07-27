use std::{alloc::System, hint::black_box};

use stats_alloc::{INSTRUMENTED_SYSTEM, Region, Stats, StatsAlloc};

#[global_allocator]
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
}
