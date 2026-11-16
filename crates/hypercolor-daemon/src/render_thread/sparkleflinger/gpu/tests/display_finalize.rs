use super::*;
use crate::render_thread::sparkleflinger::gpu::display_finalize::DisplayFinalizeRoute;

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

/// A screen with no face finalizes its backdrop alone: the backdrop seen
/// through the viewport, masked and dimmed like a face frame, whatever
/// blend the parameters name.
#[test]
fn gpu_display_finalize_shows_a_backdrop_alone_through_its_viewport() {
    let Some(mut compositor) = super::gpu_test_compositor() else {
        return;
    };
    let mut backdrop = Canvas::new(4, 2);
    for y in 0..2 {
        for x in 0..4 {
            let color = if x < 2 {
                Rgba::new(255, 0, 0, 255)
            } else {
                Rgba::new(0, 0, 255, 255)
            };
            backdrop.set_pixel(x, y, color);
        }
    }
    let backdrop = ProducerFrame::Canvas(backdrop);

    for blend_mode in [BlendMode::Replace, BlendMode::Alpha, BlendMode::Multiply] {
        let mut right_half = display_finalize_params(2, 2, blend_mode);
        right_half.viewport_position = NormalizedPosition::new(0.75, 0.5);
        right_half.viewport_size = NormalizedPosition::new(0.5, 1.0);
        right_half.brightness = 0.5;

        let surface = (0..16)
            .find_map(|_| {
                compositor
                    .finalize_display_backdrop(&backdrop, right_half)
                    .expect("backdrop finalize should not fail")
            })
            .expect("backdrop finalize should produce a surface");

        for pixel in surface.rgba_bytes().as_chunks::<4>().0 {
            assert_eq!(
                pixel,
                &[0, 0, 128, 255],
                "the {blend_mode:?} parameters should leave the dimmed blue half alone"
            );
        }
    }
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
        .and_then(DisplayFinalizeRoute::surfaces)
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
    let start = std::time::Instant::now();
    compositor.admission_clock.pin(start);

    super::super::fail_next_scoped_gpu_allocation_for_test();
    let Err(error) = compositor.begin_finalize_display_face(&scene, &face, params) else {
        panic!("a failed surface allocation must fail the finalization");
    };
    assert!(
        format!("{error:#}").contains("display finalize surfaces"),
        "{error:#}"
    );
    assert!(
        compositor
            .display_finalize_surfaces
            .get(&params.cache_key)
            .and_then(DisplayFinalizeRoute::surfaces)
            .is_none(),
        "the failed surface set must not be cached for the route"
    );

    compositor
        .admission_clock
        .pin(start + super::super::admission::ADMISSION_RETRY_INITIAL_DELAY);
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
    let start = std::time::Instant::now();
    compositor.admission_clock.pin(start);
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
        .and_then(DisplayFinalizeRoute::surfaces)
        .expect("the route's surfaces were allocated by the first finalization");
    assert!(
        surfaces.face_source.is_none(),
        "the failed source texture must not be kept"
    );
    assert_eq!(compositor.display_finalize_readback_slots_in_use(), 0);

    compositor
        .admission_clock
        .pin(start + super::super::admission::ADMISSION_RETRY_INITIAL_DELAY);
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

/// Drive `dispatch` once per 16 ms render tick from `start` while scoped
/// allocations fail, and return the tick times (ms) that attempted one.
fn admission_attempt_ticks(
    compositor: &mut GpuSparkleFlinger,
    start: std::time::Instant,
    ticks: std::ops::RangeInclusive<u64>,
    mut dispatch: impl FnMut(&mut GpuSparkleFlinger) -> anyhow::Result<GpuDisplayFinalizeDispatch>,
) -> Vec<u64> {
    let mut attempted = Vec::new();
    for tick in ticks {
        let at_ms = tick * 16;
        compositor
            .admission_clock
            .pin(start + std::time::Duration::from_millis(at_ms));
        let before = super::super::scoped_gpu_allocation_attempts_for_test();
        let result = dispatch(compositor);
        if super::super::scoped_gpu_allocation_attempts_for_test() > before {
            assert!(result.is_err(), "an attempt at {at_ms} ms should fail");
            attempted.push(at_ms);
        } else {
            assert!(
                matches!(result, Ok(GpuDisplayFinalizeDispatch::Saturated)),
                "a tick inside the backoff at {at_ms} ms should wait"
            );
        }
    }
    attempted
}

/// Sustained admission failure retries a route's surface allocation at the
/// backoff deadlines (100 ms, then 200 ms, then 400 ms after each failure),
/// not on every tick, and recovers at the first deadline after memory is
/// back.
#[test]
fn display_surface_admission_backs_off_while_allocation_keeps_failing() {
    let Some(mut compositor) = super::gpu_test_compositor() else {
        return;
    };
    let scene = ProducerFrame::Canvas(solid_canvas_with_size(2, 2, Rgba::new(0, 0, 0, 255)));
    let face = ProducerFrame::Canvas(solid_canvas_with_size(2, 2, Rgba::new(255, 0, 0, 255)));
    let params = display_finalize_params(2, 2, BlendMode::Replace);
    let start = std::time::Instant::now();

    super::super::fail_scoped_gpu_allocations_for_test(u32::MAX);
    let attempted = admission_attempt_ticks(&mut compositor, start, 0..=44, |compositor| {
        compositor.begin_finalize_display_face(&scene, &face, params)
    });
    assert_eq!(attempted, [0, 112, 320]);
    super::super::fail_scoped_gpu_allocations_for_test(0);

    // The next deadline is 720 ms (tick 45): the first attempt after the
    // pressure ends succeeds there.
    compositor
        .admission_clock
        .pin(start + std::time::Duration::from_millis(720));
    let before = super::super::scoped_gpu_allocation_attempts_for_test();
    let GpuDisplayFinalizeDispatch::Pending(pending) = compositor
        .begin_finalize_display_face(&scene, &face, params)
        .expect("admission should succeed once memory is back")
    else {
        panic!("the recovered route should dispatch its finalization");
    };
    assert!(
        super::super::scoped_gpu_allocation_attempts_for_test() > before,
        "the route allocates again at its deadline"
    );
    let Some(GpuDisplayFinalizeFrame::Rgba(surface)) = compositor
        .finish_pending_display_finalization_blocking(pending)
        .expect("the recovered finalization should finish")
    else {
        panic!("the recovered RGBA finalization should produce a surface");
    };
    assert_eq!(&surface.rgba_bytes()[0..4], &[255, 0, 0, 255]);

    // A healthy route reuses its surfaces and allocates nothing.
    let before = super::super::scoped_gpu_allocation_attempts_for_test();
    finalize_display_face_blocking(&mut compositor, &scene, &face, params);
    assert_eq!(
        super::super::scoped_gpu_allocation_attempts_for_test(),
        before
    );
}

/// The same backoff covers a source texture whose allocation keeps
/// failing on a route whose surfaces exist.
#[test]
fn display_source_admission_backs_off_while_allocation_keeps_failing() {
    let Some(mut compositor) = super::gpu_test_compositor() else {
        return;
    };
    let scene = ProducerFrame::Canvas(solid_canvas_with_size(2, 2, Rgba::new(0, 0, 0, 255)));
    let small_face = ProducerFrame::Canvas(solid_canvas_with_size(2, 2, Rgba::new(0, 0, 255, 255)));
    let large_face = ProducerFrame::Canvas(solid_canvas_with_size(4, 4, Rgba::new(255, 0, 0, 255)));
    let params = display_finalize_params(2, 2, BlendMode::Replace);
    finalize_display_face_blocking(&mut compositor, &scene, &small_face, params);
    let start = std::time::Instant::now();

    super::super::fail_scoped_gpu_allocations_for_test(u32::MAX);
    let attempted = admission_attempt_ticks(&mut compositor, start, 0..=44, |compositor| {
        compositor.begin_finalize_display_face(&scene, &large_face, params)
    });
    assert_eq!(attempted, [0, 112, 320]);
    assert_eq!(compositor.display_finalize_readback_slots_in_use(), 0);
    super::super::fail_scoped_gpu_allocations_for_test(0);

    compositor
        .admission_clock
        .pin(start + std::time::Duration::from_millis(720));
    let surface = finalize_display_face_blocking(&mut compositor, &scene, &large_face, params);
    assert_eq!(&surface.rgba_bytes()[0..4], &[255, 0, 0, 255]);
}

/// Three faceless screens one zone places: each shows a different part of
/// the zone's frame, on a panel of its own shape, brightness and format.
fn zone_screen_routes() -> [DisplayFinalizeParams; 3] {
    let mut left =
        display_finalize_params_for_format(16, 16, BlendMode::Alpha, DisplayFrameFormat::Jpeg);
    left.viewport_position = NormalizedPosition::new(0.2, 0.4);
    left.viewport_size = NormalizedPosition::new(0.3, 0.5);

    let mut middle =
        display_finalize_params_for_format(12, 20, BlendMode::Alpha, DisplayFrameFormat::Rgb);
    middle.viewport_position = NormalizedPosition::new(0.5, 0.5);
    middle.viewport_size = NormalizedPosition::new(0.25, 0.6);
    middle.viewport_rotation = 0.4;
    middle.brightness = 0.6;

    let mut right =
        display_finalize_params_for_format(20, 20, BlendMode::Alpha, DisplayFrameFormat::Jpeg);
    right.cache_key.circular = true;
    right.circular = true;
    right.viewport_position = NormalizedPosition::new(0.8, 0.6);
    right.viewport_size = NormalizedPosition::new(0.3, 0.4);
    right.viewport_scale = 1.5;
    right.brightness = 0.85;

    [left, middle, right].map(|mut route| {
        // A backdrop lane lays nothing over its backdrop.
        route.opacity = 0.0;
        route
    })
}

/// The bytes a faceless screen's panel is handed for `backdrop`, in the
/// route's own frame format.
fn finalized_backdrop_bytes(
    compositor: &mut GpuSparkleFlinger,
    backdrop: &ProducerFrame,
    route: DisplayFinalizeParams,
) -> Vec<u8> {
    for _ in 0..16 {
        let dispatch = if route.cache_key.frame_format == DisplayFrameFormat::Jpeg {
            compositor.begin_finalize_display_backdrop_yuv420(backdrop, route)
        } else {
            compositor.begin_finalize_display_backdrop(backdrop, route)
        };
        let GpuDisplayFinalizeDispatch::Pending(pending) =
            dispatch.expect("backdrop finalize should not fail")
        else {
            panic!("the route should accept a backdrop finalization");
        };
        match compositor
            .finish_pending_display_finalization_blocking(pending)
            .expect("backdrop finalize should read back")
        {
            Some(GpuDisplayFinalizeFrame::Rgba(surface)) => return surface.rgba_bytes().to_vec(),
            Some(GpuDisplayFinalizeFrame::Yuv420(frame)) => {
                return [frame.y_plane(), frame.u_plane(), frame.v_plane()].concat();
            }
            None => {}
        }
    }
    panic!("backdrop finalize should produce a frame");
}

/// How many times each route uploaded a backdrop into a texture of its own.
fn backdrop_uploads(
    compositor: &GpuSparkleFlinger,
    routes: &[DisplayFinalizeParams; 3],
) -> [usize; 3] {
    (*routes).map(|route| {
        compositor
            .display_finalize_surfaces
            .get(&route.cache_key)
            .and_then(DisplayFinalizeRoute::surfaces)
            .map_or(0, |surfaces| surfaces.scene_upload_count)
    })
}

/// What every route's panel is handed when each route uploads the backdrop
/// for itself. A canvas nothing else holds has no upload identity, so no
/// route can read another's copy of it.
fn separately_uploaded_backdrops(
    pixels: &Canvas,
    routes: &[DisplayFinalizeParams; 3],
) -> Option<[Vec<u8>; 3]> {
    let mut compositor = super::gpu_test_compositor()?;
    let backdrop = ProducerFrame::Canvas(Canvas::from_rgba(
        pixels.as_rgba_bytes(),
        pixels.width(),
        pixels.height(),
    ));
    let panels = (*routes).map(|route| finalized_backdrop_bytes(&mut compositor, &backdrop, route));
    assert!(
        backdrop_uploads(&compositor, routes)
            .iter()
            .all(|uploads| *uploads >= 1),
        "every route should upload a backdrop that has no upload identity"
    );
    Some(panels)
}

/// Screens one zone places read one upload of the zone's frame, and each
/// panel gets the bytes it got when every screen uploaded the frame itself.
#[test]
fn screens_of_one_zone_share_one_backdrop_upload_and_keep_their_bytes() {
    let Some(mut compositor) = super::gpu_test_compositor() else {
        return;
    };
    let routes = zone_screen_routes();
    let first_pixels = patterned_canvas_with_size(64, 48, 3);
    let second_pixels = patterned_canvas_with_size(64, 48, 151);
    let Some(first_expected) = separately_uploaded_backdrops(&first_pixels, &routes) else {
        return;
    };
    let Some(second_expected) = separately_uploaded_backdrops(&second_pixels, &routes) else {
        return;
    };
    assert_ne!(first_expected, second_expected);

    let first = ProducerFrame::Surface(PublishedSurface::from_owned_canvas(first_pixels, 1, 0));
    let panels = routes.map(|route| finalized_backdrop_bytes(&mut compositor, &first, route));
    assert_eq!(panels, first_expected);
    assert_eq!(backdrop_uploads(&compositor, &routes), [1, 0, 0]);

    // The zone's next frame replaces the shared upload for every screen.
    let second = ProducerFrame::Surface(PublishedSurface::from_owned_canvas(second_pixels, 2, 0));
    let panels = routes.map(|route| finalized_backdrop_bytes(&mut compositor, &second, route));
    assert_eq!(panels, second_expected);
    assert_eq!(backdrop_uploads(&compositor, &routes), [2, 0, 0]);

    // A frame that has not changed is uploaded by nobody.
    let panels = routes.map(|route| finalized_backdrop_bytes(&mut compositor, &second, route));
    assert_eq!(panels, second_expected);
    assert_eq!(backdrop_uploads(&compositor, &routes), [2, 0, 0]);
}

/// A screen that comes due without the route holding its zone's frame
/// uploads the frame itself, and the screens after it read that upload.
#[test]
fn a_screen_uploads_its_backdrop_when_no_other_route_holds_it() {
    let Some(mut compositor) = super::gpu_test_compositor() else {
        return;
    };
    let routes = zone_screen_routes();
    let first_pixels = patterned_canvas_with_size(64, 48, 3);
    let second_pixels = patterned_canvas_with_size(64, 48, 151);
    let third_pixels = patterned_canvas_with_size(64, 48, 77);
    let (Some(second_expected), Some(third_expected)) = (
        separately_uploaded_backdrops(&second_pixels, &routes),
        separately_uploaded_backdrops(&third_pixels, &routes),
    ) else {
        return;
    };

    let first = ProducerFrame::Surface(PublishedSurface::from_owned_canvas(first_pixels, 1, 0));
    for route in routes {
        finalized_backdrop_bytes(&mut compositor, &first, route);
    }
    assert_eq!(backdrop_uploads(&compositor, &routes), [1, 0, 0]);

    // The last screen comes due alone: the first route still holds the
    // frame before this one, which is not this screen's backdrop.
    let second = ProducerFrame::Surface(PublishedSurface::from_owned_canvas(second_pixels, 2, 0));
    assert_eq!(
        finalized_backdrop_bytes(&mut compositor, &second, routes[2]),
        second_expected[2]
    );
    assert_eq!(backdrop_uploads(&compositor, &routes), [1, 0, 1]);
    // The others catch up to the same frame and read the upload it made.
    assert_eq!(
        finalized_backdrop_bytes(&mut compositor, &second, routes[0]),
        second_expected[0]
    );
    assert_eq!(
        finalized_backdrop_bytes(&mut compositor, &second, routes[1]),
        second_expected[1]
    );
    assert_eq!(backdrop_uploads(&compositor, &routes), [1, 0, 1]);

    // The route that held the upload goes away with its screen.
    compositor
        .retain_display_finalize_zones(&[routes[0].cache_key.zone_id, routes[1].cache_key.zone_id]);
    let third = ProducerFrame::Surface(PublishedSurface::from_owned_canvas(third_pixels, 3, 0));
    assert_eq!(
        finalized_backdrop_bytes(&mut compositor, &third, routes[1]),
        third_expected[1]
    );
    assert_eq!(
        finalized_backdrop_bytes(&mut compositor, &third, routes[0]),
        third_expected[0]
    );
    assert_eq!(backdrop_uploads(&compositor, &routes), [1, 1, 0]);
}
