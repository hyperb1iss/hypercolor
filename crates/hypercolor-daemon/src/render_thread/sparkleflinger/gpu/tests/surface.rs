use super::*;

#[test]
fn gpu_compositor_reuses_matching_surface_sizes() {
    let mut compositor = match GpuSparkleFlinger::new() {
        Ok(compositor) => compositor,
        Err(_) => return,
    };

    compositor.ensure_surface_size(640, 480);
    let first = compositor
        .surface_snapshot()
        .expect("surface allocation should publish a snapshot");
    compositor.ensure_surface_size(640, 480);
    let second = compositor
        .surface_snapshot()
        .expect("surface snapshot should remain available");

    assert_eq!(first, second);
}

#[test]
fn gpu_resize_clears_ready_preview_surface() {
    let mut compositor = match GpuSparkleFlinger::new() {
        Ok(compositor) => compositor,
        Err(_) => return,
    };

    compositor.ready_preview_surface = Some(PublishedSurface::from_owned_canvas(
        solid_canvas_with_size(4, 4, Rgba::new(12, 34, 56, 255)),
        0,
        0,
    ));

    compositor.ensure_surface_size(8, 8);

    assert!(compositor.ready_preview_surface.is_none());
}

#[test]
fn gpu_resize_clears_sampling_bind_groups() {
    let mut compositor = match GpuSparkleFlinger::new() {
        Ok(compositor) => compositor,
        Err(_) => return,
    };
    let engine = SpatialEngine::new(sampling_layout(SamplingMode::Bilinear));
    let plan = CompositionPlan::single(
        4,
        4,
        CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(3))),
    );

    compositor
        .compose(&plan, false, None)
        .expect("GPU composition should succeed before sampling");
    let mut sampled = Vec::new();
    let _pending = compositor
        .begin_sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut sampled)
        .expect("GPU sample dispatch should succeed");

    assert!(compositor.spatial_sampler.cached_bind_group_count() > 0);

    compositor.ensure_surface_size(8, 8);

    assert_eq!(compositor.spatial_sampler.cached_bind_group_count(), 0);
}
