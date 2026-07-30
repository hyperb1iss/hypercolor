use std::num::{NonZeroU64, NonZeroUsize};

use hypercolor_core::input::screen::{
    CaptureConfig, PixelExtent, ScreenAnalysisAdmissionError, ScreenAnalysisComputeCapacity,
    ScreenAnalysisComputeLane, ScreenAnalysisWorkPlan, ScreenCaptureInput,
    ScreenComputeCapacityPolicy,
};

fn extent(width: u32, height: u32) -> PixelExtent {
    PixelExtent::new(width, height).expect("test extent is non-empty")
}

fn capacity(workers: usize, units_per_worker_second: u64) -> ScreenAnalysisComputeCapacity {
    ScreenAnalysisComputeCapacity::new(
        NonZeroUsize::new(workers).expect("test worker count is non-zero"),
        NonZeroU64::new(units_per_worker_second).expect("test throughput is non-zero"),
    )
}

fn split_capacity(
    workers: usize,
    parallel_units_per_worker_second: u64,
    serial_units_per_second: u64,
) -> ScreenAnalysisComputeCapacity {
    ScreenAnalysisComputeCapacity::new_split(
        NonZeroUsize::new(workers).expect("test worker count is non-zero"),
        NonZeroU64::new(parallel_units_per_worker_second)
            .expect("test parallel throughput is non-zero"),
        NonZeroU64::new(serial_units_per_second).expect("test serial throughput is non-zero"),
    )
}

#[test]
fn compatibility_compute_admits_exact_boundary_and_rejects_one_under() {
    let config = CaptureConfig::default();
    let plan = ScreenAnalysisWorkPlan::try_new(extent(3840, 2160), extent(640, 480), &config)
        .expect("work arithmetic is representable");
    let required = plan
        .parallel_weighted_work_units_per_second()
        .max(plan.serial_weighted_work_units_per_second());

    assert_eq!(plan.admit(capacity(1, required)), Ok(plan));
    assert!(matches!(
        plan.admit(capacity(1, required - 1)),
        Err(ScreenAnalysisAdmissionError::ComputeCapacityExceeded {
            required_weighted_work_units_per_second,
            available_weighted_work_units_per_second,
            ..
        }) if required_weighted_work_units_per_second == required
            && available_weighted_work_units_per_second == required - 1
    ));
}

#[test]
fn worker_count_does_not_multiply_serial_analysis_capacity() {
    let plan = ScreenAnalysisWorkPlan::try_new(
        extent(7680, 4320),
        extent(7680, 4320),
        &CaptureConfig::default(),
    )
    .expect("8K work arithmetic is representable");
    let serial_required = plan.serial_weighted_work_units_per_second();
    let capacity = split_capacity(
        64,
        plan.parallel_weighted_work_units_per_second(),
        serial_required - 1,
    );

    assert!(matches!(
        plan.admit(capacity),
        Err(ScreenAnalysisAdmissionError::ComputeCapacityExceeded {
            lane: ScreenAnalysisComputeLane::Serial,
            required_weighted_work_units_per_second,
            available_weighted_work_units_per_second,
            ..
        }) if required_weighted_work_units_per_second == serial_required
            && available_weighted_work_units_per_second == serial_required - 1
    ));
}

#[test]
fn dynamic_letterbox_crop_is_admitted_at_the_maximum_requested_extent() {
    let config = CaptureConfig {
        letterbox_enabled: true,
        ..CaptureConfig::default()
    };
    let input = extent(1920, 1080);
    let requested = extent(640, 480);
    let old_full_frame_plan = ScreenAnalysisWorkPlan::try_new(input, extent(640, 360), &config)
        .expect("full-frame work arithmetic is representable");
    let crop_aware_plan = ScreenAnalysisWorkPlan::try_new(input, requested, &config)
        .expect("crop-aware work arithmetic is representable");
    let available_serial = old_full_frame_plan.serial_weighted_work_units_per_second() + 1;
    let mut serial_limited = ScreenCaptureInput::with_requested_extent_and_compute_capacity(
        config.clone(),
        requested,
        split_capacity(
            1,
            crop_aware_plan.parallel_weighted_work_units_per_second(),
            available_serial,
        ),
    )
    .expect("analysis storage is admitted");

    assert_eq!(old_full_frame_plan.output_extent(), extent(640, 360));
    assert_eq!(crop_aware_plan.output_extent(), requested);
    assert!(
        available_serial < crop_aware_plan.serial_weighted_work_units_per_second(),
        "test capacity must sit between the old full-frame and crop-aware work"
    );
    assert!(matches!(
        serial_limited.admit_frame_extent(input),
        Err(ScreenAnalysisAdmissionError::ComputeCapacityExceeded {
            lane: ScreenAnalysisComputeLane::Serial,
            required_weighted_work_units_per_second,
            available_weighted_work_units_per_second,
            ..
        }) if required_weighted_work_units_per_second
            == crop_aware_plan.serial_weighted_work_units_per_second()
            && available_weighted_work_units_per_second == available_serial
    ));
    assert_eq!(serial_limited.analysis_work_plan(), None);

    let available_parallel = old_full_frame_plan.parallel_weighted_work_units_per_second() + 1;
    let mut parallel_limited = ScreenCaptureInput::with_requested_extent_and_compute_capacity(
        config,
        requested,
        split_capacity(
            1,
            available_parallel,
            crop_aware_plan.serial_weighted_work_units_per_second(),
        ),
    )
    .expect("analysis storage is admitted");

    assert!(
        available_parallel < crop_aware_plan.parallel_weighted_work_units_per_second(),
        "policy reduction must include the maximum crop-aware publication extent"
    );
    assert!(matches!(
        parallel_limited.admit_frame_extent(input),
        Err(ScreenAnalysisAdmissionError::ComputeCapacityExceeded {
            lane: ScreenAnalysisComputeLane::Parallel,
            required_weighted_work_units_per_second,
            available_weighted_work_units_per_second,
            ..
        }) if required_weighted_work_units_per_second
            == crop_aware_plan.parallel_weighted_work_units_per_second()
            && available_weighted_work_units_per_second == available_parallel
    ));
    assert_eq!(parallel_limited.analysis_work_plan(), None);
}

#[test]
fn tiny_grid_does_not_hide_8k_or_16k_source_work() {
    let config = CaptureConfig {
        grid_cols: 1,
        grid_rows: 1,
        ..CaptureConfig::default()
    };
    let explicit_four_worker_class = capacity(4, 600_000_000);

    for source in [extent(7680, 4320), extent(15360, 8640)] {
        let plan = ScreenAnalysisWorkPlan::try_new(source, extent(640, 480), &config)
            .expect("large work arithmetic is representable");
        assert!(matches!(
            plan.admit(explicit_four_worker_class),
            Err(ScreenAnalysisAdmissionError::ComputeCapacityExceeded { .. })
        ));
    }
}

#[test]
fn default_analysis_measures_work_without_an_uncalibrated_ceiling() {
    let mut analyzer =
        ScreenCaptureInput::with_requested_extent(CaptureConfig::default(), extent(640, 480))
            .expect("default analysis storage is admitted");

    let plan = analyzer
        .admit_frame_extent(extent(15_360, 8_640))
        .expect("uncalibrated default preserves the requested workload");

    assert!(plan.weighted_work_units_per_second() > 0);
    assert_eq!(analyzer.analysis_work_plan(), Some(plan));
}

#[test]
fn calibrated_policy_preserves_independent_analysis_and_exact_capacity() {
    let analysis = split_capacity(3, 17, 29);
    let policy = ScreenComputeCapacityPolicy::calibrated(
        analysis,
        NonZeroU64::new(41).expect("test exact throughput is non-zero"),
    );
    let exact = policy
        .exact(NonZeroUsize::new(5).expect("test exact workers are non-zero"))
        .expect("calibrated exact capacity is present");

    assert_eq!(policy.analysis(), Some(analysis));
    assert_eq!(exact.worker_count().get(), 5);
    assert_eq!(exact.weighted_work_units_per_worker_second().get(), 41);
}

#[test]
fn input_extent_change_is_readmitted_transactionally() {
    let config = CaptureConfig::default();
    let requested = extent(640, 480);
    let small = extent(1920, 1080);
    let large = extent(7680, 4320);
    let small_plan = ScreenAnalysisWorkPlan::try_new(small, requested, &config)
        .expect("small work arithmetic is representable");
    let mut analyzer = ScreenCaptureInput::with_requested_extent_and_compute_capacity(
        config,
        requested,
        capacity(1, small_plan.weighted_work_units_per_second()),
    )
    .expect("analysis storage is admitted");

    assert_eq!(
        analyzer
            .admit_frame_extent(small)
            .expect("small input is admitted"),
        small_plan
    );
    assert!(matches!(
        analyzer.admit_frame_extent(large),
        Err(ScreenAnalysisAdmissionError::ComputeCapacityExceeded { .. })
    ));
    assert_eq!(analyzer.analysis_work_plan(), Some(small_plan));
}

#[test]
fn proportional_grid_visits_are_charged_when_grid_exceeds_source() {
    let config = CaptureConfig {
        grid_cols: 8,
        grid_rows: 3,
        ..CaptureConfig::default()
    };
    let plan = ScreenAnalysisWorkPlan::try_new(extent(2, 1), extent(2, 1), &config)
        .expect("oversized grid work is representable");

    assert_eq!(plan.frame_work().source_reduction, 8 * 3 * 5);
    assert_eq!(plan.frame_work().policy_reduction, 8 * 3 * 5);
}

#[test]
fn heterogeneous_passes_keep_distinct_calibrated_weights() {
    let plan = ScreenAnalysisWorkPlan::try_new(
        extent(640, 480),
        extent(640, 480),
        &CaptureConfig::default(),
    )
    .expect("work arithmetic is representable");
    let work = plan.frame_work();

    assert_ne!(work.output_resample, work.temporal_smoothing);
    assert_ne!(work.temporal_smoothing, work.color_tuning);
    assert_ne!(work.source_sector_finalization, work.zone_materialization);
}
