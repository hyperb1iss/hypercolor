use std::{alloc::System, hint::black_box, sync::Arc};

use hypercolor_windows_input::{
    PendingEvents, RawDeviceDescriptor, RawDeviceKind, RawInputEvent, RawKeyPrefix,
};
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

fn interned_descriptor_control(
    pending: &mut PendingEvents,
    device: &Arc<RawDeviceDescriptor>,
) -> Stats {
    let mut region = Region::new(GLOBAL);
    region.reset();

    pending.push(RawInputEvent::Key {
        device: Arc::clone(device),
        make_code: 0x1e,
        prefix: RawKeyPrefix::None,
        vkey: 0,
        pressed: true,
    });
    assert!(pending.deliver(1, 1, None, &mut |batch| {
        black_box(batch.events);
    }));

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

    let device = Arc::new(RawDeviceDescriptor {
        source_id: Arc::from("allocation-test-keyboard"),
        interface_path: Some(Arc::from("allocation-test-interface")),
        label: Arc::from("allocation test keyboard"),
        kind: RawDeviceKind::Keyboard,
        session_generation: 1,
        device_generation: 1,
    });
    let mut pending = PendingEvents::new();
    pending.push(RawInputEvent::DeviceArrived {
        device: Arc::clone(&device),
    });
    pending.deliver(0, 1, None, &mut |_| {});

    let first_descriptor_clone = interned_descriptor_control(&mut pending, &device);
    let second_descriptor_clone = interned_descriptor_control(&mut pending, &device);

    assert_eq!(first_descriptor_clone, Stats::default());
    assert_eq!(second_descriptor_clone, first_descriptor_clone);
}
