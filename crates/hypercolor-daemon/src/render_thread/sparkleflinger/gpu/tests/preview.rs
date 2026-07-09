use super::*;

#[test]
fn gpu_scaled_preview_reuses_cached_surface_across_size_flips() {
    let mut compositor = match GpuSparkleFlinger::new() {
        Ok(compositor) => compositor,
        Err(_) => return,
    };
    let plan = CompositionPlan::with_layers(
        4,
        4,
        vec![
            CompositionLayer::replace(ProducerFrame::Surface(slot_surface(Rgba::new(
                255, 32, 0, 255,
            )))),
            CompositionLayer::alpha(
                ProducerFrame::Surface(slot_surface(Rgba::new(32, 64, 255, 255))),
                0.35,
            ),
        ],
    );
    let large_request = PreviewSurfaceRequest {
        width: 3,
        height: 3,
    };
    let small_request = PreviewSurfaceRequest {
        width: 2,
        height: 2,
    };

    compositor
        .compose(&plan, false, Some(large_request))
        .expect("large scaled preview compose should succeed");
    let _ = resolve_preview_surface_blocking(&mut compositor);

    compositor
        .compose(&plan, false, Some(small_request))
        .expect("small scaled preview compose should succeed");
    let _ = resolve_preview_surface_blocking(&mut compositor);

    let composed = compositor
        .compose(&plan, false, Some(large_request))
        .expect("restored scaled preview compose should succeed");
    let preview_surface = composed
        .preview_surface
        .expect("cached large scaled preview should be returned immediately");
    assert_eq!(preview_surface.width(), 3);
    assert_eq!(preview_surface.height(), 3);
    assert!(compositor.pending_preview_readback().is_none());
    assert!(!compositor.has_pending_output_submission());
    assert!(compositor.cached_preview_surfaces.len() >= 2);
}

#[test]
fn gpu_preview_work_can_submit_before_finalize() {
    let mut compositor = match GpuSparkleFlinger::new() {
        Ok(compositor) => compositor,
        Err(_) => return,
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
    assert!(compositor.pending_preview_submission().is_none());
    let staged = compositor
        .frame_in_flight
        .as_ref()
        .expect("preview compose should own one staged frame");
    assert_eq!(staged.generation, compositor.output_generation);
    assert!(staged.is_building());

    compositor
        .submit_pending_preview_work()
        .expect("GPU preview submit should succeed");
    assert!(compositor.pending_preview_submission().is_none());
    assert!(compositor.pending_preview_readback().is_none());
    assert!(compositor.pending_preview_map.is_some());
    assert!(!compositor.has_pending_output_submission());

    let preview_surface = resolve_preview_surface_blocking(&mut compositor);
    assert_eq!(preview_surface.width(), 2);
    assert_eq!(preview_surface.height(), 2);
    assert!(compositor.pending_preview_submission().is_none());
}

#[test]
fn gpu_active_preview_map_is_reused_on_identical_compose() {
    let mut compositor = match GpuSparkleFlinger::new() {
        Ok(compositor) => compositor,
        Err(_) => return,
    };
    let base = slot_surface(Rgba::new(24, 96, 160, 255));
    let overlay = slot_surface(Rgba::new(200, 48, 96, 255));
    let plan = CompositionPlan::with_layers(
        4,
        4,
        vec![
            CompositionLayer::replace(ProducerFrame::Surface(base.clone())),
            CompositionLayer::alpha(ProducerFrame::Surface(overlay.clone()), 0.35),
        ],
    );
    let request = PreviewSurfaceRequest {
        width: 2,
        height: 2,
    };

    compositor
        .compose(&plan, false, Some(request))
        .expect("first compose should stage a scaled preview surface");
    compositor
        .submit_pending_preview_work()
        .expect("GPU preview submit should succeed");

    let composed = compositor
        .compose(&plan, false, Some(request))
        .expect("identical compose should reuse the pending preview map");
    assert!(composed.preview_surface.is_none());
    assert!(compositor.pending_preview_submission().is_none());
    assert!(compositor.pending_preview_readback().is_none());
    assert!(compositor.pending_preview_map.is_some());
    assert!(!compositor.has_pending_output_submission());

    let preview_surface = resolve_preview_surface_blocking(&mut compositor);
    assert_eq!(preview_surface.width(), 2);
    assert_eq!(preview_surface.height(), 2);
}

#[test]
fn gpu_preview_finalize_can_defer_without_blocking() {
    let mut compositor = match GpuSparkleFlinger::new() {
        Ok(compositor) => compositor,
        Err(_) => return,
    };
    let plan = CompositionPlan::with_layers(
        4,
        4,
        vec![
            CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(12))),
            CompositionLayer::alpha(ProducerFrame::Canvas(patterned_canvas(96)), 0.35),
        ],
    );

    compositor
        .compose(
            &plan,
            false,
            Some(PreviewSurfaceRequest {
                width: 2,
                height: 2,
            }),
        )
        .expect("GPU composition should stage a scaled preview surface");
    compositor
        .submit_pending_preview_work()
        .expect("GPU preview submit should succeed");
    defer_pending_preview_map(&mut compositor);

    let preview_surface = resolve_preview_surface_blocking(&mut compositor);
    assert_eq!(preview_surface.width(), 2);
    assert_eq!(preview_surface.height(), 2);
    assert!(compositor.pending_preview_submission().is_none());
    assert!(compositor.pending_preview_readback().is_none());
    assert!(compositor.pending_preview_map.is_none());
}

#[test]
fn gpu_matching_pending_preview_map_is_reused_on_identical_compose() {
    let mut compositor = match GpuSparkleFlinger::new() {
        Ok(compositor) => compositor,
        Err(_) => return,
    };
    let base = slot_surface(Rgba::new(24, 96, 160, 255));
    let overlay = slot_surface(Rgba::new(200, 48, 96, 255));
    let plan = CompositionPlan::with_layers(
        4,
        4,
        vec![
            CompositionLayer::replace(ProducerFrame::Surface(base.clone())),
            CompositionLayer::alpha(ProducerFrame::Surface(overlay.clone()), 0.35),
        ],
    );
    let request = PreviewSurfaceRequest {
        width: 2,
        height: 2,
    };

    compositor
        .compose(&plan, false, Some(request))
        .expect("first compose should stage a scaled preview surface");
    compositor
        .submit_pending_preview_work()
        .expect("GPU preview submit should succeed");
    defer_pending_preview_map(&mut compositor);

    let composed = compositor
        .compose(&plan, false, Some(request))
        .expect("identical compose should reuse the pending preview map");
    assert!(composed.preview_surface.is_none());
    assert!(compositor.pending_preview_submission().is_none());
    assert!(compositor.pending_preview_readback().is_none());
    assert!(compositor.pending_preview_map.is_some());
    assert!(!compositor.has_pending_output_submission());

    let preview_surface = resolve_preview_surface_blocking(&mut compositor);
    assert_eq!(preview_surface.width(), 2);
    assert_eq!(preview_surface.height(), 2);
}

#[test]
fn gpu_deferred_preview_queues_next_compose_after_pending_map() {
    let mut compositor = match GpuSparkleFlinger::new() {
        Ok(compositor) => compositor,
        Err(_) => return,
    };
    let first_plan = CompositionPlan::single(
        4,
        4,
        CompositionLayer::replace(ProducerFrame::Canvas(solid_canvas(Rgba::new(
            255, 32, 0, 255,
        )))),
    );
    let second_plan = CompositionPlan::single(
        4,
        4,
        CompositionLayer::replace(ProducerFrame::Canvas(solid_canvas(Rgba::new(
            32, 64, 255, 255,
        )))),
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

    compositor
        .compose(&second_plan, false, Some(request))
        .expect("second compose should queue behind the first deferred preview");
    assert!(compositor.ready_preview_surface.is_none());
    assert!(compositor.pending_preview_readback().is_some());

    let first_preview = resolve_preview_surface_blocking(&mut compositor);
    assert_eq!(&first_preview.rgba_bytes()[0..4], &[255, 32, 0, 255]);

    let second_preview = resolve_preview_surface_blocking(&mut compositor);
    assert_eq!(&second_preview.rgba_bytes()[0..4], &[32, 64, 255, 255]);
    assert!(
        compositor
            .resolve_preview_surface()
            .expect("queued preview resolve should not fail")
            .is_none()
    );
}

#[test]
fn gpu_fresh_preview_restage_uses_alternate_readback_slot() {
    let mut compositor = match GpuSparkleFlinger::new() {
        Ok(compositor) => compositor,
        Err(_) => return,
    };
    let first_plan = CompositionPlan::single(
        4,
        4,
        CompositionLayer::replace(ProducerFrame::Canvas(solid_canvas(Rgba::new(
            255, 32, 0, 255,
        )))),
    );
    let second_plan = CompositionPlan::single(
        4,
        4,
        CompositionLayer::replace(ProducerFrame::Canvas(solid_canvas(Rgba::new(
            32, 64, 255, 255,
        )))),
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
    let second_slot = match compositor.pending_preview_readback() {
        Some(PendingPreviewReadback::PreviewBuffer { slot, .. }) => *slot,
        _ => panic!("second preview should keep a staged preview-buffer readback"),
    };
    assert_ne!(first_slot, second_slot);

    compositor
        .submit_pending_preview_work()
        .expect("second preview submit should succeed");
    assert!(compositor.pending_preview_submission().is_some());
    assert!(compositor.pending_preview_readback().is_some());

    let mapped_slot = match compositor.pending_preview_map.as_ref() {
        Some(PendingPreviewMap {
            readback: PendingPreviewReadback::PreviewBuffer { slot, .. },
            ..
        }) => *slot,
        _ => panic!("first preview should remain mapped while the newer preview is queued"),
    };
    assert_eq!(mapped_slot, first_slot);

    let first_preview = resolve_preview_surface_blocking(&mut compositor);
    assert_eq!(&first_preview.rgba_bytes()[0..4], &[255, 32, 0, 255]);

    let second_preview = resolve_preview_surface_blocking(&mut compositor);
    assert_eq!(&second_preview.rgba_bytes()[0..4], &[32, 64, 255, 255]);
    assert!(compositor.pending_preview_map.is_none());
    assert!(compositor.pending_preview_readback().is_none());
    assert!(compositor.pending_preview_submission().is_none());
}

#[test]
fn gpu_current_output_preview_restage_supersedes_retained_submitted_frame() {
    let mut compositor = match GpuSparkleFlinger::new() {
        Ok(compositor) => compositor,
        Err(_) => return,
    };
    let first_plan = CompositionPlan::single(
        4,
        4,
        CompositionLayer::replace(ProducerFrame::Canvas(solid_canvas(Rgba::new(
            255, 32, 0, 255,
        )))),
    );
    let second_plan = CompositionPlan::single(
        4,
        4,
        CompositionLayer::replace(ProducerFrame::Canvas(solid_canvas(Rgba::new(
            32, 64, 255, 255,
        )))),
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

    compositor
        .compose(&second_plan, false, Some(request))
        .expect("second compose should stage a preview behind the pending map");
    compositor
        .submit_pending_preview_work()
        .expect("second preview submit should remain retained behind the pending map");
    assert!(compositor.pending_preview_submission().is_some());
    let output_generation = compositor.output_generation;
    let output = compositor
        .current_output_frame()
        .expect("current output frame lookup should succeed")
        .expect("current output frame should exist");
    let current_output_plan = CompositionPlan::single(
        4,
        4,
        CompositionLayer::replace(ProducerFrame::GpuTexture(output)),
    );
    let superseded_before = compositor.superseded_frame_count;

    compositor
        .compose(&current_output_plan, false, Some(request))
        .expect("current-output preview restage should explicitly supersede retained work");

    assert_eq!(compositor.output_generation, output_generation);
    assert_eq!(compositor.superseded_frame_count, superseded_before + 1);
    assert!(compositor.has_pending_output_submission());
    assert!(compositor.pending_preview_submission().is_none());
    assert!(compositor.pending_preview_readback().is_some());
    assert!(compositor.pending_preview_map.is_some());

    let first_preview = resolve_preview_surface_blocking(&mut compositor);
    assert_eq!(&first_preview.rgba_bytes()[0..4], &[255, 32, 0, 255]);
    let current_preview = resolve_preview_surface_blocking(&mut compositor);
    assert_eq!(&current_preview.rgba_bytes()[0..4], &[32, 64, 255, 255]);
}

#[test]
fn gpu_deferred_preview_is_superseded_by_non_bypass_resize_compose() {
    let mut compositor = match GpuSparkleFlinger::new() {
        Ok(compositor) => compositor,
        Err(_) => return,
    };
    let first_plan = CompositionPlan::with_layers(
        4,
        4,
        vec![
            CompositionLayer::replace(ProducerFrame::Canvas(solid_canvas_with_size(
                4,
                4,
                Rgba::new(255, 32, 0, 255),
            ))),
            CompositionLayer::alpha(
                ProducerFrame::Canvas(solid_canvas_with_size(4, 4, Rgba::new(32, 64, 255, 255))),
                0.35,
            ),
        ],
    );
    let second_plan = CompositionPlan::with_layers(
        2,
        2,
        vec![
            CompositionLayer::replace(ProducerFrame::Canvas(solid_canvas_with_size(
                2,
                2,
                Rgba::new(16, 220, 32, 255),
            ))),
            CompositionLayer::alpha(
                ProducerFrame::Canvas(solid_canvas_with_size(2, 2, Rgba::new(255, 255, 255, 255))),
                0.25,
            ),
        ],
    );

    compositor
        .compose(
            &first_plan,
            false,
            Some(PreviewSurfaceRequest {
                width: 2,
                height: 2,
            }),
        )
        .expect("first compose should stage a scaled preview");
    compositor
        .submit_pending_preview_work()
        .expect("first preview submit should succeed");
    defer_pending_preview_map(&mut compositor);

    compositor
        .compose(
            &second_plan,
            false,
            Some(PreviewSurfaceRequest {
                width: 1,
                height: 1,
            }),
        )
        .expect("resize compose should supersede the older deferred preview");

    let preview = resolve_preview_surface_blocking(&mut compositor);
    assert_eq!(preview.width(), 1);
    assert_eq!(preview.height(), 1);
    assert!(
        compositor
            .resolve_preview_surface()
            .expect("superseded resize preview resolve should not fail")
            .is_none()
    );
}

#[test]
fn gpu_discard_superseded_preview_work_clears_preview_state() {
    let mut compositor = match GpuSparkleFlinger::new() {
        Ok(compositor) => compositor,
        Err(_) => return,
    };

    let encoder = compositor
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("stale cached preview test"),
        });
    compositor.stage_frame_in_flight(
        encoder,
        Some(PendingPreviewReadback::PreviewBuffer {
            request: PreviewSurfaceRequest {
                width: 2,
                height: 2,
            },
            readback_key: None,
            cache_as_full_size: false,
            slot: 0,
        }),
    );
    let (_sender, receiver) = mpsc::channel::<std::result::Result<(), wgpu::BufferAsyncError>>();
    compositor.pending_preview_map = Some(PendingPreviewMap {
        readback: PendingPreviewReadback::PreviewBuffer {
            request: PreviewSurfaceRequest {
                width: 2,
                height: 2,
            },
            readback_key: None,
            cache_as_full_size: false,
            slot: 1,
        },
        submission_index: None,
        used_bytes: 16,
        receiver,
    });
    compositor.ready_preview_surface = Some(PublishedSurface::from_owned_canvas(
        solid_canvas(Rgba::new(8, 16, 24, 255)),
        0,
        0,
    ));

    compositor.discard_superseded_preview_work();

    assert!(!compositor.has_pending_output_submission());
    assert!(compositor.pending_preview_readback().is_none());
    assert!(compositor.pending_preview_submission().is_none());
    assert!(compositor.pending_preview_map.is_none());
    assert!(compositor.ready_preview_surface.is_none());
    assert_eq!(compositor.superseded_frame_count, 1);
}
