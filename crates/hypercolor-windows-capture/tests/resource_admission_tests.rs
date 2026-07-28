#![cfg(all(
    target_os = "windows",
    feature = "capture-bench",
    target_pointer_width = "64"
))]

use hypercolor_windows_capture::{CaptureError, CaptureExtent, CaptureReductionBenchmark};

#[test]
fn public_benchmark_reports_unallocatable_plane_without_entering_d3d() {
    let width = u32::MAX;
    let row_bytes = usize::try_from(width).expect("u32 fits 64-bit usize") * 4;
    let height = u32::try_from(isize::MAX as usize / row_bytes + 1)
        .expect("capacity-overflow fixture height fits u32");
    let requested_extent = CaptureExtent::try_new(1280, 720).expect("extent is non-empty");

    let error = match CaptureReductionBenchmark::new(width, height, requested_extent) {
        Ok(_) => panic!("unallocatable source unexpectedly admitted"),
        Err(error) => error,
    };

    assert!(matches!(
        error,
        CaptureError::ResourceExhausted {
            operation: "allocate benchmark source",
            requested_bytes,
        } if requested_bytes > isize::MAX as usize
    ));
}
