use super::*;

#[test]
fn gpu_sampler_arms_preview_map_after_sampling_completion() {
    let Some(mut compositor) = super::gpu_test_compositor() else {
        return;
    };
    let engine = SpatialEngine::new(sampling_layout(SamplingMode::Bilinear));
    let plan = CompositionPlan::with_layers(
        4,
        4,
        vec![
            CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(12))),
            CompositionLayer::alpha(ProducerFrame::Canvas(patterned_canvas(96)), 0.35),
        ],
    );

    let composed = compositor
        .compose(
            &plan,
            false,
            Some(PreviewSurfaceRequest {
                width: 2,
                height: 2,
            }),
        )
        .expect("GPU composition should stage a scaled preview surface");
    assert!(composed.preview_surface.is_none());

    let mut sampled = Vec::new();
    assert!(
        compositor
            .sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut sampled)
            .expect("GPU zone sampling should succeed")
    );
    assert!(compositor.ready_preview_surface.is_none());
    assert!(compositor.pending_preview_readback().is_none());
    assert!(compositor.pending_preview_submission().is_none());
    assert!(compositor.pending_preview_map.is_some());

    let preview_surface = resolve_preview_surface_blocking(&mut compositor);
    assert_eq!(preview_surface.width(), 2);
    assert_eq!(preview_surface.height(), 2);
    assert!(compositor.pending_preview_readback().is_none());
    assert!(compositor.pending_preview_submission().is_none());
    assert!(compositor.pending_preview_map.is_none());
}

#[test]
fn gpu_sampler_preserves_mapped_preview_when_new_sample_submits() {
    let Some(mut compositor) = super::gpu_test_compositor() else {
        return;
    };
    let engine = SpatialEngine::new(sampling_layout(SamplingMode::Bilinear));
    let first_plan = CompositionPlan::with_layers(
        4,
        4,
        vec![
            CompositionLayer::replace(ProducerFrame::Canvas(solid_canvas(Rgba::new(
                255, 32, 0, 255,
            )))),
            CompositionLayer::alpha(
                ProducerFrame::Canvas(solid_canvas(Rgba::new(0, 0, 0, 0))),
                0.0,
            ),
        ],
    );
    let second_plan = CompositionPlan::with_layers(
        4,
        4,
        vec![
            CompositionLayer::replace(ProducerFrame::Canvas(solid_canvas(Rgba::new(
                32, 64, 255, 255,
            )))),
            CompositionLayer::alpha(
                ProducerFrame::Canvas(solid_canvas(Rgba::new(0, 0, 0, 0))),
                0.0,
            ),
        ],
    );
    let request = PreviewSurfaceRequest {
        width: 2,
        height: 2,
    };

    compositor
        .compose(&first_plan, false, Some(request))
        .expect("first compose should stage a preview surface");
    compositor
        .submit_pending_preview_work()
        .expect("first preview submit should succeed");
    defer_pending_preview_map(&mut compositor);

    let first_slot = match compositor.pending_preview_map.as_ref() {
        Some(PendingPreviewMap {
            readback: PendingPreviewReadback::PreviewBuffer { slot, .. },
            ..
        }) => *slot,
        _ => panic!("first preview should be waiting on a preview-buffer map"),
    };

    compositor
        .compose(&second_plan, false, Some(request))
        .expect("second compose should stage a newer preview surface");

    let mut sampled = Vec::new();
    assert!(
        compositor
            .sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut sampled)
            .expect("GPU zone sampling should submit the newer preview work")
    );

    let mapped_slot = match compositor.pending_preview_map.as_ref() {
        Some(PendingPreviewMap {
            readback: PendingPreviewReadback::PreviewBuffer { slot, .. },
            ..
        }) => *slot,
        _ => panic!("first preview map should still be pending"),
    };
    assert_eq!(mapped_slot, first_slot);
    assert!(compositor.pending_preview_readback().is_some());
    assert!(compositor.pending_preview_submission().is_some());

    let first_preview = resolve_preview_surface_blocking(&mut compositor);
    assert_eq!(&first_preview.rgba_bytes()[0..4], &[255, 32, 0, 255]);

    let second_preview = resolve_preview_surface_blocking(&mut compositor);
    assert_eq!(&second_preview.rgba_bytes()[0..4], &[32, 64, 255, 255]);
}

#[test]
fn gpu_zero_sample_plan_keeps_pending_preview_work() {
    let Some(mut compositor) = super::gpu_test_compositor() else {
        return;
    };
    let engine = SpatialEngine::new(sampling_layout_with_led_count(SamplingMode::Bilinear, 0));
    let plan = CompositionPlan::with_layers(
        4,
        4,
        vec![
            CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(12))),
            CompositionLayer::alpha(ProducerFrame::Canvas(patterned_canvas(96)), 0.35),
        ],
    );

    let composed = compositor
        .compose(
            &plan,
            false,
            Some(PreviewSurfaceRequest {
                width: 2,
                height: 2,
            }),
        )
        .expect("GPU composition should stage a scaled preview surface");
    assert!(composed.preview_surface.is_none());

    let mut sampled = Vec::new();
    assert!(
        compositor
            .sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut sampled)
            .expect("GPU zone sampling should succeed for empty plans")
    );
    assert!(sampled.is_empty());
    assert!(compositor.pending_preview_readback().is_none());
    assert!(compositor.pending_preview_submission().is_none());
    assert!(compositor.pending_preview_map.is_some());

    let preview_surface = resolve_preview_surface_blocking(&mut compositor);
    assert_eq!(preview_surface.width(), 2);
    assert_eq!(preview_surface.height(), 2);
}

#[test]
fn gpu_compositor_bypassed_canvas_shares_sampling_surface_storage() {
    let Some(mut compositor) = super::gpu_test_compositor() else {
        return;
    };
    let plan = CompositionPlan::single(
        4,
        4,
        CompositionLayer::replace(ProducerFrame::Canvas(solid_canvas(Rgba::new(
            24, 88, 160, 255,
        )))),
    );

    let composed = compositor
        .compose(&plan, true, None)
        .expect("single replace canvas should bypass on the GPU");
    let sampling_surface = composed
        .sampling_surface
        .as_ref()
        .expect("bypassed canvas should publish a sampling surface");
    let sampling_canvas = composed
        .sampling_canvas
        .as_ref()
        .expect("bypassed canvas should materialize a canvas view");

    assert_eq!(
        sampling_canvas.as_rgba_bytes().as_ptr(),
        sampling_surface.rgba_bytes().as_ptr()
    );
    assert!(composed.preview_surface.is_none());
    assert!(composed.bypassed);
}

#[test]
fn gpu_compositor_reuses_cached_shared_canvas_bypass_surfaces() {
    let Some(mut compositor) = super::gpu_test_compositor() else {
        return;
    };
    let canvas = solid_canvas(Rgba::new(24, 88, 160, 255));
    let plan = CompositionPlan::single(
        4,
        4,
        CompositionLayer::replace(ProducerFrame::Canvas(canvas.clone())),
    );

    let first = compositor
        .compose(&plan, true, full_preview_request(&plan))
        .expect("initial shared canvas bypass should succeed");
    let first_surface = first
        .sampling_surface
        .as_ref()
        .expect("bypassed shared canvas should publish a sampling surface");
    let first_ptr = first_surface.rgba_bytes().as_ptr();
    let first_upload_count = compositor
        .surfaces
        .as_ref()
        .expect("surface allocation should exist after bypass")
        .front_upload_count;

    let second = compositor
        .compose(&plan, true, full_preview_request(&plan))
        .expect("cached shared canvas bypass should succeed");
    let second_surface = second
        .sampling_surface
        .as_ref()
        .expect("cached bypass should still publish a sampling surface");
    let second_upload_count = compositor
        .surfaces
        .as_ref()
        .expect("surface allocation should persist across bypasses")
        .front_upload_count;

    assert_eq!(second_surface.rgba_bytes().as_ptr(), first_ptr);
    assert_eq!(second_upload_count, first_upload_count);
    assert!(second.bypassed);
}

#[test]
fn gpu_compositor_reuses_cached_unique_canvas_bypass_surfaces_on_second_frame() {
    let Some(mut compositor) = super::gpu_test_compositor() else {
        return;
    };
    let plan = CompositionPlan::single(
        4,
        4,
        CompositionLayer::replace(ProducerFrame::Canvas(solid_canvas(Rgba::new(
            24, 88, 160, 255,
        )))),
    );

    let first = compositor
        .compose(&plan, true, full_preview_request(&plan))
        .expect("initial unique canvas bypass should succeed");
    let first_surface = first
        .sampling_surface
        .as_ref()
        .expect("bypassed unique canvas should publish a sampling surface");
    let first_ptr = first_surface.rgba_bytes().as_ptr();
    let first_upload_count = compositor
        .surfaces
        .as_ref()
        .expect("surface allocation should exist after bypass")
        .front_upload_count;

    let second = compositor
        .compose(&plan, true, full_preview_request(&plan))
        .expect("second unique canvas bypass should reuse the cached surface");
    let second_surface = second
        .sampling_surface
        .as_ref()
        .expect("cached unique bypass should still publish a sampling surface");
    let second_upload_count = compositor
        .surfaces
        .as_ref()
        .expect("surface allocation should persist across bypasses")
        .front_upload_count;

    assert_eq!(second_surface.rgba_bytes().as_ptr(), first_ptr);
    assert_eq!(second_upload_count, first_upload_count);
    assert!(second.bypassed);
}

#[test]
fn gpu_compositor_reuses_cached_slot_backed_frame_uploads() {
    let Some(mut compositor) = super::gpu_test_compositor() else {
        return;
    };
    let engine = SpatialEngine::new(sampling_layout(SamplingMode::Bilinear));
    let retained_base = slot_surface(Rgba::new(255, 32, 0, 255));
    let retained_overlay = slot_surface(Rgba::new(32, 64, 255, 255));
    let plan = CompositionPlan::with_layers(
        4,
        4,
        vec![
            CompositionLayer::replace(ProducerFrame::Surface(retained_base)),
            CompositionLayer::alpha(ProducerFrame::Surface(retained_overlay), 0.35),
        ],
    );

    compositor
        .compose(&plan, false, None)
        .expect("initial GPU composition should succeed");
    let first_upload_count = compositor
        .surfaces
        .as_ref()
        .expect("surface allocation should exist after composition")
        .source_upload_count;
    let first_front_upload_count = compositor
        .surfaces
        .as_ref()
        .expect("surface allocation should exist after composition")
        .front_upload_count;
    let first_compose_dispatch_count = compositor
        .surfaces
        .as_ref()
        .expect("surface allocation should exist after composition")
        .compose_dispatch_count;

    compositor
        .compose(&plan, false, None)
        .expect("cached GPU composition should succeed");
    let second_upload_count = compositor
        .surfaces
        .as_ref()
        .expect("surface allocation should persist across compositions")
        .source_upload_count;
    let second_front_upload_count = compositor
        .surfaces
        .as_ref()
        .expect("surface allocation should persist across compositions")
        .front_upload_count;
    let second_compose_dispatch_count = compositor
        .surfaces
        .as_ref()
        .expect("surface allocation should persist across compositions")
        .compose_dispatch_count;

    assert_eq!(first_upload_count, 1);
    assert_eq!(second_upload_count, first_upload_count);
    assert_eq!(first_front_upload_count, 1);
    assert_eq!(second_front_upload_count, first_front_upload_count);
    assert_eq!(first_compose_dispatch_count, 1);
    assert_eq!(second_compose_dispatch_count, first_compose_dispatch_count);

    let expected =
        CpuSparkleFlinger::new().compose(plan.clone(), true, full_preview_request(&plan));
    let mut sampled = Vec::new();
    assert!(
        compositor
            .sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut sampled)
            .expect("cached no-readback composition should remain sampleable")
    );
    assert_zone_colors_within(
        &sampled,
        &engine.sample(
            expected
                .sampling_canvas
                .as_ref()
                .expect("CPU compose should materialize a canvas"),
        ),
        1,
    );
}

#[test]
fn gpu_compositor_reuses_compose_params_for_same_alpha_shape() {
    let Some(mut compositor) = super::gpu_test_compositor() else {
        return;
    };
    let first_plan = CompositionPlan::with_layers(
        4,
        4,
        vec![
            CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(12))),
            CompositionLayer::alpha(ProducerFrame::Canvas(patterned_canvas(96)), 0.35),
        ],
    );
    let second_plan = CompositionPlan::with_layers(
        4,
        4,
        vec![
            CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(44))),
            CompositionLayer::alpha(ProducerFrame::Canvas(patterned_canvas(180)), 0.35),
        ],
    );

    compositor
        .compose(&first_plan, false, None)
        .expect("first GPU composition should succeed");
    assert_eq!(
        compositor
            .surfaces
            .as_ref()
            .expect("surface allocation should exist after composition")
            .compose_param_write_count,
        1
    );

    compositor
        .compose(&second_plan, false, None)
        .expect("second GPU composition should succeed");
    assert_eq!(
        compositor
            .surfaces
            .as_ref()
            .expect("surface allocation should persist across compositions")
            .compose_param_write_count,
        1
    );
}

#[test]
fn gpu_sampler_reuses_zone_output_storage() {
    let Some(mut compositor) = super::gpu_test_compositor() else {
        return;
    };
    let engine = SpatialEngine::new(sampling_layout(SamplingMode::Bilinear));
    let plan = CompositionPlan::with_layers(
        4,
        4,
        vec![
            CompositionLayer::replace(ProducerFrame::Canvas(solid_canvas(Rgba::new(
                24, 88, 160, 255,
            )))),
            CompositionLayer::alpha(
                ProducerFrame::Canvas(solid_canvas(Rgba::new(220, 48, 24, 255))),
                0.25,
            ),
        ],
    );
    let expected =
        CpuSparkleFlinger::new().compose(plan.clone(), true, full_preview_request(&plan));
    compositor
        .compose(&plan, false, None)
        .expect("GPU composition should succeed before output reuse testing");

    let mut sampled = vec![ZoneColors {
        zone_id: "stale".into(),
        colors: vec![[0_u8; 3]; 8],
    }];
    let first_colors_ptr = sampled[0].colors.as_ptr();
    assert!(
        compositor
            .sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut sampled)
            .expect("GPU spatial sampling should succeed for bilinear plans")
    );

    assert_eq!(
        sampled,
        engine.sample(
            expected
                .sampling_canvas
                .as_ref()
                .expect("CPU compose should materialize a canvas"),
        )
    );
    assert_eq!(sampled[0].colors.as_ptr(), first_colors_ptr);
}

#[test]
fn gpu_sampler_reuses_cached_zone_results_for_identical_retained_surfaces() {
    let Some(mut compositor) = super::gpu_test_compositor() else {
        return;
    };
    let engine = SpatialEngine::new(sampling_layout(SamplingMode::Bilinear));
    let retained_base = slot_surface(Rgba::new(255, 32, 0, 255));
    let retained_overlay = slot_surface(Rgba::new(32, 64, 255, 255));
    let plan = CompositionPlan::with_layers(
        4,
        4,
        vec![
            CompositionLayer::replace(ProducerFrame::Surface(retained_base)),
            CompositionLayer::alpha(ProducerFrame::Surface(retained_overlay), 0.35),
        ],
    );

    compositor
        .compose(&plan, false, None)
        .expect("initial GPU composition should succeed");
    let mut first_sample = Vec::new();
    assert!(
        compositor
            .sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut first_sample)
            .expect("initial GPU sample should succeed")
    );
    let first_dispatch_count = compositor.spatial_sampler.sample_dispatch_count();

    compositor
        .compose(&plan, false, None)
        .expect("cached GPU composition should succeed");
    let mut second_sample = Vec::new();
    assert!(
        compositor
            .sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut second_sample)
            .expect("cached GPU sample should succeed")
    );

    assert_eq!(second_sample, first_sample);
    assert_eq!(
        compositor.spatial_sampler.sample_dispatch_count(),
        first_dispatch_count
    );
}

#[test]
fn gpu_cached_sample_hit_preserves_retained_preview_submission() {
    let Some(mut compositor) = super::gpu_test_compositor() else {
        return;
    };
    let engine = SpatialEngine::new(sampling_layout(SamplingMode::Bilinear));
    let retained_base = slot_surface(Rgba::new(255, 32, 0, 255));
    let retained_overlay = slot_surface(Rgba::new(32, 64, 255, 255));
    let plan = CompositionPlan::with_layers(
        4,
        4,
        vec![
            CompositionLayer::replace(ProducerFrame::Surface(retained_base)),
            CompositionLayer::alpha(ProducerFrame::Surface(retained_overlay), 0.35),
        ],
    );

    compositor
        .compose(&plan, false, None)
        .expect("initial GPU composition should succeed");
    let mut first_sample = Vec::new();
    assert!(
        compositor
            .sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut first_sample)
            .expect("initial GPU sample should succeed")
    );

    let composed = compositor
        .compose(
            &plan,
            false,
            Some(PreviewSurfaceRequest {
                width: 2,
                height: 2,
            }),
        )
        .expect("retained GPU composition should stage a scaled preview");
    assert!(composed.preview_surface.is_none());
    assert!(compositor.has_pending_output_submission());
    assert!(compositor.pending_preview_readback().is_some());

    let mut cached_sample = Vec::new();
    match compositor
        .begin_sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut cached_sample)
        .expect("cached GPU sample dispatch should succeed")
    {
        GpuZoneSamplingDispatch::Ready => {}
        _ => panic!("cached GPU sample should reuse the retained result"),
    }

    assert_eq!(cached_sample, first_sample);
    assert!(compositor.has_pending_output_submission());
    assert!(compositor.pending_preview_readback().is_some());

    compositor
        .submit_pending_preview_work()
        .expect("preview submit should still succeed after cached sample reuse");
    let preview_surface = resolve_preview_surface_blocking(&mut compositor);
    assert_eq!(preview_surface.width(), 2);
    assert_eq!(preview_surface.height(), 2);
}

#[test]
fn gpu_sampler_caches_bind_groups_by_output_surface() {
    let Some(mut compositor) = super::gpu_test_compositor() else {
        return;
    };
    let engine = SpatialEngine::new(sampling_layout(SamplingMode::Bilinear));
    let back_output_plan = CompositionPlan::with_layers(
        4,
        4,
        vec![
            CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(7))),
            CompositionLayer::alpha(ProducerFrame::Canvas(patterned_canvas(41)), 0.35),
        ],
    );
    let front_output_plan = CompositionPlan::with_layers(
        4,
        4,
        vec![
            CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(11))),
            CompositionLayer::alpha(ProducerFrame::Canvas(patterned_canvas(53)), 0.35),
            CompositionLayer::alpha(ProducerFrame::Canvas(patterned_canvas(97)), 0.2),
        ],
    );

    compositor
        .compose(&back_output_plan, false, None)
        .expect("back-output GPU composition should succeed");
    assert!(
        compositor
            .sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut Vec::new())
            .expect("back-output GPU sample should succeed")
    );
    assert_eq!(compositor.spatial_sampler.cached_bind_group_count(), 1);

    compositor
        .compose(&front_output_plan, false, None)
        .expect("front-output GPU composition should succeed");
    assert!(
        compositor
            .sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut Vec::new())
            .expect("front-output GPU sample should succeed")
    );
    assert_eq!(compositor.spatial_sampler.cached_bind_group_count(), 2);

    compositor
        .compose(&back_output_plan, false, None)
        .expect("second back-output GPU composition should succeed");
    assert!(
        compositor
            .sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut Vec::new())
            .expect("second back-output GPU sample should succeed")
    );
    assert_eq!(compositor.spatial_sampler.cached_bind_group_count(), 2);
}

#[test]
fn gpu_sampler_reuses_sample_params_for_same_output_shape() {
    let Some(mut compositor) = super::gpu_test_compositor() else {
        return;
    };
    let engine = SpatialEngine::new(sampling_layout(SamplingMode::Bilinear));
    let first_plan = CompositionPlan::with_layers(
        4,
        4,
        vec![
            CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(12))),
            CompositionLayer::alpha(ProducerFrame::Canvas(patterned_canvas(96)), 0.35),
        ],
    );
    let second_plan = CompositionPlan::with_layers(
        4,
        4,
        vec![
            CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(44))),
            CompositionLayer::alpha(ProducerFrame::Canvas(patterned_canvas(180)), 0.35),
        ],
    );

    compositor
        .compose(&first_plan, false, None)
        .expect("first GPU composition should succeed");
    let mut first_sample = Vec::new();
    assert!(
        compositor
            .sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut first_sample)
            .expect("first GPU sample should succeed")
    );
    assert_eq!(compositor.spatial_sampler.sample_param_write_count(), 1);

    compositor
        .compose(&second_plan, false, None)
        .expect("second GPU composition should succeed");
    let mut second_sample = Vec::new();
    assert!(
        compositor
            .sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut second_sample)
            .expect("second GPU sample should succeed")
    );

    assert_eq!(compositor.spatial_sampler.sample_param_write_count(), 1);
}

#[test]
fn gpu_sampler_copies_only_live_sample_bytes_after_capacity_growth() {
    let Some(mut compositor) = super::gpu_test_compositor() else {
        return;
    };
    let large_engine =
        SpatialEngine::new(sampling_layout_with_led_count(SamplingMode::Bilinear, 16));
    let small_engine =
        SpatialEngine::new(sampling_layout_with_led_count(SamplingMode::Bilinear, 4));
    let plan = CompositionPlan::with_layers(
        4,
        4,
        vec![
            CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(12))),
            CompositionLayer::alpha(ProducerFrame::Canvas(patterned_canvas(96)), 0.35),
        ],
    );
    let expected =
        CpuSparkleFlinger::new().compose(plan.clone(), true, full_preview_request(&plan));

    compositor
        .compose(&plan, false, None)
        .expect("GPU composition should succeed before readback sizing tests");

    let mut large_sample = Vec::new();
    assert!(
        compositor
            .sample_zone_plan_into(large_engine.sampling_plan().as_ref(), &mut large_sample)
            .expect("large GPU sample should succeed")
    );
    assert_eq!(compositor.spatial_sampler.last_readback_copy_bytes(), 64);

    let mut small_sample = Vec::new();
    assert!(
        compositor
            .sample_zone_plan_into(small_engine.sampling_plan().as_ref(), &mut small_sample)
            .expect("smaller GPU sample should succeed after capacity growth")
    );
    assert_eq!(compositor.spatial_sampler.last_readback_copy_bytes(), 16);
    assert_eq!(
        small_sample,
        small_engine.sample(
            expected
                .sampling_canvas
                .as_ref()
                .expect("CPU compose should materialize a canvas"),
        )
    );
}

#[test]
fn gpu_sampler_rotates_readback_slots_for_overlapped_dispatches() {
    let Some(mut compositor) = super::gpu_test_compositor() else {
        return;
    };
    let engine = SpatialEngine::new(sampling_layout(SamplingMode::Bilinear));
    let plan = CompositionPlan::with_layers(
        4,
        4,
        vec![
            CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(12))),
            CompositionLayer::alpha(ProducerFrame::Canvas(patterned_canvas(96)), 0.35),
        ],
    );
    let expected =
        CpuSparkleFlinger::new().compose(plan.clone(), true, full_preview_request(&plan));

    compositor
        .compose(&plan, false, None)
        .expect("GPU composition should succeed before overlapped sample dispatch testing");

    let mut first_sample = Vec::new();
    let first_pending = match compositor
        .begin_sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut first_sample)
        .expect("first GPU sample dispatch should succeed")
    {
        GpuZoneSamplingDispatch::Pending(pending) => pending,
        _ => panic!("first GPU sample dispatch should defer readback completion"),
    };

    let mut second_sample = Vec::new();
    let second_pending = match compositor
        .begin_sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut second_sample)
        .expect("second GPU sample dispatch should succeed")
    {
        GpuZoneSamplingDispatch::Pending(pending) => pending,
        _ => panic!("second GPU sample dispatch should defer readback completion"),
    };

    let mut third_sample = Vec::new();
    let third_pending = match compositor
        .begin_sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut third_sample)
        .expect("third GPU sample dispatch should succeed")
    {
        GpuZoneSamplingDispatch::Pending(pending) => pending,
        _ => panic!("third GPU sample dispatch should defer readback completion"),
    };

    assert_ne!(
        first_pending.pending_readback.readback_slot(),
        second_pending.pending_readback.readback_slot()
    );
    assert_ne!(
        first_pending.pending_readback.readback_slot(),
        third_pending.pending_readback.readback_slot()
    );
    assert_ne!(
        second_pending.pending_readback.readback_slot(),
        third_pending.pending_readback.readback_slot()
    );

    compositor
        .finish_pending_zone_sampling(first_pending, &mut first_sample)
        .expect("first pending sample finalize should succeed");
    compositor
        .finish_pending_zone_sampling(second_pending, &mut second_sample)
        .expect("second pending sample finalize should succeed");
    compositor
        .finish_pending_zone_sampling(third_pending, &mut third_sample)
        .expect("third pending sample finalize should succeed");

    assert_eq!(
        first_sample,
        engine.sample(
            expected
                .sampling_canvas
                .as_ref()
                .expect("CPU compose should materialize a canvas"),
        )
    );
    assert_eq!(second_sample, first_sample);
    assert_eq!(third_sample, first_sample);
}

#[test]
fn gpu_sampler_keeps_new_generation_slot_leased_after_retired_readback_finishes() {
    let Some(mut compositor) = super::gpu_test_compositor() else {
        return;
    };
    let old_engine = SpatialEngine::new(sampling_layout_with_led_count(SamplingMode::Bilinear, 4));
    let grown_engine =
        SpatialEngine::new(sampling_layout_with_led_count(SamplingMode::Bilinear, 16));
    let second_engine =
        SpatialEngine::new(sampling_layout_with_led_count(SamplingMode::Nearest, 8));
    let third_engine = SpatialEngine::new(sampling_layout_with_led_count(
        SamplingMode::AreaAverage {
            radius_x: 1.0,
            radius_y: 1.0,
        },
        12,
    ));
    let saturated_engine =
        SpatialEngine::new(sampling_layout_with_led_count(SamplingMode::Nearest, 6));
    let plan = CompositionPlan::with_layers(
        4,
        4,
        vec![
            CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(12))),
            CompositionLayer::alpha(ProducerFrame::Canvas(patterned_canvas(96)), 0.35),
        ],
    );

    compositor
        .compose(&plan, false, None)
        .expect("GPU composition should succeed before readback generation testing");

    let old_pending = match compositor
        .begin_sample_zone_plan_into(old_engine.sampling_plan().as_ref(), &mut Vec::new())
        .expect("old-generation sample dispatch should succeed")
    {
        GpuZoneSamplingDispatch::Pending(pending) => pending,
        _ => panic!("old-generation sample dispatch should defer readback completion"),
    };
    let grown_pending = match compositor
        .begin_sample_zone_plan_into(grown_engine.sampling_plan().as_ref(), &mut Vec::new())
        .expect("grown-generation sample dispatch should succeed")
    {
        GpuZoneSamplingDispatch::Pending(pending) => pending,
        _ => panic!("grown-generation sample dispatch should defer readback completion"),
    };

    assert_eq!(old_pending.pending_readback.readback_slot(), 0);
    assert_eq!(grown_pending.pending_readback.readback_slot(), 0);
    assert_ne!(
        old_pending.pending_readback.readback_generation(),
        grown_pending.pending_readback.readback_generation()
    );

    compositor
        .finish_pending_zone_sampling(old_pending, &mut Vec::new())
        .expect("retired old-generation readback should finish without releasing the new lease");

    let second_pending = match compositor
        .begin_sample_zone_plan_into(second_engine.sampling_plan().as_ref(), &mut Vec::new())
        .expect("second current-generation dispatch should succeed")
    {
        GpuZoneSamplingDispatch::Pending(pending) => pending,
        _ => panic!("second current-generation dispatch should defer readback completion"),
    };
    let third_pending = match compositor
        .begin_sample_zone_plan_into(third_engine.sampling_plan().as_ref(), &mut Vec::new())
        .expect("third current-generation dispatch should succeed")
    {
        GpuZoneSamplingDispatch::Pending(pending) => pending,
        _ => panic!("third current-generation dispatch should defer readback completion"),
    };

    assert!(
        matches!(
            compositor
                .begin_sample_zone_plan_into(
                    saturated_engine.sampling_plan().as_ref(),
                    &mut Vec::new()
                )
                .expect("saturated current-generation dispatch should stay non-fatal"),
            GpuZoneSamplingDispatch::Saturated
        ),
        "retired completion must not make the overlapping current-generation slot reusable"
    );

    compositor
        .finish_pending_zone_sampling(grown_pending, &mut Vec::new())
        .expect("grown-generation sample should finish");
    compositor
        .finish_pending_zone_sampling(second_pending, &mut Vec::new())
        .expect("second current-generation sample should finish");
    compositor
        .finish_pending_zone_sampling(third_pending, &mut Vec::new())
        .expect("third current-generation sample should finish");
}

#[test]
fn gpu_sampler_refuses_a_fourth_overlapped_readback_until_a_slot_is_released() {
    let Some(mut compositor) = super::gpu_test_compositor() else {
        return;
    };
    let engine = SpatialEngine::new(sampling_layout(SamplingMode::Bilinear));
    let alternate_engine = SpatialEngine::new(sampling_layout(SamplingMode::AreaAverage {
        radius_x: 1.0,
        radius_y: 1.0,
    }));
    let plan = CompositionPlan::with_layers(
        4,
        4,
        vec![
            CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(12))),
            CompositionLayer::alpha(ProducerFrame::Canvas(patterned_canvas(96)), 0.35),
        ],
    );

    compositor
        .compose(&plan, false, None)
        .expect("GPU composition should succeed before saturation testing");

    let first_pending = match compositor
        .begin_sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut Vec::new())
        .expect("first GPU sample dispatch should succeed")
    {
        GpuZoneSamplingDispatch::Pending(pending) => pending,
        _ => panic!("first GPU sample dispatch should defer readback completion"),
    };
    let second_pending = match compositor
        .begin_sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut Vec::new())
        .expect("second GPU sample dispatch should succeed")
    {
        GpuZoneSamplingDispatch::Pending(pending) => pending,
        _ => panic!("second GPU sample dispatch should defer readback completion"),
    };
    let third_pending = match compositor
        .begin_sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut Vec::new())
        .expect("third GPU sample dispatch should succeed")
    {
        GpuZoneSamplingDispatch::Pending(pending) => pending,
        _ => panic!("third GPU sample dispatch should defer readback completion"),
    };

    assert!(
        matches!(
            compositor
                .begin_sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut Vec::new())
                .expect("fourth GPU sample dispatch should stay non-fatal"),
            GpuZoneSamplingDispatch::Saturated
        ),
        "fourth overlapped GPU sample should refuse to reuse an in-flight readback slot"
    );

    compositor
        .finish_pending_zone_sampling(first_pending, &mut Vec::new())
        .expect("first pending sample finalize should succeed");

    assert!(
        matches!(
            compositor
                .begin_sample_zone_plan_into(
                    alternate_engine.sampling_plan().as_ref(),
                    &mut Vec::new()
                )
                .expect("dispatch after releasing a slot should succeed"),
            GpuZoneSamplingDispatch::Pending(_)
        ),
        "freeing one readback slot should allow the next overlapped GPU sample dispatch"
    );

    compositor
        .finish_pending_zone_sampling(second_pending, &mut Vec::new())
        .expect("second pending sample finalize should succeed");
    compositor
        .finish_pending_zone_sampling(third_pending, &mut Vec::new())
        .expect("third pending sample finalize should succeed");
}

#[test]
fn gpu_sampler_discard_releases_overlapped_readback_slot() {
    let Some(mut compositor) = super::gpu_test_compositor() else {
        return;
    };
    let engine = SpatialEngine::new(sampling_layout(SamplingMode::Bilinear));
    let alternate_engine = SpatialEngine::new(sampling_layout(SamplingMode::AreaAverage {
        radius_x: 1.0,
        radius_y: 1.0,
    }));
    let plan = CompositionPlan::with_layers(
        4,
        4,
        vec![
            CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(12))),
            CompositionLayer::alpha(ProducerFrame::Canvas(patterned_canvas(96)), 0.35),
        ],
    );

    compositor
        .compose(&plan, false, None)
        .expect("GPU composition should succeed before discard testing");

    let first_pending = match compositor
        .begin_sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut Vec::new())
        .expect("first GPU sample dispatch should succeed")
    {
        GpuZoneSamplingDispatch::Pending(pending) => pending,
        _ => panic!("first GPU sample dispatch should defer readback completion"),
    };
    let second_pending = match compositor
        .begin_sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut Vec::new())
        .expect("second GPU sample dispatch should succeed")
    {
        GpuZoneSamplingDispatch::Pending(pending) => pending,
        _ => panic!("second GPU sample dispatch should defer readback completion"),
    };
    let third_pending = match compositor
        .begin_sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut Vec::new())
        .expect("third GPU sample dispatch should succeed")
    {
        GpuZoneSamplingDispatch::Pending(pending) => pending,
        _ => panic!("third GPU sample dispatch should defer readback completion"),
    };

    compositor.discard_pending_zone_sampling(first_pending);

    assert!(
        matches!(
            compositor
                .begin_sample_zone_plan_into(
                    alternate_engine.sampling_plan().as_ref(),
                    &mut Vec::new()
                )
                .expect("dispatch after discarding a slot should succeed"),
            GpuZoneSamplingDispatch::Pending(_)
        ),
        "discarding an unfinished GPU sample should free one readback slot for new work"
    );

    compositor
        .finish_pending_zone_sampling(second_pending, &mut Vec::new())
        .expect("second pending sample finalize should succeed");
    compositor
        .finish_pending_zone_sampling(third_pending, &mut Vec::new())
        .expect("third pending sample finalize should succeed");
}

#[test]
fn gpu_sampler_decodes_overlapped_readbacks_with_distinct_sampling_plans() {
    let Some(mut compositor) = super::gpu_test_compositor() else {
        return;
    };
    let large_engine =
        SpatialEngine::new(sampling_layout_with_led_count(SamplingMode::Nearest, 16));
    let small_engine = SpatialEngine::new(sampling_layout(SamplingMode::Nearest));
    let plan = CompositionPlan::with_layers(
        4,
        4,
        vec![
            CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(12))),
            CompositionLayer::alpha(ProducerFrame::Canvas(patterned_canvas(96)), 0.35),
        ],
    );
    let expected =
        CpuSparkleFlinger::new().compose(plan.clone(), true, full_preview_request(&plan));

    compositor
        .compose(&plan, false, None)
        .expect("GPU composition should succeed before distinct overlapped sample dispatches");

    let mut large_sample = Vec::new();
    let large_pending = match compositor
        .begin_sample_zone_plan_into(large_engine.sampling_plan().as_ref(), &mut large_sample)
        .expect("large GPU sample dispatch should succeed")
    {
        GpuZoneSamplingDispatch::Pending(pending) => pending,
        _ => panic!("large GPU sample dispatch should defer readback completion"),
    };

    let mut small_sample = Vec::new();
    let small_pending = match compositor
        .begin_sample_zone_plan_into(small_engine.sampling_plan().as_ref(), &mut small_sample)
        .expect("small GPU sample dispatch should succeed")
    {
        GpuZoneSamplingDispatch::Pending(pending) => pending,
        _ => panic!("small GPU sample dispatch should defer readback completion"),
    };

    compositor
        .finish_pending_zone_sampling(large_pending, &mut large_sample)
        .expect("large pending sample finalize should succeed");
    compositor
        .finish_pending_zone_sampling(small_pending, &mut small_sample)
        .expect("small pending sample finalize should succeed");

    let expected_canvas = expected
        .sampling_canvas
        .as_ref()
        .expect("CPU compose should materialize a canvas");
    assert_eq!(large_sample, large_engine.sample(expected_canvas));
    assert_eq!(small_sample, small_engine.sample(expected_canvas));
}

#[test]
fn gpu_pending_sample_try_finish_can_prime_cache_without_blocking() {
    let Some(mut compositor) = super::gpu_test_compositor() else {
        return;
    };
    let engine = SpatialEngine::new(sampling_layout(SamplingMode::Nearest));
    let plan = CompositionPlan::with_layers(
        4,
        4,
        vec![
            CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(12))),
            CompositionLayer::alpha(ProducerFrame::Canvas(patterned_canvas(96)), 0.35),
        ],
    );
    let expected =
        CpuSparkleFlinger::new().compose(plan.clone(), true, full_preview_request(&plan));

    compositor
        .compose(&plan, false, None)
        .expect("GPU composition should succeed before nonblocking sample finalize");

    let mut sampled = Vec::new();
    let mut pending = match compositor
        .begin_sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut sampled)
        .expect("GPU sample dispatch should succeed")
    {
        GpuZoneSamplingDispatch::Pending(pending) => pending,
        _ => panic!("GPU sample dispatch should defer readback completion"),
    };
    compositor
        .device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(pending.pending_readback.submission_index()),
            timeout: None,
        })
        .expect("GPU sample submission should become ready");

    let dispatch_count_before = compositor.spatial_sampler.sample_dispatch_count();
    let mut deferred_sample = Vec::new();
    assert!(
        compositor
            .try_finish_pending_zone_sampling(&mut pending, &mut deferred_sample)
            .expect("nonblocking GPU sample finalize should succeed when ready")
    );
    assert!(!compositor.take_last_sample_readback_wait_blocked());
    assert_eq!(
        deferred_sample,
        engine.sample(
            expected
                .sampling_canvas
                .as_ref()
                .expect("CPU compose should materialize a canvas"),
        )
    );

    let mut cached_sample = Vec::new();
    match compositor
        .begin_sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut cached_sample)
        .expect("cached GPU sample should succeed after nonblocking finalize")
    {
        GpuZoneSamplingDispatch::Ready => {}
        _ => panic!("ready nonblocking finalize should prime the cached sample result"),
    }
    assert_eq!(cached_sample, deferred_sample);
    assert_eq!(
        compositor.spatial_sampler.sample_dispatch_count(),
        dispatch_count_before
    );
}

#[test]
fn gpu_stale_pending_sample_finalize_does_not_poison_new_output_cache() {
    let Some(mut compositor) = super::gpu_test_compositor() else {
        return;
    };
    let engine = SpatialEngine::new(sampling_layout(SamplingMode::Nearest));
    let first_plan = CompositionPlan::with_layers(
        4,
        4,
        vec![
            CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(12))),
            CompositionLayer::alpha(ProducerFrame::Canvas(patterned_canvas(96)), 0.35),
        ],
    );
    let second_plan = CompositionPlan::with_layers(
        4,
        4,
        vec![
            CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(44))),
            CompositionLayer::alpha(ProducerFrame::Canvas(patterned_canvas(180)), 0.5),
        ],
    );
    let first_expected = CpuSparkleFlinger::new().compose(
        first_plan.clone(),
        true,
        full_preview_request(&first_plan),
    );
    let second_expected = CpuSparkleFlinger::new().compose(
        second_plan.clone(),
        true,
        full_preview_request(&second_plan),
    );

    compositor
        .compose(&first_plan, false, None)
        .expect("first GPU composition should succeed");
    let mut stale_sample = Vec::new();
    let stale_pending = match compositor
        .begin_sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut stale_sample)
        .expect("first GPU sample dispatch should succeed")
    {
        GpuZoneSamplingDispatch::Pending(pending) => pending,
        _ => panic!("first GPU sample dispatch should defer readback completion"),
    };

    compositor
        .compose(&second_plan, false, None)
        .expect("second GPU composition should succeed");

    compositor
        .finish_pending_zone_sampling(stale_pending, &mut stale_sample)
        .expect("stale pending sample finalize should still decode successfully");
    assert_eq!(
        stale_sample,
        engine.sample(
            first_expected
                .sampling_canvas
                .as_ref()
                .expect("first CPU compose should materialize a canvas"),
        )
    );

    let dispatch_count_before = compositor.spatial_sampler.sample_dispatch_count();
    let mut current_sample = Vec::new();
    assert!(
        compositor
            .sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut current_sample)
            .expect("current GPU sample should succeed")
    );
    assert_eq!(
        current_sample,
        engine.sample(
            second_expected
                .sampling_canvas
                .as_ref()
                .expect("second CPU compose should materialize a canvas"),
        )
    );
    assert_eq!(
        compositor.spatial_sampler.sample_dispatch_count(),
        dispatch_count_before.saturating_add(1)
    );
}

#[test]
fn gpu_pending_sample_stops_matching_after_layout_generation_changes() {
    let Some(mut compositor) = super::gpu_test_compositor() else {
        return;
    };
    let mut engine = SpatialEngine::new(sampling_layout(SamplingMode::Nearest));
    let plan = CompositionPlan::single(
        4,
        4,
        CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(12))),
    );

    compositor
        .compose(&plan, false, None)
        .expect("GPU composition should succeed before pending sample dispatch");
    let pending = match compositor
        .begin_sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut Vec::new())
        .expect("GPU sample dispatch should succeed")
    {
        GpuZoneSamplingDispatch::Pending(pending) => pending,
        _ => panic!("GPU sample dispatch should defer readback completion"),
    };

    engine.update_layout(sampling_layout(SamplingMode::Nearest));

    assert!(
        !compositor
            .pending_zone_sampling_matches_current_work(&pending, engine.sampling_plan().as_ref())
    );
    compositor.discard_pending_zone_sampling(pending);
}

#[test]
fn gpu_refuses_late_full_surface_readback_for_cpu_sampling_fallback() {
    let Some(mut compositor) = super::gpu_test_compositor() else {
        return;
    };
    let plan = CompositionPlan::with_layers(
        4,
        4,
        vec![
            CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(12))),
            CompositionLayer::alpha(ProducerFrame::Canvas(patterned_canvas(96)), 0.35),
        ],
    );
    let composed = compositor
        .compose(&plan, false, None)
        .expect("GPU composition should succeed before late CPU sampling fallback");
    assert!(composed.sampling_canvas.is_none());
    assert!(composed.sampling_surface.is_none());

    assert!(
        compositor
            .current_output_frame()
            .is_ok_and(|frame| frame.is_some())
    );
}

#[test]
fn gpu_sampler_skips_blocking_wait_when_readback_is_already_mapped() {
    let Some(mut compositor) = super::gpu_test_compositor() else {
        return;
    };
    let engine = SpatialEngine::new(sampling_layout(SamplingMode::Bilinear));
    let plan = CompositionPlan::with_layers(
        4,
        4,
        vec![
            CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(12))),
            CompositionLayer::alpha(ProducerFrame::Canvas(patterned_canvas(96)), 0.35),
        ],
    );
    let expected =
        CpuSparkleFlinger::new().compose(plan.clone(), true, full_preview_request(&plan));

    compositor
        .compose(&plan, false, None)
        .expect("GPU composition should succeed before pending sample readback testing");

    let mut sampled = Vec::new();
    let pending = match compositor
        .begin_sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut sampled)
        .expect("GPU sample dispatch should succeed")
    {
        GpuZoneSamplingDispatch::Pending(pending) => pending,
        _ => panic!("GPU sample dispatch should defer readback completion"),
    };
    compositor
        .device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(pending.pending_readback.submission_index()),
            timeout: None,
        })
        .expect("GPU sample submission should become ready");

    compositor
        .finish_pending_zone_sampling(pending, &mut sampled)
        .expect("GPU pending sample finalize should succeed");

    assert_eq!(compositor.spatial_sampler.sample_readback_wait_count(), 0);
    assert_eq!(
        sampled,
        engine.sample(
            expected
                .sampling_canvas
                .as_ref()
                .expect("CPU compose should materialize a canvas"),
        )
    );
}

#[test]
fn gpu_sampler_nonblocking_finalize_eventually_completes_without_explicit_wait() {
    let Some(mut compositor) = super::gpu_test_compositor() else {
        return;
    };
    let engine = SpatialEngine::new(sampling_layout(SamplingMode::Bilinear));
    let plan = CompositionPlan::with_layers(
        4,
        4,
        vec![
            CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(12))),
            CompositionLayer::alpha(ProducerFrame::Canvas(patterned_canvas(96)), 0.35),
        ],
    );
    let expected =
        CpuSparkleFlinger::new().compose(plan.clone(), true, full_preview_request(&plan));

    compositor
        .compose(&plan, false, None)
        .expect("GPU composition should succeed before nonblocking sample finalize");

    let mut sampled = Vec::new();
    let mut pending = match compositor
        .begin_sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut sampled)
        .expect("GPU sample dispatch should succeed")
    {
        GpuZoneSamplingDispatch::Pending(pending) => pending,
        _ => panic!("GPU sample dispatch should defer readback completion"),
    };

    let start = std::time::Instant::now();
    loop {
        if compositor
            .try_finish_pending_zone_sampling(&mut pending, &mut sampled)
            .expect("nonblocking GPU sample finalize should not fail while pending")
        {
            break;
        }
        assert!(
            start.elapsed() < std::time::Duration::from_secs(2),
            "expected nonblocking GPU sample finalize to complete within 2 seconds"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }

    assert!(!compositor.take_last_sample_readback_wait_blocked());
    assert_eq!(
        sampled,
        engine.sample(
            expected
                .sampling_canvas
                .as_ref()
                .expect("CPU compose should materialize a canvas"),
        )
    );
}

#[test]
fn gpu_pending_sample_matches_and_finishes_across_retained_bypass_frames() {
    let Some(mut compositor) = super::gpu_test_compositor() else {
        return;
    };
    let engine = SpatialEngine::new(sampling_layout(SamplingMode::Bilinear));
    let source = PublishedSurface::from_owned_canvas(patterned_canvas(12), 1, 1);
    let plan = CompositionPlan::single(
        4,
        4,
        CompositionLayer::replace(ProducerFrame::Surface(source.clone())),
    );
    let expected =
        CpuSparkleFlinger::new().compose(plan.clone(), true, full_preview_request(&plan));

    compositor
        .compose(&plan, false, None)
        .expect("initial retained GPU composition should succeed");

    let mut sampled = Vec::new();
    let mut pending = match compositor
        .begin_sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut sampled)
        .expect("GPU sample dispatch should succeed")
    {
        GpuZoneSamplingDispatch::Pending(pending) => pending,
        _ => panic!("GPU sample dispatch should defer readback completion"),
    };

    let start = std::time::Instant::now();
    loop {
        compositor
            .compose(&plan, false, None)
            .expect("retained GPU composition should keep succeeding");
        assert!(
            compositor.pending_zone_sampling_matches_current_work(
                &pending,
                engine.sampling_plan().as_ref()
            ),
            "retained bypass should preserve pending GPU sample identity: pending_content={:?} current_output_generation={} pending_sampling_plan={:?} current_sampling_plan={:?} current_output={:?} cached_key_present={}",
            pending.content,
            compositor.output_generation,
            pending.sampling_plan,
            GpuSamplingPlan::key(engine.sampling_plan().as_ref()),
            compositor.current_output,
            compositor.cached_composition_key.is_some()
        );
        if compositor
            .try_finish_pending_zone_sampling(&mut pending, &mut sampled)
            .expect("nonblocking GPU sample finalize should not fail while pending")
        {
            break;
        }
        assert!(
            start.elapsed() < std::time::Duration::from_secs(2),
            "expected retained pending GPU sample to complete within 2 seconds"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }

    assert_eq!(
        sampled,
        engine.sample(
            expected
                .sampling_canvas
                .as_ref()
                .expect("CPU compose should materialize a canvas"),
        )
    );
}

mod spatial;

fn zone_texture_layout(
    output_id: &str,
    width: u32,
    height: u32,
    mode: SamplingMode,
    led_count: u32,
) -> SpatialLayout {
    let mut layout = sampling_layout_with_led_count(mode, led_count);
    layout.canvas_width = width;
    layout.canvas_height = height;
    layout.zones[0].id = output_id.into();
    layout
}

/// Upload `canvas` into a standalone texture, standing in for a zone's
/// projection snapshot.
fn uploaded_zone_texture(
    compositor: &GpuSparkleFlinger,
    canvas: &Canvas,
    storage_id: u64,
) -> GpuTextureFrame {
    let extent = wgpu::Extent3d {
        width: canvas.width(),
        height: canvas.height(),
        depth_or_array_layers: 1,
    };
    let texture = compositor.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("SparkleFlinger zone texture sampling test"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    compositor.queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        canvas.as_rgba_bytes(),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(canvas.width() * 4),
            rows_per_image: Some(canvas.height()),
        },
        extent,
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    GpuTextureFrame {
        width: canvas.width(),
        height: canvas.height(),
        storage_id,
        content_generation: 1,
        origin: GpuTextureFrameOrigin::ProducerTexture,
        texture,
        view,
        immutable_lease: None,
        native_screen_lease: None,
    }
}

fn sample_zone_textures(
    compositor: &mut GpuSparkleFlinger,
    textures: &[ZoneTextureSample],
) -> Vec<ZoneColors> {
    let mut sampled = Vec::new();
    match compositor
        .begin_sample_zone_textures_into(textures, &mut sampled)
        .expect("zone texture sampling should dispatch")
    {
        GpuZoneSamplingDispatch::Pending(pending) => compositor
            .finish_pending_zone_sampling(pending, &mut sampled)
            .expect("zone texture sampling should finish"),
        GpuZoneSamplingDispatch::Ready => {}
        GpuZoneSamplingDispatch::Unsupported | GpuZoneSamplingDispatch::Saturated => {
            panic!("zone texture sampling should admit supported plans")
        }
    }
    sampled
}

/// Test storage ids sit far above the compositor's allocation counter so they
/// never alias a real texture.
const TEST_ZONE_TEXTURE_STORAGE_BASE: u64 = u64::MAX - 1_000;

#[test]
fn gpu_sampler_samples_each_zone_texture_with_its_own_plan() {
    let Some(mut compositor) = super::gpu_test_compositor() else {
        return;
    };
    let area = SamplingMode::AreaAverage {
        radius_x: 1.0,
        radius_y: 1.0,
    };
    // Two area zones share a 4x4 extent, so the second must rescan the
    // summed-area table after the first has read it; the 8x2 zone needs a
    // table of its own; the bilinear zone needs none.
    let zones = [
        (
            zone_texture_layout("case", 4, 4, area.clone(), 4),
            patterned_canvas_with_size(4, 4, 12),
        ),
        (
            zone_texture_layout("strip", 8, 2, area.clone(), 6),
            patterned_canvas_with_size(8, 2, 77),
        ),
        (
            zone_texture_layout("pad", 4, 4, area, 5),
            patterned_canvas_with_size(4, 4, 140),
        ),
        (
            zone_texture_layout("keys", 6, 3, SamplingMode::Bilinear, 7),
            patterned_canvas_with_size(6, 3, 201),
        ),
    ];
    let engines = zones
        .iter()
        .map(|(layout, _)| SpatialEngine::new(layout.clone()))
        .collect::<Vec<_>>();
    let textures = zones
        .iter()
        .zip(&engines)
        .zip(0_u64..)
        .map(|(((_, canvas), engine), index)| ZoneTextureSample {
            frame: uploaded_zone_texture(
                &compositor,
                canvas,
                TEST_ZONE_TEXTURE_STORAGE_BASE + index,
            ),
            prepared_zones: engine.sampling_plan(),
        })
        .collect::<Vec<_>>();

    let sampled = sample_zone_textures(&mut compositor, &textures);

    let expected = zones
        .iter()
        .zip(&engines)
        .flat_map(|((_, canvas), engine)| engine.sample(canvas))
        .collect::<Vec<_>>();
    assert_eq!(sampled, expected);
}

#[test]
fn gpu_zone_texture_samples_cache_by_content_generation() {
    let Some(mut compositor) = super::gpu_test_compositor() else {
        return;
    };
    let case = SpatialEngine::new(zone_texture_layout("case", 4, 4, SamplingMode::Bilinear, 4));
    let pad = SpatialEngine::new(zone_texture_layout("pad", 8, 2, SamplingMode::Nearest, 6));
    let mut textures = vec![
        ZoneTextureSample {
            frame: uploaded_zone_texture(
                &compositor,
                &patterned_canvas_with_size(4, 4, 30),
                TEST_ZONE_TEXTURE_STORAGE_BASE + 10,
            ),
            prepared_zones: case.sampling_plan(),
        },
        ZoneTextureSample {
            frame: uploaded_zone_texture(
                &compositor,
                &patterned_canvas_with_size(8, 2, 90),
                TEST_ZONE_TEXTURE_STORAGE_BASE + 11,
            ),
            prepared_zones: pad.sampling_plan(),
        },
    ];

    let mut sampled = Vec::new();
    let GpuZoneSamplingDispatch::Pending(pending) = compositor
        .begin_sample_zone_textures_into(&textures, &mut sampled)
        .expect("first zone texture sample should dispatch")
    else {
        panic!("a fresh zone texture sample should stage a readback");
    };
    assert!(GpuSparkleFlinger::pending_zone_texture_sampling_matches(
        &pending, &textures
    ));
    compositor
        .finish_pending_zone_sampling(pending, &mut sampled)
        .expect("first zone texture sample should finish");
    let dispatches = compositor.spatial_sampler.sample_dispatch_count();

    let mut cached = Vec::new();
    assert!(matches!(
        compositor
            .begin_sample_zone_textures_into(&textures, &mut cached)
            .expect("an unchanged zone texture sample should resolve"),
        GpuZoneSamplingDispatch::Ready
    ));
    assert_eq!(cached, sampled);
    assert_eq!(
        compositor.spatial_sampler.sample_dispatch_count(),
        dispatches,
        "unchanged zone textures should reuse the cached sample"
    );

    textures[1].frame.content_generation += 1;
    let mut resampled = Vec::new();
    let GpuZoneSamplingDispatch::Pending(pending) = compositor
        .begin_sample_zone_textures_into(&textures, &mut resampled)
        .expect("a changed zone texture should dispatch")
    else {
        panic!("a changed zone texture should stage a readback");
    };
    let previous_generation = {
        let mut stale = textures.clone();
        stale[1].frame.content_generation -= 1;
        stale
    };
    assert!(!GpuSparkleFlinger::pending_zone_texture_sampling_matches(
        &pending,
        &previous_generation
    ));
    compositor
        .finish_pending_zone_sampling(pending, &mut resampled)
        .expect("changed zone texture sample should finish");
    assert_eq!(resampled, sampled);
    assert!(compositor.spatial_sampler.sample_dispatch_count() > dispatches);
}

#[test]
fn gpu_sampler_gaussian_zone_textures_match_the_cpu_sampler() {
    let Some(mut compositor) = super::gpu_test_compositor() else {
        return;
    };
    // Radius 0, an interior kernel, and kernels that clamp at the canvas edge,
    // across two extents so passes rebase their kernel offsets.
    let zones = [
        (
            zone_texture_layout(
                "tight",
                6,
                4,
                SamplingMode::GaussianArea {
                    sigma: 1.0,
                    radius: 0,
                },
                5,
            ),
            patterned_canvas_with_size(6, 4, 9),
        ),
        (
            zone_texture_layout(
                "soft",
                8,
                8,
                SamplingMode::GaussianArea {
                    sigma: 1.5,
                    radius: 2,
                },
                8,
            ),
            patterned_canvas_with_size(8, 8, 71),
        ),
        (
            zone_texture_layout(
                "wide",
                5,
                3,
                SamplingMode::GaussianArea {
                    sigma: 3.0,
                    radius: 4,
                },
                6,
            ),
            patterned_canvas_with_size(5, 3, 188),
        ),
    ];
    let engines = zones
        .iter()
        .map(|(layout, _)| SpatialEngine::new(layout.clone()))
        .collect::<Vec<_>>();
    let textures = zones
        .iter()
        .zip(&engines)
        .zip(20_u64..)
        .map(|(((_, canvas), engine), index)| ZoneTextureSample {
            frame: uploaded_zone_texture(
                &compositor,
                canvas,
                TEST_ZONE_TEXTURE_STORAGE_BASE + index,
            ),
            prepared_zones: engine.sampling_plan(),
        })
        .collect::<Vec<_>>();

    let sampled = sample_zone_textures(&mut compositor, &textures);

    let expected = zones
        .iter()
        .zip(&engines)
        .flat_map(|((_, canvas), engine)| engine.sample(canvas))
        .collect::<Vec<_>>();
    assert_eq!(sampled, expected);
}
