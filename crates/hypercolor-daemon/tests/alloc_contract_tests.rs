use std::alloc::System;
use std::hint::black_box;

use hypercolor_daemon::render_thread::sparkleflinger::ProjectedLookupAllocationFixture;
use stats_alloc::{INSTRUMENTED_SYSTEM, Region, Stats, StatsAlloc};

#[global_allocator]
static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

fn lookup_round(fixture: &ProjectedLookupAllocationFixture) -> Stats {
    let mut region = Region::new(GLOBAL);
    region.reset();
    for _ in 0..256 {
        assert!(black_box(fixture).run_round());
    }
    region.change()
}

#[test]
fn warmed_projected_lookup_and_zero_screen_detection_do_not_allocate() {
    let fixture = ProjectedLookupAllocationFixture::new(6);
    assert!(fixture.run_round());

    let first = lookup_round(&fixture);
    let second = lookup_round(&fixture);

    assert_eq!(first, Stats::default());
    assert_eq!(second, first);
}
