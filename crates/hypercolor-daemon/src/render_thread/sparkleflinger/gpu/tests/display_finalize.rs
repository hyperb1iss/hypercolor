use super::*;

#[test]
fn gpu_display_finalize_applies_replace_brightness_and_circular_mask() {
    let Some(mut compositor) = super::gpu_test_compositor() else {
        return;
    };
    let scene = ProducerFrame::Canvas(solid_canvas_with_size(4, 4, Rgba::new(0, 0, 255, 255)));
    let face = ProducerFrame::Canvas(solid_canvas_with_size(4, 4, Rgba::new(255, 0, 0, 255)));
    let mut params = display_finalize_params(4, 4, BlendMode::Replace);
    params.circular = true;
    params.brightness = 0.5;

    let surface = finalize_display_face_blocking(&mut compositor, &scene, &face, params);
    let rgba = surface.rgba_bytes();

    assert_eq!(&rgba[0..4], &[0, 0, 0, 0]);
    assert_eq!(
        &rgba[((2 * 4 + 2) * 4)..((2 * 4 + 2) * 4 + 4)],
        &[128, 0, 0, 255]
    );
    let counts = compositor.surface_pool_counts().compositor;
    assert_eq!(counts.free + counts.published + counts.dequeued, 3);
}

#[test]
fn gpu_display_finalize_alpha_blends_in_linear_light() {
    let Some(mut compositor) = super::gpu_test_compositor() else {
        return;
    };
    let scene = ProducerFrame::Canvas(solid_canvas_with_size(2, 2, Rgba::new(0, 0, 0, 255)));
    let face = ProducerFrame::Canvas(solid_canvas_with_size(2, 2, Rgba::new(255, 0, 0, 255)));
    let mut params = display_finalize_params(2, 2, BlendMode::Alpha);
    params.opacity = 0.5;

    let surface = finalize_display_face_blocking(&mut compositor, &scene, &face, params);

    assert_eq!(
        &surface.rgba_bytes()[0..4],
        &[encode_srgb_channel(0.5), 0, 0, 255],
    );
}

#[test]
fn gpu_display_finalize_yuv420_reads_back_luma_and_chroma_planes() {
    let Some(mut compositor) = super::gpu_test_compositor() else {
        return;
    };
    let scene = ProducerFrame::Canvas(solid_canvas_with_size(2, 2, Rgba::new(0, 0, 0, 255)));
    let face = ProducerFrame::Canvas(solid_canvas_with_size(2, 2, Rgba::new(255, 0, 0, 255)));
    let params = display_finalize_params(2, 2, BlendMode::Replace);

    let frame = finalize_display_face_yuv420_blocking(&mut compositor, &scene, &face, params);

    assert_eq!(frame.width, 2);
    assert_eq!(frame.height, 2);
    assert_eq!(frame.y_stride, 2);
    assert_eq!(frame.uv_stride, 1);
    assert_eq!(frame.y_plane(), &[76, 76, 76, 76]);
    assert_eq!(frame.u_plane(), &[85]);
    assert_eq!(frame.v_plane(), &[255]);
}

#[test]
fn gpu_display_finalize_yuv420_samples_same_size_face_on_texel_centers() {
    let Some(mut compositor) = super::gpu_test_compositor() else {
        return;
    };
    let scene = ProducerFrame::Canvas(solid_canvas_with_size(2, 1, Rgba::new(0, 0, 0, 255)));
    let mut face_canvas = Canvas::new(2, 1);
    face_canvas.set_pixel(0, 0, Rgba::new(255, 0, 255, 0));
    face_canvas.set_pixel(1, 0, Rgba::new(0, 0, 255, 255));
    let face = ProducerFrame::Canvas(face_canvas);
    let params = display_finalize_params(2, 1, BlendMode::Replace);

    let frame = finalize_display_face_yuv420_blocking(&mut compositor, &scene, &face, params);

    assert_eq!(frame.width, 2);
    assert_eq!(frame.height, 1);
    assert_eq!(frame.y_plane(), &[0, 29]);
}

#[cfg(feature = "servo-gpu-import")]
#[test]
fn gpu_display_finalize_copies_imported_face_into_owned_source_texture() {
    let Some(mut compositor) = super::gpu_test_compositor() else {
        return;
    };
    let width = 2;
    let height = 2;
    let texture = compositor.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("SparkleFlinger test imported display face"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let top_left_origin = [
        255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
    ];
    compositor.queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &top_left_origin,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * 4),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let texture = Arc::new(texture);
    let face = ProducerFrame::Gpu(hypercolor_gpu_frame::ImportedEffectFrame {
        width,
        height,
        format: hypercolor_gpu_frame::ImportedFrameFormat::Rgba8Unorm,
        allocation_id: hypercolor_gpu_frame::ImportedFrameAllocationId::new(42),
        content_generation: 7,
        origin: hypercolor_gpu_frame::FrameOrigin::TopLeft,
        lease: hypercolor_gpu_frame::ImportedFrameLease::new(Arc::clone(&texture)),
        texture,
        view: Arc::new(view),
        timings: hypercolor_gpu_frame::ImportedFrameTimings::default(),
    });
    let scene = ProducerFrame::Canvas(solid_canvas_with_size(
        width,
        height,
        Rgba::new(0, 0, 0, 255),
    ));
    let params = display_finalize_params_for_format(
        width,
        height,
        BlendMode::Replace,
        DisplayFrameFormat::Jpeg,
    );

    let frame = finalize_display_face_yuv420_blocking(&mut compositor, &scene, &face, params);
    let surfaces = compositor
        .display_finalize_surfaces
        .get(&params.cache_key)
        .expect("display finalize surfaces should remain cached");
    let face_source = surfaces
        .face_source
        .as_ref()
        .expect("imported face should be copied into owned display source");

    assert_eq!(frame.y_plane(), &[76, 150, 29, 255]);
    assert_eq!(
        face_source.cached_gpu_copy,
        Some(CachedGpuSourceCopy {
            storage_id: 42,
            content_generation: 7,
            width,
            height,
        }),
    );
    assert!(face_source.cached_upload.is_none());
}

#[test]
fn gpu_display_finalize_async_ring_releases_slots_after_discard() {
    let Some(mut compositor) = super::gpu_test_compositor() else {
        return;
    };
    let scene = ProducerFrame::Canvas(solid_canvas_with_size(2, 2, Rgba::new(0, 0, 255, 255)));
    let face = ProducerFrame::Canvas(solid_canvas_with_size(2, 2, Rgba::new(255, 0, 0, 255)));
    let params = display_finalize_params(2, 2, BlendMode::Replace);
    let mut pending = Vec::new();

    for _ in 0..DISPLAY_FINALIZE_READBACK_SLOT_COUNT {
        match compositor
            .begin_finalize_display_face(&scene, &face, params)
            .expect("display finalize should not fail")
        {
            GpuDisplayFinalizeDispatch::Pending(work) => pending.push(work),
            GpuDisplayFinalizeDispatch::Unsupported | GpuDisplayFinalizeDispatch::Saturated => {
                panic!("display finalizer ring should accept available slots");
            }
        }
    }

    assert!(matches!(
        compositor
            .begin_finalize_display_face(&scene, &face, params)
            .expect("display finalize should not fail"),
        GpuDisplayFinalizeDispatch::Saturated,
    ));

    for work in pending {
        compositor.discard_pending_display_finalization(work);
    }

    match compositor
        .begin_finalize_display_face(&scene, &face, params)
        .expect("display finalize should not fail")
    {
        GpuDisplayFinalizeDispatch::Pending(work) => {
            compositor.discard_pending_display_finalization(work);
        }
        GpuDisplayFinalizeDispatch::Unsupported | GpuDisplayFinalizeDispatch::Saturated => {
            panic!("discarded display finalizer slots should be reusable");
        }
    }
}

#[test]
fn gpu_display_finalize_keeps_route_surface_sets_independent() {
    let Some(mut compositor) = super::gpu_test_compositor() else {
        return;
    };
    let scene_small =
        ProducerFrame::Canvas(solid_canvas_with_size(2, 2, Rgba::new(0, 0, 255, 255)));
    let face_small = ProducerFrame::Canvas(solid_canvas_with_size(2, 2, Rgba::new(255, 0, 0, 255)));
    let scene_large =
        ProducerFrame::Canvas(solid_canvas_with_size(4, 4, Rgba::new(0, 255, 0, 255)));
    let face_large = ProducerFrame::Canvas(solid_canvas_with_size(4, 4, Rgba::new(0, 0, 255, 255)));
    let small_params = display_finalize_params(2, 2, BlendMode::Replace);
    let large_params = display_finalize_params(4, 4, BlendMode::Replace);

    let small_pending = match compositor
        .begin_finalize_display_face(&scene_small, &face_small, small_params)
        .expect("small display finalize should not fail")
    {
        GpuDisplayFinalizeDispatch::Pending(work) => work,
        GpuDisplayFinalizeDispatch::Unsupported | GpuDisplayFinalizeDispatch::Saturated => {
            panic!("small display finalizer route should accept work");
        }
    };
    let large_pending = match compositor
        .begin_finalize_display_face(&scene_large, &face_large, large_params)
        .expect("large display finalize should not fail")
    {
        GpuDisplayFinalizeDispatch::Pending(work) => work,
        GpuDisplayFinalizeDispatch::Unsupported | GpuDisplayFinalizeDispatch::Saturated => {
            panic!("large display finalizer route should accept work");
        }
    };

    assert_eq!(compositor.display_finalize_surfaces.len(), 2);
    assert!(
        compositor
            .display_finalize_surfaces
            .contains_key(&small_params.cache_key)
    );
    assert!(
        compositor
            .display_finalize_surfaces
            .contains_key(&large_params.cache_key)
    );

    let small = compositor
        .finish_pending_display_finalization_blocking(small_pending)
        .expect("small pending finalization should not fail")
        .expect("small pending finalization should complete");
    let large = compositor
        .finish_pending_display_finalization_blocking(large_pending)
        .expect("large pending finalization should not fail")
        .expect("large pending finalization should complete");

    let GpuDisplayFinalizeFrame::Rgba(small) = small else {
        panic!("small display finalization should produce RGBA");
    };
    let GpuDisplayFinalizeFrame::Rgba(large) = large else {
        panic!("large display finalization should produce RGBA");
    };
    assert_eq!((small.width(), small.height()), (2, 2));
    assert_eq!((large.width(), large.height()), (4, 4));
    assert_eq!(&small.rgba_bytes()[0..4], &[255, 0, 0, 255]);
    assert_eq!(&large.rgba_bytes()[0..4], &[0, 0, 255, 255]);

    compositor.retain_display_finalize_zones(&[small_params.cache_key.zone_id]);
    assert!(
        compositor
            .display_finalize_surfaces
            .contains_key(&small_params.cache_key)
    );
    assert!(
        !compositor
            .display_finalize_surfaces
            .contains_key(&large_params.cache_key)
    );
}

/// A surface set the device could not allocate must not be cached for the
/// route, or every later finalization would keep using its invalid
/// resources after memory came back.
#[test]
fn display_finalize_surfaces_that_fail_to_allocate_are_rebuilt_on_retry() {
    let Some(mut compositor) = super::gpu_test_compositor() else {
        return;
    };
    let scene = ProducerFrame::Canvas(solid_canvas_with_size(2, 2, Rgba::new(0, 0, 0, 255)));
    let face = ProducerFrame::Canvas(solid_canvas_with_size(2, 2, Rgba::new(255, 0, 0, 255)));
    let params = display_finalize_params(2, 2, BlendMode::Replace);

    super::super::fail_next_scoped_gpu_allocation_for_test();
    let Err(error) = compositor.begin_finalize_display_face(&scene, &face, params) else {
        panic!("a failed surface allocation must fail the finalization");
    };
    assert!(
        format!("{error:#}").contains("display finalize surfaces"),
        "{error:#}"
    );
    assert!(
        !compositor
            .display_finalize_surfaces
            .contains_key(&params.cache_key),
        "the failed surface set must not be cached for the route"
    );

    let surface = finalize_display_face_blocking(&mut compositor, &scene, &face, params);
    assert_eq!(&surface.rgba_bytes()[0..4], &[255, 0, 0, 255]);
}

/// The same holds for the per-route source texture a CPU face is uploaded
/// into: one that failed to allocate is dropped, and the next finalization
/// allocates it again.
#[test]
fn display_source_textures_that_fail_to_allocate_are_rebuilt_on_retry() {
    let Some(mut compositor) = super::gpu_test_compositor() else {
        return;
    };
    let scene = ProducerFrame::Canvas(solid_canvas_with_size(2, 2, Rgba::new(0, 0, 0, 255)));
    let small_face = ProducerFrame::Canvas(solid_canvas_with_size(2, 2, Rgba::new(0, 0, 255, 255)));
    let large_face = ProducerFrame::Canvas(solid_canvas_with_size(4, 4, Rgba::new(255, 0, 0, 255)));
    let params = display_finalize_params(2, 2, BlendMode::Replace);
    finalize_display_face_blocking(&mut compositor, &scene, &small_face, params);

    // A larger face needs a new source texture, and that allocation fails.
    super::super::fail_next_scoped_gpu_allocation_for_test();
    let Err(error) = compositor.begin_finalize_display_face(&scene, &large_face, params) else {
        panic!("a failed source texture allocation must fail the finalization");
    };
    assert!(
        format!("{error:#}").contains("display source texture"),
        "{error:#}"
    );
    let surfaces = compositor
        .display_finalize_surfaces
        .get(&params.cache_key)
        .expect("the route's surfaces were allocated by the first finalization");
    assert!(
        surfaces.face_source.is_none(),
        "the failed source texture must not be kept"
    );
    assert_eq!(compositor.display_finalize_readback_slots_in_use(), 0);

    let surface = finalize_display_face_blocking(&mut compositor, &scene, &large_face, params);
    assert_eq!(&surface.rgba_bytes()[0..4], &[255, 0, 0, 255]);
}

/// A map callback that reports failure leaves exactly one unmap, done by
/// the failed-map cleanup, and frees the readback slot.
#[test]
fn a_failed_display_readback_map_unmaps_its_buffer_once() {
    let Some(mut compositor) = super::gpu_test_compositor() else {
        return;
    };
    let scene = ProducerFrame::Canvas(solid_canvas_with_size(2, 2, Rgba::new(0, 0, 0, 255)));
    let face = ProducerFrame::Canvas(solid_canvas_with_size(2, 2, Rgba::new(255, 0, 0, 255)));
    let params = display_finalize_params(2, 2, BlendMode::Replace);
    let GpuDisplayFinalizeDispatch::Pending(mut pending) = compositor
        .begin_finalize_display_face(&scene, &face, params)
        .expect("RGBA display finalize should dispatch")
    else {
        panic!("a 2x2 RGBA display finalize should be admitted");
    };
    pending.fail_map_for_test();

    let Err(error) = compositor.try_finish_pending_display_finalization(&mut pending) else {
        panic!("a failed map must fail the finalization");
    };
    assert!(format!("{error:#}").contains("mapping failed"), "{error:#}");
    assert_eq!(pending.unmap_count(), 1, "a failed map unmaps once");
    assert_eq!(compositor.display_finalize_readback_slots_in_use(), 0);
}

/// A display readback that finishes normally unmaps its buffer once, for
/// both formats, through the finalization's own unmap.
#[test]
fn finished_display_readbacks_unmap_their_buffer_once() {
    let Some(mut compositor) = super::gpu_test_compositor() else {
        return;
    };
    let scene = ProducerFrame::Canvas(solid_canvas_with_size(2, 2, Rgba::new(0, 0, 0, 255)));
    let face = ProducerFrame::Canvas(solid_canvas_with_size(2, 2, Rgba::new(255, 0, 0, 255)));
    for (format, frame_format) in [
        ("RGBA", DisplayFrameFormat::Rgb),
        ("YUV", DisplayFrameFormat::Jpeg),
    ] {
        let params = display_finalize_params_for_format(2, 2, BlendMode::Replace, frame_format);
        let dispatch = if frame_format == DisplayFrameFormat::Jpeg {
            compositor.begin_finalize_display_face_yuv420(&scene, &face, params)
        } else {
            compositor.begin_finalize_display_face(&scene, &face, params)
        };
        let GpuDisplayFinalizeDispatch::Pending(mut pending) =
            dispatch.expect("display finalize should dispatch")
        else {
            panic!("a 2x2 {format} display finalize should be admitted");
        };
        compositor.wait_for_gpu_idle();
        assert!(
            compositor
                .try_finish_pending_display_finalization(&mut pending)
                .expect("the finished readback should resolve")
                .is_some(),
            "the {format} readback finished on an idle GPU"
        );
        assert_eq!(
            pending.unmap_count(),
            1,
            "a finished {format} readback unmaps once"
        );
    }
}
