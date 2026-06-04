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
