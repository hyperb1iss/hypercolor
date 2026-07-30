use super::super::*;

#[test]
fn gpu_sampler_matches_cpu_spatial_sampling_for_bilinear_plans() {
    let mut compositor = match GpuSparkleFlinger::new() {
        Ok(compositor) => compositor,
        Err(_) => return,
    };
    let engine = SpatialEngine::new(sampling_layout(SamplingMode::Bilinear));
    let plan = CompositionPlan::with_layers(
        4,
        4,
        vec![
            CompositionLayer::replace(ProducerFrame::Canvas(solid_canvas(Rgba::new(
                255, 32, 0, 255,
            )))),
            CompositionLayer::alpha(
                ProducerFrame::Canvas(solid_canvas(Rgba::new(32, 64, 255, 255))),
                0.35,
            ),
        ],
    );
    let expected =
        CpuSparkleFlinger::new().compose(plan.clone(), true, full_preview_request(&plan));
    let expected_zones = engine.sample(
        expected
            .sampling_canvas
            .as_ref()
            .expect("CPU compose should materialize a canvas"),
    );
    compositor
        .compose(&plan, false, None)
        .expect("GPU composition should succeed before GPU sampling");
    let mut sampled = Vec::new();
    assert!(
        compositor
            .sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut sampled)
            .expect("GPU spatial sampling should succeed")
    );

    assert_zone_colors_within(&sampled, &expected_zones, 1);
}

#[test]
fn gpu_sampler_matches_cpu_spatial_sampling_with_fade_edges() {
    let mut compositor = match GpuSparkleFlinger::new() {
        Ok(compositor) => compositor,
        Err(_) => return,
    };
    let engine = SpatialEngine::new(fade_sampling_layout(SamplingMode::Bilinear));
    let plan = CompositionPlan::with_layers(
        4,
        4,
        vec![
            CompositionLayer::replace(ProducerFrame::Canvas(solid_canvas(Rgba::new(
                255, 32, 0, 255,
            )))),
            CompositionLayer::alpha(
                ProducerFrame::Canvas(solid_canvas(Rgba::new(32, 64, 255, 255))),
                0.35,
            ),
        ],
    );
    let expected =
        CpuSparkleFlinger::new().compose(plan.clone(), true, full_preview_request(&plan));
    let expected_zones = engine.sample(
        expected
            .sampling_canvas
            .as_ref()
            .expect("CPU compose should materialize a canvas"),
    );

    compositor
        .compose(&plan, false, None)
        .expect("GPU composition should succeed before GPU fade sampling");
    let mut sampled = Vec::new();
    assert!(
        compositor
            .sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut sampled)
            .expect("GPU spatial sampling should support prepared attenuation")
    );

    assert_zone_colors_within(&sampled, &expected_zones, 1);
}

#[test]
fn gpu_sampling_matches_cpu_after_canvas_resize() {
    let mut compositor = match GpuSparkleFlinger::new() {
        Ok(compositor) => compositor,
        Err(_) => return,
    };
    let engine = SpatialEngine::new(sampling_layout(SamplingMode::Bilinear));
    let plan = CompositionPlan::single(
        8,
        4,
        CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas_with_size(8, 4, 21))),
    );
    let expected =
        CpuSparkleFlinger::new().compose(plan.clone(), true, full_preview_request(&plan));
    let expected_zones = engine.sample(
        expected
            .sampling_canvas
            .as_ref()
            .expect("CPU compose should materialize resized canvas"),
    );

    compositor
        .compose(&plan, false, None)
        .expect("GPU composition should succeed before resized sampling");
    let mut sampled = Vec::new();
    assert!(
        compositor
            .sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut sampled)
            .expect("GPU sampling should succeed for resized canvas")
    );

    assert_zone_colors_within(&sampled, &expected_zones, 1);
}

#[test]
fn gpu_sampler_rejects_gaussian_plans_without_dispatch() {
    let mut compositor = match GpuSparkleFlinger::new() {
        Ok(compositor) => compositor,
        Err(_) => return,
    };
    let engine = SpatialEngine::new(sampling_layout(SamplingMode::GaussianArea {
        sigma: 1.0,
        radius: 2,
    }));
    let plan = CompositionPlan::single(
        4,
        4,
        CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(21))),
    );

    assert!(!compositor.can_sample_zone_plan(engine.sampling_plan().as_ref()));
    compositor
        .compose(&plan, false, None)
        .expect("GPU composition should still succeed before Gaussian fallback");
    assert!(matches!(
        compositor
            .begin_sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut Vec::new())
            .expect("unsupported GPU sampling mode should be non-fatal"),
        GpuZoneSamplingDispatch::Unsupported
    ));
    assert_eq!(compositor.spatial_sampler.sample_dispatch_count(), 0);
}

#[test]
fn gpu_sampler_matches_cpu_spatial_sampling_for_area_plans() {
    let mut compositor = match GpuSparkleFlinger::new() {
        Ok(compositor) => compositor,
        Err(_) => return,
    };
    let engine = SpatialEngine::new(sampling_layout(SamplingMode::AreaAverage {
        radius_x: 1.0,
        radius_y: 1.0,
    }));
    let plan = CompositionPlan::with_layers(
        4,
        4,
        vec![
            CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(12))),
            CompositionLayer::screen(ProducerFrame::Canvas(patterned_canvas(96)), 0.6),
        ],
    );
    let expected =
        CpuSparkleFlinger::new().compose(plan.clone(), true, full_preview_request(&plan));
    let expected_zones = engine.sample(
        expected
            .sampling_canvas
            .as_ref()
            .expect("CPU compose should materialize a canvas"),
    );
    compositor
        .compose(&plan, false, None)
        .expect("GPU composition should succeed before GPU area sampling");
    let mut sampled = Vec::new();
    assert!(
        compositor
            .sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut sampled)
            .expect("GPU sampler should support area plans")
    );
    assert_zone_colors_within(&sampled, &expected_zones, 1);
}

#[test]
fn gpu_area_sampling_leaves_cpu_workspace_unallocated() {
    let mut compositor = match GpuSparkleFlinger::new() {
        Ok(compositor) => compositor,
        Err(_) => return,
    };
    let engine = SpatialEngine::new(sampling_layout(SamplingMode::AreaAverage {
        radius_x: 2.0,
        radius_y: 1.0,
    }));
    assert_eq!(
        engine.sampling_workspace_usage(),
        hypercolor_core::spatial::SpatialSamplingWorkspaceUsage::default()
    );

    let plan = CompositionPlan::single(
        4,
        4,
        CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(27))),
    );
    assert!(compositor.can_sample_zone_plan(engine.sampling_plan().as_ref()));
    compositor
        .compose(&plan, false, None)
        .expect("GPU composition should succeed before resident area sampling");
    assert!(
        compositor
            .sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut Vec::new())
            .expect("GPU area sampling should remain resident")
    );

    assert_eq!(
        engine.sampling_workspace_usage(),
        hypercolor_core::spatial::SpatialSamplingWorkspaceUsage::default()
    );
}

#[test]
fn gpu_area_sampling_carries_across_u32_prefix_limb() {
    let mut compositor = match GpuSparkleFlinger::new() {
        Ok(compositor) => compositor,
        Err(_) => return,
    };
    let mut layout = sampling_layout(SamplingMode::AreaAverage {
        radius_x: 128.0,
        radius_y: 128.0,
    });
    layout.canvas_width = 257;
    layout.canvas_height = 257;
    let engine = SpatialEngine::new(layout);
    let mut canvas = Canvas::new(257, 257);
    canvas.fill(Rgba::new(255, 255, 255, 255));
    let expected_zones = engine.sample(&canvas);
    let plan = CompositionPlan::single(
        257,
        257,
        CompositionLayer::replace(ProducerFrame::Canvas(canvas)),
    );

    compositor
        .compose(&plan, false, None)
        .expect("GPU composition should cross the area scan tile boundary");
    let mut sampled = Vec::new();
    assert!(
        compositor
            .sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut sampled)
            .expect("GPU area sampling should carry into the high prefix limb")
    );

    assert_zone_colors_within(&sampled, &expected_zones, 0);
}

#[test]
fn gpu_sampler_matches_cpu_area_sampling_with_fade_edges() {
    let mut compositor = match GpuSparkleFlinger::new() {
        Ok(compositor) => compositor,
        Err(_) => return,
    };
    let engine = SpatialEngine::new(fade_sampling_layout(SamplingMode::AreaAverage {
        radius_x: 2.0,
        radius_y: 1.0,
    }));
    let canvas = patterned_canvas(63);
    let plan = CompositionPlan::single(
        4,
        4,
        CompositionLayer::replace(ProducerFrame::Canvas(canvas.clone())),
    );
    let expected_zones = engine.sample(&canvas);

    compositor
        .compose(&plan, false, None)
        .expect("GPU composition should succeed before faded area sampling");
    let mut sampled = Vec::new();
    assert!(
        compositor
            .sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut sampled)
            .expect("GPU sampler should support attenuated area plans")
    );

    assert_zone_colors_within(&sampled, &expected_zones, 1);
}

#[test]
fn gpu_sampler_matches_cpu_for_anisotropic_area_radii() {
    let mut compositor = match GpuSparkleFlinger::new() {
        Ok(compositor) => compositor,
        Err(_) => return,
    };
    let engine = SpatialEngine::new(sampling_layout(SamplingMode::AreaAverage {
        radius_x: 3.0,
        radius_y: 1.0,
    }));
    let canvas = patterned_canvas(37);
    let plan = CompositionPlan::single(
        4,
        4,
        CompositionLayer::replace(ProducerFrame::Canvas(canvas.clone())),
    );
    let expected_zones = engine.sample(&canvas);

    compositor
        .compose(&plan, false, None)
        .expect("GPU composition should succeed before anisotropic area sampling");
    let mut sampled = Vec::new();
    assert!(
        compositor
            .sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut sampled)
            .expect("GPU sampler should support anisotropic area plans")
    );

    assert_zone_colors_within(&sampled, &expected_zones, 1);
}

#[test]
fn gpu_sampler_matches_cpu_for_area_radius_above_u16_at_clamped_borders() {
    let mut compositor = match GpuSparkleFlinger::new() {
        Ok(compositor) => compositor,
        Err(_) => return,
    };
    let engine = SpatialEngine::new(sampling_layout(SamplingMode::AreaAverage {
        radius_x: 65_536.0,
        radius_y: 1.0,
    }));
    let canvas = patterned_canvas(91);
    let plan = CompositionPlan::single(
        4,
        4,
        CompositionLayer::replace(ProducerFrame::Canvas(canvas.clone())),
    );
    let expected_zones = engine.sample(&canvas);

    compositor
        .compose(&plan, false, None)
        .expect("GPU composition should succeed before large-radius area sampling");
    let mut sampled = Vec::new();
    assert!(
        compositor
            .sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut sampled)
            .expect("GPU sampler should preserve radii above u16")
    );

    assert_zone_colors_within(&sampled, &expected_zones, 1);
}

#[test]
fn gpu_sampling_admission_rolls_back_retries_and_reuses_resources() {
    let mut compositor = match GpuSparkleFlinger::new() {
        Ok(compositor) => compositor,
        Err(_) => return,
    };
    let initial = SpatialEngine::new(sampling_layout(SamplingMode::AreaAverage {
        radius_x: 1.0,
        radius_y: 1.0,
    }));
    assert!(compositor.can_sample_zone_plan(initial.sampling_plan().as_ref()));
    let initial_area_generation = compositor.spatial_sampler.area_generation();
    let initial_buffer_generation = compositor.spatial_sampler.buffer_generation();

    let mut replacement_layout = sampling_layout(SamplingMode::AreaAverage {
        radius_x: 4.0,
        radius_y: 2.0,
    });
    replacement_layout.canvas_width = 8;
    let replacement = SpatialEngine::new(replacement_layout);
    compositor.spatial_sampler.fail_next_plan_preparation();
    assert!(!compositor.can_sample_zone_plan(replacement.sampling_plan().as_ref()));
    assert!(compositor.spatial_sampler.has_transient_retry());
    assert!(!compositor.spatial_sampler.has_deterministic_fallback());
    assert_eq!(
        compositor.spatial_sampler.area_generation(),
        initial_area_generation
    );
    assert_eq!(
        compositor.spatial_sampler.buffer_generation(),
        initial_buffer_generation
    );
    assert!(!compositor.can_sample_zone_plan(replacement.sampling_plan().as_ref()));

    compositor.spatial_sampler.make_transient_retry_due();
    assert!(compositor.can_sample_zone_plan(replacement.sampling_plan().as_ref()));
    assert!(!compositor.spatial_sampler.has_transient_retry());
    let retried_area_generation = compositor.spatial_sampler.area_generation();
    let retried_buffer_generation = compositor.spatial_sampler.buffer_generation();
    assert!(retried_area_generation > initial_area_generation);
    assert_eq!(retried_buffer_generation, initial_buffer_generation);

    assert!(compositor.can_sample_zone_plan(replacement.sampling_plan().as_ref()));
    assert_eq!(
        compositor.spatial_sampler.area_generation(),
        retried_area_generation
    );
    assert_eq!(
        compositor.spatial_sampler.buffer_generation(),
        retried_buffer_generation
    );
}

#[test]
fn deterministic_gpu_sampling_limits_are_cached_without_retrying() {
    let mut compositor = match GpuSparkleFlinger::new() {
        Ok(compositor) => compositor,
        Err(_) => return,
    };
    let width = compositor
        .device
        .limits()
        .max_texture_dimension_2d
        .checked_add(1)
        .expect("texture limit should leave room for an oversized test descriptor");
    let mut layout = sampling_layout(SamplingMode::AreaAverage {
        radius_x: 1.0,
        radius_y: 1.0,
    });
    layout.canvas_width = width;
    layout.canvas_height = 1;
    let engine = SpatialEngine::new(layout);

    assert!(!compositor.can_sample_zone_plan(engine.sampling_plan().as_ref()));
    assert!(compositor.spatial_sampler.has_deterministic_fallback());
    assert!(!compositor.spatial_sampler.has_transient_retry());
    assert!(!compositor.can_sample_zone_plan(engine.sampling_plan().as_ref()));
    assert!(compositor.spatial_sampler.has_deterministic_fallback());
}

#[test]
fn gpu_sampler_matches_cpu_when_negative_area_radius_clamps_to_zero() {
    let mut compositor = match GpuSparkleFlinger::new() {
        Ok(compositor) => compositor,
        Err(_) => return,
    };
    let engine = SpatialEngine::new(sampling_layout(SamplingMode::AreaAverage {
        radius_x: -1.0,
        radius_y: -1.0,
    }));
    let prepared = engine.sampling_plan();
    let hypercolor_core::spatial::PreparedZoneSamples::Area(samples) =
        &prepared[0].prepared_samples
    else {
        panic!("negative area radius should remain an area-sampling plan");
    };
    assert!(
        samples
            .iter()
            .all(|sample| sample.radius_x == 0 && sample.radius_y == 0)
    );
    assert!(compositor.can_sample_zone_plan(prepared.as_ref()));

    let plan = CompositionPlan::with_layers(
        4,
        4,
        vec![
            CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(12))),
            CompositionLayer::screen(ProducerFrame::Canvas(patterned_canvas(96)), 0.6),
        ],
    );
    let expected =
        CpuSparkleFlinger::new().compose(plan.clone(), true, full_preview_request(&plan));
    let expected_zones = engine.sample(
        expected
            .sampling_canvas
            .as_ref()
            .expect("CPU compose should materialize a canvas"),
    );

    compositor
        .compose(&plan, false, None)
        .expect("GPU composition should succeed before clamped area sampling");
    let mut sampled = Vec::new();
    assert!(
        compositor
            .sample_zone_plan_into(prepared.as_ref(), &mut sampled)
            .expect("GPU sampler should accept the clamped area plan")
    );
    assert_zone_colors_within(&sampled, &expected_zones, 1);
}
