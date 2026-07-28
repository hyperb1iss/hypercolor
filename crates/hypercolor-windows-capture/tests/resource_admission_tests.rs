#![cfg(all(
    target_os = "windows",
    feature = "capture-bench",
    target_pointer_width = "64"
))]

use hypercolor_windows_capture::{
    CaptureError, CaptureExtent, CaptureReductionBenchmark, classify_allocation_pressure_for_test,
};

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

#[test]
fn d3d_out_of_memory_remains_typed_resource_pressure() {
    for operation in [
        "create capture compute shader",
        "create reduction constants",
        "create reduction texture",
        "create reduction SRV",
        "create reduction UAV",
        "create reduction event query",
    ] {
        let error = classify_allocation_pressure_for_test(operation, 4096);
        assert!(matches!(
            error,
            CaptureError::ResourceExhausted {
                operation: classified_operation,
                requested_bytes: 4096,
            } if classified_operation == operation
        ));
    }
}
