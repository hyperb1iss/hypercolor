#[cfg(any(
    all(feature = "servo-gpu-import", target_os = "linux"),
    all(feature = "servo-gpu-import", target_os = "macos")
))]
use std::sync::Arc;
use std::sync::mpsc;

use hypercolor_core::blend_math::encode_srgb_channel;
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_core::types::canvas::{
    Canvas, PublishedSurface, RenderSurfacePool, Rgba, SurfaceDescriptor,
};
use hypercolor_types::device::{DeviceId, DisplayFrameFormat};
use hypercolor_types::event::ZoneColors;
use hypercolor_types::scene::{DisplayFaceBlendMode, ZoneId};
use hypercolor_types::spatial::{
    EdgeBehavior, LedTopology, NormalizedPosition, Output, SamplingMode, SpatialLayout,
    StripDirection,
};

#[cfg(all(feature = "servo-gpu-import", target_os = "linux"))]
use super::CachedGpuSourceCopy;
use super::{
    DISPLAY_FINALIZE_READBACK_SLOT_COUNT, DisplayYuv420Frame, GpuDisplayFinalizeDispatch,
    GpuDisplayFinalizeFrame, GpuSparkleFlinger, GpuZoneSamplingDispatch,
    MEDIA_UPLOAD_TEXTURE_POOL_IDLE_FRAMES, MEDIA_UPLOAD_TEXTURE_RING_LEN, MediaTextureSourceKey,
    MediaUploadTextureKey, PendingPreviewMap, PendingPreviewReadback,
};
use crate::performance::CompositorBackendKind;
use crate::render_thread::producer_queue::{GpuTextureFrameOrigin, ProducerFrame};
use crate::render_thread::sparkleflinger::gpu_sampling::GpuSamplingPlan;
use crate::render_thread::sparkleflinger::{
    CompositionLayer, CompositionPlan, DisplayFinalizeCacheKey, DisplayFinalizeParams,
    PreviewSurfaceRequest, cpu::CpuSparkleFlinger,
};

fn solid_canvas(color: Rgba) -> Canvas {
    let mut canvas = Canvas::new(4, 4);
    canvas.fill(color);
    canvas
}

fn solid_canvas_with_size(width: u32, height: u32, color: Rgba) -> Canvas {
    let mut canvas = Canvas::new(width, height);
    canvas.fill(color);
    canvas
}

fn display_finalize_params(
    width: u32,
    height: u32,
    blend_mode: DisplayFaceBlendMode,
) -> DisplayFinalizeParams {
    display_finalize_params_for_format(width, height, blend_mode, DisplayFrameFormat::Rgb)
}

fn display_finalize_params_for_format(
    width: u32,
    height: u32,
    blend_mode: DisplayFaceBlendMode,
    frame_format: DisplayFrameFormat,
) -> DisplayFinalizeParams {
    DisplayFinalizeParams {
        cache_key: DisplayFinalizeCacheKey {
            group_id: ZoneId::new(),
            device_id: DeviceId::new(),
            width,
            height,
            circular: false,
            frame_format,
        },
        width,
        height,
        circular: false,
        brightness: 1.0,
        viewport_position: NormalizedPosition::new(0.5, 0.5),
        viewport_size: NormalizedPosition::new(1.0, 1.0),
        viewport_rotation: 0.0,
        viewport_scale: 1.0,
        viewport_edge_behavior: EdgeBehavior::Clamp,
        blend_mode,
        opacity: 1.0,
    }
}

fn patterned_canvas(seed: u8) -> Canvas {
    patterned_canvas_with_size(4, 4, seed)
}

fn patterned_canvas_with_size(width: u32, height: u32, seed: u8) -> Canvas {
    let mut canvas = Canvas::new(width, height);
    for y in 0..height {
        for x in 0..width {
            let base = seed.wrapping_add(u8::try_from(x * 31 + y * 17).unwrap_or_default());
            canvas.set_pixel(
                x,
                y,
                Rgba::new(base, base.wrapping_add(53), base.wrapping_add(101), 255),
            );
        }
    }
    canvas
}

fn slot_surface(color: Rgba) -> PublishedSurface {
    let mut pool = RenderSurfacePool::with_slot_count(SurfaceDescriptor::rgba8888(4, 4), 1);
    let mut lease = pool.dequeue().expect("surface slot should be available");
    lease.canvas_mut().fill(color);
    lease.submit(0, 0)
}

fn slot_surface_with_size(width: u32, height: u32, color: Rgba) -> PublishedSurface {
    let mut pool =
        RenderSurfacePool::with_slot_count(SurfaceDescriptor::rgba8888(width, height), 1);
    let mut lease = pool.dequeue().expect("surface slot should be available");
    lease.canvas_mut().fill(color);
    lease.submit(0, 0)
}

#[allow(
    clippy::unnecessary_wraps,
    reason = "test helper mirrors the Option<PreviewSurfaceRequest> shape accepted by compositor entry points"
)]
fn full_preview_request(plan: &CompositionPlan) -> Option<PreviewSurfaceRequest> {
    Some(PreviewSurfaceRequest {
        width: plan.width,
        height: plan.height,
    })
}

fn assert_zone_colors_within(actual: &[ZoneColors], expected: &[ZoneColors], tolerance: u8) {
    assert_eq!(actual.len(), expected.len());
    for (zone_index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        assert_eq!(actual.zone_id, expected.zone_id);
        assert_eq!(actual.colors.len(), expected.colors.len());
        for (color_index, (actual, expected)) in
            actual.colors.iter().zip(&expected.colors).enumerate()
        {
            for channel in 0..3 {
                assert!(
                    actual[channel].abs_diff(expected[channel]) <= tolerance,
                    "zone {zone_index} color {color_index} channel {channel}: actual {}, expected {}, tolerance {tolerance}",
                    actual[channel],
                    expected[channel],
                );
            }
        }
    }
}

fn assert_gpu_samples_match_cpu(
    compositor: &mut GpuSparkleFlinger,
    plan: &CompositionPlan,
    tolerance: u8,
) {
    let engine = SpatialEngine::new(sampling_layout(SamplingMode::Bilinear));
    let expected = CpuSparkleFlinger::new().compose(plan.clone(), true, full_preview_request(plan));
    let expected_zones = engine.sample(
        expected
            .sampling_canvas
            .as_ref()
            .expect("CPU compose should materialize a canvas"),
    );
    let composed = compositor
        .compose(plan, false, None)
        .expect("GPU composition should succeed");
    assert!(composed.sampling_canvas.is_none());
    assert!(composed.sampling_surface.is_none());

    let mut sampled = Vec::new();
    assert!(
        compositor
            .sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut sampled)
            .expect("GPU sampling should succeed")
    );
    assert_zone_colors_within(&sampled, &expected_zones, tolerance);
}

fn resolve_preview_surface_blocking(compositor: &mut GpuSparkleFlinger) -> PublishedSurface {
    loop {
        if let Some(surface) = compositor
            .resolve_preview_surface()
            .expect("GPU preview finalize should succeed")
        {
            return surface;
        }

        if let Some(submission_index) = compositor.pending_preview_submission.clone() {
            compositor
                .device
                .poll(wgpu::PollType::Wait {
                    submission_index: Some(submission_index),
                    timeout: None,
                })
                .expect("GPU preview wait should succeed");
        } else {
            assert!(
                compositor.pending_preview_map.is_some(),
                "pending preview work should remain available",
            );
            compositor
                .device
                .poll(wgpu::PollType::Poll)
                .expect("GPU preview map poll should succeed");
        }
    }
}

fn finalize_display_face_blocking(
    compositor: &mut GpuSparkleFlinger,
    scene: &ProducerFrame,
    face: &ProducerFrame,
    params: DisplayFinalizeParams,
) -> PublishedSurface {
    for _ in 0..16 {
        if let Some(surface) = compositor
            .finalize_display_face(scene, face, params)
            .expect("display finalize should not fail")
        {
            return surface;
        }
    }

    panic!("display finalize should produce a surface");
}

fn finalize_display_face_yuv420_blocking(
    compositor: &mut GpuSparkleFlinger,
    scene: &ProducerFrame,
    face: &ProducerFrame,
    mut params: DisplayFinalizeParams,
) -> DisplayYuv420Frame {
    params.cache_key.frame_format = DisplayFrameFormat::Jpeg;
    for _ in 0..16 {
        if let Some(frame) = compositor
            .finalize_display_face_yuv420(scene, face, params)
            .expect("display YUV finalize should not fail")
        {
            return frame;
        }
    }

    panic!("display YUV finalize should produce a frame");
}

fn defer_pending_preview_map(compositor: &mut GpuSparkleFlinger) {
    compositor.defer_next_preview_map_resolve();
    assert!(
        compositor
            .resolve_preview_surface()
            .expect("deferred preview finalize should not fail")
            .is_none()
    );

    if let Some(submission_index) = compositor.pending_preview_submission.clone() {
        compositor
            .device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(submission_index),
                timeout: None,
            })
            .expect("GPU preview wait should succeed");
        compositor.defer_next_preview_map_resolve();
        assert!(
            compositor
                .resolve_preview_surface()
                .expect("deferred preview map finalize should not fail")
                .is_none()
        );
    }

    assert!(compositor.pending_preview_submission.is_none());
    assert!(compositor.pending_preview_readback.is_none());
    assert!(compositor.pending_preview_map.is_some());
}

fn sampling_layout(mode: SamplingMode) -> SpatialLayout {
    sampling_layout_with_led_count(mode, 4)
}

fn sampling_layout_with_led_count(mode: SamplingMode, led_count: u32) -> SpatialLayout {
    SpatialLayout {
        id: "gpu-sampling".into(),
        name: "GPU Sampling".into(),
        description: None,
        canvas_width: 4,
        canvas_height: 4,
        zones: vec![Output {
            id: "zone".into(),
            name: "zone".into(),
            device_id: "device:zone".into(),
            zone_name: None,
            position: NormalizedPosition::new(0.5, 0.5),
            size: NormalizedPosition::new(1.0, 1.0),
            rotation: 0.0,
            scale: 1.0,
            orientation: None,
            topology: LedTopology::Strip {
                count: led_count,
                direction: StripDirection::LeftToRight,
            },
            led_positions: Vec::new(),
            led_mapping: None,
            sampling_mode: Some(mode),
            edge_behavior: Some(EdgeBehavior::Clamp),
            shape: None,
            shape_preset: None,
            display_order: 0,
            attachment: None,
            brightness: None,
        }],
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    }
}

fn fade_sampling_layout(mode: SamplingMode) -> SpatialLayout {
    SpatialLayout {
        id: "gpu-sampling-fade".into(),
        name: "GPU Sampling Fade".into(),
        description: None,
        canvas_width: 4,
        canvas_height: 4,
        zones: vec![Output {
            id: "zone".into(),
            name: "zone".into(),
            device_id: "device:zone".into(),
            zone_name: None,
            position: NormalizedPosition::new(1.25, 0.5),
            size: NormalizedPosition::new(1.0, 1.0),
            rotation: 0.0,
            scale: 1.0,
            orientation: None,
            topology: LedTopology::Point,
            led_positions: Vec::new(),
            led_mapping: None,
            sampling_mode: Some(mode),
            edge_behavior: Some(EdgeBehavior::FadeToBlack { falloff: 8.0 }),
            shape: None,
            shape_preset: None,
            display_order: 0,
            attachment: None,
            brightness: None,
        }],
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    }
}

#[test]
fn gpu_compositor_probe_reports_a_texture_format() {
    let probe = match GpuSparkleFlinger::new() {
        Ok(compositor) => compositor.probe.clone(),
        Err(_) => return,
    };

    assert!(!probe.adapter_name.is_empty());
    assert!(!probe.texture_format.is_empty());
}

#[cfg(all(feature = "servo-gpu-import", target_os = "macos"))]
#[test]
fn gpu_macos_imported_frame_composes_without_cpu_readback() {
    let mut compositor = match GpuSparkleFlinger::new() {
        Ok(compositor) => compositor,
        Err(_) => return,
    };
    let width = 2;
    let height = 2;
    let texture = compositor.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("SparkleFlinger test BGRA imported source"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Bgra8Unorm,
        usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let bgra_bottom_left_origin = [
        255, 0, 0, 255, 0, 255, 255, 255, 0, 0, 255, 255, 0, 255, 0, 255,
    ];
    compositor.queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &bgra_bottom_left_origin,
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
    let frame = hypercolor_core::effect::ImportedEffectFrame {
        width,
        height,
        format: hypercolor_core::effect::ImportedFrameFormat::Bgra8Unorm,
        storage_id: 1,
        texture: Arc::new(texture),
        view: Arc::new(view),
        timings: hypercolor_core::effect::ImportedFrameTimings::default(),
    };

    let composed = compositor
        .compose(
            &CompositionPlan::single(
                width,
                height,
                CompositionLayer::replace(ProducerFrame::Gpu(frame)),
            ),
            false,
            None,
        )
        .expect("imported frame should compose on the GPU");

    assert_eq!(composed.backend, CompositorBackendKind::Gpu);
    assert!(composed.sampling_canvas.is_none());
    assert!(composed.sampling_surface.is_none());
    assert!(
        compositor
            .current_output_frame()
            .is_ok_and(|frame| frame.is_some())
    );
}

#[test]
fn gpu_compositor_passthroughs_current_output_texture() {
    let mut compositor = match GpuSparkleFlinger::new() {
        Ok(compositor) => compositor,
        Err(_) => return,
    };
    let source_plan = CompositionPlan::single(
        4,
        4,
        CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(7))),
    );

    compositor
        .compose(&source_plan, false, None)
        .expect("initial GPU composition should succeed");
    let output_generation = compositor.output_generation;
    let output_surface = compositor.current_output;
    let frame = compositor
        .current_output_frame()
        .expect("current output frame lookup should succeed")
        .expect("current output frame should exist");

    assert_eq!(frame.origin, GpuTextureFrameOrigin::CompositorOutput);

    let passthrough_plan = CompositionPlan::single(
        4,
        4,
        CompositionLayer::replace(ProducerFrame::GpuTexture(frame)),
    );
    let composed = compositor
        .compose(&passthrough_plan, false, None)
        .expect("current output texture pass-through should succeed");

    assert!(composed.sampling_canvas.is_none());
    assert_eq!(compositor.output_generation, output_generation);
    assert_eq!(compositor.current_output, output_surface);
}

#[test]
fn gpu_compositor_does_not_passthrough_producer_texture() {
    let mut compositor = match GpuSparkleFlinger::new() {
        Ok(compositor) => compositor,
        Err(_) => return,
    };
    let source_plan = CompositionPlan::single(
        4,
        4,
        CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(9))),
    );

    compositor
        .compose(&source_plan, false, None)
        .expect("initial GPU composition should succeed");
    let output_generation = compositor.output_generation;
    let producer_frame = compositor
        .upload_canvas_frame(&patterned_canvas(11))
        .expect("producer canvas upload should succeed");

    assert_eq!(
        producer_frame.origin,
        GpuTextureFrameOrigin::ProducerTexture
    );
    assert_eq!(producer_frame.storage_id, output_generation);

    let producer_plan = CompositionPlan::single(
        4,
        4,
        CompositionLayer::replace(ProducerFrame::GpuTexture(producer_frame)),
    );
    compositor
        .compose(&producer_plan, false, None)
        .expect("producer texture composition should not be passed through");

    assert_eq!(compositor.output_generation, output_generation + 1);
}

#[test]
fn gpu_compositor_matches_cpu_alpha_composition() {
    let mut compositor = match GpuSparkleFlinger::new() {
        Ok(compositor) => compositor,
        Err(_) => return,
    };

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
    assert_gpu_samples_match_cpu(&mut compositor, &plan, 1);
}

#[test]
fn gpu_compositor_matches_cpu_add_composition() {
    let mut compositor = match GpuSparkleFlinger::new() {
        Ok(compositor) => compositor,
        Err(_) => return,
    };

    let plan = CompositionPlan::with_layers(
        4,
        4,
        vec![
            CompositionLayer::replace(ProducerFrame::Canvas(solid_canvas(Rgba::new(
                32, 12, 96, 255,
            )))),
            CompositionLayer::add(
                ProducerFrame::Canvas(solid_canvas(Rgba::new(96, 64, 48, 255))),
                0.4,
            ),
        ],
    );
    assert_gpu_samples_match_cpu(&mut compositor, &plan, 1);
}

#[test]
fn gpu_compositor_matches_cpu_screen_composition() {
    let mut compositor = match GpuSparkleFlinger::new() {
        Ok(compositor) => compositor,
        Err(_) => return,
    };

    let plan = CompositionPlan::with_layers(
        4,
        4,
        vec![
            CompositionLayer::replace(ProducerFrame::Canvas(solid_canvas(Rgba::new(
                12, 120, 48, 255,
            )))),
            CompositionLayer::screen(
                ProducerFrame::Canvas(solid_canvas(Rgba::new(200, 32, 64, 255))),
                0.6,
            ),
        ],
    );
    assert_gpu_samples_match_cpu(&mut compositor, &plan, 0);
}

#[test]
fn gpu_compositor_matches_cpu_for_distinct_multi_pass_params() {
    let mut compositor = match GpuSparkleFlinger::new() {
        Ok(compositor) => compositor,
        Err(_) => return,
    };

    let plan = CompositionPlan::with_layers(
        4,
        4,
        vec![
            CompositionLayer::alpha(
                ProducerFrame::Canvas(solid_canvas(Rgba::new(220, 28, 16, 255))),
                0.45,
            ),
            CompositionLayer::add(
                ProducerFrame::Canvas(solid_canvas(Rgba::new(24, 180, 64, 255))),
                0.3,
            ),
            CompositionLayer::screen(
                ProducerFrame::Canvas(solid_canvas(Rgba::new(32, 48, 240, 255))),
                0.55,
            ),
        ],
    );
    assert_gpu_samples_match_cpu(&mut compositor, &plan, 1);
}

#[test]
fn gpu_compositor_bypasses_single_replace_surfaces() {
    let mut compositor = match GpuSparkleFlinger::new() {
        Ok(compositor) => compositor,
        Err(_) => return,
    };
    let source =
        PublishedSurface::from_owned_canvas(solid_canvas(Rgba::new(12, 34, 56, 255)), 1, 2);
    let plan = CompositionPlan::single(
        4,
        4,
        CompositionLayer::replace(ProducerFrame::Surface(source.clone())),
    );
    let composed = compositor
        .compose(&plan, true, full_preview_request(&plan))
        .expect("single replace surface should bypass GPU composition");

    let surface = composed
        .sampling_surface
        .expect("bypass path should preserve the source surface");
    assert_eq!(surface.rgba_bytes().as_ptr(), source.rgba_bytes().as_ptr());
}

#[test]
fn gpu_compositor_bypass_surfaces_still_support_gpu_zone_sampling() {
    let mut compositor = match GpuSparkleFlinger::new() {
        Ok(compositor) => compositor,
        Err(_) => return,
    };
    let engine = SpatialEngine::new(sampling_layout(SamplingMode::Bilinear));
    let source = slot_surface(Rgba::new(24, 88, 160, 255));
    let plan = CompositionPlan::single(
        4,
        4,
        CompositionLayer::replace(ProducerFrame::Surface(source.clone())),
    );
    let expected = engine.sample(&Canvas::from_published_surface(&source));

    let composed = compositor
        .compose(&plan, false, None)
        .expect("single replace surface should still compose on the GPU");
    assert!(composed.sampling_canvas.is_none());
    assert!(composed.preview_surface.is_none());

    let mut sampled = Vec::new();
    assert!(
        compositor
            .sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut sampled)
            .expect("GPU sampler should reuse bypassed front textures")
    );
    assert_eq!(sampled, expected);
}

#[test]
fn gpu_compositor_skips_cpu_readback_when_canvas_is_not_required() {
    let mut compositor = match GpuSparkleFlinger::new() {
        Ok(compositor) => compositor,
        Err(_) => return,
    };
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

    let composed = compositor
        .compose(&plan, false, None)
        .expect("GPU composition should support no-readback mode");

    assert!(composed.sampling_canvas.is_none());
    assert!(composed.sampling_surface.is_none());
    assert!(!composed.bypassed);
}

#[test]
fn gpu_compositor_scales_preview_surface_to_requested_size() {
    let mut compositor = match GpuSparkleFlinger::new() {
        Ok(compositor) => compositor,
        Err(_) => return,
    };
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

    let composed = compositor
        .compose(
            &plan,
            false,
            Some(PreviewSurfaceRequest {
                width: 2,
                height: 2,
            }),
        )
        .expect("GPU composition should support scaled preview surfaces");

    assert!(composed.sampling_canvas.is_none());
    assert!(composed.sampling_surface.is_none());
    assert!(composed.preview_surface.is_none());
    let preview_surface = resolve_preview_surface_blocking(&mut compositor);
    assert_eq!(preview_surface.width(), 2);
    assert_eq!(preview_surface.height(), 2);
}

#[test]
fn gpu_full_size_preview_stages_publication_without_sampling_canvas() {
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
    let request = PreviewSurfaceRequest {
        width: 4,
        height: 4,
    };

    let composed = compositor
        .compose(&plan, false, Some(request))
        .expect("GPU composition should preserve a full-size GPU preview");

    assert!(composed.sampling_canvas.is_none());
    assert!(composed.sampling_surface.is_none());
    assert!(composed.preview_surface.is_none());
    assert!(compositor.preview_surfaces.is_some());
    assert!(compositor.pending_preview_readback.is_some());
    assert!(compositor.pending_output_submission.is_some());
    assert!(compositor.cached_readback_surface.is_none());
    assert!(compositor.cached_preview_surfaces.is_empty());

    let preview_surface = resolve_preview_surface_blocking(&mut compositor);
    assert_eq!(preview_surface.width(), 4);
    assert_eq!(preview_surface.height(), 4);
    assert!(compositor.cached_readback_surface.is_some());
    assert!(compositor.cached_preview_surfaces.is_empty());
}

#[test]
fn gpu_full_size_preview_uses_texture_copy_for_aligned_rows() {
    let mut compositor = match GpuSparkleFlinger::new() {
        Ok(compositor) => compositor,
        Err(_) => return,
    };
    let plan = CompositionPlan::with_layers(
        64,
        4,
        vec![
            CompositionLayer::replace(ProducerFrame::Surface(slot_surface_with_size(
                64,
                4,
                Rgba::new(255, 32, 0, 255),
            ))),
            CompositionLayer::alpha(
                ProducerFrame::Surface(slot_surface_with_size(64, 4, Rgba::new(32, 64, 255, 255))),
                0.35,
            ),
        ],
    );
    let request = PreviewSurfaceRequest {
        width: 64,
        height: 4,
    };

    let composed = compositor
        .compose(&plan, false, Some(request))
        .expect("GPU composition should preserve a full-size GPU preview");

    assert!(composed.sampling_canvas.is_none());
    assert!(composed.sampling_surface.is_none());
    assert!(composed.preview_surface.is_none());
    assert_eq!(
        compositor
            .preview_surfaces
            .as_ref()
            .expect("preview surfaces should be allocated")
            .scale_param_write_count,
        0
    );

    let preview_surface = resolve_preview_surface_blocking(&mut compositor);
    assert_eq!(preview_surface.width(), 64);
    assert_eq!(preview_surface.height(), 4);
}

#[test]
fn gpu_scaled_preview_reuses_bind_groups_and_scale_params() {
    let mut compositor = match GpuSparkleFlinger::new() {
        Ok(compositor) => compositor,
        Err(_) => return,
    };
    let request = PreviewSurfaceRequest {
        width: 2,
        height: 2,
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
            CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(33))),
            CompositionLayer::alpha(ProducerFrame::Canvas(patterned_canvas(144)), 0.35),
        ],
    );

    compositor
        .compose(&first_plan, false, Some(request))
        .expect("first scaled preview compose should succeed");
    let _ = resolve_preview_surface_blocking(&mut compositor);
    {
        let preview_surfaces = compositor
            .preview_surfaces
            .as_ref()
            .expect("scaled preview should allocate preview surfaces");
        assert_eq!(preview_surfaces.scale_param_write_count, 1);
        assert_eq!(preview_surfaces.preview_bind_group_count, 2);
    }

    compositor
        .compose(&second_plan, false, Some(request))
        .expect("second scaled preview compose should succeed");
    let _ = resolve_preview_surface_blocking(&mut compositor);

    let preview_surfaces = compositor
        .preview_surfaces
        .as_ref()
        .expect("preview surfaces should stay allocated across same-size requests");
    assert_eq!(preview_surfaces.scale_param_write_count, 1);
    assert_eq!(preview_surfaces.preview_bind_group_count, 2);
}

#[test]
fn gpu_scaled_preview_reuses_buffers_across_smaller_requests() {
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
    assert_eq!(compositor.preview_surface_allocation_count, 1);

    compositor
        .compose(&plan, false, Some(small_request))
        .expect("small scaled preview compose should succeed");
    let _ = resolve_preview_surface_blocking(&mut compositor);

    let preview_surfaces = compositor
        .preview_surfaces
        .as_ref()
        .expect("scaled preview should keep preview surfaces allocated");
    assert_eq!(preview_surfaces.width, 2);
    assert_eq!(preview_surfaces.height, 2);
    assert_eq!(preview_surfaces.capacity_width, 3);
    assert_eq!(preview_surfaces.capacity_height, 3);
    assert_eq!(preview_surfaces.preview_bind_group_count, 2);
    assert_eq!(preview_surfaces.last_readback_bytes, 16);
    assert_eq!(compositor.preview_surface_allocation_count, 1);

    let composed = compositor
        .compose(&plan, false, Some(large_request))
        .expect("restored scaled preview compose should succeed");
    let _ = composed
        .preview_surface
        .unwrap_or_else(|| resolve_preview_surface_blocking(&mut compositor));
    assert_eq!(compositor.preview_surface_allocation_count, 1);
}

#[test]
fn gpu_scaled_preview_reuses_readback_surface_pools_across_size_flips() {
    let mut compositor = match GpuSparkleFlinger::new() {
        Ok(compositor) => compositor,
        Err(_) => return,
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
            CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(24))),
            CompositionLayer::alpha(ProducerFrame::Canvas(patterned_canvas(144)), 0.35),
        ],
    );
    let third_plan = CompositionPlan::with_layers(
        4,
        4,
        vec![
            CompositionLayer::replace(ProducerFrame::Canvas(patterned_canvas(48))),
            CompositionLayer::alpha(ProducerFrame::Canvas(patterned_canvas(192)), 0.35),
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
        .compose(&first_plan, false, Some(large_request))
        .expect("first scaled preview compose should succeed");
    let _ = resolve_preview_surface_blocking(&mut compositor);

    compositor
        .compose(&second_plan, false, Some(small_request))
        .expect("second scaled preview compose should succeed");
    let _ = resolve_preview_surface_blocking(&mut compositor);

    compositor
        .compose(&third_plan, false, Some(large_request))
        .expect("third scaled preview compose should succeed");
    let _ = resolve_preview_surface_blocking(&mut compositor);

    let preview_surfaces = compositor
        .preview_surfaces
        .as_ref()
        .expect("scaled preview should keep preview surfaces allocated");
    assert_eq!(preview_surfaces.readback_surface_pool_allocation_count, 2);
}

mod display_finalize;
mod media_upload;
mod preview;
mod sampler;
mod surface;
