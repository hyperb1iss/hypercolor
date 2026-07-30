#[cfg(feature = "spatial-workspace-test-hooks")]
use std::sync::Arc;
#[cfg(feature = "spatial-workspace-test-hooks")]
use std::time::Duration;

#[cfg(feature = "spatial-workspace-test-hooks")]
use hypercolor_core::spatial::SpatialWorkspaceAllocationTestHook;
use hypercolor_core::spatial::{
    PreparedAreaSample, PreparedZoneSamples, SpatialEngine, SpatialPlanError,
    SpatialSamplingCapacity, SpatialSamplingError, SpatialSamplingWorkspaceUsage,
};
use hypercolor_types::canvas::{Canvas, Rgba, linear_to_srgb, srgb_to_linear};
use hypercolor_types::spatial::{
    EdgeBehavior, LedTopology, NormalizedPosition, Output, SamplingMode, SpatialLayout,
};

fn layout(zones: Vec<Output>, width: u32, height: u32) -> SpatialLayout {
    SpatialLayout {
        id: "area-integral".into(),
        name: "Area Integral".into(),
        description: None,
        canvas_width: width,
        canvas_height: height,
        zones,
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    }
}

fn point_zone(id: String, position: NormalizedPosition, radius_x: f32, radius_y: f32) -> Output {
    Output {
        id: id.clone(),
        name: id.clone(),
        device_id: format!("test:{id}"),
        zone_name: None,
        position,
        size: NormalizedPosition::new(0.0, 0.0),
        rotation: 0.0,
        scale: 1.0,
        display_order: 0,
        orientation: None,
        topology: LedTopology::Point,
        led_positions: Vec::new(),
        led_mapping: None,
        sampling_mode: Some(SamplingMode::AreaAverage { radius_x, radius_y }),
        edge_behavior: Some(EdgeBehavior::Clamp),
        shape: None,
        shape_preset: None,
        attachment: None,
        brightness: None,
    }
}

fn patterned_canvas(width: u32, height: u32) -> Canvas {
    let mut canvas = Canvas::new(width, height);
    for y in 0..height {
        for x in 0..width {
            let ordinal = x + y * width;
            let byte = ordinal.to_le_bytes()[0];
            canvas.set_pixel(
                x,
                y,
                Rgba::new(
                    byte.wrapping_mul(31).wrapping_add(7),
                    byte.wrapping_mul(17).wrapping_add(19),
                    byte.wrapping_mul(11).wrapping_add(29),
                    255,
                ),
            );
        }
    }
    canvas
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn decode(channel: u8) -> u16 {
    (srgb_to_linear(f32::from(channel) / 255.0) * 65535.0)
        .round()
        .clamp(0.0, 65535.0) as u16
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn encode(channel: u16) -> u8 {
    (linear_to_srgb(f32::from(channel) / 65535.0) * 255.0)
        .round()
        .clamp(0.0, 255.0) as u8
}

fn scalar_area(canvas: &Canvas, sample: &PreparedAreaSample) -> [u8; 3] {
    let mut sum = [0_u64; 3];
    let mut count = 0_u64;
    let max_x = i64::from(canvas.width() - 1);
    let max_y = i64::from(canvas.height() - 1);
    for dy in -i64::from(sample.radius_y)..=i64::from(sample.radius_y) {
        let y = u32::try_from((i64::from(sample.center_y) + dy).clamp(0, max_y))
            .expect("clamped y fits u32");
        for dx in -i64::from(sample.radius_x)..=i64::from(sample.radius_x) {
            let x = u32::try_from((i64::from(sample.center_x) + dx).clamp(0, max_x))
                .expect("clamped x fits u32");
            let pixel = canvas.get_pixel(x, y);
            sum[0] += u64::from(decode(pixel.r));
            sum[1] += u64::from(decode(pixel.g));
            sum[2] += u64::from(decode(pixel.b));
            count += 1;
        }
    }
    [
        encode(u16::try_from(sum[0] / count).expect("average fits u16")),
        encode(u16::try_from(sum[1] / count).expect("average fits u16")),
        encode(u16::try_from(sum[2] / count).expect("average fits u16")),
    ]
}

#[allow(clippy::cast_precision_loss)]
fn normalized_coordinate(coordinate: u32, dimension: u32) -> f32 {
    if dimension == 1 {
        0.5
    } else {
        coordinate as f32 / (dimension - 1) as f32
    }
}

fn workspace_bytes(width: u32, height: u32) -> usize {
    usize::try_from(width + 1).expect("test width fits usize")
        * usize::try_from(height + 1).expect("test height fits usize")
        * std::mem::size_of::<[u64; 3]>()
}

#[test]
fn area_workspace_is_lazy_until_cpu_sampling_needs_it() {
    let engine = SpatialEngine::try_new(layout(
        vec![point_zone(
            "area".into(),
            NormalizedPosition::new(0.5, 0.5),
            1.0,
            1.0,
        )],
        8,
        8,
    ))
    .expect("area workspace geometry should be admitted");

    assert_eq!(
        engine.sampling_workspace_usage(),
        SpatialSamplingWorkspaceUsage::default()
    );

    engine
        .try_sample(&patterned_canvas(8, 8))
        .expect("first CPU sample should allocate the admitted workspace");
    assert_eq!(
        engine.sampling_workspace_usage(),
        SpatialSamplingWorkspaceUsage {
            retained_workspaces: 1,
            retained_bytes: workspace_bytes(8, 8),
            reserved_workspaces: 0,
            reserved_bytes: 0,
        }
    );
}

#[test]
fn summed_area_matches_scalar_clamped_sampling_for_rectangular_radii() {
    let radii = [(0.0, 0.0), (1.0, 0.0), (0.0, 2.0), (1.0, 2.0), (3.0, 1.0)];
    for (width, height) in [(1, 1), (1, 4), (4, 1), (4, 3), (5, 4)] {
        let canvas = patterned_canvas(width, height);
        let mut zones = Vec::new();
        for y in 0..height {
            for x in 0..width {
                for (radius_x, radius_y) in radii {
                    zones.push(point_zone(
                        format!("{x}-{y}-{radius_x}-{radius_y}"),
                        NormalizedPosition::new(
                            normalized_coordinate(x, width),
                            normalized_coordinate(y, height),
                        ),
                        radius_x,
                        radius_y,
                    ));
                }
            }
        }

        let engine = SpatialEngine::try_new(layout(zones, width, height))
            .expect("small summed-area workspace must prepare");
        let plan = engine.sampling_plan();
        let actual = engine
            .try_sample(&canvas)
            .expect("canonical workspace is pre-admitted");
        assert_eq!(actual.len(), plan.len());

        for (zone, prepared) in actual.iter().zip(plan.iter()) {
            let PreparedZoneSamples::Area(samples) = &prepared.prepared_samples else {
                panic!("expected area samples");
            };
            assert_eq!(samples.len(), 1);
            assert_eq!(zone.colors, [scalar_area(&canvas, &samples[0])]);
        }
    }
}

#[test]
#[allow(clippy::cast_precision_loss)]
fn maximum_radii_use_constant_time_exact_border_multiplicity() {
    let mut canvas = Canvas::new(2, 1);
    canvas.set_pixel(0, 0, Rgba::new(255, 0, 0, 255));
    canvas.set_pixel(1, 0, Rgba::new(0, 0, 255, 255));
    let radius = u32::MAX;
    let zone = point_zone(
        "maximum".into(),
        NormalizedPosition::new(0.0, 0.5),
        radius as f32,
        0.0,
    );
    let engine = SpatialEngine::try_new(layout(vec![zone], 2, 1))
        .expect("workspace size depends on canvas dimensions, not sampling radius");
    let plan = engine.sampling_plan();
    let PreparedZoneSamples::Area(samples) = &plan[0].prepared_samples else {
        panic!("expected area samples");
    };
    assert_eq!(samples[0].radius_x, u32::MAX);
    assert_eq!(samples[0].radius_y, 0);

    let left_count = u128::from(radius) + 1;
    let right_count = u128::from(radius);
    let total = left_count + right_count;
    let expected = [
        encode(u16::try_from(u128::from(decode(255)) * left_count / total).expect("fits")),
        0,
        encode(u16::try_from(u128::from(decode(255)) * right_count / total).expect("fits")),
    ];
    assert_eq!(
        engine.try_sample(&canvas).expect("sampling succeeds")[0].colors[0],
        expected
    );
}

#[test]
fn capacity_rejection_preserves_the_canonical_workspace_and_outputs() {
    let capacity = SpatialSamplingCapacity::new(512);
    assert_eq!(capacity.max_area_workspace_bytes(), 512);
    assert_eq!(
        SpatialSamplingCapacity::UNBOUNDED.max_area_workspace_bytes(),
        usize::MAX
    );
    let zone = point_zone("area".into(), NormalizedPosition::new(0.5, 0.5), 1.0, 1.0);
    let engine = SpatialEngine::try_new_with_sampling_capacity(layout(vec![zone], 2, 2), capacity)
        .expect("canonical workspace fits capacity");
    let canonical = patterned_canvas(2, 2);
    let before = engine
        .try_sample(&canonical)
        .expect("canonical sample works");

    assert_eq!(
        engine.try_prepare_sampling_canvas(8, 8),
        Err(SpatialSamplingError::AreaWorkspaceCapacityExceeded {
            width: 8,
            height: 8,
            required_bytes: 9 * 9 * 24,
            capacity_bytes: 512,
        })
    );

    let mut retained = before.clone();
    assert_eq!(
        engine.try_sample_into(&patterned_canvas(8, 8), &mut retained),
        Err(SpatialSamplingError::AreaWorkspaceCapacityExceeded {
            width: 8,
            height: 8,
            required_bytes: 9 * 9 * 24,
            capacity_bytes: 512,
        })
    );
    assert_eq!(retained, before);
    assert_eq!(
        engine.try_sample(&canonical).expect("workspace survives"),
        before
    );
    assert_eq!(
        engine.sampling_workspace_usage(),
        SpatialSamplingWorkspaceUsage {
            retained_workspaces: 1,
            retained_bytes: workspace_bytes(2, 2),
            reserved_workspaces: 0,
            reserved_bytes: 0,
        }
    );

    let rejected = SpatialEngine::try_new_with_sampling_capacity(
        layout(
            vec![point_zone(
                "rejected".into(),
                NormalizedPosition::new(0.5, 0.5),
                1.0,
                1.0,
            )],
            8,
            8,
        ),
        capacity,
    )
    .expect_err("canonical admission must reject an undersized capacity");
    assert_eq!(
        rejected,
        SpatialPlanError::SamplingResources(SpatialSamplingError::AreaWorkspaceCapacityExceeded {
            width: 8,
            height: 8,
            required_bytes: 9 * 9 * 24,
            capacity_bytes: 512,
        })
    );
}

#[test]
fn sequential_descriptors_transactionally_reuse_one_idle_workspace() {
    let capacity_bytes = workspace_bytes(8, 8);
    let engine = SpatialEngine::try_new_with_sampling_capacity(
        layout(
            vec![point_zone(
                "area".into(),
                NormalizedPosition::new(0.5, 0.5),
                1.0,
                1.0,
            )],
            2,
            2,
        ),
        SpatialSamplingCapacity::new(capacity_bytes),
    )
    .expect("canonical workspace fits capacity");

    let mut retained_bytes = 0;
    for (width, height) in [(3, 5), (5, 3), (8, 8), (2, 2)] {
        engine
            .try_prepare_sampling_canvas(width, height)
            .expect("idle workspace can change descriptors within capacity");
        retained_bytes = retained_bytes.max(workspace_bytes(width, height));
        assert_eq!(
            engine.sampling_workspace_usage(),
            SpatialSamplingWorkspaceUsage {
                retained_workspaces: 1,
                retained_bytes,
                reserved_workspaces: 0,
                reserved_bytes: 0,
            }
        );
    }
}

#[cfg(feature = "spatial-workspace-test-hooks")]
fn hooked_engine(
    fail_first: bool,
) -> (Arc<SpatialEngine>, Arc<SpatialWorkspaceAllocationTestHook>) {
    let engine = Arc::new(
        SpatialEngine::try_new_with_sampling_capacity(
            layout(
                vec![point_zone(
                    "area".into(),
                    NormalizedPosition::new(0.5, 0.5),
                    1.0,
                    1.0,
                )],
                2,
                2,
            ),
            SpatialSamplingCapacity::new(workspace_bytes(8, 8)),
        )
        .expect("canonical workspace fits capacity"),
    );
    let hook = Arc::new(SpatialWorkspaceAllocationTestHook::new(fail_first));
    assert!(engine.install_sampling_workspace_allocation_test_hook(Arc::clone(&hook)));
    (engine, hook)
}

#[cfg(feature = "spatial-workspace-test-hooks")]
#[test]
fn same_descriptor_preparation_is_single_flight_after_reservation() {
    let (engine, hook) = hooked_engine(false);
    let timeout = Duration::from_secs(5);
    let (first_reached, waiter_reached, attempts_at_gate, usage_at_gate, results) =
        std::thread::scope(|scope| {
            let first_engine = Arc::clone(&engine);
            let first = scope.spawn(move || first_engine.try_prepare_sampling_canvas(8, 8));
            let first_reached = hook.wait_for_first_reservation(timeout);

            let second_engine = Arc::clone(&engine);
            let second = scope.spawn(move || second_engine.try_prepare_sampling_canvas(8, 8));
            let waiter_reached = hook.wait_for_waiters(1, timeout);
            let attempts_at_gate = hook.allocation_attempts();
            let usage_at_gate = engine.sampling_workspace_usage();
            hook.release_first_allocation();
            let results = [
                first.join().expect("first preparation thread completes"),
                second.join().expect("second preparation thread completes"),
            ];
            (
                first_reached,
                waiter_reached,
                attempts_at_gate,
                usage_at_gate,
                results,
            )
        });

    assert!(first_reached);
    assert!(waiter_reached);
    assert_eq!(attempts_at_gate, 1);
    assert_eq!(
        usage_at_gate,
        SpatialSamplingWorkspaceUsage {
            retained_workspaces: 0,
            retained_bytes: 0,
            reserved_workspaces: 1,
            reserved_bytes: workspace_bytes(8, 8),
        }
    );
    assert_eq!(results, [Ok(()), Ok(())]);
    assert_eq!(hook.allocation_attempts(), 1);
    assert_eq!(hook.waiter_count(), 1);
    assert_eq!(
        engine.sampling_workspace_usage(),
        SpatialSamplingWorkspaceUsage {
            retained_workspaces: 1,
            retained_bytes: workspace_bytes(8, 8),
            reserved_workspaces: 0,
            reserved_bytes: 0,
        }
    );
}

#[cfg(feature = "spatial-workspace-test-hooks")]
#[test]
fn distinct_descriptor_reservations_charge_all_live_backing() {
    let first_bytes = workspace_bytes(5, 5);
    let second_bytes = workspace_bytes(4, 4);
    let capacity_bytes = first_bytes + second_bytes;
    let engine = Arc::new(
        SpatialEngine::try_new_with_sampling_capacity(
            layout(
                vec![point_zone(
                    "area".into(),
                    NormalizedPosition::new(0.5, 0.5),
                    1.0,
                    1.0,
                )],
                2,
                2,
            ),
            SpatialSamplingCapacity::new(capacity_bytes),
        )
        .expect("canonical workspace fits capacity"),
    );
    let hook = Arc::new(SpatialWorkspaceAllocationTestHook::new_gated(false, 2));
    assert!(engine.install_sampling_workspace_allocation_test_hook(Arc::clone(&hook)));
    let timeout = Duration::from_secs(5);

    let (first_reached, second_reached, usage_at_gate, results) = std::thread::scope(|scope| {
        let first_engine = Arc::clone(&engine);
        let first = scope.spawn(move || first_engine.try_prepare_sampling_canvas(5, 5));
        let first_reached = hook.wait_for_allocation_attempts(1, timeout);

        let second_engine = Arc::clone(&engine);
        let second = scope.spawn(move || second_engine.try_prepare_sampling_canvas(4, 4));
        let second_reached = hook.wait_for_allocation_attempts(2, timeout);
        let usage_at_gate = engine.sampling_workspace_usage();
        hook.release_first_allocation();
        let results = [
            first.join().expect("first preparation thread completes"),
            second.join().expect("second preparation thread completes"),
        ];
        (first_reached, second_reached, usage_at_gate, results)
    });

    assert!(first_reached);
    assert!(second_reached);
    assert_eq!(
        usage_at_gate,
        SpatialSamplingWorkspaceUsage {
            retained_workspaces: 0,
            retained_bytes: 0,
            reserved_workspaces: 2,
            reserved_bytes: capacity_bytes,
        }
    );
    assert_eq!(results, [Ok(()), Ok(())]);
    assert_eq!(
        engine.sampling_workspace_usage(),
        SpatialSamplingWorkspaceUsage {
            retained_workspaces: 2,
            retained_bytes: capacity_bytes,
            reserved_workspaces: 0,
            reserved_bytes: 0,
        }
    );
}

#[cfg(feature = "spatial-workspace-test-hooks")]
#[test]
fn shrunk_workspace_capacity_still_bounds_concurrent_allocation() {
    let capacity_bytes = workspace_bytes(8, 8);
    let engine = Arc::new(
        SpatialEngine::try_new_with_sampling_capacity(
            layout(
                vec![point_zone(
                    "area".into(),
                    NormalizedPosition::new(0.5, 0.5),
                    1.0,
                    1.0,
                )],
                2,
                2,
            ),
            SpatialSamplingCapacity::new(capacity_bytes),
        )
        .expect("canonical workspace fits capacity"),
    );
    engine
        .try_prepare_sampling_canvas(8, 8)
        .expect("workspace grows to its admitted high-water mark");
    engine
        .try_prepare_sampling_canvas(2, 2)
        .expect("workspace shrinks logically without hiding its backing");
    assert_eq!(
        engine.sampling_workspace_usage().retained_bytes,
        capacity_bytes
    );

    let hook = Arc::new(SpatialWorkspaceAllocationTestHook::new(false));
    assert!(engine.install_sampling_workspace_allocation_test_hook(Arc::clone(&hook)));
    let timeout = Duration::from_secs(5);
    let (first_reached, rejected, first_result) = std::thread::scope(|scope| {
        let first_engine = Arc::clone(&engine);
        let first = scope.spawn(move || first_engine.try_prepare_sampling_canvas(4, 4));
        let first_reached = hook.wait_for_first_reservation(timeout);
        let rejected = engine.try_prepare_sampling_canvas(3, 3);
        hook.release_first_allocation();
        let first_result = first
            .join()
            .expect("replacement preparation thread completes");
        (first_reached, rejected, first_result)
    });

    assert!(first_reached);
    assert_eq!(first_result, Ok(()));
    assert_eq!(
        rejected,
        Err(SpatialSamplingError::AreaWorkspaceCapacityExceeded {
            width: 3,
            height: 3,
            required_bytes: capacity_bytes + workspace_bytes(3, 3),
            capacity_bytes,
        })
    );
    assert_eq!(
        engine.sampling_workspace_usage(),
        SpatialSamplingWorkspaceUsage {
            retained_workspaces: 1,
            retained_bytes: capacity_bytes,
            reserved_workspaces: 0,
            reserved_bytes: 0,
        }
    );
}

#[cfg(feature = "spatial-workspace-test-hooks")]
#[test]
fn failed_allocation_wakes_waiter_and_cleans_the_reservation() {
    let (engine, hook) = hooked_engine(true);
    let timeout = Duration::from_secs(5);
    let (first_reached, waiter_reached, results) = std::thread::scope(|scope| {
        let first_engine = Arc::clone(&engine);
        let first = scope.spawn(move || first_engine.try_prepare_sampling_canvas(8, 8));
        let first_reached = hook.wait_for_first_reservation(timeout);

        let second_engine = Arc::clone(&engine);
        let second = scope.spawn(move || second_engine.try_prepare_sampling_canvas(8, 8));
        let waiter_reached = hook.wait_for_waiters(1, timeout);
        hook.release_first_allocation();
        let results = [
            first.join().expect("first preparation thread completes"),
            second.join().expect("second preparation thread completes"),
        ];
        (first_reached, waiter_reached, results)
    });

    assert!(first_reached);
    assert!(waiter_reached);
    assert_eq!(
        results[0],
        Err(SpatialSamplingError::AreaWorkspaceAllocation {
            width: 8,
            height: 8,
            entry_count: 9 * 9,
        })
    );
    assert_eq!(results[1], Ok(()));
    assert_eq!(hook.allocation_attempts(), 2);
    assert_eq!(hook.waiter_count(), 1);
    assert_eq!(
        engine.sampling_workspace_usage(),
        SpatialSamplingWorkspaceUsage {
            retained_workspaces: 1,
            retained_bytes: workspace_bytes(8, 8),
            reserved_workspaces: 0,
            reserved_bytes: 0,
        }
    );
}
