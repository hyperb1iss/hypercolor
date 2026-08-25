use std::{alloc::System, hint::black_box, sync::Arc};

use hypercolor_types::host_input::{
    HostInputEvent, HostKeyIdentity, HostKeySignal, HostRepeatEvidence,
};
use hypercolor_windows_input::PendingEvents;
use stats_alloc::{INSTRUMENTED_SYSTEM, Region, Stats, StatsAlloc};

#[global_allocator]
static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

fn publish_preinterned(pending: &mut PendingEvents, key: &Arc<str>, code: &Arc<str>) -> Stats {
    let mut region = Region::new(GLOBAL);
    region.reset();
    pending.push(HostInputEvent::Key {
        device: None,
        identity: HostKeyIdentity {
            key: Arc::clone(key),
            physical_code: Arc::clone(code),
        },
        signal: HostKeySignal::Edge {
            pressed: true,
            repeat: HostRepeatEvidence::Unknown,
        },
    });
    assert!(pending.deliver(1, 1, None, &mut |batch| {
        black_box(batch.events);
    }));
    region.change()
}

#[test]
fn preinterned_neutral_event_delivery_does_not_allocate() {
    let key: Arc<str> = Arc::from("a");
    let code: Arc<str> = Arc::from("windows:set1:none:1e");
    let mut pending = PendingEvents::new();
    pending.push(HostInputEvent::StateGap {
        device: None,
        reason: hypercolor_types::host_input::HostInputGapReason::SynchronizationLost,
    });
    pending.deliver(0, 1, None, &mut |_| {});

    let first = publish_preinterned(&mut pending, &key, &code);
    let second = publish_preinterned(&mut pending, &key, &code);
    assert_eq!(first, Stats::default());
    assert_eq!(second, first);
}
