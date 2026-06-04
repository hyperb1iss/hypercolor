use super::*;

#[test]
fn gpu_display_finalize_applies_replace_brightness_and_circular_mask() {
    let mut compositor = match GpuSparkleFlinger::new() {
        Ok(compositor) => compositor,
        Err(_) => return,
    };
    let scene = ProducerFrame::Canvas(solid_canvas_with_size(4, 4, Rgba::new(0, 0, 255, 255)));
    let face = ProducerFrame::Canvas(solid_canvas_with_size(4, 4, Rgba::new(255, 0, 0, 255)));
    let mut params = display_finalize_params(4, 4, DisplayFaceBlendMode::Replace);
    params.circular = true;
    params.brightness = 0.5;

    let surface = finalize_display_face_blocking(&mut compositor, &scene, &face, params);
    let rgba = surface.rgba_bytes();

    assert_eq!(&rgba[0..4], &[0, 0, 0, 0]);
    assert_eq!(
        &rgba[((2 * 4 + 2) * 4)..((2 * 4 + 2) * 4 + 4)],
        &[128, 0, 0, 255]
    );
}

#[test]
fn gpu_display_finalize_alpha_blends_in_linear_light() {
    let mut compositor = match GpuSparkleFlinger::new() {
        Ok(compositor) => compositor,
        Err(_) => return,
    };
    let scene = ProducerFrame::Canvas(solid_canvas_with_size(2, 2, Rgba::new(0, 0, 0, 255)));
    let face = ProducerFrame::Canvas(solid_canvas_with_size(2, 2, Rgba::new(255, 0, 0, 255)));
    let mut params = display_finalize_params(2, 2, DisplayFaceBlendMode::Alpha);
    params.opacity = 0.5;

    let surface = finalize_display_face_blocking(&mut compositor, &scene, &face, params);

    assert_eq!(
        &surface.rgba_bytes()[0..4],
        &[encode_srgb_channel(0.5), 0, 0, 255],
    );
}

#[test]
fn gpu_display_finalize_yuv420_reads_back_luma_and_chroma_planes() {
    let mut compositor = match GpuSparkleFlinger::new() {
        Ok(compositor) => compositor,
        Err(_) => return,
    };
    let scene = ProducerFrame::Canvas(solid_canvas_with_size(2, 2, Rgba::new(0, 0, 0, 255)));
    let face = ProducerFrame::Canvas(solid_canvas_with_size(2, 2, Rgba::new(255, 0, 0, 255)));
    let params = display_finalize_params(2, 2, DisplayFaceBlendMode::Replace);

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
    let mut compositor = match GpuSparkleFlinger::new() {
        Ok(compositor) => compositor,
        Err(_) => return,
    };
    let scene = ProducerFrame::Canvas(solid_canvas_with_size(2, 1, Rgba::new(0, 0, 0, 255)));
    let mut face_canvas = Canvas::new(2, 1);
    face_canvas.set_pixel(0, 0, Rgba::new(255, 0, 255, 0));
    face_canvas.set_pixel(1, 0, Rgba::new(0, 0, 255, 255));
    let face = ProducerFrame::Canvas(face_canvas);
    let params = display_finalize_params(2, 1, DisplayFaceBlendMode::Replace);

    let frame = finalize_display_face_yuv420_blocking(&mut compositor, &scene, &face, params);

    assert_eq!(frame.width, 2);
    assert_eq!(frame.height, 1);
    assert_eq!(frame.y_plane(), &[0, 29]);
}

#[cfg(all(feature = "servo-gpu-import", target_os = "linux"))]
#[test]
fn gpu_display_finalize_copies_imported_face_into_owned_source_texture() {
    let mut compositor = match GpuSparkleFlinger::new() {
        Ok(compositor) => compositor,
        Err(_) => return,
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
    let face = ProducerFrame::Gpu(hypercolor_core::effect::ImportedEffectFrame {
        width,
        height,
        format: hypercolor_core::effect::ImportedFrameFormat::Rgba8Unorm,
        storage_id: 42,
        texture: Arc::new(texture),
        view: Arc::new(view),
        timings: hypercolor_core::effect::ImportedFrameTimings::default(),
    });
    let scene = ProducerFrame::Canvas(solid_canvas_with_size(
        width,
        height,
        Rgba::new(0, 0, 0, 255),
    ));
    let params = display_finalize_params_for_format(
        width,
        height,
        DisplayFaceBlendMode::Replace,
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
            width,
            height,
        }),
    );
    assert!(face_source.cached_upload.is_none());
}

#[test]
fn gpu_display_finalize_async_ring_releases_slots_after_discard() {
    let mut compositor = match GpuSparkleFlinger::new() {
        Ok(compositor) => compositor,
        Err(_) => return,
    };
    let scene = ProducerFrame::Canvas(solid_canvas_with_size(2, 2, Rgba::new(0, 0, 255, 255)));
    let face = ProducerFrame::Canvas(solid_canvas_with_size(2, 2, Rgba::new(255, 0, 0, 255)));
    let params = display_finalize_params(2, 2, DisplayFaceBlendMode::Replace);
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
    let mut compositor = match GpuSparkleFlinger::new() {
        Ok(compositor) => compositor,
        Err(_) => return,
    };
    let scene_small =
        ProducerFrame::Canvas(solid_canvas_with_size(2, 2, Rgba::new(0, 0, 255, 255)));
    let face_small = ProducerFrame::Canvas(solid_canvas_with_size(2, 2, Rgba::new(255, 0, 0, 255)));
    let scene_large =
        ProducerFrame::Canvas(solid_canvas_with_size(4, 4, Rgba::new(0, 255, 0, 255)));
    let face_large = ProducerFrame::Canvas(solid_canvas_with_size(4, 4, Rgba::new(0, 0, 255, 255)));
    let small_params = display_finalize_params(2, 2, DisplayFaceBlendMode::Replace);
    let large_params = display_finalize_params(4, 4, DisplayFaceBlendMode::Replace);

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

    compositor.retain_display_finalize_groups(&[small_params.cache_key.group_id]);
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
